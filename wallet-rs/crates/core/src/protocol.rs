//! Wire types for the daemon websocket and the wallet RPC.
//!
//! Field names mirror `chia/util/ws_message.py` and `chia/rpc/wallet_rpc_api.py`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The name we register under; the wallet pushes `state_changed` here.
pub const UI_SERVICE: &str = "wallet_ui";
pub const DAEMON: &str = "daemon";
pub const WALLET: &str = "chia_wallet";

/// Every daemon message, request or reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    pub command: String,
    #[serde(default)]
    pub ack: bool,
    #[serde(default)]
    pub data: Value,
    pub request_id: String,
    #[serde(default)]
    pub destination: String,
    #[serde(default)]
    pub origin: String,
}

/// Just enough of a message to route it without parsing `data`.
#[derive(Debug, Deserialize)]
pub(crate) struct EnvelopeHead {
    pub request_id: String,
    #[serde(default)]
    pub command: String,
}

/// 32 random bytes as hex, like the GUI.
pub fn new_request_id() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("OS random source unavailable");
    hex::encode(buf)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub enum WalletType {
    Standard,
    RateLimited,
    ColouredCoin,
    DistributedId,
    Other(u8),
}

impl From<u8> for WalletType {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::Standard,
            1 => Self::RateLimited,
            6 => Self::ColouredCoin,
            8 => Self::DistributedId,
            n => Self::Other(n),
        }
    }
}

impl From<WalletType> for u8 {
    fn from(v: WalletType) -> u8 {
        match v {
            WalletType::Standard => 0,
            WalletType::RateLimited => 1,
            WalletType::ColouredCoin => 6,
            WalletType::DistributedId => 8,
            WalletType::Other(n) => n,
        }
    }
}

impl WalletType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "Chia",
            Self::RateLimited => "Rate limited",
            Self::ColouredCoin => "Coloured coin",
            Self::DistributedId => "DID",
            Self::Other(_) => "Unknown",
        }
    }

    /// Wallet kinds this client can send from and receive into.
    pub fn is_spendable(self) -> bool {
        matches!(self, Self::Standard | Self::ColouredCoin)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletInfo {
    pub id: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub wallet_type: WalletType,
    #[serde(default)]
    pub data: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletBalance {
    pub wallet_id: u32,
    pub confirmed_wallet_balance: u64,
    pub unconfirmed_wallet_balance: u64,
    pub spendable_balance: u64,
    #[serde(default)]
    pub pending_change: u64,
    #[serde(default)]
    pub max_send_amount: u64,
}

impl WalletBalance {
    /// Signed difference unconfirmed − confirmed, as the GUI's "Pending".
    pub fn pending_delta(&self) -> i128 {
        i128::from(self.unconfirmed_wallet_balance) - i128::from(self.confirmed_wallet_balance)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u32", into = "u32")]
pub enum TxType {
    Incoming,
    Outgoing,
    CoinbaseReward,
    FeeReward,
    IncomingTrade,
    OutgoingTrade,
    Other(u32),
}

impl From<u32> for TxType {
    fn from(v: u32) -> Self {
        match v {
            0 => Self::Incoming,
            1 => Self::Outgoing,
            2 => Self::CoinbaseReward,
            3 => Self::FeeReward,
            4 => Self::IncomingTrade,
            5 => Self::OutgoingTrade,
            n => Self::Other(n),
        }
    }
}

impl From<TxType> for u32 {
    fn from(v: TxType) -> u32 {
        match v {
            TxType::Incoming => 0,
            TxType::Outgoing => 1,
            TxType::CoinbaseReward => 2,
            TxType::FeeReward => 3,
            TxType::IncomingTrade => 4,
            TxType::OutgoingTrade => 5,
            TxType::Other(n) => n,
        }
    }
}

impl TxType {
    pub fn is_incoming(self) -> bool {
        matches!(
            self,
            Self::Incoming | Self::CoinbaseReward | Self::FeeReward | Self::IncomingTrade
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Incoming => "Received",
            Self::Outgoing => "Sent",
            Self::CoinbaseReward => "Farming reward",
            Self::FeeReward => "Fee reward",
            Self::IncomingTrade => "Trade in",
            Self::OutgoingTrade => "Trade out",
            Self::Other(_) => "Transaction",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coin {
    pub parent_coin_info: String,
    pub puzzle_hash: String,
    pub amount: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionRecord {
    #[serde(default)]
    pub confirmed_at_height: u32,
    #[serde(default)]
    pub created_at_time: u64,
    #[serde(default)]
    pub to_puzzle_hash: String,
    pub amount: u64,
    #[serde(default)]
    pub fee_amount: u64,
    #[serde(default)]
    pub confirmed: bool,
    #[serde(default)]
    pub sent: u32,
    pub wallet_id: u32,
    #[serde(rename = "type")]
    pub tx_type: TxType,
    /// The transaction id (`0x…`).
    pub name: String,
    #[serde(default)]
    pub to_address: Option<String>,
    /// `[peer, MempoolInclusionStatus (1 ok, 2 pending, 3 failed), error]`.
    #[serde(default)]
    pub sent_to: Vec<(String, u8, Option<String>)>,
    #[serde(default)]
    pub additions: Vec<Coin>,
    #[serde(default)]
    pub removals: Vec<Coin>,
}

impl TransactionRecord {
    /// The first mempool rejection reason, if every peer rejected it.
    pub fn rejection(&self) -> Option<&str> {
        if self.sent_to.is_empty() || self.sent_to.iter().any(|(_, status, _)| *status != 3) {
            return None;
        }
        self.sent_to.iter().find_map(|(_, _, err)| err.as_deref())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct SyncStatus {
    pub synced: bool,
    pub syncing: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct NetworkInfo {
    pub network_name: String,
    pub network_prefix: String,
}

/// Push notifications from the wallet or daemon.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The websocket is up and we are registered as `wallet_ui`.
    Connected,
    /// The websocket dropped; the client is reconnecting.
    Disconnected { reason: String },
    /// `state_changed` from the wallet state manager.
    StateChanged {
        state: String,
        wallet_id: Option<u32>,
        additional_data: Value,
    },
    /// The wallet pushed a new peer list.
    Connections { count: usize },
}

impl Event {
    pub(crate) fn from_push(msg: &WsMessage) -> Option<Self> {
        match msg.command.as_str() {
            "state_changed" => Some(Event::StateChanged {
                state: msg.data.get("state")?.as_str()?.to_owned(),
                wallet_id: msg
                    .data
                    .get("wallet_id")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok()),
                additional_data: msg.data.get("additional_data").cloned().unwrap_or(Value::Null),
            }),
            "get_connections" => Some(Event::Connections {
                count: msg
                    .data
                    .get("connections")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_transaction_record() {
        let json = serde_json::json!({
            "confirmed_at_height": 12, "created_at_time": 1_700_000_000u64,
            "to_puzzle_hash": "0x00", "amount": 1_000_000_000_000u64, "fee_amount": 0,
            "confirmed": true, "sent": 0, "wallet_id": 1, "type": 1, "name": "0xab",
            "trade_id": null, "spend_bundle": null,
            "sent_to": [["peer", 3, "DOUBLE_SPEND"], ["peer2", 3, null]],
            "additions": [{"parent_coin_info": "0x01", "puzzle_hash": "0x02", "amount": 5}],
            "removals": [], "to_address": "xch1…"
        });
        let tx: TransactionRecord = serde_json::from_value(json).unwrap();
        assert_eq!(tx.tx_type, TxType::Outgoing);
        assert_eq!(tx.rejection(), Some("DOUBLE_SPEND"));
        assert_eq!(tx.additions[0].amount, 5);
    }

    #[test]
    fn wallet_types_round_trip() {
        let w: WalletInfo = serde_json::from_str(r#"{"id":2,"name":"x","type":6,"data":""}"#).unwrap();
        assert_eq!(w.wallet_type, WalletType::ColouredCoin);
        assert_eq!(serde_json::to_value(w.wallet_type).unwrap(), 6);
        assert_eq!(WalletType::from(42), WalletType::Other(42));
    }

    #[test]
    fn push_events() {
        let msg: WsMessage = serde_json::from_value(serde_json::json!({
            "command": "state_changed", "ack": false, "request_id": "x", "origin": "chia_wallet",
            "destination": "wallet_ui", "data": {"state": "coin_added", "wallet_id": 1}
        }))
        .unwrap();
        assert_eq!(
            Event::from_push(&msg),
            Some(Event::StateChanged {
                state: "coin_added".into(),
                wallet_id: Some(1),
                additional_data: Value::Null
            })
        );
    }

    #[test]
    fn request_ids_are_unique_hex() {
        let a = new_request_id();
        assert_eq!(a.len(), 64);
        assert_ne!(a, new_request_id());
    }
}
