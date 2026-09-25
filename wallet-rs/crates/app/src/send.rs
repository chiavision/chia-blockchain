//! The send form. It only ever produces a fully validated [`PendingSend`];
//! submitting happens after the confirmation dialog in the shell.

use chia_wallet_core::address::{chunk_address, decode_address};
use chia_wallet_core::units::{Denom, MOJO_PER_XCH, format_amount, format_amount_plain, parse_amount};
use gpui::prelude::*;
use gpui::{Context, Entity, EventEmitter, FocusHandle, Focusable, Rgba, SharedString, Subscription, Window, div, px};

use crate::assets::Icon;
use crate::store::Store;
use crate::text_input::{self, InputEvent, TextInput};
use crate::theme;
use crate::widgets::{Button, Callout, callout, card, eyebrow, icon};

/// Fees above this trigger a warning (the GUI had none).
const HIGH_FEE: u64 = MOJO_PER_XCH / 10;

#[derive(Clone, Debug)]
pub struct PendingSend {
    pub wallet_id: u32,
    pub address: String,
    pub amount: u64,
    pub fee: u64,
    pub denom: Denom,
    pub ticker: SharedString,
}

pub enum SendFormEvent {
    Review(PendingSend),
}

pub struct SendForm {
    store: Entity<Store>,
    pub wallet_id: u32,
    address: Entity<TextInput>,
    amount: Entity<TextInput>,
    fee: Entity<TextInput>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SendFormEvent> for SendForm {}

impl Focusable for SendForm {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.address.focus_handle(cx)
    }
}

struct Check {
    address: Result<(), String>,
    amount: Result<u64, String>,
    fee: Result<u64, String>,
    over_balance: bool,
}

impl Check {
    fn ready(&self) -> bool {
        self.address.is_ok() && self.amount.is_ok() && self.fee.is_ok() && !self.over_balance
    }
}

impl SendForm {
    pub fn new(store: Entity<Store>, wallet_id: u32, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| {
            TextInput::new(cx, "xch1… recipient address")
                .mono()
                .filter(text_input::address_chars)
                .max_len(100)
        });
        let amount = cx.new(|cx| {
            TextInput::new(cx, "0.00")
                .large()
                .filter(text_input::amount_chars)
                .max_len(24)
        });
        let fee = cx.new(|cx| TextInput::new(cx, "0").filter(text_input::amount_chars).max_len(16));
        let mut subs = vec![cx.observe(&store, |_, _, cx| cx.notify())];
        for input in [&address, &amount, &fee] {
            subs.push(cx.subscribe(input, |this: &mut Self, _, ev: &InputEvent, cx| {
                match ev {
                    InputEvent::Submit => this.review(cx),
                    InputEvent::Changed => this.flag_invalid(cx),
                }
                cx.notify();
            }));
        }
        Self {
            store,
            wallet_id,
            address,
            amount,
            fee,
            _subs: subs,
        }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        for i in [&self.address, &self.amount, &self.fee] {
            i.update(cx, |i, cx| i.clear(cx));
        }
    }

    fn check(&self, cx: &Context<Self>) -> Check {
        let store = self.store.read(cx);
        let denom = store.denom_for(self.wallet_id);
        let prefix = store.network.network_prefix.clone();
        let spendable = store.balances.get(&self.wallet_id).map_or(0, |b| b.spendable_balance);

        let address = decode_address(self.address.read(cx).text(), &prefix)
            .map(|_| ())
            .map_err(|e| e.to_string());
        let amount = match parse_amount(self.amount.read(cx).text(), denom) {
            Ok(0) => Err("amount must be greater than zero".into()),
            other => other.map_err(|e| e.to_string()),
        };
        let fee_text = self.fee.read(cx).text();
        let fee = if denom == Denom::ColouredCoin || fee_text.trim().is_empty() {
            Ok(0)
        } else {
            parse_amount(fee_text, Denom::Xch).map_err(|e| e.to_string())
        };
        let over_balance = match (&amount, &fee) {
            (Ok(a), Ok(f)) => a.checked_add(*f).is_none_or(|t| t > spendable),
            _ => false,
        };
        Check {
            address,
            amount,
            fee,
            over_balance,
        }
    }

    /// Outline fields red once they hold something invalid.
    fn flag_invalid(&mut self, cx: &mut Context<Self>) {
        let c = self.check(cx);
        let addr_bad = !self.address.read(cx).text().is_empty() && c.address.is_err();
        let amount_bad = !self.amount.read(cx).text().is_empty() && (c.amount.is_err() || c.over_balance);
        let fee_bad = c.fee.is_err();
        self.address.update(cx, |i, cx| i.set_invalid(addr_bad, cx));
        self.amount.update(cx, |i, cx| i.set_invalid(amount_bad, cx));
        self.fee.update(cx, |i, cx| i.set_invalid(fee_bad, cx));
    }

    fn review(&mut self, cx: &mut Context<Self>) {
        let c = self.check(cx);
        let (Ok(()), Ok(amount), Ok(fee), false) = (c.address, c.amount, c.fee, c.over_balance) else {
            return;
        };
        let store = self.store.read(cx);
        let pending = PendingSend {
            wallet_id: self.wallet_id,
            address: self.address.read(cx).text().trim().to_owned(),
            amount,
            fee,
            denom: store.denom_for(self.wallet_id),
            ticker: store.ticker(self.wallet_id),
        };
        cx.emit(SendFormEvent::Review(pending));
    }

    fn set_max(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let denom = store.denom_for(self.wallet_id);
        let max = store
            .balances
            .get(&self.wallet_id)
            .map_or(0, |b| b.max_send_amount.min(b.spendable_balance));
        let fee = parse_amount(self.fee.read(cx).text(), Denom::Xch).unwrap_or(0);
        let value = format_amount_plain(max.saturating_sub(if denom == Denom::Xch { fee } else { 0 }), denom);
        self.amount.update(cx, |i, cx| i.set_text(&value, cx));
    }

    fn field(label: &str, input: impl IntoElement, hint: Option<(SharedString, Rgba)>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(eyebrow(label.to_owned()))
            .child(input)
            .child(
                div()
                    .h(px(18.))
                    .text_xs()
                    .when_some(hint, |d, (text, color)| d.text_color(color).child(text)),
            )
    }
}

impl Render for SendForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.check(cx);
        let store = self.store.read(cx);
        let denom = store.denom_for(self.wallet_id);
        let ticker = store.ticker(self.wallet_id);
        let prefix = store.network.network_prefix.clone();
        let spendable = store.balances.get(&self.wallet_id).map_or(0, |b| b.spendable_balance);
        let spendable_text = store.money(spendable, denom);
        let is_cc = denom == Denom::ColouredCoin;

        let addr_text = self.address.read(cx).text().to_owned();
        let amount_text = self.amount.read(cx).text().to_owned();
        let fee_text = self.fee.read(cx).text().to_owned();

        let addr_hint = match (&c.address, addr_text.is_empty()) {
            (_, true) => Some((
                format!("Only {prefix} addresses are accepted on this network").into(),
                theme::muted(),
            )),
            (Ok(()), false) => {
                let chunks = chunk_address(&addr_text, 4);
                let head = chunks.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
                let tail = chunks.iter().rev().take(2).rev().cloned().collect::<Vec<_>>().join(" ");
                Some((format!("✓ Valid address · {head} … {tail}").into(), theme::accent_hi()))
            }
            (Err(e), false) => Some((e.clone().into(), theme::danger())),
        };
        let amount_hint = match (&c.amount, amount_text.is_empty()) {
            (_, true) => Some((format!("Spendable: {spendable_text} {ticker}").into(), theme::muted())),
            (Ok(_), false) if c.over_balance => Some((
                format!("More than your spendable {spendable_text} {ticker}").into(),
                theme::danger(),
            )),
            (Ok(m), false) => Some((format!("= {m} mojo").into(), theme::muted())),
            (Err(e), false) => Some((e.clone().into(), theme::danger())),
        };
        let fee_hint = match &c.fee {
            _ if is_cc => Some(("Coloured coin spends don't support fees yet".into(), theme::muted())),
            Ok(f) if *f >= HIGH_FEE => Some((
                format!("That's a high fee ({} XCH)", format_amount(*f, Denom::Xch, 0)).into(),
                theme::warning(),
            )),
            Ok(_) if fee_text.is_empty() => Some((
                "Optional. A small fee helps when blocks are full.".into(),
                theme::muted(),
            )),
            Ok(_) => None,
            Err(e) => Some((e.clone().into(), theme::danger())),
        };

        let cc_note = fee_hint.clone().map(|h| h.0);
        let fee_presets = [
            ("None", ""),
            ("0.00001", "0.00001"),
            ("0.0001", "0.0001"),
            ("0.001", "0.001"),
        ];
        let ready = c.ready();

        card()
            .p_6()
            .gap_2()
            .max_w(px(640.))
            .child(Self::field("Recipient", self.address.clone(), addr_hint))
            .child(Self::field(
                &format!("Amount ({ticker})"),
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().child(self.amount.clone()))
                    .child(Button::new("max", "Max").on_click(cx.listener(|this, _, _, cx| this.set_max(cx)))),
                amount_hint,
            ))
            .when(!is_cc, |d| {
                d.child(Self::field(
                    "Network fee (XCH)",
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(div().w(px(160.)).child(self.fee.clone()))
                        .children(fee_presets.iter().enumerate().map(|(i, (label, value))| {
                            let selected = fee_text == *value;
                            let value = *value;
                            div()
                                .id(("fee", i))
                                .px_3()
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .rounded_full()
                                .border_1()
                                .text_xs()
                                .cursor_pointer()
                                .border_color(if selected { theme::accent() } else { theme::border_hi() })
                                .bg(if selected {
                                    theme::accent_soft()
                                } else {
                                    theme::surface()
                                })
                                .text_color(if selected {
                                    theme::accent_hi()
                                } else {
                                    theme::text_dim()
                                })
                                .hover(|s| s.bg(theme::surface_hover()))
                                .child(*label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.fee.update(cx, |i, cx| i.set_text(value, cx));
                                }))
                        })),
                    fee_hint,
                ))
            })
            .when(is_cc, |d| {
                d.child(div().text_xs().text_color(theme::muted()).mb_2().children(cc_note))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pt_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(icon(Icon::Shield).size_3p5().text_color(theme::muted()))
                            .child("You'll review every detail before anything is signed"),
                    )
                    .child(
                        Button::new("review", "Review")
                            .primary()
                            .large()
                            .icon(Icon::Send)
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, _, cx| this.review(cx))),
                    ),
            )
            .when(c.over_balance && store.sync.syncing, |d| {
                d.child(callout(
                    Callout::Warning,
                    "Still syncing",
                    "Balances may be incomplete until the wallet finishes syncing.",
                ))
            })
    }
}
