//! Adapter YAML config: transport block (re-used from
//! `not-yet-done-transport`) plus the Postgres-side credentials.
//!
//! Database list is *not* configured here — it's fetched live from
//! `pg_database` on first list. There is also no `databases:` block:
//! the user is expected to discover the catalogue from the server,
//! same as DBeaver / psql `\l`.

use fieldsmith::Buildable;
use serde::Deserialize;

use not_yet_done_content::{AuthSpec, CredentialProvider};
use not_yet_done_transport::{SshAuth, TransportConfig};

use crate::adapter::auth::{FIELD_PASSWORD, FIELD_SSH_KEY_PASSPHRASE, FIELD_SSH_PASSWORD};

#[derive(Deserialize, Buildable, Clone, Debug)]
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

    /// Where the interactive secrets come from, when any provider slot
    /// below asks for them (`{type: script-result}` / `{type: prompt}`).
    ///
    /// One block for the whole connection rather than one per slot: a
    /// credential script that unlocks a password store should run *once*
    /// and hand back the database password and the tunnel's secret
    /// together — that is the entire point of routing them through the
    /// auth system instead of letting two `command` providers open two
    /// pinentry windows.
    #[serde(default)]
    pub auth: Option<AuthSpec>,

    pub transport: TransportConfig,

    pub postgres: PostgresAuth,
}

/// A provider slot in this config that the `auth:` block owns, paired
/// with the mechanism field expected to fill it.
///
/// The mapping is positional, not spelled out in YAML: `postgres.password`
/// takes `password`, a hop's password takes `ssh_password`, a key
/// passphrase takes `ssh_key_passphrase`. That keeps the common case free
/// of ceremony; the price is that only *one* hop may delegate each kind of
/// secret, which [`PostgresConfig::frontend_slots`] enforces instead of
/// silently feeding the wrong hop.
#[derive(Debug, PartialEq, Eq)]
pub struct FrontendSlot {
    pub field: &'static str,
    /// Where in the config the slot sits, for error messages.
    pub origin: String,
}

#[derive(Deserialize, Buildable, Clone, Debug)]
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
#[derive(Deserialize, Buildable, Clone, Copy, Debug, PartialEq, Eq, Default)]
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
    /// Validate cross-field invariants: everything
    /// `TransportConfig::validate` checks, plus the pairing between the
    /// `auth:` block and the provider slots that delegate to it.
    pub fn validate(&self) -> Result<(), String> {
        self.transport.validate()?;
        let slots = self.frontend_slots()?;

        match &self.auth {
            None => {
                if let Some(slot) = slots.first() {
                    return Err(format!(
                        "{} needs an interactive credential but there is no `auth:` block to \
                         supply it — add one binding `{}`, or give the slot a self-contained \
                         provider (command / keyring / env / file)",
                        slot.origin, slot.field
                    ));
                }
            }
            Some(spec) => {
                if slots.is_empty() {
                    return Err(
                        "`auth:` is configured but nothing consumes it — point a slot such as \
                         `postgres.password` at `{type: script-result}`"
                            .into(),
                    );
                }
                for slot in &slots {
                    if !spec.bindings.iter().any(|b| b.field == slot.field) {
                        return Err(format!(
                            "{} delegates to `auth:`, which has no binding for field `{}`",
                            slot.origin, slot.field
                        ));
                    }
                }
                for binding in &spec.bindings {
                    if !slots.iter().any(|s| s.field == binding.field) {
                        return Err(format!(
                            "auth binding `{}` is never used — no provider slot delegates to it",
                            binding.field
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Every provider slot that cannot resolve on its own and therefore
    /// belongs to the `auth:` block, in config order. Errors when two
    /// slots would claim the same mechanism field, because the mapping is
    /// positional and there would be no way to tell which hop meant which
    /// secret.
    pub fn frontend_slots(&self) -> Result<Vec<FrontendSlot>, String> {
        let mut slots: Vec<FrontendSlot> = Vec::new();
        let mut push = |field: &'static str, origin: String| -> Result<(), String> {
            if let Some(prev) = slots.iter().find(|s| s.field == field) {
                return Err(format!(
                    "{origin} and {} both delegate `{field}` to `auth:` — the auth block has one \
                     value per field, so at most one slot may take it; give the other an explicit \
                     provider",
                    prev.origin
                ));
            }
            slots.push(FrontendSlot { field, origin });
            Ok(())
        };

        if self.postgres.password.needs_frontend() {
            push(FIELD_PASSWORD, "postgres.password".into())?;
        }
        for (i, hop) in self.transport.ssh.iter().enumerate() {
            match &hop.auth {
                SshAuth::Password { password } if password.needs_frontend() => {
                    push(
                        FIELD_SSH_PASSWORD,
                        format!("transport.ssh[{i}].auth.password"),
                    )?;
                }
                SshAuth::PublicKey {
                    passphrase: Some(p),
                    ..
                } if p.needs_frontend() => {
                    push(
                        FIELD_SSH_KEY_PASSPHRASE,
                        format!("transport.ssh[{i}].auth.passphrase"),
                    )?;
                }
                _ => {}
            }
        }
        Ok(slots)
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

    /// Config where the database password and the tunnel's password both
    /// come out of one credential script — the case the `auth:` block
    /// exists for. `extra_binding` / `second_hop` let the tests below bend
    /// exactly one thing about it.
    fn delegating_yaml(extra_binding: &str, second_hop: &str) -> String {
        format!(
            r#"
transport:
  mode: ssh_tunnel
  ssh:
    - host: bastion.example.invalid
      user: alice
      auth:
        kind: password
        password: {{ type: script-result }}
{second_hop}
  target: {{ host: db.internal.invalid, port: 5432 }}
auth:
  mechanism: password
  script: /home/alice/.config/not_yet_done/scripts/pass_credentials.py
  bindings:
    - field: password
      provider: {{ type: script-result }}
    - field: ssh_password
      provider: {{ type: script-result }}
{extra_binding}
postgres:
  user: warehouse_ro
  password: {{ type: script-result }}
"#
        )
    }

    #[test]
    fn auth_block_feeds_the_database_password_and_the_tunnel() {
        let cfg = parse(&delegating_yaml("", ""));
        cfg.validate().expect("valid");

        let slots = cfg.frontend_slots().expect("unambiguous");
        assert_eq!(
            slots.iter().map(|s| s.field).collect::<Vec<_>>(),
            vec![FIELD_PASSWORD, FIELD_SSH_PASSWORD],
        );
        // The vocabulary the factory checks the block against has to know
        // these fields, or the config would validate here and fail there.
        cfg.auth
            .as_ref()
            .expect("auth block")
            .validate_against(crate::adapter::auth::MECHANISMS)
            .expect("mechanism and fields exist");
    }

    #[test]
    fn rejects_a_delegating_slot_without_an_auth_block() {
        let cfg = parse(
            r#"
transport:
  target: { host: db.internal.invalid, port: 5432 }
postgres:
  user: dbuser
  password: { type: script-result }
"#,
        );
        let err = cfg.validate().expect_err("nothing can supply that slot");
        assert!(
            err.contains("postgres.password") && err.contains("auth:"),
            "{err}"
        );
    }

    #[test]
    fn rejects_an_auth_block_nothing_consumes() {
        let cfg = parse(
            r#"
transport:
  target: { host: db.internal.invalid, port: 5432 }
auth:
  mechanism: password
  bindings:
    - field: password
      provider: { type: literal, value: x }
postgres:
  user: dbuser
  password: { type: command, script: pass postgres/example }
"#,
        );
        let err = cfg.validate().expect_err("auth block is dead weight");
        assert!(err.contains("nothing consumes it"), "{err}");
    }

    #[test]
    fn rejects_a_delegating_slot_with_no_binding() {
        // The tunnel delegates its password, but the block only binds the
        // database one — the hop would end up with no secret at all.
        let yaml = delegating_yaml("", "").replace(
            "    - field: ssh_password\n      provider: { type: script-result }\n",
            "",
        );
        let err = parse(&yaml).validate().expect_err("hop has no binding");
        assert!(
            err.contains("transport.ssh[0].auth.password") && err.contains("ssh_password"),
            "{err}"
        );
    }

    #[test]
    fn rejects_a_binding_no_slot_claims() {
        let cfg = parse(&delegating_yaml(
            "    - field: ssh_key_passphrase\n      provider: { type: script-result }\n",
            "",
        ));
        let err = cfg.validate().expect_err("binding is never read");
        assert!(
            err.contains("ssh_key_passphrase") && err.contains("never used"),
            "{err}"
        );
    }

    #[test]
    fn rejects_two_hops_delegating_the_same_field() {
        // The slot-to-field mapping is positional, so a second delegating
        // hop has no way to say which secret it means.
        let cfg = parse(&delegating_yaml(
            "",
            r#"    - host: jump.example.invalid
      user: alice
      auth:
        kind: password
        password: { type: script-result }"#,
        ));
        let err = cfg.validate().expect_err("ambiguous");
        assert!(
            err.contains("transport.ssh[1].auth.password") && err.contains("ssh_password"),
            "{err}"
        );
    }

    #[test]
    fn shipped_example_adapter_config_parses_and_validates() {
        // The example referenced by docs/examples/views/postgres.yaml must
        // stay schema-valid (PostgresConfig uses deny_unknown_fields, so a
        // renamed/removed field would silently break the shipped example).
        let yaml = include_str!("../../docs/examples/views/postgres-adapter.yaml");
        let cfg = parse(yaml);
        cfg.validate()
            .expect("example adapter config should validate");
        assert_eq!(cfg.name.as_deref(), Some("example-warehouse"));
        assert_eq!(cfg.postgres.sslmode, SslMode::Require);
    }
}
