//! A stand-in for `chia/daemon/server.py` for end-to-end testing of the real
//! (non-demo) client path: mutual TLS, `register_service`, routing to
//! `chia_wallet`, and `state_changed` pushes — backed by the demo wallet.
//!
//! ```text
//! cargo run -p chia-wallet-core --example mock_daemon -- /tmp/chia-root [port]
//! CHIA_ROOT=/tmp/chia-root cargo run -p chia-wallet
//! ```
//!
//! On first run it writes a Chia-style root: `config/config.yaml` plus a
//! private CA and `private_daemon.crt/.key`, laid out like `chia init` does.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chia_wallet_core::DemoTransport;
use chia_wallet_core::client::{RpcError, Transport};
use chia_wallet_core::protocol::Event;
use futures::{SinkExt, StreamExt};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;
use zeroize::Zeroizing;

const SENSITIVE: [&str; 3] = ["generate_mnemonic", "add_key", "get_private_key"];

fn write_root(root: &Path, port: u16) {
    let ssl = root.join("config/ssl");
    if ssl.join("ca/private_ca.crt").exists() {
        return;
    }
    std::fs::create_dir_all(ssl.join("ca")).unwrap();
    std::fs::create_dir_all(ssl.join("daemon")).unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "Chia CA");
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().unwrap()).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::new(vec!["chia.net".to_string()]).unwrap();
    leaf.distinguished_name.push(DnType::CommonName, "Chia");
    let leaf = leaf.signed_by(&leaf_key, &ca).unwrap();
    std::fs::write(ssl.join("ca/private_ca.crt"), ca.pem()).unwrap();
    std::fs::write(ssl.join("daemon/private_daemon.crt"), leaf.pem()).unwrap();
    std::fs::write(ssl.join("daemon/private_daemon.key"), leaf_key.serialize_pem()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            ssl.join("daemon/private_daemon.key"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let config = format!(
        r#"self_hostname: &self_hostname "localhost"
daemon_port: {port}
network_overrides: &network_overrides
  config:
    mainnet:
      address_prefix: "xch"
selected_network: &selected_network "mainnet"
private_ssl_ca:
  crt: "config/ssl/ca/private_ca.crt"
  key: "config/ssl/ca/private_ca.key"
ui:
  daemon_host: *self_hostname
  daemon_port: {port}
  daemon_ssl:
    private_crt: config/ssl/daemon/private_daemon.crt
    private_key: config/ssl/daemon/private_daemon.key
  selected_network: *selected_network
"#
    );
    std::fs::write(root.join("config/config.yaml"), config).unwrap();
    println!("wrote a Chia root at {}", root.display());
}

fn acceptor(root: &Path) -> TlsAcceptor {
    let read = |p: &str| std::fs::read(root.join(p)).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(&read("config/ssl/ca/private_ca.crt")).unwrap())
        .unwrap();
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(&read("config/ssl/daemon/private_daemon.crt")).unwrap()],
            PrivateKeyDer::from_pem_slice(&read("config/ssl/daemon/private_daemon.key")).unwrap(),
        )
        .unwrap();
    TlsAcceptor::from(Arc::new(config))
}

async fn answer(wallet: &DemoTransport, req: &Value) -> Value {
    let command = req["command"].as_str().unwrap_or_default();
    let destination = req["destination"].as_str().unwrap_or_default();
    let data = req["data"].clone();
    let t = Duration::from_secs(30);
    let result: Result<Value, RpcError> = if destination == "daemon" {
        match command {
            "register_service" | "start_service" | "ping" => Ok(json!({ "success": true })),
            other => Ok(json!({ "success": false, "error": format!("unknown daemon command {other}") })),
        }
    } else if SENSITIVE.contains(&command) {
        let text = Zeroizing::new(data.to_string());
        match wallet.call_sensitive(destination, command, text, t).await {
            Ok(raw) => Ok(serde_json::from_str::<Value>(&raw)
                .map(|v| v["data"].clone())
                .unwrap_or(Value::Null)),
            Err(e) => Err(e),
        }
    } else {
        wallet.call(destination, command, data, t).await
    };
    let mut data = match result {
        Ok(v) => v,
        Err(e) => json!({ "success": false, "error": e.to_string() }),
    };
    if data.get("success").is_none()
        && let Some(obj) = data.as_object_mut()
    {
        obj.insert("success".into(), Value::Bool(true));
    }
    json!({
        "command": command, "ack": true, "data": data, "request_id": req["request_id"],
        "origin": destination, "destination": req["origin"],
    })
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .map(std::path::PathBuf::from)
        .expect("usage: mock_daemon <chia-root> [port]");
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(55400);
    write_root(&root, port);

    let wallet = DemoTransport::new(Duration::from_millis(40));
    let (push_tx, _) = broadcast::channel::<String>(256);
    let mut events = wallet.take_events().unwrap();
    let tx = push_tx.clone();
    tokio::spawn(async move {
        while let Some(ev) = events.next().await {
            if let Event::StateChanged {
                state,
                wallet_id,
                additional_data,
            } = ev
            {
                let msg = json!({
                    "command": "state_changed", "ack": false, "request_id": "00", "origin": "chia_wallet",
                    "destination": "wallet_ui",
                    "data": { "state": state, "wallet_id": wallet_id, "additional_data": additional_data },
                });
                let _ = tx.send(msg.to_string());
            }
        }
    });

    let acceptor = acceptor(&root);
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    println!("mock daemon listening on wss://127.0.0.1:{port} (client certificate required)");
    loop {
        let (tcp, peer) = listener.accept().await.unwrap();
        let (acceptor, wallet, mut pushes) = (acceptor.clone(), wallet.clone(), push_tx.subscribe());
        tokio::spawn(async move {
            let tls = match acceptor.accept(tcp).await {
                Ok(t) => t,
                Err(e) => return println!("{peer}: TLS rejected: {e}"),
            };
            let Ok(ws) = tokio_tungstenite::accept_async(tls).await else {
                return;
            };
            println!("{peer}: connected");
            let (mut sink, mut stream) = ws.split();
            let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let writer = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Some(m) = out_rx.recv() => if sink.send(Message::text(m)).await.is_err() { break },
                        Ok(m) = pushes.recv() => if sink.send(Message::text(m)).await.is_err() { break },
                        else => break,
                    }
                }
            });
            while let Some(Ok(Message::Text(text))) = stream.next().await {
                let Ok(req) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                // Answer concurrently: log_in is slow and must not block pings.
                let (wallet, out_tx) = (wallet.clone(), out_tx.clone());
                tokio::spawn(async move {
                    let reply = answer(&wallet, &req).await;
                    let _ = out_tx.send(reply.to_string());
                });
            }
            writer.abort();
            println!("{peer}: disconnected");
        });
    }
}
