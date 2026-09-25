//! Top-level view: routes between connection, key and wallet screens, shows
//! toasts, and engages privacy mode on blur or idle.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, AnyElement, Context, Entity, FocusHandle, FontWeight, SharedString, Subscription, Task,
    Window, div, linear_color_stop, linear_gradient, px,
};

use crate::assets::Icon;
use crate::keys::KeysScreen;
use crate::shell::Shell;
use crate::store::{Phase, Store, ToastKind};
use crate::theme;
use crate::widgets::{Button, Dismiss, OpenSettings, card, icon, icon_button, mono, spinner};

const IDLE_HIDE: Duration = Duration::from_secs(120);

pub struct WalletApp {
    store: Entity<Store>,
    keys: Entity<KeysScreen>,
    shell: Option<Entity<Shell>>,
    fatal: Option<SharedString>,
    /// Holds keyboard focus whenever nothing else does, so Esc and the
    /// settings shortcut always reach us.
    focus: FocusHandle,
    last_input: Instant,
    _subs: Vec<Subscription>,
    _idle: Task<()>,
}

impl WalletApp {
    pub fn new(store: Entity<Store>, fatal: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let keys = cx.new(|cx| KeysScreen::new(store.clone(), cx));
        let subs = vec![
            cx.observe(&store, |this: &mut Self, store, cx| {
                let ready = store.read(cx).phase == Phase::Ready;
                match (ready, this.shell.is_some()) {
                    (true, false) => this.shell = Some(cx.new(|cx| Shell::new(store.clone(), cx))),
                    // Dropping the shell drops its inputs, wiping anything typed.
                    (false, true) => this.shell = None,
                    _ => {}
                }
                cx.notify();
            }),
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if !window.is_window_active() {
                    this.store.update(cx, |s, cx| {
                        if s.auto_privacy && s.phase == Phase::Ready {
                            s.set_privacy(true, cx);
                        }
                    });
                }
            }),
        ];
        let idle = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.last_input.elapsed() > IDLE_HIDE {
                        this.store.update(cx, |s, cx| {
                            if s.auto_privacy && s.phase == Phase::Ready {
                                s.set_privacy(true, cx);
                            }
                        });
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            store,
            keys,
            focus,
            shell: None,
            fatal: fatal.map(Into::into),
            last_input: Instant::now(),
            _subs: subs,
            _idle: idle,
        }
    }

    fn centered(content: impl IntoElement) -> AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(linear_gradient(
                180.,
                linear_color_stop(theme::surface(), 0.),
                linear_color_stop(theme::bg(), 0.6),
            ))
            .child(content)
            .into_any_element()
    }

    fn logo() -> impl IntoElement {
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
            .child(icon(Icon::Leaf).size(px(30.)).text_color(theme::rgb_white()))
    }

    fn status_screen(title: SharedString, detail: SharedString, footer: Option<SharedString>) -> AnyElement {
        Self::centered(
            div()
                .w(px(460.))
                .max_w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap_4()
                .child(Self::logo())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .pt_2()
                        .child(spinner("status-spin"))
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::text())
                                .child(title),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::text_dim())
                        .text_center()
                        .child(detail),
                )
                .when_some(footer, |d, f| {
                    d.child(mono(f).text_xs().text_color(theme::muted()).pt_4())
                }),
        )
    }

    fn fatal_screen(error: SharedString, cx: &mut Context<Self>) -> AnyElement {
        Self::centered(
            card()
                .w(px(560.))
                .max_w_full()
                .p_8()
                .gap_4()
                .child(div().flex().items_center().gap_3().child(icon(Icon::Alert).size(px(22.)).text_color(theme::danger())).child(
                    div().text_lg().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child("Can't reach your Chia installation"),
                ))
                .child(mono(error).text_xs().text_color(theme::text_dim()).p_3().rounded_lg().bg(theme::bg()))
                .child(div().text_sm().text_color(theme::text_dim()).child(
                    "Run `chia init` and start the daemon, point CHIA_ROOT (or --root) at your Chia directory, or try `--demo` to explore safely without funds.",
                ))
                .child(div().flex().justify_end().child(Button::new("quit", "Quit").on_click(cx.listener(|_, _, _, cx| cx.quit())))),
        )
    }

    fn toasts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let toasts = self.store.read(cx).toasts.clone();
        div()
            .absolute()
            .bottom(px(20.))
            .right(px(20.))
            .flex()
            .flex_col()
            .gap_2()
            .w(px(380.))
            .children(toasts.into_iter().map(|t| {
                let (ic, color) = match t.kind {
                    ToastKind::Success => (Icon::Check, theme::accent_hi()),
                    ToastKind::Error => (Icon::Alert, theme::danger()),
                    ToastKind::Info => (Icon::Clipboard, theme::info()),
                };
                let id = t.id;
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .pl_4()
                    .pr_2()
                    .py_2()
                    .rounded_lg()
                    .bg(theme::surface_hi())
                    .border_1()
                    .border_color(theme::border_hi())
                    .shadow_lg()
                    .child(icon(ic).text_color(color))
                    .child(div().flex_1().text_sm().text_color(theme::text()).child(t.text))
                    .child(icon_button(("toast-x", id as usize), Icon::X).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.store.update(cx, |s, cx| s.dismiss_toast(id, cx));
                        },
                    )))
                    .with_animation(
                        ("toast", id as usize),
                        Animation::new(Duration::from_millis(180)),
                        |el, delta| el.opacity(delta).mt(px(12. * (1. - delta))),
                    )
            }))
    }
}

impl Render for WalletApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // When a focused input disappears (dialog closed, screen changed),
        // take focus back so keyboard shortcuts keep working.
        if window.focused(cx).is_none() {
            let focus = self.focus.clone();
            window.defer(cx, move |window, _| window.focus(&focus));
        }
        let body = if let Some(err) = self.fatal.clone() {
            Self::fatal_screen(err, cx)
        } else {
            let store = self.store.read(cx);
            match &store.phase {
                Phase::Connecting { detail } => Self::status_screen(
                    "Connecting".into(),
                    detail.clone(),
                    Some(store.security.endpoint.clone().into()),
                ),
                Phase::Unlocking(fp) => Self::status_screen(
                    if *fp == 0 {
                        "Adding key".into()
                    } else {
                        format!("Unlocking key {fp}").into()
                    },
                    "Starting the wallet for this key. A first sync can take a while.".into(),
                    None,
                ),
                Phase::Keys => self.keys.clone().into_any_element(),
                Phase::Ready => match &self.shell {
                    Some(shell) => shell.clone().into_any_element(),
                    None => Self::status_screen("Loading".into(), "".into(), None),
                },
            }
        };
        div()
            .relative()
            .size_full()
            .bg(theme::bg())
            .font_family(theme::SANS)
            .text_color(theme::text())
            // Handled here so they work whatever is focused inside.
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| match &this.shell {
                Some(shell) => shell.update(cx, |s, cx| s.dismiss(cx)),
                None => this.keys.update(cx, |k, cx| k.dismiss(cx)),
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                if let Some(shell) = &this.shell {
                    shell.update(cx, |s, cx| s.open_settings_general(cx));
                }
            }))
            .on_mouse_move(cx.listener(|this, _, _, _| this.last_input = Instant::now()))
            .capture_key_down(cx.listener(|this, _, _, _| this.last_input = Instant::now()))
            .child(body)
            .child(self.toasts(cx))
    }
}
