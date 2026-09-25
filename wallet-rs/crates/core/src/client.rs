//! Mutual-TLS websocket client for the Chia daemon.
//!
//! Differences from the Electron GUI, all deliberate:
//! - The daemon's certificate is **verified** against `private_ca.crt`
//!   (the GUI used `rejectUnauthorized: false`).
//! - Every request has a deadline; the daemon silently drops messages for
//!   services that aren't running, so without one a call could hang forever.
//! - Messages carrying a mnemonic are built and read back in zeroizing memory
//!   and never pass through `serde_json::Value`.
//! - Oversized frames are refused.
//!
//! The client owns a small Tokio runtime on its own thread. Its public API is
//! runtime-agnostic (futures oneshot/mpsc), so a GPUI executor can await it.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::{mpsc as fmpsc, oneshot};
use futures::{SinkExt, StreamExt};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use serde::Deserialize;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use zeroize::Zeroizing;

use crate::config::DaemonConfig;
use crate::protocol::{EnvelopeHead, Event, UI_SERVICE, WsMessage, new_request_id};

/// Every Chia private certificate carries this DNS SAN (`chia/ssl/create_ssl.py`).
const CHIA_CERT_SAN: &str = "chia.net";
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_BACKOFF: Duration = Duration::from_secs(10);

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RpcError {
    #[error("not connected to the Chia daemon")]
    NotConnected,
    #[error("the Chia daemon did not answer in time")]
    Timeout,
    #[error("connection to the Chia daemon was lost")]
    Disconnected,
    #[error("{message}")]
    Remote { message: String },
    #[error("unexpected response from the wallet: {0}")]
    Decode(String),
}

/// Anything that can carry wallet RPCs: the real daemon client, or the
/// in-process demo backend.
pub trait Transport: Send + Sync + 'static {
    fn call(&self, destination: &str, command: &str, data: Value, timeout: Duration)
    -> BoxFut<Result<Value, RpcError>>;

    /// Like `call`, but `data_json` is raw JSON object text held in zeroizing
    /// memory, and the full reply message is returned the same way.
    fn call_sensitive(
        &self,
        destination: &str,
        command: &str,
        data_json: Zeroizing<String>,
        timeout: Duration,
    ) -> BoxFut<Result<Zeroizing<String>, RpcError>>;

    /// The push-event stream. Can be taken once.
    fn take_events(&self) -> Option<fmpsc::UnboundedReceiver<Event>>;

    /// Short human description, e.g. `wss://localhost:55400 (mTLS)`.
    fn describe(&self) -> String;
}

#[derive(Debug, thiserror::Error)]
pub enum ClientSetupError {
    #[error("could not load {what} from {path}: {detail}")]
    Pem {
        what: &'static str,
        path: String,
        detail: String,
    },
    #[error("TLS configuration rejected: {0}")]
    Tls(#[from] rustls::Error),
}

/// Build the rustls config: trust only the Chia private CA, present our daemon cert.
pub fn tls_config(cfg: &DaemonConfig) -> Result<Arc<ClientConfig>, ClientSetupError> {
    let pem_err = |what, path: &std::path::Path, e: &dyn std::fmt::Display| ClientSetupError::Pem {
        what,
        path: path.display().to_string(),
        detail: e.to_string(),
    };

    let mut roots = RootCertStore::empty();
    let ca_iter =
        CertificateDer::pem_file_iter(&cfg.ca_cert).map_err(|e| pem_err("CA certificate", &cfg.ca_cert, &e))?;
    for cert in ca_iter {
        let cert = cert.map_err(|e| pem_err("CA certificate", &cfg.ca_cert, &e))?;
        roots.add(cert)?;
    }
    if roots.is_empty() {
        return Err(pem_err("CA certificate", &cfg.ca_cert, &"no certificates in file"));
    }

    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&cfg.client_cert)
        .map_err(|e| pem_err("client certificate", &cfg.client_cert, &e))?
        .collect::<Result<_, _>>()
        .map_err(|e| pem_err("client certificate", &cfg.client_cert, &e))?;

    // Read the key into memory we wipe, rather than letting a helper own the buffer.
    let key_pem =
        Zeroizing::new(std::fs::read(&cfg.client_key).map_err(|e| pem_err("client key", &cfg.client_key, &e))?);
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|e| pem_err("client key", &cfg.client_key, &e))?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_client_auth_cert(chain, key)?;
    Ok(Arc::new(config))
}

enum Reply {
    Value(oneshot::Sender<Result<Value, RpcError>>),
    Raw(oneshot::Sender<Result<Zeroizing<String>, RpcError>>),
}

impl Reply {
    fn fail(self, err: RpcError) {
        match self {
            Reply::Value(tx) => drop(tx.send(Err(err))),
            Reply::Raw(tx) => drop(tx.send(Err(err))),
        }
    }
}

struct Outgoing {
    request_id: String,
    text: Zeroizing<String>,
    reply: Reply,
    timeout: Duration,
}

/// Handle to the background connection. Dropping it shuts the connection down.
pub struct DaemonClient {
    tx: mpsc::UnboundedSender<Outgoing>,
    events: Mutex<Option<fmpsc::UnboundedReceiver<Event>>>,
    connected: Arc<AtomicBool>,
    endpoint: String,
}

impl DaemonClient {
    pub fn start(cfg: &DaemonConfig) -> Result<Arc<Self>, ClientSetupError> {
        let tls = tls_config(cfg)?;
        Ok(Self::start_with(cfg.host.clone(), cfg.port, tls))
    }

    /// Start with an explicit TLS config (used by tests).
    pub fn start_with(host: String, port: u16, tls: Arc<ClientConfig>) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = fmpsc::unbounded();
        let connected = Arc::new(AtomicBool::new(false));
        let endpoint = format!("wss://{host}:{port}");
        let flag = connected.clone();
        std::thread::Builder::new()
            .name("chia-daemon-client".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(connection_loop(host, port, tls, rx, ev_tx, flag));
            })
            .expect("spawn daemon client thread");
        Arc::new(Self {
            tx,
            events: Mutex::new(Some(ev_rx)),
            connected,
            endpoint,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    fn submit(&self, request_id: String, text: Zeroizing<String>, reply: Reply, timeout: Duration) {
        if !self.is_connected() {
            reply.fail(RpcError::NotConnected);
            return;
        }
        if let Err(e) = self.tx.send(Outgoing {
            request_id,
            text,
            reply,
            timeout,
        }) {
            e.0.reply.fail(RpcError::NotConnected);
        }
    }
}

impl Transport for DaemonClient {
    fn call(
        &self,
        destination: &str,
        command: &str,
        data: Value,
        timeout: Duration,
    ) -> BoxFut<Result<Value, RpcError>> {
        let request_id = new_request_id();
        let msg = WsMessage {
            command: command.to_owned(),
            ack: false,
            data,
            request_id: request_id.clone(),
            destination: destination.to_owned(),
            origin: UI_SERVICE.to_owned(),
        };
        let (tx, rx) = oneshot::channel();
        match serde_json::to_string(&msg) {
            Ok(text) => self.submit(request_id, Zeroizing::new(text), Reply::Value(tx), timeout),
            Err(e) => drop(tx.send(Err(RpcError::Decode(e.to_string())))),
        }
        Box::pin(async move { rx.await.unwrap_or(Err(RpcError::Disconnected)) })
    }

    fn call_sensitive(
        &self,
        destination: &str,
        command: &str,
        data_json: Zeroizing<String>,
        timeout: Duration,
    ) -> BoxFut<Result<Zeroizing<String>, RpcError>> {
        let request_id = new_request_id();
        let (tx, rx) = oneshot::channel();
        match raw_envelope(destination, command, &request_id, &data_json) {
            Some(text) => self.submit(request_id, text, Reply::Raw(tx), timeout),
            None => drop(tx.send(Err(RpcError::Decode("invalid command name".into())))),
        }
        Box::pin(async move {
            let text = rx.await.unwrap_or(Err(RpcError::Disconnected))?;
            check_raw_success(&text)?;
            Ok(text)
        })
    }

    fn take_events(&self) -> Option<fmpsc::UnboundedReceiver<Event>> {
        self.events.lock().ok()?.take()
    }

    fn describe(&self) -> String {
        format!("{} · mutual TLS, pinned private CA", self.endpoint)
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Build a request envelope around pre-serialised `data` without a `Value`.
fn raw_envelope(destination: &str, command: &str, request_id: &str, data_json: &str) -> Option<Zeroizing<String>> {
    if !is_ident(destination) || !is_ident(command) || !request_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = Zeroizing::new(String::with_capacity(data_json.len() + 200));
    out.push_str(r#"{"command":""#);
    out.push_str(command);
    out.push_str(r#"","ack":false,"data":"#);
    out.push_str(data_json);
    out.push_str(r#","request_id":""#);
    out.push_str(request_id);
    out.push_str(r#"","destination":""#);
    out.push_str(destination);
    out.push_str(r#"","origin":""#);
    out.push_str(UI_SERVICE);
    out.push_str(r#""}"#);
    Some(out)
}

/// Check `data.success` of a raw reply without materialising secret fields.
pub(crate) fn check_raw_success(text: &str) -> Result<(), RpcError> {
    #[derive(Deserialize)]
    struct Head {
        data: DataHead,
    }
    #[derive(Deserialize)]
    struct DataHead {
        #[serde(default)]
        success: Option<bool>,
        #[serde(default)]
        error: Option<String>,
    }
    let head: Head = serde_json::from_str(text).map_err(|e| RpcError::Decode(e.to_string()))?;
    if head.data.success == Some(false) {
        return Err(RpcError::Remote {
            message: head.data.error.unwrap_or_else(|| "request failed".into()),
        });
    }
    Ok(())
}

/// Turn a reply's `data` into a result, honouring `success: false`.
pub(crate) fn data_to_result(data: Value) -> Result<Value, RpcError> {
    if data.get("success").and_then(Value::as_bool) == Some(false) {
        let message = data
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("request failed")
            .to_owned();
        return Err(RpcError::Remote { message });
    }
    Ok(data)
}

async fn connection_loop(
    host: String,
    port: u16,
    tls: Arc<ClientConfig>,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    events: fmpsc::UnboundedSender<Event>,
    connected: Arc<AtomicBool>,
) {
    let mut backoff = Duration::from_millis(500);
    let mut last_reason = String::new();
    loop {
        let reason = match connect(&host, port, tls.clone()).await {
            Ok(ws) => {
                backoff = Duration::from_millis(500);
                connected.store(true, Ordering::Relaxed);
                let _ = events.unbounded_send(Event::Connected);
                let outcome = session(ws, &mut rx, &events).await;
                connected.store(false, Ordering::Relaxed);
                match outcome {
                    SessionEnd::Shutdown => return,
                    SessionEnd::Lost(reason) => reason,
                }
            }
            Err(reason) => reason,
        };
        if reason != last_reason {
            log::warn!("daemon connection: {reason}");
            let _ = events.unbounded_send(Event::Disconnected { reason: reason.clone() });
            last_reason = reason;
        }

        // Wait before reconnecting; fail anything submitted in the meantime.
        let sleep = tokio::time::sleep(backoff);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                cmd = rx.recv() => match cmd {
                    Some(c) => c.reply.fail(RpcError::NotConnected),
                    None => return,
                },
            }
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;

async fn connect(host: &str, port: u16, tls: Arc<ClientConfig>) -> Result<Ws, String> {
    let tcp = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect((host, port)))
        .await
        .map_err(|_| format!("timed out connecting to {host}:{port}"))?
        .map_err(|e| format!("cannot reach daemon at {host}:{port}: {e}"))?;
    tcp.set_nodelay(true).ok();
    let name = ServerName::try_from(CHIA_CERT_SAN).expect("static name");
    let stream = TlsConnector::from(tls)
        .connect(name, tcp)
        .await
        .map_err(|e| format!("TLS handshake failed (is this really your daemon?): {e}"))?;

    let mut ws_config = WebSocketConfig::default();
    ws_config.max_message_size = Some(MAX_MESSAGE_BYTES);
    ws_config.max_frame_size = Some(MAX_MESSAGE_BYTES);
    let url = format!("wss://{host}:{port}/");
    let (ws, _resp) = tokio_tungstenite::client_async_with_config(url, stream, Some(ws_config))
        .await
        .map_err(|e| format!("websocket handshake failed: {e}"))?;
    Ok(ws)
}

enum SessionEnd {
    Shutdown,
    Lost(String),
}

async fn session(
    ws: Ws,
    rx: &mut mpsc::UnboundedReceiver<Outgoing>,
    events: &fmpsc::UnboundedSender<Event>,
) -> SessionEnd {
    let (mut sink, mut stream) = ws.split();
    let mut pending: HashMap<String, (Reply, Instant)> = HashMap::new();

    let register = WsMessage {
        command: "register_service".into(),
        ack: false,
        data: serde_json::json!({ "service": UI_SERVICE }),
        request_id: new_request_id(),
        destination: "daemon".into(),
        origin: UI_SERVICE.into(),
    };
    let text = serde_json::to_string(&register).expect("static message");
    if let Err(e) = sink.send(Message::text(text)).await {
        return SessionEnd::Lost(format!("could not register: {e}"));
    }

    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let end = loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else {
                    let _ = sink.send(Message::Close(None)).await;
                    break SessionEnd::Shutdown;
                };
                let deadline = Instant::now() + cmd.timeout;
                // tungstenite needs an owned String; the zeroizing copy is wiped on drop.
                let frame = Message::text(cmd.text.as_str());
                if let Err(e) = sink.send(frame).await {
                    cmd.reply.fail(RpcError::Disconnected);
                    break SessionEnd::Lost(format!("send failed: {e}"));
                }
                pending.insert(cmd.request_id, (cmd.reply, deadline));
            }
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) => handle_text(text.as_str(), &mut pending, events),
                Some(Ok(Message::Close(_))) | None => break SessionEnd::Lost("daemon closed the connection".into()),
                Some(Ok(_)) => {}
                Some(Err(e)) => break SessionEnd::Lost(format!("connection error: {e}")),
            },
            _ = tick.tick() => {
                let now = Instant::now();
                let expired: Vec<String> = pending.iter().filter(|(_, (_, d))| *d <= now).map(|(k, _)| k.clone()).collect();
                for id in expired {
                    if let Some((reply, _)) = pending.remove(&id) {
                        reply.fail(RpcError::Timeout);
                    }
                }
                // Flush any queued pong replies to keep-alive pings.
                if let Err(e) = sink.flush().await {
                    break SessionEnd::Lost(format!("connection error: {e}"));
                }
            }
        }
    };
    for (_, (reply, _)) in pending.drain() {
        reply.fail(RpcError::Disconnected);
    }
    end
}

fn handle_text(text: &str, pending: &mut HashMap<String, (Reply, Instant)>, events: &fmpsc::UnboundedSender<Event>) {
    let Ok(head) = serde_json::from_str::<EnvelopeHead>(text) else {
        log::debug!("ignoring malformed daemon message");
        return;
    };
    if let Some((reply, _)) = pending.remove(&head.request_id) {
        match reply {
            Reply::Raw(tx) => {
                let _ = tx.send(Ok(Zeroizing::new(text.to_owned())));
            }
            Reply::Value(tx) => {
                let result = serde_json::from_str::<WsMessage>(text)
                    .map_err(|e| RpcError::Decode(e.to_string()))
                    .and_then(|m| data_to_result(m.data));
                let _ = tx.send(result);
            }
        }
        return;
    }
    // Not ours: either a push, or a reply to another `wallet_ui` client.
    if let Ok(msg) = serde_json::from_str::<WsMessage>(text) {
        if let Some(ev) = Event::from_push(&msg) {
            let _ = events.unbounded_send(ev);
        } else {
            log::trace!("ignoring unsolicited `{}`", head.command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_envelope_is_valid_json() {
        let env = raw_envelope(
            "chia_wallet",
            "add_key",
            "abcd",
            r#"{"mnemonic":["a","b"],"type":"skip"}"#,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&env).unwrap();
        assert_eq!(v["command"], "add_key");
        assert_eq!(v["data"]["mnemonic"][1], "b");
        assert_eq!(v["origin"], "wallet_ui");
        assert!(raw_envelope("chia_wallet", "x\",\"y", "ab", "{}").is_none());
    }

    #[test]
    fn success_checks() {
        assert!(check_raw_success(r#"{"data":{"success":true,"mnemonic":["x"]}}"#).is_ok());
        assert_eq!(
            check_raw_success(r#"{"data":{"success":false,"error":"nope"}}"#),
            Err(RpcError::Remote { message: "nope".into() })
        );
        assert!(data_to_result(serde_json::json!({"success": false})).is_err());
        assert!(data_to_result(serde_json::json!({"height": 1})).is_ok());
    }
}
