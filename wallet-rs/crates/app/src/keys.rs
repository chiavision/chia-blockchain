//! Key picker, key creation with backup verification, and import.

use std::time::Duration;

use chia_wallet_core::KeyOrigin;
use chia_wallet_core::bip39::{self, MnemonicError};
use chia_wallet_core::secret::{Mnemonic, SecretString};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Entity, Focusable, FontWeight, SharedString, Subscription, Window, div, linear_color_stop,
    linear_gradient, px,
};

use crate::assets::Icon;
use crate::store::{Store, ToastKind};
use crate::text_input::{self, InputEvent, TextInput};
use crate::theme;
use crate::widgets::{Button, Callout, Dismiss, callout, card, eyebrow, icon, icon_button, modal, mono, spinner};

const REVEAL_FOR: Duration = Duration::from_secs(30);

enum Mode {
    List,
    Generating,
    Show { mnemonic: Mnemonic, revealed: bool },
    Verify { mnemonic: Mnemonic, positions: [usize; 3] },
    Import,
}

enum Dialog {
    Delete(u32),
    RevealWarn(u32),
    Revealed { fingerprint: u32, phrase: SecretString },
}

pub struct KeysScreen {
    store: Entity<Store>,
    mode: Mode,
    dialog: Option<Dialog>,
    verify_inputs: [Entity<TextInput>; 3],
    verify_error: Option<SharedString>,
    import_input: Entity<TextInput>,
    delete_input: Entity<TextInput>,
    _subs: Vec<Subscription>,
}

fn random_positions(len: usize) -> [usize; 3] {
    let mut out = [0usize; 3];
    let mut n = 0;
    while n < 3 {
        let mut b = [0u8; 2];
        getrandom::fill(&mut b).expect("OS randomness");
        let p = usize::from(u16::from_le_bytes(b)) % len;
        if !out[..n].contains(&p) {
            out[n] = p;
            n += 1;
        }
    }
    out.sort_unstable();
    out
}

impl KeysScreen {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let verify_inputs = std::array::from_fn(|_| {
            cx.new(|cx| {
                TextInput::new(cx, "word")
                    .mono()
                    .filter(text_input::mnemonic_chars)
                    .max_len(12)
            })
        });
        let import_input = cx.new(|cx| {
            TextInput::new(cx, "Paste or type your 24 words, separated by spaces")
                .secret()
                .mono()
                .filter(text_input::mnemonic_chars)
                .max_len(300)
        });
        let delete_input = cx.new(|cx| {
            TextInput::new(cx, "Fingerprint")
                .mono()
                .filter(text_input::digits)
                .max_len(10)
        });
        let mut subs = vec![cx.observe(&store, |_, _, cx| cx.notify())];
        for input in [&import_input, &delete_input] {
            subs.push(cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify()));
        }
        for input in &verify_inputs {
            subs.push(cx.subscribe(input, |this: &mut Self, _, ev: &InputEvent, cx| {
                this.verify_error = None;
                if matches!(ev, InputEvent::Submit) {
                    this.check_verification(cx);
                }
                cx.notify();
            }));
        }
        Self {
            store,
            mode: Mode::List,
            dialog: None,
            verify_inputs,
            verify_error: None,
            import_input,
            delete_input,
            _subs: subs,
        }
    }

    fn back_to_list(&mut self, cx: &mut Context<Self>) {
        // Dropping the mode drops (and wipes) any mnemonic it held.
        self.mode = Mode::List;
        self.verify_error = None;
        for i in &self.verify_inputs {
            i.update(cx, |i, cx| i.clear(cx));
        }
        self.import_input.update(cx, |i, cx| {
            i.clear(cx);
            i.set_masked(true, cx);
        });
        cx.notify();
    }

    fn start_create(&mut self, cx: &mut Context<Self>) {
        self.mode = Mode::Generating;
        cx.notify();
        let task = self.store.update(cx, |s, cx| s.generate_mnemonic(cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                match r {
                    Ok(mnemonic) => {
                        this.mode = Mode::Show {
                            mnemonic,
                            revealed: false,
                        }
                    }
                    Err(e) => {
                        this.mode = Mode::List;
                        this.store
                            .update(cx, |s, cx| s.toast(ToastKind::Error, e.to_string(), cx));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn check_verification(&mut self, cx: &mut Context<Self>) {
        let Mode::Verify { mnemonic, positions } = &self.mode else {
            return;
        };
        let ok = positions.iter().zip(&self.verify_inputs).all(|(p, input)| {
            let typed = input.read(cx).text().trim().to_ascii_lowercase();
            mnemonic.words().get(*p).is_some_and(|w| *w == typed)
        });
        if !ok {
            self.verify_error = Some("Those words don't match. Check your written copy.".into());
            cx.notify();
            return;
        }
        let Mode::Verify { mnemonic, .. } = std::mem::replace(&mut self.mode, Mode::List) else {
            return;
        };
        self.store
            .update(cx, |s, cx| s.add_key(mnemonic, KeyOrigin::NewWallet, cx));
        self.back_to_list(cx);
    }

    fn import_status(&self, cx: &Context<Self>) -> (usize, Result<(), MnemonicError>) {
        let m = Mnemonic::parse(self.import_input.read(cx).text());
        (m.len(), bip39::validate(&m))
    }

    fn do_import(&mut self, cx: &mut Context<Self>) {
        let m = Mnemonic::parse(self.import_input.read(cx).text());
        if bip39::validate(&m).is_err() {
            return;
        }
        self.store.update(cx, |s, cx| s.add_key(m, KeyOrigin::Imported, cx));
        self.back_to_list(cx);
    }

    fn reveal(&mut self, fingerprint: u32, cx: &mut Context<Self>) {
        let task = self.store.update(cx, |s, cx| s.recovery_phrase(fingerprint, cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let shown = this
                .update(cx, |this, cx| {
                    match r {
                        Ok(Some(phrase)) => {
                            this.dialog = Some(Dialog::Revealed { fingerprint, phrase });
                            cx.notify();
                            return true;
                        }
                        Ok(None) => this.store.update(cx, |s, cx| {
                            s.toast(ToastKind::Error, "This key has no stored recovery phrase", cx)
                        }),
                        Err(e) => this
                            .store
                            .update(cx, |s, cx| s.toast(ToastKind::Error, e.to_string(), cx)),
                    }
                    this.dialog = None;
                    cx.notify();
                    false
                })
                .unwrap_or(false);
            if shown {
                cx.background_executor().timer(REVEAL_FOR).await;
                this.update(cx, |this, cx| {
                    if matches!(this.dialog, Some(Dialog::Revealed { fingerprint: f, .. }) if f == fingerprint) {
                        this.dialog = None;
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    // ----- rendering ------------------------------------------------------------------

    fn header(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(
                div()
                    .size(px(56.))
                    .rounded_2xl()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(linear_gradient(
                        135.,
                        linear_color_stop(theme::accent_hi(), 0.),
                        linear_color_stop(theme::teal(), 1.),
                    ))
                    .shadow_lg()
                    .child(icon(Icon::Leaf).size(px(30.)).text_color(theme::rgb_white())),
            )
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text())
                    .child("Chia Wallet"),
            )
            .child(div().text_sm().text_color(theme::muted()).child("Native · Rust · GPUI"))
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let fps = self.store.read(cx).fingerprints.clone();
        let rows = fps.iter().enumerate().map(|(i, fp)| {
            let fp = *fp;
            div()
                .id(("key", i))
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_3()
                .when(i > 0, |d| d.border_t_1().border_color(theme::border()))
                .cursor_pointer()
                .hover(|s| s.bg(theme::surface_hover()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.store.update(cx, |s, cx| s.log_in(fp, cx));
                }))
                .child(
                    div()
                        .size(px(36.))
                        .rounded_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme::accent_soft())
                        .child(icon(Icon::Fingerprint).text_color(theme::accent_hi())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .child(div().text_xs().text_color(theme::muted()).child("Private key"))
                        .child(
                            mono(fp.to_string())
                                .text_color(theme::text())
                                .font_weight(FontWeight::MEDIUM),
                        ),
                )
                .child(
                    icon_button(("reveal", i), Icon::Eye).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.dialog = Some(Dialog::RevealWarn(fp));
                        cx.notify();
                    })),
                )
                .child(
                    icon_button(("delete", i), Icon::Trash).on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.dialog = Some(Dialog::Delete(fp));
                        this.delete_input.update(cx, |i, cx| i.clear(cx));
                        window.focus(&this.delete_input.focus_handle(cx));
                        cx.notify();
                    })),
                )
                .child(icon(Icon::ChevronRight).text_color(theme::muted()))
        });

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(eyebrow("Choose a key to unlock"))
            .child(if fps.is_empty() {
                card()
                    .p_6()
                    .items_center()
                    .gap_2()
                    .child(icon(Icon::Key).size(px(28.)).text_color(theme::muted()))
                    .child(div().text_color(theme::text()).child("No keys on this computer yet"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::muted())
                            .child("Create a new key or import a recovery phrase."),
                    )
                    .into_any_element()
            } else {
                card().overflow_hidden().children(rows).into_any_element()
            })
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        div().flex_1().child(
                            Button::new("create", "Create new key")
                                .primary()
                                .icon(Icon::Plus)
                                .full_width()
                                .large()
                                .on_click(cx.listener(|this, _, _, cx| this.start_create(cx))),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            Button::new("import", "Import phrase")
                                .icon(Icon::Import)
                                .full_width()
                                .large()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.mode = Mode::Import;
                                    window.focus(&this.import_input.focus_handle(cx));
                                    cx.notify();
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }

    /// Only revealed words are copied (into GPUI strings) for display.
    fn word_grid<'a>(words: impl Iterator<Item = &'a str>, revealed: bool) -> impl IntoElement {
        div()
            .grid()
            .grid_cols(4)
            .gap_2()
            .children(words.enumerate().map(|(i, w)| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py(px(7.))
                    .rounded_md()
                    .bg(theme::bg())
                    .border_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .w(px(18.))
                            .text_xs()
                            .text_color(theme::muted())
                            .child(format!("{}", i + 1)),
                    )
                    .child(
                        mono(if revealed {
                            SharedString::from(w.to_owned())
                        } else {
                            "•••••".into()
                        })
                        .text_sm()
                        .text_color(if revealed { theme::text() } else { theme::muted() }),
                    )
            }))
    }

    fn back_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("back", "Back")
            .ghost()
            .icon(Icon::ArrowLeft)
            .on_click(cx.listener(|this, _, _, cx| this.back_to_list(cx)))
    }

    fn render_show(&self, words: &[String], revealed: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(eyebrow("Step 1 of 2 · Write down your recovery phrase"))
            .child(callout(
                Callout::Warning,
                "These 24 words are the key to your funds",
                "Write them on paper, in order. Never type them into a website, never photograph or screenshot them. Anyone who sees them can take everything.",
            ))
            .child(card().p_3().child(Self::word_grid(words.iter().map(String::as_str), revealed)))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(self.back_button(cx))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("toggle", if revealed { "Hide words" } else { "Reveal words" })
                                    .icon(if revealed { Icon::EyeOff } else { Icon::Eye })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Mode::Show { revealed, .. } = &mut this.mode {
                                            *revealed = !*revealed;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(Button::new("written", "I've written them down").primary().on_click(cx.listener(
                                |this, _, window, cx| {
                                    let Mode::Show { mnemonic, .. } = std::mem::replace(&mut this.mode, Mode::List) else {
                                        return;
                                    };
                                    let positions = random_positions(mnemonic.len());
                                    this.mode = Mode::Verify { mnemonic, positions };
                                    window.focus(&this.verify_inputs[0].focus_handle(cx));
                                    cx.notify();
                                },
                            ))),
                    ),
            )
            .into_any_element()
    }

    fn render_verify(&self, positions: [usize; 3], cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(eyebrow("Step 2 of 2 · Prove you have a copy"))
            .child(
                div()
                    .text_color(theme::text_dim())
                    .child("Enter these words from your written phrase. We never skip this step."),
            )
            .child(
                card()
                    .p_4()
                    .gap_3()
                    .children(positions.iter().zip(&self.verify_inputs).map(|(p, input)| {
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(72.))
                                    .text_sm()
                                    .text_color(theme::muted())
                                    .child(format!("Word #{}", p + 1)),
                            )
                            .child(div().flex_1().child(input.clone()))
                    })),
            )
            .when_some(self.verify_error.clone(), |d, e| {
                d.child(callout(Callout::Danger, "Not quite", e))
            })
            .child(
                div().flex().justify_between().child(self.back_button(cx)).child(
                    Button::new("verify", "Create wallet")
                        .primary()
                        .icon(Icon::Check)
                        .on_click(cx.listener(|this, _, _, cx| this.check_verification(cx))),
                ),
            )
            .into_any_element()
    }

    fn render_import(&self, cx: &mut Context<Self>) -> AnyElement {
        let (count, status) = self.import_status(cx);
        let masked = self.import_input.read(cx).is_masked();
        let (msg, color) = match (&status, count) {
            (_, 0) => (
                SharedString::from("Your phrase stays in wiped memory and is only sent to your local daemon."),
                theme::muted(),
            ),
            (Ok(()), n) => (format!("{n} words · checksum valid").into(), theme::accent_hi()),
            (Err(MnemonicError::UnknownWord { position, .. }), _) => {
                // Don't echo the (possibly secret) word itself while masked.
                (
                    format!("Word {position} isn't in the BIP-39 word list").into(),
                    theme::danger(),
                )
            }
            (Err(MnemonicError::WrongLength(n)), _) if *n < 24 => {
                (format!("{n} of 24 words").into(), theme::text_dim())
            }
            (Err(MnemonicError::WrongLength(n)), _) => (
                format!("{n} words — a recovery phrase has at most 24").into(),
                theme::danger(),
            ),
            (Err(e), _) => (e.to_string().into(), theme::danger()),
        };
        let valid = status.is_ok();
        let input = self.import_input.clone();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(eyebrow("Import a recovery phrase"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(self.import_input.clone()))
                    .child(icon_button("mask", if masked { Icon::Eye } else { Icon::EyeOff }).on_click(move |_, _, cx| {
                        input.update(cx, |i, cx| {
                            let m = i.is_masked();
                            i.set_masked(!m, cx)
                        })
                    })),
            )
            .child(div().text_sm().text_color(color).child(msg))
            .child(callout(
                Callout::Safe,
                "Offline check first",
                "Words and checksum are verified locally before anything is sent. Copy is disabled in this field and it never touches the clipboard.",
            ))
            .child(
                div().flex().justify_between().child(self.back_button(cx)).child(
                    Button::new("do-import", "Import key")
                        .primary()
                        .icon(Icon::Import)
                        .disabled(!valid)
                        .on_click(cx.listener(|this, _, _, cx| this.do_import(cx))),
                ),
            )
            .into_any_element()
    }

    fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialog.as_ref()?;
        let close = cx.listener(|this, _, _, cx| {
            this.dialog = None;
            cx.notify();
        });
        Some(match dialog {
            Dialog::Delete(fp) => {
                let fp = *fp;
                let typed_ok = self.delete_input.read(cx).text() == fp.to_string();
                modal(
                    "delete-dialog",
                    440.,
                    div()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(format!("Delete key {fp}?")))
                        .child(callout(
                            Callout::Danger,
                            "This cannot be undone",
                            "The key is removed from this computer's keychain. Without its 24-word recovery phrase, funds on this key are lost forever.",
                        ))
                        .child(div().text_sm().text_color(theme::text_dim()).child("Type the fingerprint to confirm"))
                        .child(self.delete_input.clone())
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(Button::new("cancel", "Cancel").ghost().on_click(close))
                                .child(Button::new("confirm-delete", "Delete key").danger().icon(Icon::Trash).disabled(!typed_ok).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.dialog = None;
                                        this.store.update(cx, |s, cx| s.delete_key(fp, cx));
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
            }
            Dialog::RevealWarn(fp) => {
                let fp = *fp;
                modal(
                    "reveal-dialog",
                    440.,
                    div()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child("Show recovery phrase?"))
                        .child(callout(
                            Callout::Warning,
                            "Make sure nobody can see your screen",
                            "Screen sharing, cameras and people nearby can all capture these words. The phrase hides itself after 30 seconds.",
                        ))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(Button::new("cancel", "Cancel").ghost().on_click(close))
                                .child(Button::new("do-reveal", "Reveal for 30s").primary().icon(Icon::Eye).on_click(cx.listener(
                                    move |this, _, _, cx| this.reveal(fp, cx),
                                ))),
                        ),
                )
            }
            Dialog::Revealed { fingerprint, phrase } => modal(
                "phrase-dialog",
                620.,
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child(format!("Recovery phrase · {fingerprint}")),
                            )
                            .child(icon_button("close", Icon::X).on_click(close)),
                    )
                    .child(Self::word_grid(phrase.expose().split_whitespace(), true))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child("Hides automatically in 30 seconds. Copying is disabled."),
                    ),
            ),
        })
    }
}

impl Render for KeysScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.mode {
            Mode::List => self.render_list(cx),
            Mode::Generating => div().flex().justify_center().child(spinner("gen")).into_any_element(),
            Mode::Show { mnemonic, revealed } => self.render_show(mnemonic.words(), *revealed, cx),
            Mode::Verify { positions, .. } => {
                let p = *positions;
                self.render_verify(p, cx)
            }
            Mode::Import => self.render_import(cx),
        };
        let wide = matches!(self.mode, Mode::Show { .. });
        div()
            .id("keys-screen")
            .relative()
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                this.dialog = None;
                cx.notify();
            }))
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .overflow_y_scroll()
            .bg(linear_gradient(
                180.,
                linear_color_stop(theme::surface(), 0.),
                linear_color_stop(theme::bg(), 0.55),
            ))
            .child(
                div()
                    .w(px(if wide { 640. } else { 480. }))
                    .max_w_full()
                    .px_4()
                    .pt(px(72.))
                    .pb_10()
                    .flex()
                    .flex_col()
                    .gap_8()
                    .child(self.header())
                    .child(body),
            )
            .children(self.render_dialog(cx))
    }
}
