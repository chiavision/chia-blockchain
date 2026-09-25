//! The settings dialog: a sectioned modal with its own scrolling body.

use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, FontWeight, MouseDownEvent, SharedString, div, linear_color_stop, linear_gradient, px,
};

use super::{Shell, ShellModal, group};
use crate::assets::Icon;
use crate::theme;
use crate::widgets::{Button, Callout, backdrop, callout, dot, eyebrow, icon, icon_button, mono, switch};

#[cfg(target_os = "macos")]
pub const SHORTCUT_HINT: &str = "⌘ ,";
#[cfg(not(target_os = "macos"))]
pub const SHORTCUT_HINT: &str = "Ctrl ,";

const WIDTH: f32 = 820.;
const HEIGHT: f32 = 580.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    General,
    Security,
    Connection,
    About,
}

impl Section {
    const ALL: [Section; 4] = [Section::General, Section::Security, Section::Connection, Section::About];

    fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Security => "Security",
            Section::Connection => "Connection",
            Section::About => "About",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Section::General => "Privacy and your session",
            Section::Security => "What protects your keys and funds",
            Section::Connection => "Daemon, network and sync",
            Section::About => "Version, shortcuts and known limits",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Section::General => Icon::Settings,
            Section::Security => Icon::Shield,
            Section::Connection => Icon::Wifi,
            Section::About => Icon::Info,
        }
    }
}

/// A titled group of rows inside one rounded card.
fn section_group(title: &str, rows: Vec<AnyElement>) -> Div {
    div().flex().flex_col().gap_2().child(eyebrow(title.to_owned())).child(
        div()
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .children(rows.into_iter().enumerate().map(|(i, r)| {
                div()
                    .when(i > 0, |d| d.border_t_1().border_color(theme::border()))
                    .child(r)
            })),
    )
}

/// Title + description on the left, an optional control on the right.
fn row(title: impl Into<SharedString>, desc: impl Into<SharedString>, trailing: Option<AnyElement>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_4()
        .px_4()
        .py_3()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(title.into()),
                )
                .child(div().text_xs().text_color(theme::text_dim()).child(desc.into())),
        )
        .children(trailing)
        .into_any_element()
}

/// A protection that is (or isn't) in effect.
fn check(ok: bool, title: &str, desc: &str) -> AnyElement {
    div()
        .flex()
        .gap_3()
        .px_4()
        .py_3()
        .child(
            icon(if ok { Icon::Check } else { Icon::Alert })
                .text_color(if ok { theme::accent_hi() } else { theme::warning() })
                .mt(px(1.)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(title.to_owned()),
                )
                .child(div().text_xs().text_color(theme::text_dim()).child(desc.to_owned())),
        )
        .into_any_element()
}

/// Label on the left, value on the right.
fn value_row(label: &str, value: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_6()
        .px_4()
        .h(px(44.))
        .child(
            div()
                .text_sm()
                .text_color(theme::text_dim())
                .flex_none()
                .child(label.to_owned()),
        )
        .child(div().min_w_0().text_sm().text_color(theme::text()).child(value))
        .into_any_element()
}

fn text_block(text: &str) -> AnyElement {
    div()
        .px_4()
        .py_3()
        .text_sm()
        .text_color(theme::text_dim())
        .child(text.to_owned())
        .into_any_element()
}

fn keycap(k: &str) -> Div {
    div()
        .px_2()
        .py(px(1.))
        .rounded_md()
        .border_1()
        .border_color(theme::border_hi())
        .bg(theme::bg())
        .font_family(theme::MONO)
        .text_xs()
        .text_color(theme::text())
        .child(k.to_owned())
}

impl Shell {
    fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if let Some(ShellModal::Settings(s)) = &mut self.modal {
            *s = section;
            cx.notify();
        }
    }

    pub(super) fn render_settings(&self, section: Section, cx: &Context<Self>) -> AnyElement {
        let rail = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(208.))
            .h_full()
            .p_3()
            .gap_1()
            .bg(theme::sidebar())
            .border_r_1()
            .border_color(theme::border())
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_3()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child("Settings"),
            )
            .children(Section::ALL.into_iter().map(|s| {
                let active = s == section;
                div()
                    .id(SharedString::from(format!("settings-{s:?}")))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .h(px(36.))
                    .rounded_lg()
                    .text_sm()
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme::surface_hi()).text_color(theme::text()))
                    .when(!active, |d| {
                        d.text_color(theme::text_dim())
                            .hover(|st| st.bg(theme::surface_hover()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_section(s, cx)))
                    .child(icon(s.icon()).text_color(if active { theme::accent_hi() } else { theme::text_dim() }))
                    .child(s.title())
            }))
            .child(div().flex_1())
            .child(
                div()
                    .px_3()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(format!("Chia Wallet {}", env!("CARGO_PKG_VERSION"))),
            );

        let body = match section {
            Section::General => self.settings_general(cx),
            Section::Security => self.settings_security(cx),
            Section::Connection => self.settings_connection(cx),
            Section::About => self.settings_about(),
        };

        let panel = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_6()
                    .h(px(64.))
                    .flex_none()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child(section.title()),
                            )
                            .child(div().text_xs().text_color(theme::muted()).child(section.subtitle())),
                    )
                    .child(
                        icon_button("settings-close", Icon::X).on_click(cx.listener(|this, _, _, cx| {
                            this.modal = None;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                // Its own scroll state per section, so switching starts at the top.
                div()
                    .id(("settings-body", section as usize))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(div().flex().flex_col().flex_none().gap_6().p_6().child(body)),
            );

        backdrop(
            "settings",
            div()
                .id("settings-dialog")
                .w(px(WIDTH))
                .h(px(HEIGHT))
                .max_w_full()
                .max_h_full()
                .flex()
                .overflow_hidden()
                .rounded_xl()
                .border_1()
                .border_color(theme::border_hi())
                .bg(theme::surface_hi())
                .shadow_lg()
                .child(rail)
                .child(panel),
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.modal = None;
                cx.notify();
            }),
        )
    }

    fn settings_general(&self, cx: &Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let (privacy, auto) = (store.privacy, store.auto_privacy);
        let fingerprint = store.fingerprint.map(|f| f.to_string()).unwrap_or_else(|| "—".into());
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(section_group(
                "Privacy",
                vec![
                    row(
                        "Hide balances",
                        "Replace every amount with dots. The eye in the header toggles this too.",
                        Some(
                            switch("sw-privacy", privacy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.store.update(cx, |s, cx| s.set_privacy(!privacy, cx))
                                }))
                                .into_any_element(),
                        ),
                    ),
                    row(
                        "Auto-hide balances",
                        "Hide amounts when the window loses focus or after 2 minutes idle.",
                        Some(
                            switch("sw-auto", auto)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.store.update(cx, |s, cx| {
                                        s.auto_privacy = !auto;
                                        cx.notify();
                                    })
                                }))
                                .into_any_element(),
                        ),
                    ),
                ],
            ))
            .child(section_group(
                "Session",
                vec![
                    row(
                        format!("Unlocked key {fingerprint}"),
                        "Locking returns to the key picker and clears cached balances and history from this window.",
                        Some(
                            Button::new("settings-lock", "Lock")
                                .icon(Icon::Lock)
                                .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.lock(cx))))
                                .into_any_element(),
                        ),
                    ),
                    row(
                        "Clipboard",
                        "Copied addresses and transaction IDs are wiped from the clipboard after 45 seconds.",
                        None,
                    ),
                ],
            ))
            .into_any_element()
    }

    fn settings_security(&self, cx: &Context<Self>) -> AnyElement {
        let sec = self.store.read(cx).security.clone();
        let mut warnings: Vec<AnyElement> = Vec::new();
        if let Some(w) = &sec.key_file_warning {
            warnings.push(callout(Callout::Warning, "Loose key file permissions", w.clone()).into_any_element());
        }
        if sec.remote_allowed {
            warnings.push(
                callout(
                    Callout::Warning,
                    "Remote daemon allowed",
                    "--allow-remote-daemon is set, so the daemon may be on another machine. Make sure you control it.",
                )
                .into_any_element(),
            );
        }
        let transport = if sec.demo {
            vec![check(
                true,
                "Demo backend",
                "Running against an in-process simulator. Nothing leaves this process.",
            )]
        } else {
            vec![
                check(
                    true,
                    "Daemon identity is verified",
                    "The daemon's certificate must chain to your private CA (private_ca.crt). The Electron wallet accepted any certificate.",
                ),
                check(
                    true,
                    "Mutual TLS",
                    "This app proves itself to the daemon with your private_daemon certificate.",
                ),
                check(
                    !sec.remote_allowed,
                    "Local daemon only",
                    "Non-loopback daemon hosts are refused unless you pass --allow-remote-daemon.",
                ),
            ]
        };
        div()
            .flex()
            .flex_col()
            .gap_6()
            .children(warnings)
            .child(section_group("Connection", transport))
            .child(section_group(
                "Sending",
                vec![
                    check(
                        true,
                        "Strict address checks",
                        "bech32m checksum, this network's prefix and a 32-byte payload — checked in the form and again right before sending.",
                    ),
                    check(true, "Exact amounts", "Integer mojo end to end. No floating point, no rounding; overflow is an error."),
                    check(
                        true,
                        "Deliberate sending",
                        "Every spend gets a full review, and the confirm button arms after 1.5 s to stop double-clicks and click-jacking.",
                    ),
                ],
            ))
            .child(section_group(
                "Secrets",
                vec![
                    check(
                        true,
                        "Recovery phrases stay put",
                        "Held in memory that is wiped on drop, masked by default, never copyable and never logged.",
                    ),
                    check(
                        true,
                        "No backup-server lookups",
                        "Keys unlock in skip mode, so nothing derived from your key is sent to backup.chia.net.",
                    ),
                    check(
                        true,
                        "No web engine",
                        "No JavaScript, no remote content, no nodeIntegration. Every icon and font is compiled into the binary.",
                    ),
                ],
            ))
            .into_any_element()
    }

    fn settings_connection(&self, cx: &Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let (status_color, status): (_, SharedString) = if store.connected {
            (theme::accent_hi(), "Connected".into())
        } else {
            (
                theme::danger(),
                store.disconnect_reason.clone().unwrap_or_else(|| "Offline".into()),
            )
        };
        let sync = if store.sync.synced {
            "Synced"
        } else if store.sync.syncing {
            "Syncing…"
        } else {
            "Not synced"
        };
        let network = format!("{} ({})", store.network.network_name, store.network.network_prefix);
        let transport = if store.security.demo {
            "In-process simulator"
        } else {
            "Mutual TLS · pinned private CA"
        };
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(section_group(
                "Daemon",
                vec![
                    value_row("Endpoint", mono(store.security.endpoint.clone()).text_xs().truncate()),
                    value_row("Transport", transport),
                    value_row(
                        "Status",
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(dot(status_color))
                            .child(div().truncate().child(status)),
                    ),
                ],
            ))
            .child(section_group(
                "Wallet",
                vec![
                    value_row("Network", network),
                    value_row("Sync", sync),
                    value_row("Block height", group(u64::from(store.height))),
                    value_row("Peers", store.peers.to_string()),
                ],
            ))
            .child(
                div().flex().child(
                    Button::new("settings-refresh", "Refresh now")
                        .icon(Icon::Refresh)
                        .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.refresh_all(cx)))),
                ),
            )
            .into_any_element()
    }

    fn settings_about(&self) -> AnyElement {
        let shortcut = |keys: &[&str], what: &str| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_4()
                .h(px(40.))
                .child(div().text_sm().text_color(theme::text_dim()).child(what.to_owned()))
                .child(div().flex().gap_1().children(keys.iter().map(|k| keycap(k))))
                .into_any_element()
        };
        #[cfg(target_os = "macos")]
        let modk = "⌘";
        #[cfg(not(target_os = "macos"))]
        let modk = "Ctrl";
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .size(px(48.))
                            .rounded_xl()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(linear_gradient(
                                135.,
                                linear_color_stop(theme::accent_hi(), 0.),
                                linear_color_stop(theme::teal(), 1.),
                            ))
                            .child(icon(Icon::Leaf).size(px(26.)).text_color(theme::rgb_white())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child("Chia Wallet"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::muted())
                                    .child(format!("Version {} · Native · Rust · GPUI", env!("CARGO_PKG_VERSION"))),
                            ),
                    ),
            )
            .child(section_group(
                "Keyboard",
                vec![
                    shortcut(&[modk, ","], "Open settings"),
                    shortcut(&["Esc"], "Close a dialog"),
                    shortcut(&["Tab"], "Next field"),
                    shortcut(&["Enter"], "Review / submit"),
                ],
            ))
            .child(section_group(
                "Honest limits",
                vec![
                    text_block(
                        "Locking returns to the key picker, but the daemon has no password: it keeps the key loaded until it stops. Stop the daemon for a hard lock.",
                    ),
                    text_block(
                        "Keys ultimately live in the Chia daemon's keychain. On Linux this chia version encrypts it with a fixed password (chia/util/keychain.py), so protect your user account and disk.",
                    ),
                    text_block(
                        "Words you reveal are copied into the text renderer while on screen. Wiping shortens how long secrets stay in memory; it cannot eliminate every copy.",
                    ),
                ],
            ))
            .into_any_element()
    }
}
