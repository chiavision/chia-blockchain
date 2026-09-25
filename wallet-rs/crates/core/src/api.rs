//! Typed wallet operations over any [`Transport`].

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::address::decode_address;
use crate::client::{RpcError, Transport};
use crate::protocol::{DAEMON, NetworkInfo, SyncStatus, TransactionRecord, WALLET, WalletBalance, WalletInfo};
use crate::secret::{Mnemonic, SecretString};

const QUICK: Duration = Duration::from_secs(20);
const SLOW: Duration = Duration::from_secs(120);
/// Starting a wallet for a key can take minutes on first sync.
const LOGIN: Duration = Duration::from_secs(600);

/// What a newly added key should do on first start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrigin {
    /// Freshly generated: nothing to restore.
    NewWallet,
    /// Imported: scan the chain, but never fetch a backup from a remote host.
    Imported,
}

#[derive(Clone)]
pub struct WalletApi {
    transport: Arc<dyn Transport>,
}

fn decode<T: DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    serde_json::from_value(v).map_err(|e| RpcError::Decode(e.to_string()))
}

impl WalletApi {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport }
    }

    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    async fn wallet(&self, command: &str, data: Value, timeout: Duration) -> Result<Value, RpcError> {
        self.transport.call(WALLET, command, data, timeout).await
    }

    // ---- service lifecycle -------------------------------------------------

    /// Ask the daemon to start the wallet service. "Already running" is success.
    pub async fn start_wallet_service(&self) -> Result<(), RpcError> {
        match self
            .transport
            .call(DAEMON, "start_service", json!({ "service": WALLET }), QUICK)
            .await
        {
            Ok(_) => Ok(()),
            Err(RpcError::Remote { message }) if message.contains("already running") => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn ping_wallet(&self) -> Result<(), RpcError> {
        self.transport
            .call(WALLET, "ping", json!({}), Duration::from_secs(3))
            .await
            .map(|_| ())
    }

    // ---- keys -------------------------------------------------------------

    pub async fn public_keys(&self) -> Result<Vec<u32>, RpcError> {
        #[derive(Deserialize)]
        struct R {
            public_key_fingerprints: Vec<u32>,
        }
        Ok(decode::<R>(self.wallet("get_public_keys", json!({}), QUICK).await?)?.public_key_fingerprints)
    }

    /// Log in with `type: "skip"` so the backend never sends key-derived data
    /// to a remote backup host (the GUI's default contacted backup.chia.net).
    pub async fn log_in(&self, fingerprint: u32) -> Result<u32, RpcError> {
        #[derive(Deserialize)]
        struct R {
            fingerprint: u32,
        }
        let data = json!({ "fingerprint": fingerprint, "type": "skip", "host": "" });
        Ok(decode::<R>(self.wallet("log_in", data, LOGIN).await?)?.fingerprint)
    }

    pub async fn generate_mnemonic(&self) -> Result<Mnemonic, RpcError> {
        #[derive(Deserialize)]
        struct R {
            data: D,
        }
        #[derive(Deserialize)]
        struct D {
            mnemonic: Mnemonic,
        }
        let text = self
            .transport
            .call_sensitive(WALLET, "generate_mnemonic", Zeroizing::new("{}".into()), QUICK)
            .await?;
        let r: R = serde_json::from_str(&text).map_err(|_| RpcError::Decode("malformed mnemonic reply".into()))?;
        Ok(r.data.mnemonic)
    }

    pub async fn add_key(&self, mnemonic: &Mnemonic, origin: KeyOrigin) -> Result<u32, RpcError> {
        #[derive(Deserialize)]
        struct R {
            data: D,
        }
        #[derive(Deserialize)]
        struct D {
            fingerprint: u32,
        }
        let kind = match origin {
            KeyOrigin::NewWallet => "new_wallet",
            KeyOrigin::Imported => "skip",
        };
        let words = mnemonic.to_json_array();
        let mut data = Zeroizing::new(String::with_capacity(words.len() + 40));
        data.push_str(r#"{"mnemonic":"#);
        data.push_str(&words);
        data.push_str(r#","type":""#);
        data.push_str(kind);
        data.push_str(r#""}"#);
        let text = self.transport.call_sensitive(WALLET, "add_key", data, LOGIN).await?;
        let r: R = serde_json::from_str(&text).map_err(|e| RpcError::Decode(e.to_string()))?;
        Ok(r.data.fingerprint)
    }

    pub async fn delete_key(&self, fingerprint: u32) -> Result<(), RpcError> {
        self.wallet("delete_key", json!({ "fingerprint": fingerprint }), SLOW)
            .await
            .map(|_| ())
    }

    /// The 24-word recovery phrase for a key, if the keychain has it.
    pub async fn recovery_phrase(&self, fingerprint: u32) -> Result<Option<SecretString>, RpcError> {
        #[derive(Deserialize)]
        struct R {
            data: D,
        }
        #[derive(Deserialize)]
        struct D {
            private_key: K,
        }
        #[derive(Deserialize)]
        struct K {
            #[serde(default)]
            seed: Option<SecretString>,
        }
        let data = Zeroizing::new(format!(r#"{{"fingerprint":{fingerprint}}}"#));
        let text = self
            .transport
            .call_sensitive(WALLET, "get_private_key", data, QUICK)
            .await?;
        let r: R = serde_json::from_str(&text).map_err(|_| RpcError::Decode("malformed key reply".into()))?;
        Ok(r.data.private_key.seed)
    }

    // ---- wallet state -----------------------------------------------------

    pub async fn network_info(&self) -> Result<NetworkInfo, RpcError> {
        decode(self.wallet("get_network_info", json!({}), QUICK).await?)
    }

    pub async fn sync_status(&self) -> Result<SyncStatus, RpcError> {
        decode(self.wallet("get_sync_status", json!({}), QUICK).await?)
    }

    pub async fn height(&self) -> Result<u32, RpcError> {
        #[derive(Deserialize)]
        struct R {
            height: u32,
        }
        Ok(decode::<R>(self.wallet("get_height_info", json!({}), QUICK).await?)?.height)
    }

    pub async fn connection_count(&self) -> Result<usize, RpcError> {
        let v = self.wallet("get_connections", json!({}), QUICK).await?;
        Ok(v.get("connections").and_then(Value::as_array).map_or(0, Vec::len))
    }

    pub async fn wallets(&self) -> Result<Vec<WalletInfo>, RpcError> {
        #[derive(Deserialize)]
        struct R {
            wallets: Vec<WalletInfo>,
        }
        Ok(decode::<R>(self.wallet("get_wallets", json!({}), QUICK).await?)?.wallets)
    }

    pub async fn balance(&self, wallet_id: u32) -> Result<WalletBalance, RpcError> {
        #[derive(Deserialize)]
        struct R {
            wallet_balance: WalletBalance,
        }
        Ok(decode::<R>(
            self.wallet("get_wallet_balance", json!({ "wallet_id": wallet_id }), QUICK)
                .await?,
        )?
        .wallet_balance)
    }

    /// Most recent transactions, newest first.
    pub async fn transactions(&self, wallet_id: u32, limit: u32) -> Result<Vec<TransactionRecord>, RpcError> {
        #[derive(Deserialize)]
        struct R {
            transactions: Vec<TransactionRecord>,
        }
        // Integers only: the backend splices these into SQL.
        let data = json!({ "wallet_id": wallet_id, "start": 0u32, "end": limit });
        let mut txs = decode::<R>(self.wallet("get_transactions", data, QUICK).await?)?.transactions;
        txs.sort_by(|a, b| b.created_at_time.cmp(&a.created_at_time));
        Ok(txs)
    }

    pub async fn next_address(&self, wallet_id: u32, new_address: bool) -> Result<String, RpcError> {
        #[derive(Deserialize)]
        struct R {
            address: String,
        }
        let data = json!({ "wallet_id": wallet_id, "new_address": new_address });
        Ok(decode::<R>(self.wallet("get_next_address", data, QUICK).await?)?.address)
    }

    pub async fn cc_name(&self, wallet_id: u32) -> Result<String, RpcError> {
        #[derive(Deserialize)]
        struct R {
            name: String,
        }
        Ok(decode::<R>(
            self.wallet("cc_get_name", json!({ "wallet_id": wallet_id }), QUICK)
                .await?,
        )?
        .name)
    }

    // ---- spending ---------------------------------------------------------

    /// Send XCH. The address is re-validated against `prefix` here as a last
    /// line of defence, whatever the UI already checked.
    pub async fn send_xch(
        &self,
        wallet_id: u32,
        address: &str,
        amount: u64,
        fee: u64,
        prefix: &str,
    ) -> Result<TransactionRecord, RpcError> {
        decode_address(address, prefix).map_err(|e| RpcError::Remote { message: e.to_string() })?;
        if amount == 0 {
            return Err(RpcError::Remote {
                message: "amount must be greater than zero".into(),
            });
        }
        #[derive(Deserialize)]
        struct R {
            transaction: TransactionRecord,
        }
        let data = json!({ "wallet_id": wallet_id, "address": address.trim(), "amount": amount, "fee": fee });
        Ok(decode::<R>(self.wallet("send_transaction", data, SLOW).await?)?.transaction)
    }

    /// Send a coloured coin. The backend doesn't support fees for CC spends yet.
    pub async fn send_cc(
        &self,
        wallet_id: u32,
        address: &str,
        amount: u64,
        prefix: &str,
    ) -> Result<TransactionRecord, RpcError> {
        decode_address(address, prefix).map_err(|e| RpcError::Remote { message: e.to_string() })?;
        if amount == 0 {
            return Err(RpcError::Remote {
                message: "amount must be greater than zero".into(),
            });
        }
        #[derive(Deserialize)]
        struct R {
            transaction: TransactionRecord,
        }
        let data = json!({ "wallet_id": wallet_id, "inner_address": address.trim(), "amount": amount, "fee": 0u64 });
        Ok(decode::<R>(self.wallet("cc_spend", data, SLOW).await?)?.transaction)
    }
}
