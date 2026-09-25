//! The unlocked wallet: sidebar, pages and the confirmation dialogs.

use std::ops::Range;
use std::time::Duration;

use chia_wallet_core::address::chunk_address;
use chia_wallet_core::protocol::{TransactionRecord, WalletType};
use chia_wallet_core::units::{Denom, format_amount};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, FontWeight, Rgba, SharedString, Stateful, Subscription, Window, div,
    linear_color_stop, linear_gradient, px, rgba, uniform_list,
};

use crate::assets::Icon;
use crate::qr::qr_code;
use crate::send::{PendingSend, SendForm, SendFormEvent};
use crate::store::Store;
use crate::theme;
use crate::widgets::{
    Button, Callout, badge, callout, card, dot, eyebrow, icon, icon_button, logo, modal, mono, spinner, titlebar_area,
};

mod settings;

const SIDEBAR_W: f32 = 252.;
const ARM_DELAY: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Overview,
    Send,
    Receive,
    Activity,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Send => "Send",
            Page::Receive => "Receive",
            Page::Activity => "Activity",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Page::Overview => Icon::Grid,
            Page::Send => Icon::Send,
            Page::Receive => Icon::Receive,
            Page::Activity => Icon::Activity,
        }
    }
}

enum ShellModal {
    Confirm {
        pending: PendingSend,
        armed: bool,
        submitting: bool,
    },
    Tx(Box<TransactionRecord>),
    Settings(settings::Section),
}

pub struct Shell {
    store: Entity<Store>,
    page: Page,
    wallet_id: u32,
    send: Entity<SendForm>,
    modal: Option<ShellModal>,
    _store_sub: Subscription,
    _send_sub: Subscription,
}

// ----- formatting helpers ------------------------------------------------------------

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn fmt_time(ts: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, m, d) = civil_from_days((ts / 86_400) as i64);
    let secs = ts % 86_400;
    format!(
        "{} {d}, {y} · {:02}:{:02} UTC",
        MONTHS[(m - 1) as usize],
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// Thousands separators for counts like block heights.
fn group(n: u64) -> String {
    format_amount(n * 1000, Denom::ColouredCoin, 0)
}

fn short_hash(h: &str) -> String {
    let h = h.trim_start_matches("0x");
    if h.len() <= 16 {
        h.to_owned()
    } else {
        format!("{}…{}", &h[..8], &h[h.len() - 8..])
    }
}

fn short_addr(a: &str) -> String {
    if a.len() <= 20 {
        a.to_owned()
    } else {
        format!("{}…{}", &a[..10], &a[a.len() - 6..])
    }
}

/// An address rendered in alternating-tone groups of four for eyeball comparison.
fn chunked_address(address: &str, size: f32) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_x(px(7.))
        .gap_y_1()
        .font_family(theme::MONO)
        .text_size(px(size))
        .children(chunk_address(address, 4).into_iter().enumerate().map(|(i, c)| {
            div()
                .text_color(if i % 2 == 0 { theme::text() } else { theme::accent_hi() })
                .child(c)
        }))
}

impl Shell {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let wallet_id = store.read(cx).wallets.first().map_or(1, |w| w.id);
        let send = cx.new(|cx| SendForm::new(store.clone(), wallet_id, cx));
        let send_sub = cx.subscribe(&send, Self::on_send_event);
        let store_sub = cx.observe(&store, |this: &mut Self, store, cx| {
            // Keep the selection valid as wallets load.
            let s = store.read(cx);
            if !s.wallets.is_empty() && s.wallet(this.wallet_id).is_none() {
                let first = s.wallets[0].id;
                this.select_wallet(first, cx);
            }
            cx.notify();
        });
        Self {
            store,
            page: Page::Overview,
            wallet_id,
            send,
            modal: None,
            _store_sub: store_sub,
            _send_sub: send_sub,
        }
    }

    fn on_send_event(&mut self, _: Entity<SendForm>, ev: &SendFormEvent, cx: &mut Context<Self>) {
        let SendFormEvent::Review(pending) = ev;
        self.modal = Some(ShellModal::Confirm {
            pending: pending.clone(),
            armed: false,
            submitting: false,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ARM_DELAY).await;
            this.update(cx, |this, cx| {
                if let Some(ShellModal::Confirm { armed, .. }) = &mut this.modal {
                    *armed = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn select_wallet(&mut self, id: u32, cx: &mut Context<Self>) {
        if self.wallet_id == id {
            return;
        }
        self.wallet_id = id;
        self.send = cx.new(|cx| SendForm::new(self.store.clone(), id, cx));
        self._send_sub = cx.subscribe(&self.send, Self::on_send_event);
        let spendable = self
            .store
            .read(cx)
            .wallet(id)
            .is_some_and(|w| w.wallet_type.is_spendable());
        if !spendable && matches!(self.page, Page::Send | Page::Receive) {
            self.page = Page::Overview;
        }
        cx.notify();
    }

    /// Close the open dialog (Esc), unless a send is mid-submission.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.modal.is_some() && !matches!(self.modal, Some(ShellModal::Confirm { submitting: true, .. })) {
            self.modal = None;
            cx.notify();
        }
    }

    /// Open settings (the sidebar entry or the platform shortcut).
    pub fn open_settings_general(&mut self, cx: &mut Context<Self>) {
        self.open_settings(settings::Section::General, cx);
    }

    fn open_settings(&mut self, section: settings::Section, cx: &mut Context<Self>) {
        // Never replace a send that's being confirmed or submitted.
        if !matches!(self.modal, Some(ShellModal::Confirm { .. })) {
            self.modal = Some(ShellModal::Settings(section));
            cx.notify();
        }
    }

    fn go(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        if page == Page::Send {
            let handle = self.send.focus_handle(cx);
            window.focus(&handle);
        }
        cx.notify();
    }

    fn confirm_send(&mut self, cx: &mut Context<Self>) {
        let Some(ShellModal::Confirm {
            pending,
            armed: true,
            submitting,
        }) = &mut self.modal
        else {
            return;
        };
        if *submitting {
            return;
        }
        *submitting = true;
        let p = pending.clone();
        let task = self
            .store
            .update(cx, |s, cx| s.send(p.wallet_id, p.address, p.amount, p.fee, cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                match r {
                    Ok(_) => {
                        this.modal = None;
                        this.send.update(cx, |f, cx| f.reset(cx));
                        this.page = Page::Activity;
                    }
                    Err(_) => {
                        if let Some(ShellModal::Confirm { submitting, .. }) = &mut this.modal {
                            *submitting = false;
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ----- sidebar ----------------------------------------------------------------------

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let spendable = store
            .wallet(self.wallet_id)
            .is_some_and(|w| w.wallet_type.is_spendable());

        let wallets = store.wallets.iter().map(|w| {
            let id = w.id;
            let selected = id == self.wallet_id;
            let denom = store.denom_for(id);
            let bal = store.balances.get(&id).map(|b| b.confirmed_wallet_balance);
            let ic = match w.wallet_type {
                WalletType::Standard => Icon::Wallet,
                WalletType::ColouredCoin => Icon::Coin,
                WalletType::DistributedId => Icon::Id,
                _ => Icon::Cube,
            };
            let amount = match (w.wallet_type, bal) {
                (WalletType::DistributedId, _) => SharedString::from("Identity"),
                (_, Some(b)) => format!("{} {}", store.money(b, denom), store.ticker(id)).into(),
                (_, None) => "…".into(),
            };
            div()
                .id(("wallet", id as usize))
                .relative()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_lg()
                .cursor_pointer()
                .when(selected, |d| d.bg(theme::surface_hi()))
                .hover(|s| s.bg(theme::surface_hover()))
                .on_click(cx.listener(move |this, _, _, cx| this.select_wallet(id, cx)))
                .when(selected, |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(-12.))
                            .top(px(10.))
                            .w(px(3.))
                            .h(px(20.))
                            .rounded_full()
                            .bg(theme::accent()),
                    )
                })
                .child(
                    div()
                        .size(px(30.))
                        .rounded_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if selected {
                            theme::accent_soft()
                        } else {
                            theme::surface()
                        })
                        .child(icon(ic).text_color(if selected {
                            theme::accent_hi()
                        } else {
                            theme::text_dim()
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme::text())
                                .truncate()
                                .child(w.name.clone()),
                        )
                        .child(div().text_xs().text_color(theme::muted()).truncate().child(amount)),
                )
        });

        let nav = [Page::Overview, Page::Send, Page::Receive, Page::Activity]
            .into_iter()
            .map(|p| {
                let active = p == self.page;
                let enabled = spendable || !matches!(p, Page::Send | Page::Receive);
                div()
                    .id(SharedString::from(format!("nav-{:?}", p)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .h(px(36.))
                    .rounded_lg()
                    .text_sm()
                    .when(active, |d| d.bg(theme::surface_hi()).text_color(theme::text()))
                    .when(!active, |d| d.text_color(theme::text_dim()))
                    .when(enabled, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(theme::surface_hover()))
                            .on_click(cx.listener(move |this, _, window, cx| this.go(p, window, cx)))
                    })
                    .when(!enabled, |d| d.opacity(0.35))
                    .child(icon(p.icon()).text_color(if active { theme::accent_hi() } else { theme::text_dim() }))
                    .child(p.title())
            });

        let (sync_color, sync_label) = if !store.connected {
            (theme::danger(), "Daemon offline")
        } else if store.sync.synced {
            (theme::accent_hi(), "Synced")
        } else if store.sync.syncing {
            (theme::warning(), "Syncing…")
        } else {
            (theme::muted(), "Not synced")
        };
        let fingerprint = store.fingerprint.map(|f| f.to_string()).unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_W))
            .h_full()
            .bg(theme::sidebar())
            .border_r_1()
            .border_color(theme::border())
            .child(div().px_5().pt_4().pb_2().child(eyebrow("Wallets")))
            .child(div().flex().flex_col().gap_1().px_3().children(wallets))
            .child(div().px_5().pt_6().pb_2().child(eyebrow("Navigate")))
            .child(
                div().flex().flex_col().gap_1().px_3().children(nav).child(
                    div()
                        .id("nav-settings")
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .h(px(36.))
                        .rounded_lg()
                        .text_sm()
                        .text_color(theme::text_dim())
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::surface_hover()))
                        .on_click(cx.listener(|this, _, _, cx| this.open_settings(settings::Section::General, cx)))
                        .child(icon(Icon::Settings))
                        .child("Settings")
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(settings::SHORTCUT_HINT),
                        ),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .m_3()
                    .p_3()
                    .rounded_lg()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(dot(sync_color))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text())
                                    .child(sync_label),
                            )
                            .when(store.sync.syncing, |d| {
                                d.child(div().flex_1()).child(spinner("sync-spin"))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .items_center()
                                    .child(icon(Icon::Cube).size_3())
                                    .child(group(u64::from(store.height))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .items_center()
                                    .child(icon(Icon::Wifi).size_3())
                                    .child(format!("{} peers", store.peers)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .h(px(52.))
                    .border_t_1()
                    .border_color(theme::border())
                    .child(icon(Icon::Fingerprint).text_color(theme::muted()))
                    .child(mono(fingerprint).text_xs().text_color(theme::text_dim()))
                    .child(div().flex_1())
                    .child(icon_button("lock", Icon::Lock).on_click(cx.listener(|this, _, _, cx| {
                        this.store.update(cx, |s, cx| s.lock(cx));
                    }))),
            )
    }

    // ----- pages -----------------------------------------------------------------------------

    /// The unified top bar: brand over the sidebar, then breadcrumb, status
    /// badges and quick actions. On macOS the traffic lights sit at its left.
    fn topbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let wallet_name = store.wallet(self.wallet_id).map(|w| w.name.clone()).unwrap_or_default();
        let privacy = store.privacy;
        let demo = store.security.demo;
        let offline = !store.connected;
        let net = store.network.network_name.to_uppercase();
        titlebar_area("topbar")
            .flex()
            .flex_none()
            .h(px(theme::TOPBAR_H))
            .child(
                // Continues the sidebar column upward.
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .w(px(SIDEBAR_W))
                    .h_full()
                    .pl(px(20. + theme::TRAFFIC_LIGHTS_W))
                    .pr_4()
                    .bg(theme::sidebar())
                    .border_r_1()
                    .border_color(theme::border())
                    .child(logo(22.))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text())
                            .child("Chia"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px_4()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().text_sm().text_color(theme::muted()).truncate().child(wallet_name))
                    .child(icon(Icon::ChevronRight).size_3().text_color(theme::muted()))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child(self.page.title()),
                    )
                    .child(div().flex_1())
                    .when(offline, |d| {
                        d.child(badge("Reconnecting…", theme::danger(), theme::danger_soft()))
                    })
                    .when(demo, |d| {
                        d.child(badge("DEMO", theme::warning(), theme::warning_soft()))
                    })
                    .when(!net.is_empty(), |d| {
                        d.child(badge(net, theme::accent_hi(), theme::accent_soft()))
                    })
                    .child(div().w(px(6.)))
                    .child(
                        icon_button("privacy", if privacy { Icon::EyeOff } else { Icon::Eye })
                            .size(px(28.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.store.update(cx, |s, cx| s.set_privacy(!privacy, cx))
                            })),
                    )
                    .child(
                        icon_button("refresh", Icon::Refresh)
                            .size(px(28.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.store.update(cx, |s, cx| s.refresh_all(cx));
                            })),
                    )
                    .child(
                        icon_button("topbar-settings", Icon::Settings)
                            .size(px(28.))
                            .on_click(cx.listener(|this, _, _, cx| this.open_settings_general(cx))),
                    ),
            )
    }

    fn tx_row(&self, store: &Store, tx: &TransactionRecord, ix: usize, cx: &Context<Self>) -> Stateful<Div> {
        let incoming = tx.tx_type.is_incoming();
        let denom = store.denom_for(tx.wallet_id);
        let sign = if incoming { "+" } else { "−" };
        let amount = format!("{sign}{} {}", store.money(tx.amount, denom), store.ticker(tx.wallet_id));
        let (status, status_color) = if tx.rejection().is_some() {
            ("Rejected".to_string(), theme::danger())
        } else if tx.confirmed {
            (
                format!("Block {}", group(u64::from(tx.confirmed_at_height))),
                theme::muted(),
            )
        } else {
            ("Pending".to_string(), theme::warning())
        };
        let counterparty = tx
            .to_address
            .as_deref()
            .map(short_addr)
            .unwrap_or_else(|| short_hash(&tx.to_puzzle_hash));
        let record = tx.clone();
        div()
            .id(("tx", ix))
            .w_full()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .h(px(64.))
            .rounded_lg()
            .cursor_pointer()
            .hover(|s| s.bg(theme::surface_hover()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.modal = Some(ShellModal::Tx(Box::new(record.clone())));
                cx.notify();
            }))
            .child(
                div()
                    .size(px(38.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .bg(if incoming {
                        theme::accent_soft()
                    } else {
                        theme::surface_hi()
                    })
                    .child(
                        icon(if incoming { Icon::Receive } else { Icon::Send }).text_color(if incoming {
                            theme::accent_hi()
                        } else {
                            theme::text_dim()
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child(tx.tx_type.label()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(fmt_time(tx.created_at_time)),
                    ),
            )
            .child(
                div()
                    .w(px(200.))
                    .flex_none()
                    .child(mono(counterparty).text_xs().text_color(theme::text_dim())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .w(px(220.))
                    .flex_none()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if incoming { theme::accent_hi() } else { theme::text() })
                            .child(amount),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(status_color)
                            .child(dot(status_color).size(px(5.)))
                            .child(status),
                    ),
            )
    }

    fn stat(label: &str, value: SharedString, sub: Option<SharedString>, accent: Rgba) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(rgba(0xffffffa0)).child(label.to_owned()))
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(accent)
                    .child(value),
            )
            .when_some(sub, |d, s| {
                d.child(div().text_xs().text_color(rgba(0xffffff80)).child(s))
            })
    }

    fn overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let id = self.wallet_id;
        let denom = store.denom_for(id);
        let ticker = store.ticker(id);
        let wallet = store.wallet(id).cloned();
        let bal = store.balances.get(&id).cloned().unwrap_or_default();
        let is_did = wallet
            .as_ref()
            .is_some_and(|w| w.wallet_type == WalletType::DistributedId);
        let spendable_wallet = wallet.as_ref().is_some_and(|w| w.wallet_type.is_spendable());
        let pending = bal.pending_delta();
        let pending_text: SharedString = if store.privacy {
            "••••".into()
        } else if pending == 0 {
            "None".into()
        } else {
            let sign = if pending > 0 { "+" } else { "−" };
            format!("{sign}{}", format_amount(pending.unsigned_abs() as u64, denom, 2)).into()
        };
        let big = store.money(bal.confirmed_wallet_balance, denom);
        let (whole, frac): (SharedString, Option<SharedString>) = match big.split_once('.') {
            Some((w, f)) => (w.to_owned().into(), Some(format!(".{f}").into())),
            None => (big.clone(), None),
        };
        let txs: Vec<TransactionRecord> = store
            .txs
            .get(&id)
            .map(|t| t.iter().take(6).cloned().collect())
            .unwrap_or_default();
        let has_txs = !txs.is_empty();

        let hero = div()
            .relative()
            .overflow_hidden()
            .rounded_2xl()
            .p_8()
            .flex()
            .flex_col()
            .gap_6()
            .bg(linear_gradient(
                135.,
                linear_color_stop(gpui::rgb(0x1d7a46), 0.),
                linear_color_stop(gpui::rgb(0x0b3f3b), 1.),
            ))
            .shadow_lg()
            .child(
                div()
                    .absolute()
                    .right(px(-70.))
                    .top(px(-90.))
                    .size(px(300.))
                    .rounded_full()
                    .bg(rgba(0xffffff0c)),
            )
            .child(
                div()
                    .absolute()
                    .right(px(90.))
                    .bottom(px(-140.))
                    .size(px(240.))
                    .rounded_full()
                    .bg(rgba(0xffffff08)),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_start()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(div().text_sm().text_color(rgba(0xffffffb0)).child(if is_did {
                                "Distributed identity"
                            } else {
                                "Total balance"
                            }))
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex()
                                            .items_baseline()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme::rgb_white())
                                            .child(div().text_size(px(48.)).line_height(px(56.)).child(whole))
                                            .when_some(frac, |d, f| {
                                                d.child(div().text_size(px(30.)).text_color(rgba(0xffffff99)).child(f))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgba(0xffffffb0))
                                            .child(ticker.clone()),
                                    ),
                            ),
                    )
                    .when(spendable_wallet, |d| {
                        d.child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .id("hero-send")
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .px_4()
                                        .h(px(38.))
                                        .rounded_lg()
                                        .bg(rgba(0xffffff1f))
                                        .hover(|s| s.bg(rgba(0xffffff33)))
                                        .cursor_pointer()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::rgb_white())
                                        .child(icon(Icon::Send).text_color(theme::rgb_white()))
                                        .child("Send")
                                        .on_click(cx.listener(|this, _, window, cx| this.go(Page::Send, window, cx))),
                                )
                                .child(
                                    div()
                                        .id("hero-receive")
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .px_4()
                                        .h(px(38.))
                                        .rounded_lg()
                                        .bg(theme::rgb_white())
                                        .hover(|s| s.bg(rgba(0xffffffdd)))
                                        .cursor_pointer()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(gpui::rgb(0x0b3f3b))
                                        .child(icon(Icon::Receive).text_color(gpui::rgb(0x0b3f3b)))
                                        .child("Receive")
                                        .on_click(
                                            cx.listener(|this, _, window, cx| this.go(Page::Receive, window, cx)),
                                        ),
                                ),
                        )
                    }),
            )
            .when(!is_did, |d| {
                d.child(
                    div()
                        .flex()
                        .gap_10()
                        .pt_4()
                        .border_t_1()
                        .border_color(rgba(0xffffff1a))
                        .child(Self::stat(
                            "Spendable",
                            store.money(bal.spendable_balance, denom),
                            None,
                            theme::rgb_white(),
                        ))
                        .child(Self::stat("Pending", pending_text, None, theme::rgb_white()))
                        .child(Self::stat(
                            "Max single send",
                            store.money(bal.max_send_amount, denom),
                            None,
                            theme::rgb_white(),
                        ))
                        .child(Self::stat(
                            "Pending change",
                            store.money(bal.pending_change, denom),
                            None,
                            theme::rgb_white(),
                        )),
                )
            });

        let recent_rows: Vec<Stateful<Div>> = txs
            .iter()
            .enumerate()
            .map(|(i, t)| self.tx_row(store, t, i, cx))
            .collect();

        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(hero)
            .child(
                card()
                    .p_4()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .px_2()
                            .pb_1()
                            .child(eyebrow("Recent activity"))
                            .when(has_txs, |d| {
                                d.child(
                                    Button::new("all-activity", "View all").ghost().on_click(
                                        cx.listener(|this, _, window, cx| this.go(Page::Activity, window, cx)),
                                    ),
                                )
                            }),
                    )
                    .when(!has_txs, |d| {
                        d.child(div().p_6().text_sm().text_color(theme::muted()).child(if is_did {
                            "DID wallets are managed from the Chia CLI for now."
                        } else {
                            "No transactions yet."
                        }))
                    })
                    .children(recent_rows),
            )
            .into_any_element()
    }

    fn receive(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let id = self.wallet_id;
        let ticker = store.ticker(id);
        let net = store.network.network_name.clone();
        let address = store.addresses.get(&id).cloned();
        let Some(address) = address else {
            return card()
                .p_8()
                .items_center()
                .child(spinner("addr-spin"))
                .into_any_element();
        };
        let copy_addr = address.clone();
        card()
            .p_8()
            .flex_row()
            .flex_none()
            .items_start()
            .gap_8()
            .max_w(px(820.))
            .child(qr_code(&address, 232.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .flex_1()
                    .min_w_0()
                    .child(eyebrow(format!("Your {ticker} address")))
                    .child(chunked_address(&address, 17.).line_height(px(28.)))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("copy-addr", "Copy address")
                                    .primary()
                                    .icon(Icon::Copy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let a = copy_addr.clone();
                                        this.store.update(cx, |s, cx| s.copy_ephemeral(a, "Address", cx));
                                    })),
                            )
                            .child(
                                Button::new("new-addr", "New address")
                                    .icon(Icon::Refresh)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.store.update(cx, |s, cx| s.new_address(id, cx));
                                    })),
                            ),
                    )
                    .child(callout(
                        Callout::Safe,
                        format!("Only send {ticker} on {net}"),
                        "Use a fresh address for each payer to keep your history private. Old addresses keep working.",
                    )),
            )
            .into_any_element()
    }

    fn activity(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.store.read(cx).txs.get(&self.wallet_id).map_or(0, Vec::len);
        if count == 0 {
            return card()
                .p_8()
                .child(div().text_sm().text_color(theme::muted()).child("No transactions yet."))
                .into_any_element();
        }
        card()
            .p_2()
            .flex_1()
            .min_h(px(200.))
            .child(
                uniform_list(
                    "activity-list",
                    count,
                    cx.processor(|this: &mut Self, range: Range<usize>, _window, cx| {
                        let store = this.store.read(cx);
                        let txs = store
                            .txs
                            .get(&this.wallet_id)
                            .and_then(|t| t.get(range.clone()))
                            .unwrap_or_default();
                        txs.iter()
                            .zip(range)
                            .map(|(t, i)| this.tx_row(store, t, i, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .h_full(),
            )
            .into_any_element()
    }

    // ----- dialogs ------------------------------------------------------------------------------

    fn render_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.modal.as_ref()?;
        let close = cx.listener(|this, _, _, cx| {
            this.modal = None;
            cx.notify();
        });
        let store = self.store.read(cx);
        let row = |label: &str, value: AnyElement| {
            div()
                .flex()
                .justify_between()
                .items_start()
                .gap_6()
                .py_2()
                .border_b_1()
                .border_color(theme::border())
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::muted())
                        .flex_none()
                        .child(label.to_owned()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::text())
                        .flex()
                        .justify_end()
                        .min_w_0()
                        .child(value),
                )
        };
        Some(match m {
            ShellModal::Confirm {
                pending,
                armed,
                submitting,
            } => {
                let p = pending;
                let fmt = |m: u64, d: Denom| format_amount(m, d, 2);
                let fee_ticker = store.network.network_prefix.to_uppercase();
                let remaining = store.balances.get(&p.wallet_id).map(|b| {
                    b.spendable_balance
                        .saturating_sub(p.amount + if p.denom == Denom::Xch { p.fee } else { 0 })
                });
                let net = format!("{} ({})", store.network.network_name, store.network.network_prefix);
                let label = if *submitting {
                    "Sending…"
                } else if *armed {
                    "Confirm & send"
                } else {
                    "Check the details…"
                };
                modal(
                    "confirm-send",
                    520.,
                    cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                        if !matches!(this.modal, Some(ShellModal::Confirm { submitting: true, .. })) {
                            this.modal = None;
                            cx.notify();
                        }
                    }),
                    div()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child("Review transaction"))
                                .child(icon_button("close", Icon::X).on_click(close)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_1()
                                .py_3()
                                .child(div().text_xs().text_color(theme::muted()).child("You are sending"))
                                .child(
                                    div()
                                        .text_size(px(36.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme::text())
                                        .child(format!("{} {}", fmt(p.amount, p.denom), p.ticker)),
                                ),
                        )
                        .child(
                            div()
                                .p_4()
                                .rounded_lg()
                                .bg(theme::bg())
                                .border_1()
                                .border_color(theme::border_hi())
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(eyebrow("To"))
                                .child(chunked_address(&p.address, 15.)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(row("Network", div().child(net).into_any_element()))
                                .child(row("Network fee", div().child(format!("{} {fee_ticker}", fmt(p.fee, Denom::Xch))).into_any_element()))
                                .when_some(remaining, |d, r| {
                                    d.child(row(
                                        "Spendable afterwards",
                                        div().child(format!("{} {}", fmt(r, p.denom), p.ticker)).into_any_element(),
                                    ))
                                }),
                        )
                        .child(callout(
                            Callout::Warning,
                            "Transactions can't be reversed",
                            "Compare the address with the recipient's copy group by group — clipboard malware often swaps the middle and keeps the ends.",
                        ))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(Button::new("cancel-send", "Cancel").ghost().on_click(cx.listener(|this, _, _, cx| {
                                    this.modal = None;
                                    cx.notify();
                                })))
                                .child(
                                    Button::new("confirm-send-btn", label)
                                        .primary()
                                        .large()
                                        .icon(Icon::Check)
                                        .disabled(!*armed || *submitting)
                                        .on_click(cx.listener(|this, _, _, cx| this.confirm_send(cx))),
                                ),
                        ),
                )
            }
            ShellModal::Settings(section) => self.render_settings(*section, cx),
            ShellModal::Tx(tx) => {
                let denom = store.denom_for(tx.wallet_id);
                let ticker = store.ticker(tx.wallet_id);
                let id_copy: SharedString = tx.name.clone().into();
                let status = if let Some(r) = tx.rejection() {
                    format!("Rejected: {r}")
                } else if tx.confirmed {
                    format!("Confirmed at block {}", tx.confirmed_at_height)
                } else {
                    "Pending".into()
                };
                modal(
                    "tx-details",
                    560.,
                    cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                        this.modal = None;
                        cx.notify();
                    }),
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
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
                                        .child(tx.tx_type.label()),
                                )
                                .child(icon_button("close", Icon::X).on_click(close)),
                        )
                        .child(row(
                            "Amount",
                            div()
                                .child(format!("{} {ticker}", store.money(tx.amount, denom)))
                                .into_any_element(),
                        ))
                        .child(row(
                            "Fee",
                            div()
                                .child(format!("{} XCH", store.money(tx.fee_amount, Denom::Xch)))
                                .into_any_element(),
                        ))
                        .child(row("Status", div().child(status).into_any_element()))
                        .child(row(
                            "Created",
                            div().child(fmt_time(tx.created_at_time)).into_any_element(),
                        ))
                        .when_some(tx.to_address.clone(), |d, a| {
                            d.child(row(
                                "Address",
                                chunked_address(&a, 12.).justify_end().into_any_element(),
                            ))
                        })
                        .child(row(
                            "Transaction ID",
                            mono(short_hash(&tx.name)).text_xs().into_any_element(),
                        ))
                        .child(
                            div().flex().justify_end().child(
                                Button::new("copy-tx", "Copy transaction ID")
                                    .icon(Icon::Copy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let id = id_copy.clone();
                                        this.store
                                            .update(cx, |s, cx| s.copy_ephemeral(id, "Transaction ID", cx));
                                    })),
                            ),
                        ),
                )
            }
        })
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.page {
            Page::Overview => self.overview(cx),
            Page::Send => div()
                .flex()
                .flex_col()
                .gap_4()
                .child(self.send.clone())
                .into_any_element(),
            Page::Receive => div().flex().flex_col().child(self.receive(cx)).into_any_element(),
            Page::Activity => self.activity(cx),
        };
        let fill_height = self.page == Page::Activity;
        let content = div()
            .id(("page", self.page as usize))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_8()
            .pt_6()
            .pb_8()
            .when(!fill_height, |d| d.overflow_y_scroll())
            .child(
                div()
                    .flex_none()
                    .pb_5()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text())
                    .child(self.page.title()),
            )
            .child(if fill_height {
                page
            } else {
                div().flex().flex_col().flex_none().child(page).into_any_element()
            });
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .child(self.topbar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(cx))
                    .child(div().flex().flex_col().flex_1().min_w_0().h_full().child(content)),
            )
            .children(self.render_modal(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(fmt_time(0), "Jan 1, 1970 · 00:00 UTC");
        assert_eq!(fmt_time(1_790_000_000), "Sep 21, 2026 · 14:13 UTC");
        assert_eq!(fmt_time(951_782_400), "Feb 29, 2000 · 00:00 UTC");
    }

    #[test]
    fn shortening() {
        assert_eq!(short_hash("0x0123456789abcdef0123"), "01234567…cdef0123");
        assert_eq!(short_addr("xch1abcdefghijklmnopqrstuvwxyz"), "xch1abcdef…uvwxyz");
    }
}
