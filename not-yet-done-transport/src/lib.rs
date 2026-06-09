//! Generic transport layer for adapters that need to reach a remote
//! TCP service either directly or through an SSH bastion.
//!
//! Adapters embed a [`TransportConfig`] block in their YAML config and
//! call [`connect`]. The returned [`Connection`] exposes a local
//! `host:port` the adapter connects to with its native client (e.g.
//! `tokio_postgres`, `mysql_async`, `redis`, …); whether that endpoint
//! is the literal target or the local end of an SSH-forwarded channel
//! is transparent to the adapter.
//!
//! Crate boundaries:
//!
//! - This crate has **no** content-tree concepts. It deals in TCP
//!   plumbing, SSH auth, and YAML config — nothing more.
//! - It depends on `not-yet-done-content` solely for the
//!   [`CredentialProvider`] vocabulary, so SSH auth feels identical to
//!   service auth (same kinds: `literal | env | file | command |
//!   keyring`).
//! - russh is an internal implementation detail; nothing in the public
//!   API exposes russh types.

mod config;
mod tunnel;

pub use config::{Endpoint, SshAuth, SshHop, TransportConfig, TransportMode};

use std::sync::Arc;

use thiserror::Error;

use tunnel::Tunnel;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("invalid transport config: {0}")]
    InvalidConfig(String),
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("ssh connect failed: {0}")]
    SshConnect(String),
    #[error("ssh auth failed: {0}")]
    SshAuth(String),
    #[error("ssh channel error: {0}")]
    Channel(String),
    #[error("credential provider error: {0}")]
    Provider(String),
}

/// A reachable TCP endpoint. In direct mode this is the literal
/// `target`; in tunnel mode it's the local end of the running SSH
/// forward. Holding the [`Connection`] keeps the tunnel open; drop it
/// to tear the tunnel down.
pub struct Connection {
    pub host: String,
    pub port: u16,
    // `Arc` so multiple consumers can share one tunnel cheaply if the
    // adapter ever wants to. For now the adapter holds exactly one
    // `Connection` per `ContentAdapter` instance.
    _tunnel: Option<Arc<Tunnel>>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tunneled", &self._tunnel.is_some())
            .finish()
    }
}

impl Connection {
    /// Convenience: `host:port`, ready to feed into a TCP/Postgres URL.
    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Establish whatever transport the config asks for. Returns a
/// [`Connection`] whose `host:port` is the address the adapter should
/// open its native client connection against.
pub async fn connect(config: &TransportConfig) -> Result<Connection, TransportError> {
    config.validate().map_err(TransportError::InvalidConfig)?;

    match config.mode {
        TransportMode::Direct => Ok(Connection {
            host: config.target.host.clone(),
            port: config.target.port,
            _tunnel: None,
        }),
        TransportMode::SshTunnel => {
            let tunnel = tunnel::start(config.ssh.clone(), config.target.clone()).await?;
            let port = tunnel.local_port;
            Ok(Connection {
                host: "127.0.0.1".into(),
                port,
                _tunnel: Some(Arc::new(tunnel)),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn direct_mode_returns_target_verbatim() {
        let cfg: TransportConfig = serde_yaml::from_str(
            r#"
target:
  host: db.internal.invalid
  port: 5432
"#,
        )
        .unwrap();

        let conn = connect(&cfg).await.expect("direct must succeed");
        assert_eq!(conn.host, "db.internal.invalid");
        assert_eq!(conn.port, 5432);
        assert_eq!(conn.socket_addr(), "db.internal.invalid:5432");
    }

    #[tokio::test]
    async fn invalid_config_surfaces_clear_error() {
        let cfg: TransportConfig = serde_yaml::from_str(
            r#"
mode: ssh_tunnel
target:
  host: db.internal.invalid
  port: 5432
"#,
        )
        .unwrap();

        let err = connect(&cfg).await.expect_err("must reject");
        assert!(matches!(err, TransportError::InvalidConfig(_)));
        assert!(
            err.to_string().contains("ssh_tunnel"),
            "error mentions ssh_tunnel: {err}"
        );
    }
}
