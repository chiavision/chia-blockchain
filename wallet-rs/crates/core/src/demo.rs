//! An in-process stand-in for the Chia daemon and wallet, for `--demo` mode,
//! UI development and tests. It speaks the same command names and JSON shapes
//! as `chia/rpc/wallet_rpc_api.py`, simulates block production, confirms sent
//! transactions a couple of blocks later and emits `state_changed` pushes.
//! Nothing here touches the network or disk.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::channel::{mpsc as fmpsc, oneshot};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::address::{decode_address, encode_puzzle_hash};
use crate::bip39;
use crate::client::{BoxFut, RpcError, Transport, check_raw_success, data_to_result};
use crate::protocol::{Coin, Event, TransactionRecord, TxType, WalletInfo, WalletType};
use crate::secret::Mnemonic;
use crate::units::{MOJO_PER_CC, MOJO_PER_XCH};

const BLOCK_INTERVAL: Duration = Duration::from_secs(6);
const CONFIRMATIONS: u32 = 2;

struct DemoWallet {
    info: WalletInfo,
    confirmed: u64,
    unconfirmed: u64,
    txs: Vec<TransactionRecord>,
    address_index: u32,
}

struct State {
    prefix: String,
    keys: Vec<(u32, Mnemonic)>,
    logged_in: Option<u32>,
    login_height: u32,
    wallets: HashMap<u32, DemoWallet>,
    height: u32,
    peers: usize,
    events: fmpsc::UnboundedSender<Event>,
}

/// The demo backend. Cheap to clone via `Arc`.
pub struct DemoTransport {
    state: Arc<Mutex<State>>,
    events: Mutex<Option<fmpsc::UnboundedReceiver<Event>>>,
    latency: Duration,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hash32(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn hex0x(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

impl DemoTransport {
    /// `latency` simulates daemon round-trips; use zero in tests.
    pub fn new(latency: Duration) -> Arc<Self> {
        let (tx, rx) = fmpsc::unbounded();
        let mut state = State {
            prefix: "xch".into(),
            keys: Vec::new(),
            logged_in: None,
            login_height: 0,
            wallets: HashMap::new(),
            height: 1_204_512,
            peers: 8,
            events: tx,
        };
        for seed in [0x11u8, 0x5a] {
            let m = bip39::from_entropy(&hash32(&[b"demo-key", &[seed]]));
            state.keys.push((fingerprint_of(&m), m));
        }
        let this = Arc::new(Self {
            state: Arc::new(Mutex::new(state)),
            events: Mutex::new(Some(rx)),
            latency,
        });
        this.spawn_chain();
        this
    }

    fn spawn_chain(self: &Arc<Self>) {
        let weak = Arc::downgrade(&self.state);
        std::thread::Builder::new()
            .name("demo-chain".into())
            .spawn(move || {
                if let Some(state) = weak.upgrade()
                    && let Ok(s) = state.lock()
                {
                    s.send(Event::Connected);
                }
                loop {
                    std::thread::sleep(BLOCK_INTERVAL);
                    let Some(state) = weak.upgrade() else { return };
                    let Ok(mut s) = state.lock() else { return };
                    s.new_block();
                }
            })
            .expect("spawn demo chain");
    }

    fn handle(&self, destination: &str, command: &str, data: &Value) -> Result<Value, RpcError> {
        let mut s = self.state.lock().map_err(|_| RpcError::Disconnected)?;
        if destination == "daemon" {
            return match command {
                "start_service" | "register_service" | "ping" => Ok(json!({ "success": true })),
                _ => Err(unsupported(command)),
            };
        }
        s.dispatch(command, data)
    }

    fn delayed<T: Send + 'static>(&self, result: T) -> BoxFut<T> {
        let latency = self.latency;
        if latency.is_zero() {
            return Box::pin(async move { result });
        }
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            std::thread::sleep(latency);
            let _ = tx.send(result);
        });
        Box::pin(async move { rx.await.expect("demo timer thread") })
    }
}

fn unsupported(command: &str) -> RpcError {
    RpcError::Remote {
        message: format!("`{command}` is not available in demo mode"),
    }
}

fn remote(msg: impl Into<String>) -> RpcError {
    RpcError::Remote { message: msg.into() }
}

fn fingerprint_of(m: &Mnemonic) -> u32 {
    let h = hash32(&[m.words().join(" ").as_bytes()]);
    u32::from_be_bytes([h[0], h[1], h[2], h[3]])
}

fn arg_u64(data: &Value, key: &str) -> Result<u64, RpcError> {
    data.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| remote(format!("missing integer `{key}`")))
}

impl State {
    fn send(&self, ev: Event) {
        let _ = self.events.unbounded_send(ev);
    }

    fn changed(&self, state: &str, wallet_id: Option<u32>) {
        self.send(Event::StateChanged {
            state: state.into(),
            wallet_id,
            additional_data: Value::Null,
        });
    }

    fn address(&self, fingerprint: u32, wallet_id: u32, index: u32) -> (String, [u8; 32]) {
        let ph = hash32(&[
            &fingerprint.to_be_bytes(),
            &wallet_id.to_be_bytes(),
            &index.to_be_bytes(),
        ]);
        (encode_puzzle_hash(&ph, &self.prefix).expect("valid prefix"), ph)
    }

    fn syncing(&self) -> bool {
        self.logged_in.is_some() && self.height < self.login_height + 1
    }

    fn new_block(&mut self) {
        self.height += 1;
        let height = self.height;
        let mut touched = Vec::new();
        for w in self.wallets.values_mut() {
            for tx in w.txs.iter_mut().filter(|t| !t.confirmed) {
                if height >= tx.confirmed_at_height + CONFIRMATIONS {
                    tx.confirmed = true;
                    tx.confirmed_at_height = height;
                    let total = tx.amount + tx.fee_amount;
                    if tx.tx_type.is_incoming() {
                        w.confirmed += tx.amount;
                    } else {
                        w.confirmed = w.confirmed.saturating_sub(total);
                    }
                    touched.push(w.info.id);
                }
            }
        }
        if self.logged_in.is_some() {
            self.changed("new_block", None);
            if height == self.login_height + 1 {
                self.changed("sync_changed", None);
            }
        }
        for id in touched {
            self.changed("coin_added", Some(id));
        }
        // An occasional farming reward keeps the demo alive.
        if let Some(fp) = self.logged_in
            && height.is_multiple_of(7)
        {
            let (to_address, ph) = self.address(fp, 1, height);
            if let Some(w) = self.wallets.get_mut(&1) {
                let reward = 1_750_000_000_000;
                w.unconfirmed += reward;
                let name = hex0x(&hash32(&[b"reward", &height.to_be_bytes()]));
                w.txs.push(TransactionRecord {
                    confirmed_at_height: height,
                    created_at_time: now(),
                    to_puzzle_hash: hex0x(&ph),
                    amount: reward,
                    fee_amount: 0,
                    confirmed: false,
                    sent: 0,
                    wallet_id: 1,
                    tx_type: TxType::CoinbaseReward,
                    name,
                    to_address: Some(to_address),
                    sent_to: vec![],
                    additions: vec![],
                    removals: vec![],
                });
                self.changed("coin_added", Some(1));
            }
        }
    }

    fn log_in(&mut self, fingerprint: u32) -> Result<Value, RpcError> {
        if !self.keys.iter().any(|(fp, _)| *fp == fingerprint) {
            return Err(remote("Unknown Error"));
        }
        if self.logged_in == Some(fingerprint) {
            return Ok(json!({ "fingerprint": fingerprint }));
        }
        self.logged_in = Some(fingerprint);
        self.login_height = self.height;
        self.seed_wallets(fingerprint);
        Ok(json!({ "fingerprint": fingerprint }))
    }

    fn seed_wallets(&mut self, fp: u32) {
        self.wallets.clear();
        let t = now();
        let mut xch = DemoWallet {
            info: WalletInfo {
                id: 1,
                name: "Chia Wallet".into(),
                wallet_type: WalletType::Standard,
                data: String::new(),
            },
            confirmed: 0,
            unconfirmed: 0,
            txs: vec![],
            address_index: 0,
        };
        // A plausible two months of history.
        let script: [(TxType, u64, u64, u64); 12] = [
            (TxType::Incoming, 1_000 * MOJO_PER_XCH, 0, 61),
            (TxType::CoinbaseReward, 1_750_000_000_000, 0, 55),
            (TxType::Outgoing, 42 * MOJO_PER_XCH, 50_000_000, 48),
            (TxType::CoinbaseReward, 1_750_000_000_000, 0, 41),
            (TxType::FeeReward, 250_000_000_000, 0, 41),
            (TxType::Incoming, 312_500_000_000_000, 0, 30),
            (TxType::Outgoing, 12_750_000_000_000, 100_000_000, 22),
            (TxType::CoinbaseReward, 1_750_000_000_000, 0, 14),
            (TxType::Outgoing, 3 * MOJO_PER_XCH, 0, 9),
            (TxType::Incoming, 26_985_000_000_000, 0, 5),
            (TxType::CoinbaseReward, 1_750_000_000_000, 0, 2),
            (TxType::Outgoing, 1_234_567_890_123, 10_000_000, 1),
        ];
        let mut balance: u64 = 0;
        for (i, (kind, amount, fee, days_ago)) in script.into_iter().enumerate() {
            let ts = t - days_ago * 86_400 - (i as u64 * 3_917) % 40_000;
            if kind.is_incoming() {
                balance += amount;
            } else {
                balance -= amount + fee;
            }
            let (to_address, ph) = if kind.is_incoming() {
                self.address(fp, 1, i as u32)
            } else {
                let ph = hash32(&[b"counterparty", &[i as u8]]);
                (encode_puzzle_hash(&ph, &self.prefix).expect("prefix"), ph)
            };
            xch.txs.push(TransactionRecord {
                confirmed_at_height: self.height - (days_ago as u32) * 4_608,
                created_at_time: ts,
                to_puzzle_hash: hex0x(&ph),
                amount,
                fee_amount: fee,
                confirmed: true,
                sent: 1,
                wallet_id: 1,
                tx_type: kind,
                name: hex0x(&hash32(&[b"tx", &fp.to_be_bytes(), &[i as u8]])),
                to_address: Some(to_address),
                sent_to: vec![],
                additions: vec![Coin {
                    parent_coin_info: hex0x(&[1; 32]),
                    puzzle_hash: hex0x(&ph),
                    amount,
                }],
                removals: vec![],
            });
        }
        xch.confirmed = balance;
        xch.unconfirmed = balance;

        let cc = DemoWallet {
            info: WalletInfo {
                id: 2,
                name: "Marmot".into(),
                wallet_type: WalletType::ColouredCoin,
                data: String::new(),
            },
            confirmed: 25_000 * MOJO_PER_CC,
            unconfirmed: 25_000 * MOJO_PER_CC,
            txs: vec![],
            address_index: 0,
        };
        let did = DemoWallet {
            info: WalletInfo {
                id: 3,
                name: "DID Wallet".into(),
                wallet_type: WalletType::DistributedId,
                data: String::new(),
            },
            confirmed: 1,
            unconfirmed: 1,
            txs: vec![],
            address_index: 0,
        };
        for w in [xch, cc, did] {
            self.wallets.insert(w.info.id, w);
        }
    }

    fn wallet(&mut self, data: &Value) -> Result<&mut DemoWallet, RpcError> {
        if self.logged_in.is_none() {
            return Err(remote("not logged in"));
        }
        let id = arg_u64(data, "wallet_id")? as u32;
        self.wallets
            .get_mut(&id)
            .ok_or_else(|| remote(format!("wallet {id} not found")))
    }

    fn spend(&mut self, data: &Value, address_key: &str, cc: bool) -> Result<Value, RpcError> {
        let prefix = self.prefix.clone();
        let height = self.height;
        if self.syncing() {
            return Err(remote("Wallet needs to be fully synced before sending transactions"));
        }
        let address = data
            .get(address_key)
            .and_then(Value::as_str)
            .ok_or_else(|| remote("missing address"))?
            .to_owned();
        let ph = decode_address(&address, &prefix).map_err(|e| remote(e.to_string()))?;
        let amount = arg_u64(data, "amount")?;
        let fee = if cc { 0 } else { arg_u64(data, "fee")? };
        let w = self.wallet(data)?;
        if cc != (w.info.wallet_type == WalletType::ColouredCoin) {
            return Err(remote("wrong wallet type for this spend"));
        }
        let total = amount.checked_add(fee).ok_or_else(|| remote("amount overflow"))?;
        let spendable = w.confirmed.min(w.unconfirmed);
        if total > spendable {
            return Err(remote(format!(
                "Can't send more than {spendable} in a single transaction"
            )));
        }
        w.unconfirmed -= total;
        let tx = TransactionRecord {
            confirmed_at_height: height,
            created_at_time: now(),
            to_puzzle_hash: hex0x(&ph),
            amount,
            fee_amount: fee,
            confirmed: false,
            sent: 1,
            wallet_id: w.info.id,
            tx_type: TxType::Outgoing,
            name: hex0x(&hash32(&[b"sent", &ph, &amount.to_be_bytes(), &now().to_be_bytes()])),
            to_address: Some(address),
            sent_to: vec![("demo-peer".into(), 1, None)],
            additions: vec![],
            removals: vec![],
        };
        w.txs.push(tx.clone());
        let id = w.info.id;
        self.changed("pending_transaction", Some(id));
        self.send(Event::StateChanged {
            state: "tx_update".into(),
            wallet_id: Some(id),
            additional_data: json!({ "transaction": tx }),
        });
        Ok(json!({ "transaction": tx, "transaction_id": tx.name }))
    }

    fn dispatch(&mut self, command: &str, data: &Value) -> Result<Value, RpcError> {
        let fp = self.logged_in.unwrap_or(0);
        match command {
            "ping" => Ok(json!({})),
            "get_public_keys" => {
                Ok(json!({ "public_key_fingerprints": self.keys.iter().map(|k| k.0).collect::<Vec<_>>() }))
            }
            "log_in" => self.log_in(arg_u64(data, "fingerprint")? as u32),
            "delete_key" => {
                let target = arg_u64(data, "fingerprint")? as u32;
                self.keys.retain(|(k, _)| *k != target);
                if self.logged_in == Some(target) {
                    self.logged_in = None;
                }
                Ok(json!({}))
            }
            "get_network_info" => Ok(json!({ "network_name": "mainnet", "network_prefix": self.prefix })),
            "get_sync_status" => {
                let syncing = self.syncing();
                Ok(json!({ "synced": !syncing, "syncing": syncing, "genesis_initialized": true }))
            }
            "get_height_info" => Ok(json!({ "height": self.height })),
            "get_connections" => Ok(json!({ "connections": vec![json!({"type": 1}); self.peers] })),
            "get_wallets" => {
                if self.logged_in.is_none() {
                    return Err(remote("not logged in"));
                }
                let mut ws: Vec<&WalletInfo> = self.wallets.values().map(|w| &w.info).collect();
                ws.sort_by_key(|w| w.id);
                Ok(json!({ "wallets": ws }))
            }
            "get_wallet_balance" => {
                let w = self.wallet(data)?;
                let spendable = w.confirmed.min(w.unconfirmed);
                Ok(json!({ "wallet_balance": {
                    "wallet_id": w.info.id,
                    "confirmed_wallet_balance": w.confirmed,
                    "unconfirmed_wallet_balance": w.unconfirmed,
                    "spendable_balance": spendable,
                    "pending_change": 0,
                    "max_send_amount": spendable,
                }}))
            }
            "get_transactions" => {
                let end = data.get("end").and_then(Value::as_u64).unwrap_or(50) as usize;
                let w = self.wallet(data)?;
                let mut txs = w.txs.clone();
                txs.sort_by(|a, b| b.created_at_time.cmp(&a.created_at_time));
                txs.truncate(end);
                Ok(json!({ "transactions": txs, "wallet_id": w.info.id }))
            }
            "get_next_address" => {
                let new = data
                    .get("new_address")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| remote("missing new_address"))?;
                let w = self.wallet(data)?;
                if !w.info.wallet_type.is_spendable() {
                    return Err(remote("Wallet type cannot create puzzle hashes"));
                }
                if new {
                    w.address_index += 1;
                }
                let (id, idx) = (w.info.id, w.address_index);
                let (address, _) = self.address(fp, id, 1000 + idx);
                Ok(json!({ "wallet_id": id, "address": address }))
            }
            "cc_get_name" => {
                let w = self.wallet(data)?;
                Ok(json!({ "wallet_id": w.info.id, "name": w.info.name }))
            }
            "send_transaction" => self.spend(data, "address", false),
            "cc_spend" => self.spend(data, "inner_address", true),
            _ => Err(unsupported(command)),
        }
    }

    fn dispatch_sensitive(&mut self, command: &str, data_json: &str) -> Result<Zeroizing<String>, RpcError> {
        match command {
            "generate_mnemonic" => {
                let mut entropy = Zeroizing::new([0u8; 32]);
                getrandom::fill(entropy.as_mut()).map_err(|_| remote("no randomness"))?;
                let m = bip39::from_entropy(entropy.as_ref());
                Ok(Zeroizing::new(format!(
                    r#"{{"data":{{"success":true,"mnemonic":{}}}}}"#,
                    m.to_json_array().as_str()
                )))
            }
            "add_key" => {
                #[derive(Deserialize)]
                struct Req {
                    mnemonic: Mnemonic,
                }
                let req: Req = serde_json::from_str(data_json).map_err(|_| remote("Mnemonic not in request"))?;
                if let Err(e) = bip39::validate(&req.mnemonic) {
                    return Err(remote(e.to_string()));
                }
                let fp = fingerprint_of(&req.mnemonic);
                if !self.keys.iter().any(|(k, _)| *k == fp) {
                    self.keys.push((fp, req.mnemonic.clone()));
                }
                self.logged_in = None;
                self.log_in(fp)?;
                Ok(Zeroizing::new(format!(
                    r#"{{"data":{{"success":true,"fingerprint":{fp}}}}}"#
                )))
            }
            "get_private_key" => {
                let req: Value = serde_json::from_str(data_json).map_err(|_| remote("bad request"))?;
                let fp = arg_u64(&req, "fingerprint")? as u32;
                let (_, m) = self
                    .keys
                    .iter()
                    .find(|(k, _)| *k == fp)
                    .ok_or_else(|| remote("key not found"))?;
                let mut seed = Zeroizing::new(m.words().join(" "));
                let out = Zeroizing::new(format!(
                    r#"{{"data":{{"success":true,"private_key":{{"fingerprint":{fp},"seed":"{}"}}}}}}"#,
                    seed.as_str()
                ));
                seed.clear();
                Ok(out)
            }
            _ => Err(unsupported(command)),
        }
    }
}

impl Transport for DemoTransport {
    fn call(
        &self,
        destination: &str,
        command: &str,
        data: Value,
        _timeout: Duration,
    ) -> BoxFut<Result<Value, RpcError>> {
        let result = self.handle(destination, command, &data).and_then(data_to_result);
        let slow = matches!(command, "log_in" | "send_transaction" | "cc_spend");
        if slow && !self.latency.is_zero() {
            let (tx, rx) = oneshot::channel();
            let latency = self.latency * 6;
            std::thread::spawn(move || {
                std::thread::sleep(latency);
                let _ = tx.send(result);
            });
            return Box::pin(async move { rx.await.expect("demo timer thread") });
        }
        self.delayed(result)
    }

    fn call_sensitive(
        &self,
        _destination: &str,
        command: &str,
        data_json: Zeroizing<String>,
        _timeout: Duration,
    ) -> BoxFut<Result<Zeroizing<String>, RpcError>> {
        let result = self
            .state
            .lock()
            .map_err(|_| RpcError::Disconnected)
            .and_then(|mut s| s.dispatch_sensitive(command, &data_json))
            .and_then(|text| check_raw_success(&text).map(|_| text));
        self.delayed(result)
    }

    fn take_events(&self) -> Option<fmpsc::UnboundedReceiver<Event>> {
        self.events.lock().ok()?.take()
    }

    fn describe(&self) -> String {
        "demo backend · in-process, no network".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{KeyOrigin, WalletApi};
    use futures::executor::block_on;

    #[test]
    fn full_flow() {
        let t = DemoTransport::new(Duration::ZERO);
        let api = WalletApi::new(t.clone());
        block_on(async {
            let keys = api.public_keys().await.unwrap();
            assert_eq!(keys.len(), 2);
            api.log_in(keys[0]).await.unwrap();
            let wallets = api.wallets().await.unwrap();
            assert_eq!(wallets.len(), 3);
            let before = api.balance(1).await.unwrap();
            assert!(before.confirmed_wallet_balance > 0);

            // Block while syncing, then advance a block.
            let to = api.next_address(1, true).await.unwrap();
            assert!(api.send_xch(1, &to, 1, 0, "xch").await.is_err());
            t.state.lock().unwrap().new_block();

            // Wrong-network addresses never reach the backend.
            let txch = encode_puzzle_hash(&[7; 32], "txch").unwrap();
            assert!(api.send_xch(1, &txch, 1, 0, "xch").await.is_err());

            let tx = api.send_xch(1, &to, 5 * MOJO_PER_XCH, 1_000, "xch").await.unwrap();
            assert!(!tx.confirmed);
            let after = api.balance(1).await.unwrap();
            assert_eq!(
                after.unconfirmed_wallet_balance,
                before.unconfirmed_wallet_balance - 5 * MOJO_PER_XCH - 1_000
            );
            assert!(api.send_xch(1, &to, u64::MAX / 2, 0, "xch").await.is_err());

            // Mnemonics round-trip through the sensitive path.
            let m = api.generate_mnemonic().await.unwrap();
            assert_eq!(m.len(), 24);
            let fp = api.add_key(&m, KeyOrigin::NewWallet).await.unwrap();
            let seed = api.recovery_phrase(fp).await.unwrap().unwrap();
            assert_eq!(seed.expose(), m.words().join(" "));
            let bad = Mnemonic::parse(&"abandon ".repeat(24));
            assert!(api.add_key(&bad, KeyOrigin::Imported).await.is_err());
        });
    }
}
