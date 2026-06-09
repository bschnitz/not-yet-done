//! Adapter YAML config: transport block (re-used from
//! `not-yet-done-transport`) plus the Postgres-side credentials.
//!
//! Database list is *not* configured here — it's fetched live from
//! `pg_database` on first list. There is also no `databases:` block:
//! the user is expected to discover the catalogue from the server,
//! same as DBeaver / psql `\l`.

use serde::Deserialize;

use not_yet_done_content::CredentialProvider;
use not_yet_done_transport::TransportConfig;

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PostgresConfig {
    /// Optional human-readable name for the connection (shown as the
    /// root label in the TUI). Defaults to `postgres@<target.host>`.
    #[serde(default)]
    pub name: Option<String>,

    /// Per-call timeout for postgres operations (list databases, list
    /// schemas/tables, paged row fetches, custom SQL). When the
    /// deadline fires the in-flight session and the underlying
    /// transport (SSH tunnel) are torn down — the next call lazily
    /// reconnects via the normal session-recovery path. `None` (the
    /// default) means "wait forever", matching libpq's default.
    ///
    /// Why this exists: long-lived SSH tunnels can stall silently
    /// (half-open TCP after a network blip) and tokio-postgres has no
    /// way to notice until the kernel times the socket out, which can
    /// take minutes. A short, explicit deadline lets the user fail
    /// fast and trigger a fresh connect.
    #[serde(default)]
    pub query_timeout_secs: Option<u64>,

    pub transport: TransportConfig,

    pub postgres: PostgresAuth,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PostgresAuth {
    pub user: String,
    pub password: CredentialProvider,
    /// The Postgres "maintenance" database to connect to in order to
    /// query `pg_database`. Defaults to `postgres`, which exists on
    /// every standard install.
    #[serde(default = "default_admin_db")]
    pub admin_database: String,
    /// Postgres `sslmode`. Defaults to `prefer`, matching libpq.
    #[serde(default)]
    pub sslmode: SslMode,
}

/// SSL mode mirroring `tokio_postgres`'s subset of libpq sslmodes.
/// `allow` is intentionally omitted — tokio-postgres does not support
/// it, and the typical use case ("try plaintext, fall back to TLS")
/// is the inverse of `prefer` and rarely useful in practice.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
}

fn default_admin_db() -> String {
    "postgres".into()
}

impl PostgresConfig {
    /// Validate cross-field invariants. Wraps `TransportConfig::validate`
    /// and adds Postgres-specific checks (currently none beyond what
    /// `serde(deny_unknown_fields)` already enforces).
    pub fn validate(&self) -> Result<(), String> {
        self.transport.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> PostgresConfig {
        serde_yaml::from_str(yaml).expect("yaml parses")
    }

    #[test]
    fn direct_with_keyring_password() {
        let cfg = parse(
            r#"
transport:
  target:
    host: db.internal.invalid
    port: 5432
postgres:
  user: dbuser
  password:
    type: keyring
    service: nyd-postgres-prod
    account: dbuser
"#,
        );
        cfg.validate().expect("valid");
        assert_eq!(cfg.postgres.user, "dbuser");
        assert_eq!(cfg.postgres.admin_database, "postgres");
        assert_eq!(cfg.postgres.sslmode, SslMode::Prefer);
        assert!(matches!(
            cfg.postgres.password,
            CredentialProvider::Keyring { .. }
        ));
    }

    #[test]
    fn ssh_tunnel_with_command_password_and_explicit_admin_db() {
        let cfg = parse(
            r#"
name: prod-warehouse
transport:
  mode: ssh_tunnel
  ssh:
    - host: bastion.example.invalid
      user: alice
      auth:
        kind: public_key
        identity: /home/alice/.ssh/id_ed25519
  target:
    host: db.internal.invalid
    port: 5432
postgres:
  user: warehouse_ro
  password:
    type: command
    script: pass postgres/prod-warehouse
  admin_database: warehouse
  sslmode: require
"#,
        );
        cfg.validate().expect("valid");
        assert_eq!(cfg.name.as_deref(), Some("prod-warehouse"));
        assert_eq!(cfg.postgres.admin_database, "warehouse");
        assert_eq!(cfg.postgres.sslmode, SslMode::Require);
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r#"
transport:
  target: { host: db.internal.invalid, port: 5432 }
postgres:
  user: dbuser
  password: { type: literal, value: x }
foo: bar
"#;
        let res: Result<PostgresConfig, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err(), "unknown top-level field must fail to parse");
    }

    #[test]
    fn rejects_unknown_postgres_field() {
        let yaml = r#"
transport:
  target: { host: db.internal.invalid, port: 5432 }
postgres:
  user: dbuser
  password: { type: literal, value: x }
  schema: public
"#;
        let res: Result<PostgresConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            res.is_err(),
            "unknown postgres field must fail to parse: {res:?}"
        );
    }

    #[test]
    fn rejects_invalid_transport() {
        let cfg = parse(
            r#"
transport:
  mode: ssh_tunnel
  target: { host: db.internal.invalid, port: 5432 }
postgres:
  user: dbuser
  password: { type: literal, value: x }
"#,
        );
        let err = cfg.validate().expect_err("ssh_tunnel needs ssh block");
        assert!(err.contains("ssh_tunnel"), "{err}");
    }
}
