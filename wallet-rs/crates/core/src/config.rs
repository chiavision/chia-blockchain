//! Locate the Chia root and read what we need from `config/config.yaml`.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_yaml::Value;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("could not parse {path}: {source}")]
    Parse { path: PathBuf, source: serde_yaml::Error },
    #[error("no home directory; set CHIA_ROOT")]
    NoHome,
    #[error(
        "daemon host `{0}` is not a loopback address. The daemon holds your keys; \
         pass --allow-remote-daemon only if you run it on a machine you control"
    )]
    RemoteDaemonRefused(String),
}

/// Everything needed to reach the daemon.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub root: PathBuf,
    pub host: String,
    pub port: u16,
    /// Our TLS client identity (`private_daemon.crt/.key`).
    pub client_cert: PathBuf,
    pub client_key: PathBuf,
    /// The private CA the daemon's certificate must chain to.
    pub ca_cert: PathBuf,
    pub network: String,
    pub address_prefix: String,
}

/// `$CHIA_ROOT`, or `~/.chia/mainnet` like the Python backend.
pub fn default_root() -> Result<PathBuf, ConfigError> {
    if let Some(root) = std::env::var_os("CHIA_ROOT") {
        return Ok(PathBuf::from(root));
    }
    Ok(dirs::home_dir()
        .ok_or(ConfigError::NoHome)?
        .join(".chia")
        .join("mainnet"))
}

fn get<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, key| cur.get(*key))
}

fn get_str(v: &Value, path: &[&str]) -> Option<String> {
    get(v, path).and_then(Value::as_str).map(str::to_owned)
}

impl DaemonConfig {
    pub fn load(root: &Path, allow_remote: bool) -> Result<Self, ConfigError> {
        let path = root.join("config").join("config.yaml");
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        Self::from_yaml(root, &text, allow_remote).map_err(|e| match e {
            ConfigError::Parse { source, .. } => ConfigError::Parse { path, source },
            other => other,
        })
    }

    pub fn from_yaml(root: &Path, text: &str, allow_remote: bool) -> Result<Self, ConfigError> {
        let cfg: Value = serde_yaml::from_str(text).map_err(|source| ConfigError::Parse {
            path: PathBuf::new(),
            source,
        })?;

        let host = get_str(&cfg, &["ui", "daemon_host"])
            .or_else(|| get_str(&cfg, &["self_hostname"]))
            .unwrap_or_else(|| "localhost".into());
        if !allow_remote && !is_loopback(&host) {
            return Err(ConfigError::RemoteDaemonRefused(host));
        }
        let port = get(&cfg, &["ui", "daemon_port"])
            .or_else(|| get(&cfg, &["daemon_port"]))
            .and_then(Value::as_u64)
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or(55400);

        let rel = |p: Option<String>, default: &str| root.join(p.unwrap_or_else(|| default.into()));
        let client_cert = rel(
            get_str(&cfg, &["ui", "daemon_ssl", "private_crt"])
                .or_else(|| get_str(&cfg, &["daemon_ssl", "private_crt"])),
            "config/ssl/daemon/private_daemon.crt",
        );
        let client_key = rel(
            get_str(&cfg, &["ui", "daemon_ssl", "private_key"])
                .or_else(|| get_str(&cfg, &["daemon_ssl", "private_key"])),
            "config/ssl/daemon/private_daemon.key",
        );
        let ca_cert = rel(
            get_str(&cfg, &["private_ssl_ca", "crt"]),
            "config/ssl/ca/private_ca.crt",
        );

        let network = get_str(&cfg, &["selected_network"]).unwrap_or_else(|| "mainnet".into());
        let address_prefix = get_str(&cfg, &["network_overrides", "config", &network, "address_prefix"])
            .unwrap_or_else(|| {
                if network == "mainnet" {
                    "xch".into()
                } else {
                    "txch".into()
                }
            });

        Ok(Self {
            root: root.to_path_buf(),
            host,
            port,
            client_cert,
            client_key,
            ca_cert,
            network,
            address_prefix,
        })
    }
}

/// `localhost` or a loopback IP literal.
pub fn is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Returns a warning if a private key file can be read by other users.
#[cfg(unix)]
pub fn key_file_warning(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).ok()?;
    let mode = meta.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then(|| {
        format!(
            "{} is readable by other users (mode {mode:o}); run chmod 600",
            path.display()
        )
    })
}

#[cfg(not(unix))]
pub fn key_file_warning(_path: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
daemon_port: 55400
self_hostname: &self_hostname "localhost"
network_overrides: &network_overrides
  config:
    mainnet:
      address_prefix: "xch"
    testnet7:
      address_prefix: "txch"
selected_network: &selected_network "testnet7"
private_ssl_ca:
  crt: "config/ssl/ca/private_ca.crt"
ui:
  daemon_host: *self_hostname
  daemon_port: 55401
  daemon_ssl:
    private_crt: config/ssl/daemon/private_daemon.crt
    private_key: config/ssl/daemon/private_daemon.key
  selected_network: *selected_network
"#;

    #[test]
    fn reads_chia_config_with_anchors() {
        let c = DaemonConfig::from_yaml(Path::new("/r"), SAMPLE, false).unwrap();
        assert_eq!(c.host, "localhost");
        assert_eq!(c.port, 55401);
        assert_eq!(c.network, "testnet7");
        assert_eq!(c.address_prefix, "txch");
        assert_eq!(c.client_cert, Path::new("/r/config/ssl/daemon/private_daemon.crt"));
        assert_eq!(c.ca_cert, Path::new("/r/config/ssl/ca/private_ca.crt"));
    }

    #[test]
    fn refuses_remote_daemon_unless_allowed() {
        let remote = SAMPLE.replace("daemon_host: *self_hostname", "daemon_host: 10.0.0.5");
        assert!(matches!(
            DaemonConfig::from_yaml(Path::new("/r"), &remote, false),
            Err(ConfigError::RemoteDaemonRefused(_))
        ));
        assert!(DaemonConfig::from_yaml(Path::new("/r"), &remote, true).is_ok());
    }

    #[test]
    fn loopback() {
        assert!(is_loopback("localhost"));
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("[::1]"));
        assert!(!is_loopback("192.168.1.2"));
        assert!(!is_loopback("localhost.evil.com"));
    }
}
