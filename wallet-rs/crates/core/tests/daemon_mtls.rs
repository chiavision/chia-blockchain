//! End-to-end test of the daemon client against a real mutual-TLS websocket
//! server laid out like a Chia root (private CA, daemon cert/key).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chia_wallet_core::client::{DaemonClient, RpcError, Transport};
use chia_wallet_core::config::DaemonConfig;
use chia_wallet_core::protocol::Event;
use futures::{SinkExt, StreamExt};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;

struct Pki {
    ca_pem: String,
    daemon_cert_pem: String,
    daemon_key_pem: String,
}

/// A CA plus a leaf with the `chia.net` SAN, like `chia init` produces.
fn make_pki() -> Pki {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "Chia CA");
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().unwrap()).unwrap();

    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf_params = CertificateParams::new(vec!["chia.net".to_string()]).unwrap();
    leaf_params.distinguished_name.push(DnType::CommonName, "Chia");
    let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();
    Pki {
        ca_pem: ca.pem(),
        daemon_cert_pem: leaf.pem(),
        daemon_key_pem: leaf_key.serialize_pem(),
    }
}

fn write_root(dir: &Path, pki: &Pki) -> DaemonConfig {
    let ssl = dir.join("config/ssl");
    std::fs::create_dir_all(ssl.join("ca")).unwrap();
    std::fs::create_dir_all(ssl.join("daemon")).unwrap();
    std::fs::write(ssl.join("ca/private_ca.crt"), &pki.ca_pem).unwrap();
    std::fs::write(ssl.join("daemon/private_daemon.crt"), &pki.daemon_cert_pem).unwrap();
    std::fs::write(ssl.join("daemon/private_daemon.key"), &pki.daemon_key_pem).unwrap();
    DaemonConfig::from_yaml(dir, "selected_network: mainnet\n", false).unwrap()
}

/// A tiny daemon: requires client certs from `pki`'s CA, answers a few commands.
async fn spawn_daemon(pki: &Pki) -> u16 {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(pki.ca_pem.as_bytes()).unwrap())
        .unwrap();
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(pki.daemon_cert_pem.as_bytes()).unwrap()],
            PrivateKeyDer::from_pem_slice(pki.daemon_key_pem.as_bytes()).unwrap(),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let Ok(mut ws) = tokio_tungstenite::accept_async(tls).await else {
                    return;
                };
                let mut registered = false;
                while let Some(Ok(Message::Text(text))) = ws.next().await {
                    let req: Value = serde_json::from_str(text.as_str()).unwrap();
                    let reply = |data: Value| {
                        json!({
                            "command": req["command"], "ack": true, "data": data,
                            "request_id": req["request_id"], "origin": req["destination"],
                            "destination": req["origin"],
                        })
                    };
                    let out = match req["command"].as_str().unwrap() {
                        "register_service" => {
                            assert_eq!(req["data"]["service"], "wallet_ui");
                            registered = true;
                            reply(json!({ "success": true }))
                        }
                        "get_height_info" => {
                            assert!(registered, "must register before calling");
                            reply(json!({ "height": 424242, "success": true }))
                        }
                        "generate_mnemonic" => reply(json!({ "mnemonic": ["abandon", "art"], "success": true })),
                        "fail" => reply(json!({ "success": false, "error": "boom" })),
                        "push" => json!({
                            "command": "state_changed", "ack": false, "request_id": "ff",
                            "origin": "chia_wallet", "destination": "wallet_ui",
                            "data": { "state": "coin_added", "wallet_id": 7 },
                        }),
                        // Never answer: exercises the client deadline.
                        _ => continue,
                    };
                    ws.send(Message::text(out.to_string())).await.unwrap();
                }
            });
        }
    });
    port
}

async fn wait_connected(client: &DaemonClient) {
    for _ in 0..100 {
        if client.is_connected() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("client never connected");
}

#[tokio::test]
async fn mutual_tls_round_trip() {
    let pki = make_pki();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = write_root(dir.path(), &pki);
    cfg.host = "127.0.0.1".into();
    cfg.port = spawn_daemon(&pki).await;

    let client = DaemonClient::start(&cfg).unwrap();
    let mut events = client.take_events().unwrap();
    wait_connected(&client).await;
    assert_eq!(events.next().await, Some(Event::Connected));

    let t = Duration::from_secs(5);
    let v = client
        .call("chia_wallet", "get_height_info", json!({}), t)
        .await
        .unwrap();
    assert_eq!(v["height"], 424242);

    let err = client.call("chia_wallet", "fail", json!({}), t).await.unwrap_err();
    assert_eq!(err, RpcError::Remote { message: "boom".into() });

    let raw = client
        .call_sensitive(
            "chia_wallet",
            "generate_mnemonic",
            zeroize::Zeroizing::new("{}".into()),
            t,
        )
        .await
        .unwrap();
    assert!(raw.contains("abandon"));

    let _ = client
        .call("chia_wallet", "push", json!({}), Duration::from_millis(1500))
        .await;
    let ev = events.next().await.unwrap();
    assert_eq!(
        ev,
        Event::StateChanged {
            state: "coin_added".into(),
            wallet_id: Some(7),
            additional_data: Value::Null
        }
    );

    let slow = client
        .call("chia_wallet", "never_answers", json!({}), Duration::from_secs(1))
        .await;
    assert_eq!(slow, Err(RpcError::Timeout));
}

#[tokio::test]
async fn refuses_impostor_daemon() {
    // The daemon's cert comes from a different CA than the one on disk.
    let real = make_pki();
    let impostor = make_pki();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = write_root(dir.path(), &real);
    cfg.host = "127.0.0.1".into();
    cfg.port = spawn_daemon(&impostor).await;

    let client = DaemonClient::start(&cfg).unwrap();
    let mut events = client.take_events().unwrap();
    match tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .unwrap()
    {
        Some(Event::Disconnected { reason }) => assert!(reason.contains("TLS"), "{reason}"),
        other => panic!("expected a TLS failure, got {other:?}"),
    }
    assert!(!client.is_connected());
    let r = client
        .call("chia_wallet", "get_height_info", json!({}), Duration::from_secs(1))
        .await;
    assert_eq!(r, Err(RpcError::NotConnected));
}

#[test]
fn missing_certificates_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DaemonConfig::from_yaml(dir.path(), "{}", false).unwrap();
    let err = DaemonClient::start(&cfg).err().expect("should fail").to_string();
    assert!(err.contains("private_ca.crt"), "{err}");
}
