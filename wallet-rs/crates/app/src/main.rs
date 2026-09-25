//! Native Chia wallet.
//!
//! ```text
//! chia-wallet                      connect to the local daemon (~/.chia/mainnet or $CHIA_ROOT)
//! chia-wallet --root <dir>         use another Chia root
//! chia-wallet --demo               explore with a simulated wallet, no daemon, no funds
//! chia-wallet --allow-remote-daemon   permit a non-loopback ui.daemon_host
//! chia-wallet --no-auto-hide       don't hide balances on blur/idle
//! ```

mod assets;
mod keys;
mod qr;
mod root;
mod send;
mod shell;
mod store;
mod text_input;
mod theme;
mod widgets;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chia_wallet_core::client::{BoxFut, RpcError, Transport};
use chia_wallet_core::config::{DaemonConfig, default_root, key_file_warning};
use chia_wallet_core::protocol::Event;
use chia_wallet_core::{DaemonClient, DemoTransport, WalletApi};
use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions, point, px,
    size,
};
use zeroize::Zeroizing;

use crate::store::{SecurityInfo, Store};
use crate::text_input as ti;

actions!(app, [Quit]);

struct Args {
    demo: bool,
    root: Option<PathBuf>,
    allow_remote: bool,
    no_auto_hide: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        demo: false,
        root: None,
        allow_remote: false,
        no_auto_hide: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--demo" => args.demo = true,
            "--root" => args.root = it.next().map(PathBuf::from),
            "--allow-remote-daemon" => args.allow_remote = true,
            "--no-auto-hide" => args.no_auto_hide = true,
            "-h" | "--help" => {
                println!(
                    "{}",
                    include_str!("main.rs")
                        .lines()
                        .skip(3)
                        .take(6)
                        .map(|l| l.trim_start_matches("//! "))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument `{other}` (try --help)");
                std::process::exit(2);
            }
        }
    }
    args
}

/// Stand-in used when the Chia config can't be loaded; the UI shows the error.
struct Offline;

impl Transport for Offline {
    fn call(&self, _: &str, _: &str, _: serde_json::Value, _: Duration) -> BoxFut<Result<serde_json::Value, RpcError>> {
        Box::pin(async { Err(RpcError::NotConnected) })
    }
    fn call_sensitive(
        &self,
        _: &str,
        _: &str,
        _: Zeroizing<String>,
        _: Duration,
    ) -> BoxFut<Result<Zeroizing<String>, RpcError>> {
        Box::pin(async { Err(RpcError::NotConnected) })
    }
    fn take_events(&self) -> Option<futures::channel::mpsc::UnboundedReceiver<Event>> {
        None
    }
    fn describe(&self) -> String {
        "offline".into()
    }
}

struct Backend {
    transport: Arc<dyn Transport>,
    security: SecurityInfo,
    prefix: String,
    fatal: Option<String>,
}

fn backend(args: &Args) -> Backend {
    if args.demo {
        let t = DemoTransport::new(Duration::from_millis(90));
        let security = SecurityInfo {
            endpoint: t.describe(),
            demo: true,
            ..Default::default()
        };
        return Backend {
            transport: t,
            security,
            prefix: "xch".into(),
            fatal: None,
        };
    }
    let setup = (|| -> Result<(Arc<DaemonClient>, DaemonConfig), String> {
        let root = match &args.root {
            Some(r) => r.clone(),
            None => default_root().map_err(|e| e.to_string())?,
        };
        let cfg = DaemonConfig::load(&root, args.allow_remote).map_err(|e| e.to_string())?;
        let client = DaemonClient::start(&cfg).map_err(|e| e.to_string())?;
        Ok((client, cfg))
    })();
    match setup {
        Ok((client, cfg)) => {
            let security = SecurityInfo {
                endpoint: client.describe(),
                demo: false,
                remote_allowed: args.allow_remote,
                key_file_warning: key_file_warning(&cfg.client_key),
            };
            if let Some(w) = &security.key_file_warning {
                log::warn!("{w}");
            }
            Backend {
                transport: client,
                security,
                prefix: cfg.address_prefix,
                fatal: None,
            }
        }
        Err(e) => Backend {
            transport: Arc::new(Offline),
            security: SecurityInfo {
                endpoint: "not configured".into(),
                ..Default::default()
            },
            prefix: "xch".into(),
            fatal: Some(e),
        },
    }
}

fn bind_keys(cx: &mut App) {
    let ctx = Some(ti::CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", ti::Backspace, ctx),
        KeyBinding::new("delete", ti::Delete, ctx),
        KeyBinding::new("left", ti::Left, ctx),
        KeyBinding::new("right", ti::Right, ctx),
        KeyBinding::new("shift-left", ti::SelectLeft, ctx),
        KeyBinding::new("shift-right", ti::SelectRight, ctx),
        KeyBinding::new("secondary-a", ti::SelectAll, ctx),
        KeyBinding::new("home", ti::Home, ctx),
        KeyBinding::new("end", ti::End, ctx),
        KeyBinding::new("secondary-v", ti::Paste, ctx),
        KeyBinding::new("secondary-c", ti::Copy, ctx),
        KeyBinding::new("secondary-x", ti::Cut, ctx),
        KeyBinding::new("enter", ti::Enter, ctx),
        KeyBinding::new("tab", ti::Tab, ctx),
        KeyBinding::new("shift-tab", ti::TabPrev, ctx),
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("escape", widgets::Dismiss, None),
        KeyBinding::new("secondary-,", widgets::OpenSettings, None),
    ]);
}

fn main() {
    // Warnings only by default; payloads are never logged at any level.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = parse_args();
    let Backend {
        transport,
        security,
        prefix,
        fatal,
    } = backend(&args);
    let auto_hide = !args.no_auto_hide;

    Application::new().with_assets(assets::Assets).run(move |cx: &mut App| {
        let fonts = assets::FONTS.iter().map(|f| Cow::Borrowed(*f)).collect();
        if let Err(e) = cx.text_system().add_fonts(fonts) {
            log::warn!("could not load bundled fonts: {e}");
        }
        bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Chia Wallet".into()),
                appears_transparent: false,
                traffic_light_position: Some(point(px(12.), px(12.))),
            }),
            window_min_size: Some(size(px(980.), px(640.))),
            app_id: Some("net.chia.wallet-native".into()),
            ..Default::default()
        };
        let api = WalletApi::new(transport.clone());
        let opened = cx.open_window(options, move |window, cx| {
            let store = cx.new(|cx| {
                let mut s = Store::new(api, security, prefix, cx);
                s.auto_privacy = auto_hide;
                s
            });
            cx.new(|cx| root::WalletApp::new(store, fatal, window, cx))
        });
        if let Err(e) = opened {
            eprintln!("could not open a window: {e}");
            cx.quit();
        }
        cx.activate(true);
    });
}
