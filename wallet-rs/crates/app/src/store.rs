//! Application state and the async controller that keeps it fresh.
//!
//! Views never talk to the backend directly: they call methods here, which
//! spawn tasks on GPUI's executor and write results back into the store.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chia_wallet_core::protocol::{
    Event, NetworkInfo, SyncStatus, TransactionRecord, WalletBalance, WalletInfo, WalletType,
};
use chia_wallet_core::secret::{Mnemonic, SecretString};
use chia_wallet_core::units::{Denom, format_amount};
use chia_wallet_core::{KeyOrigin, RpcError, WalletApi};
use futures::StreamExt;
use gpui::{App, AsyncApp, ClipboardItem, Context, SharedString, Task, WeakEntity};

const TX_LIMIT: u32 = 200;
const POLL_EVERY: Duration = Duration::from_secs(20);
const CLIPBOARD_TTL: Duration = Duration::from_secs(45);
const TOAST_TTL: Duration = Duration::from_millis(4500);

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Connecting { detail: SharedString },
    Keys,
    Unlocking(u32),
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
    Info,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: SharedString,
}

/// Facts shown on the Security page.
#[derive(Debug, Clone, Default)]
pub struct SecurityInfo {
    pub endpoint: String,
    pub demo: bool,
    pub remote_allowed: bool,
    pub key_file_warning: Option<String>,
}

pub struct Store {
    api: WalletApi,
    pub security: SecurityInfo,
    pub phase: Phase,
    pub connected: bool,
    pub disconnect_reason: Option<SharedString>,
    pub fingerprints: Vec<u32>,
    pub fingerprint: Option<u32>,
    pub network: NetworkInfo,
    pub sync: SyncStatus,
    pub height: u32,
    pub peers: usize,
    pub wallets: Vec<WalletInfo>,
    pub balances: HashMap<u32, WalletBalance>,
    pub txs: HashMap<u32, Vec<TransactionRecord>>,
    pub addresses: HashMap<u32, SharedString>,
    pub privacy: bool,
    pub auto_privacy: bool,
    pub toasts: Vec<Toast>,
    toast_seq: u64,
    dirty: HashSet<u32>,
    flush_scheduled: bool,
    last_height_fetch: Option<Instant>,
    bootstrapping: bool,
    _events: Option<Task<()>>,
    _poller: Option<Task<()>>,
}

fn err_text(e: &RpcError) -> SharedString {
    SharedString::from(e.to_string())
}

impl Store {
    pub fn new(api: WalletApi, security: SecurityInfo, default_prefix: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            api,
            security,
            phase: Phase::Connecting {
                detail: "Connecting to the Chia daemon…".into(),
            },
            connected: false,
            disconnect_reason: None,
            fingerprints: Vec::new(),
            fingerprint: None,
            network: NetworkInfo {
                network_name: String::new(),
                network_prefix: default_prefix,
            },
            sync: SyncStatus::default(),
            height: 0,
            peers: 0,
            wallets: Vec::new(),
            balances: HashMap::new(),
            txs: HashMap::new(),
            addresses: HashMap::new(),
            privacy: false,
            auto_privacy: true,
            toasts: Vec::new(),
            toast_seq: 0,
            dirty: HashSet::new(),
            flush_scheduled: false,
            last_height_fetch: None,
            bootstrapping: false,
            _events: None,
            _poller: None,
        };
        this.listen(cx);
        this
    }

    // ----- helpers for views ---------------------------------------------------------

    pub fn denom_for(&self, wallet_id: u32) -> Denom {
        match self.wallets.iter().find(|w| w.id == wallet_id).map(|w| w.wallet_type) {
            Some(WalletType::ColouredCoin) => Denom::ColouredCoin,
            _ => Denom::Xch,
        }
    }

    pub fn wallet(&self, wallet_id: u32) -> Option<&WalletInfo> {
        self.wallets.iter().find(|w| w.id == wallet_id)
    }

    pub fn ticker(&self, wallet_id: u32) -> SharedString {
        match self.wallet(wallet_id) {
            Some(w) if w.wallet_type == WalletType::ColouredCoin => "CC".into(),
            _ => self.network.network_prefix.to_uppercase().into(),
        }
    }

    /// Format an amount, or bullets in privacy mode.
    pub fn money(&self, mojos: u64, denom: Denom) -> SharedString {
        if self.privacy {
            "••••••".into()
        } else {
            format_amount(mojos, denom, 2).into()
        }
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast_seq += 1;
        let id = self.toast_seq;
        self.toasts.push(Toast {
            id,
            kind,
            text: text.into(),
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(TOAST_TTL).await;
            this.update(cx, |this, cx| {
                this.toasts.retain(|t| t.id != id);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toasts.retain(|t| t.id != id);
        cx.notify();
    }

    pub fn set_privacy(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.privacy != on {
            self.privacy = on;
            cx.notify();
        }
    }

    /// Copy to the clipboard, then wipe it after a while if it still holds our text.
    pub fn copy_ephemeral(&mut self, text: SharedString, what: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        self.toast(
            ToastKind::Info,
            format!("{what} copied · clipboard clears in {}s", CLIPBOARD_TTL.as_secs()),
            cx,
        );
        cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(CLIPBOARD_TTL).await;
            cx.update(|cx: &mut App| {
                let still_ours = cx
                    .read_from_clipboard()
                    .and_then(|c| c.text())
                    .is_some_and(|t| t == text.as_ref());
                if still_ours {
                    cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
                }
            })
            .ok();
        })
        .detach();
    }

    // ----- connection lifecycle -------------------------------------------------------

    fn listen(&mut self, cx: &mut Context<Self>) {
        let Some(mut events) = self.api.transport().take_events() else {
            return;
        };
        self._events = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Some(ev) = events.next().await {
                if this.update(cx, |this, cx| this.on_event(ev, cx)).is_err() {
                    break;
                }
            }
        }));
        self._poller = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(POLL_EVERY).await;
                let alive = this.update(cx, |this, cx| {
                    if this.phase == Phase::Ready && this.connected {
                        this.refresh_all(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    fn on_event(&mut self, ev: Event, cx: &mut Context<Self>) {
        match ev {
            Event::Connected => {
                self.connected = true;
                self.disconnect_reason = None;
                self.bootstrap(cx);
            }
            Event::Disconnected { reason } => {
                self.connected = false;
                self.disconnect_reason = Some(reason.clone().into());
                if !matches!(self.phase, Phase::Ready) {
                    self.phase = Phase::Connecting { detail: reason.into() };
                }
            }
            Event::Connections { count } => self.peers = count,
            Event::StateChanged {
                state,
                wallet_id,
                additional_data,
            } => match state.as_str() {
                "coin_added" | "coin_removed" | "pending_transaction" => {
                    if let Some(id) = wallet_id {
                        self.mark_dirty(id, cx);
                    }
                }
                "tx_update" => {
                    if let Some(tx) = additional_data
                        .get("transaction")
                        .and_then(|t| serde_json::from_value::<TransactionRecord>(t.clone()).ok())
                    {
                        if let Some(reason) = tx.rejection() {
                            self.toast(
                                ToastKind::Error,
                                format!("Transaction rejected by the network: {reason}"),
                                cx,
                            );
                        }
                        self.mark_dirty(tx.wallet_id, cx);
                    }
                }
                "sync_changed" => self.refresh_sync(cx),
                "new_block" | "new_peak" => {
                    let due = self
                        .last_height_fetch
                        .is_none_or(|t| t.elapsed() > Duration::from_secs(2));
                    if due {
                        self.last_height_fetch = Some(Instant::now());
                        self.refresh_sync(cx);
                    }
                }
                _ => {}
            },
        }
        cx.notify();
    }

    /// Start the wallet service, wait for it to answer, then list keys (or
    /// silently resume the session after a reconnect).
    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        if self.bootstrapping {
            return;
        }
        self.bootstrapping = true;
        let resume = self.fingerprint.filter(|_| self.phase == Phase::Ready);
        if resume.is_none() {
            self.phase = Phase::Connecting {
                detail: "Starting the wallet service…".into(),
            };
        }
        let api = self.api.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let started = api.start_wallet_service().await;
            let mut ready = false;
            for _ in 0..90 {
                if api.ping_wallet().await.is_ok() {
                    ready = true;
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
            let keys = if ready {
                api.public_keys().await
            } else {
                Err(RpcError::Timeout)
            };
            let resumed = match (resume, &keys) {
                (Some(fp), Ok(_)) => Some(api.log_in(fp).await),
                _ => None,
            };
            this.update(cx, |this, cx| {
                this.bootstrapping = false;
                match keys {
                    Ok(fps) => {
                        this.fingerprints = fps;
                        match resumed {
                            Some(Ok(_)) => this.refresh_all(cx),
                            Some(Err(e)) => {
                                this.toast(ToastKind::Error, format!("Could not resume session: {e}"), cx);
                                this.lock(cx);
                            }
                            None => this.phase = Phase::Keys,
                        }
                    }
                    Err(e) => {
                        let hint = match started {
                            Err(s) => format!("Wallet service did not start: {s}"),
                            Ok(()) => format!("Wallet service is not answering: {e}"),
                        };
                        this.phase = Phase::Connecting { detail: hint.into() };
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- keys -------------------------------------------------------------------------

    pub fn refresh_keys(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let r = api.public_keys().await;
            this.update(cx, |this, cx| match r {
                Ok(fps) => {
                    this.fingerprints = fps;
                    cx.notify();
                }
                Err(e) => this.toast(ToastKind::Error, err_text(&e), cx),
            })
            .ok();
        })
        .detach();
    }

    pub fn log_in(&mut self, fingerprint: u32, cx: &mut Context<Self>) {
        self.phase = Phase::Unlocking(fingerprint);
        cx.notify();
        let api = self.api.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let r = api.log_in(fingerprint).await;
            this.update(cx, |this, cx| this.finish_login(r, cx)).ok();
        })
        .detach();
    }

    fn finish_login(&mut self, r: Result<u32, RpcError>, cx: &mut Context<Self>) {
        match r {
            Ok(fp) => {
                self.fingerprint = Some(fp);
                self.wallets.clear();
                self.balances.clear();
                self.txs.clear();
                self.addresses.clear();
                self.phase = Phase::Ready;
                self.refresh_all(cx);
            }
            Err(e) => {
                self.phase = Phase::Keys;
                self.toast(ToastKind::Error, format!("Could not unlock key: {e}"), cx);
            }
        }
        cx.notify();
    }

    /// Return to the key picker and forget cached wallet data. The daemon has
    /// no password, so this is a UI lock; the Security page says so.
    pub fn lock(&mut self, cx: &mut Context<Self>) {
        self.fingerprint = None;
        self.wallets.clear();
        self.balances.clear();
        self.txs.clear();
        self.addresses.clear();
        self.phase = Phase::Keys;
        self.refresh_keys(cx);
        cx.notify();
    }

    pub fn generate_mnemonic(&self, cx: &mut Context<Self>) -> Task<Result<Mnemonic, RpcError>> {
        let api = self.api.clone();
        cx.background_executor()
            .spawn(async move { api.generate_mnemonic().await })
    }

    pub fn add_key(&mut self, mnemonic: Mnemonic, origin: KeyOrigin, cx: &mut Context<Self>) {
        self.phase = Phase::Unlocking(0);
        cx.notify();
        let api = self.api.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let r = api.add_key(&mnemonic, origin).await;
            drop(mnemonic);
            // add_key starts the wallet for the new key; log_in is then a no-op.
            let r = match r {
                Ok(fp) => api.log_in(fp).await,
                Err(e) => Err(e),
            };
            this.update(cx, |this, cx| {
                if r.is_ok() {
                    this.refresh_keys(cx);
                }
                this.finish_login(r, cx)
            })
            .ok();
        })
        .detach();
    }

    pub fn delete_key(&mut self, fingerprint: u32, cx: &mut Context<Self>) {
        let api = self.api.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let r = api.delete_key(fingerprint).await;
            this.update(cx, |this, cx| {
                match r {
                    Ok(()) => {
                        this.toast(
                            ToastKind::Success,
                            format!("Key {fingerprint} deleted from the keychain"),
                            cx,
                        );
                        if this.fingerprint == Some(fingerprint) {
                            this.lock(cx);
                        }
                    }
                    Err(e) => this.toast(ToastKind::Error, err_text(&e), cx),
                }
                this.refresh_keys(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn recovery_phrase(
        &self,
        fingerprint: u32,
        cx: &mut Context<Self>,
    ) -> Task<Result<Option<SecretString>, RpcError>> {
        let api = self.api.clone();
        cx.background_executor()
            .spawn(async move { api.recovery_phrase(fingerprint).await })
    }

    // ----- wallet data --------------------------------------------------------------------

    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let net = api.network_info().await;
            let wallets = api.wallets().await;
            this.update(cx, |this, cx| {
                if let Ok(n) = net {
                    this.network = n;
                }
                match wallets {
                    Ok(ws) => {
                        let ids: Vec<u32> = ws.iter().map(|w| w.id).collect();
                        this.wallets = ws;
                        for id in ids {
                            this.refresh_wallet(id, cx);
                        }
                    }
                    Err(e) => this.toast(ToastKind::Error, format!("Could not load wallets: {e}"), cx),
                }
                this.refresh_sync(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_sync(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (sync, height, peers) = futures::join!(api.sync_status(), api.height(), api.connection_count());
            this.update(cx, |this, cx| {
                if let Ok(s) = sync {
                    this.sync = s;
                }
                if let Ok(h) = height {
                    this.height = h;
                }
                if let Ok(p) = peers {
                    this.peers = p;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Coalesce bursts of coin events into one refresh per wallet.
    fn mark_dirty(&mut self, wallet_id: u32, cx: &mut Context<Self>) {
        self.dirty.insert(wallet_id);
        if self.flush_scheduled {
            return;
        }
        self.flush_scheduled = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            this.update(cx, |this, cx| {
                this.flush_scheduled = false;
                let ids: Vec<u32> = this.dirty.drain().collect();
                for id in ids {
                    this.refresh_wallet(id, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh_wallet(&mut self, wallet_id: u32, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let want_address = !self.addresses.contains_key(&wallet_id)
            && self.wallet(wallet_id).is_some_and(|w| w.wallet_type.is_spendable());
        let is_cc = self
            .wallet(wallet_id)
            .is_some_and(|w| w.wallet_type == WalletType::ColouredCoin);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (balance, txs) = futures::join!(api.balance(wallet_id), api.transactions(wallet_id, TX_LIMIT));
            let address = if want_address {
                Some(api.next_address(wallet_id, false).await)
            } else {
                None
            };
            let cc_name = if is_cc { api.cc_name(wallet_id).await.ok() } else { None };
            this.update(cx, |this, cx| {
                if let Ok(b) = balance {
                    this.balances.insert(wallet_id, b);
                }
                if let Ok(t) = txs {
                    this.txs.insert(wallet_id, t);
                }
                if let Some(Ok(a)) = address {
                    this.addresses.insert(wallet_id, a.into());
                }
                if let Some(name) = cc_name
                    && let Some(w) = this.wallets.iter_mut().find(|w| w.id == wallet_id)
                {
                    w.name = name;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn new_address(&mut self, wallet_id: u32, cx: &mut Context<Self>) {
        let api = self.api.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let r = api.next_address(wallet_id, true).await;
            this.update(cx, |this, cx| match r {
                Ok(a) => {
                    this.addresses.insert(wallet_id, a.into());
                    this.toast(ToastKind::Success, "Fresh receive address generated", cx);
                }
                Err(e) => this.toast(ToastKind::Error, err_text(&e), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Submit a spend. The returned task resolves when the wallet accepted it.
    pub fn send(
        &mut self,
        wallet_id: u32,
        address: String,
        amount: u64,
        fee: u64,
        cx: &mut Context<Self>,
    ) -> Task<Result<TransactionRecord, RpcError>> {
        let api = self.api.clone();
        let prefix = self.network.network_prefix.clone();
        let is_cc = self.denom_for(wallet_id) == Denom::ColouredCoin;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let r = if is_cc {
                api.send_cc(wallet_id, &address, amount, &prefix).await
            } else {
                api.send_xch(wallet_id, &address, amount, fee, &prefix).await
            };
            this.update(cx, |this, cx| {
                match &r {
                    Ok(tx) => {
                        let short = tx.name.trim_start_matches("0x").chars().take(10).collect::<String>();
                        this.toast(ToastKind::Success, format!("Transaction {short}… submitted"), cx);
                    }
                    Err(e) => this.toast(ToastKind::Error, format!("Send failed: {e}"), cx),
                }
                this.refresh_wallet(wallet_id, cx);
            })
            .ok();
            r
        })
    }
}
