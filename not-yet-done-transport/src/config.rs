//! Transport YAML config: direct connection or SSH-tunneled.
//!
//! `mode: direct` (default) skips tunneling entirely; the consumer
//! connects to `target.host:target.port` itself.
//!
//! `mode: ssh_tunnel` opens an in-process SSH session to the first
//! `ssh` hop (no `ssh` subprocess) and forwards a local TCP listener
//! on `127.0.0.1:<ephemeral>` to `target.host:target.port` through it.
//!
//! Multiple hops form a jump chain: each subsequent hop is reached via
//! a `direct-tcpip` channel on the previous hop's session, then a
//! fresh SSH handshake runs over that channel. Names and addresses on
//! every hop except the first are resolved by the **previous** hop —
//! so `localhost:2222` on hop #2 means localhost relative to hop #1.
//! `target.host:target.port` is resolved by the last hop.
//!
//! Auth re-uses the [`CredentialProvider`] vocabulary from
//! `not-yet-done-content` for password and key passphrase fields, so
//! adapters get keyring/env/command/literal/file consistently across
//! service auth and SSH auth.

use std::path::PathBuf;

use fieldsmith::Buildable;
use serde::Deserialize;

use not_yet_done_content::CredentialProvider;

/// Top-level transport block.
#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TransportConfig {
    #[serde(default)]
    pub mode: TransportMode,
    /// SSH hop chain. Required when `mode = ssh_tunnel` (≥1 entry),
    /// must be empty when `mode = direct`. Each entry adds one SSH
    /// session in front of the previous; only the last hop opens the
    /// `direct-tcpip` to `target`.
    #[serde(default)]
    pub ssh: Vec<SshHop>,
    /// The endpoint the consumer ultimately wants to reach. In direct
    /// mode this is the literal address; in tunnel mode the **last**
    /// SSH hop resolves it on the remote side.
    pub target: Endpoint,
}

#[derive(Deserialize, Buildable, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    #[default]
    Direct,
    SshTunnel,
}

#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

/// One SSH hop in the chain. The first hop's `host:port` is resolved
/// locally; every subsequent hop's `host:port` is resolved by the
/// preceding hop.
#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SshHop {
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
}

fn default_ssh_port() -> u16 {
    22
}

/// SSH authentication strategy.
#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SshAuth {
    /// Password auth. The value comes from any [`CredentialProvider`]
    /// shape (literal / env / file / command / keyring).
    Password { password: CredentialProvider },
    /// Public-key auth. `identity` is the path to the OpenSSH private
    /// key (`~/.ssh/id_ed25519`, `~/.ssh/id_rsa`, …). `passphrase` is
    /// only consulted if the key file is encrypted.
    PublicKey {
        identity: PathBuf,
        #[serde(default)]
        passphrase: Option<CredentialProvider>,
    },
    /// Forward to a running `ssh-agent`. Requires `SSH_AUTH_SOCK`.
    Agent,
}

impl TransportConfig {
    /// Validate cross-field invariants. Adapters call this during
    /// config parsing so misconfigurations fail fast with a clear
    /// message instead of later inside the tunnel runtime.
    pub fn validate(&self) -> Result<(), String> {
        match self.mode {
            TransportMode::Direct => {
                if !self.ssh.is_empty() {
                    return Err("transport.mode=direct must not include any `ssh:` hops".into());
                }
            }
            TransportMode::SshTunnel => {
                if self.ssh.is_empty() {
                    return Err(
                        "transport.mode=ssh_tunnel requires at least one entry under `ssh:`".into(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> TransportConfig {
        serde_yaml::from_str(yaml).expect("yaml parses")
    }

    #[test]
    fn direct_mode_is_default() {
        let cfg = parse(
            r#"
target:
  host: db.internal.invalid
  port: 5432
"#,
        );
        cfg.validate().expect("valid");
        assert_eq!(cfg.mode, TransportMode::Direct);
        assert!(cfg.ssh.is_empty());
        assert_eq!(cfg.target.host, "db.internal.invalid");
        assert_eq!(cfg.target.port, 5432);
    }

    #[test]
    fn ssh_tunnel_with_public_key_default_port() {
        let cfg = parse(
            r#"
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
"#,
        );
        cfg.validate().expect("valid");
        assert_eq!(cfg.mode, TransportMode::SshTunnel);
        assert_eq!(cfg.ssh.len(), 1);
        let hop = &cfg.ssh[0];
        assert_eq!(hop.port, 22, "ssh port defaults to 22");
        match &hop.auth {
            SshAuth::PublicKey { passphrase, .. } => assert!(passphrase.is_none()),
            other => panic!("unexpected auth: {other:?}"),
        }
    }

    #[test]
    fn ssh_tunnel_with_password_via_keyring() {
        let cfg = parse(
            r#"
mode: ssh_tunnel
ssh:
  - host: bastion.example.invalid
    port: 2222
    user: alice
    auth:
      kind: password
      password:
        type: keyring
        service: nyd-ssh-bastion
        account: alice
target:
  host: db.internal.invalid
  port: 5432
"#,
        );
        cfg.validate().expect("valid");
        assert_eq!(cfg.ssh.len(), 1);
        let hop = &cfg.ssh[0];
        assert_eq!(hop.port, 2222);
        match &hop.auth {
            SshAuth::Password { password } => {
                assert!(matches!(password, CredentialProvider::Keyring { .. }))
            }
            other => panic!("unexpected auth: {other:?}"),
        }
    }

    #[test]
    fn ssh_tunnel_with_encrypted_key_and_command_passphrase() {
        let cfg = parse(
            r#"
mode: ssh_tunnel
ssh:
  - host: bastion.example.invalid
    user: alice
    auth:
      kind: public_key
      identity: /home/alice/.ssh/id_ed25519
      passphrase:
        type: command
        script: pass ssh/bastion-key
target:
  host: db.internal.invalid
  port: 5432
"#,
        );
        cfg.validate().expect("valid");
        match &cfg.ssh[0].auth {
            SshAuth::PublicKey { passphrase, .. } => {
                assert!(matches!(
                    passphrase,
                    Some(CredentialProvider::Command { .. })
                ));
            }
            other => panic!("unexpected auth: {other:?}"),
        }
    }

    #[test]
    fn ssh_tunnel_agent_auth() {
        let cfg = parse(
            r#"
mode: ssh_tunnel
ssh:
  - host: bastion.example.invalid
    user: alice
    auth:
      kind: agent
target:
  host: db.internal.invalid
  port: 5432
"#,
        );
        cfg.validate().expect("valid");
        match &cfg.ssh[0].auth {
            SshAuth::Agent => {}
            other => panic!("unexpected auth: {other:?}"),
        }
    }

    #[test]
    fn ssh_tunnel_two_hops_mixed_auth() {
        let cfg = parse(
            r#"
mode: ssh_tunnel
ssh:
  - host: jump.example.invalid
    user: jumper
    auth:
      kind: public_key
      identity: /home/jumper/.ssh/id_rsa
  - host: localhost
    port: 2222
    user: someone
    auth:
      kind: password
      password:
        type: literal
        value: hunter2
target:
  host: localhost
  port: 5432
"#,
        );
        cfg.validate().expect("valid");
        assert_eq!(cfg.ssh.len(), 2);
        assert!(matches!(cfg.ssh[0].auth, SshAuth::PublicKey { .. }));
        assert!(matches!(cfg.ssh[1].auth, SshAuth::Password { .. }));
        assert_eq!(cfg.ssh[1].port, 2222);
    }

    #[test]
    fn rejects_direct_mode_with_ssh_hops() {
        let cfg = parse(
            r#"
mode: direct
ssh:
  - host: bastion.example.invalid
    user: alice
    auth: { kind: agent }
target:
  host: db.internal.invalid
  port: 5432
"#,
        );
        let err = cfg
            .validate()
            .expect_err("direct + ssh hops must not validate");
        assert!(err.contains("direct"), "error mentions direct: {err}");
    }

    #[test]
    fn rejects_ssh_tunnel_without_hops() {
        let cfg = parse(
            r#"
mode: ssh_tunnel
target:
  host: db.internal.invalid
  port: 5432
"#,
        );
        let err = cfg
            .validate()
            .expect_err("ssh_tunnel without hops must not validate");
        assert!(
            err.contains("ssh_tunnel"),
            "error mentions ssh_tunnel: {err}"
        );
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r#"
mode: direct
foo: bar
target: { host: db.internal.invalid, port: 5432 }
"#;
        let res: Result<TransportConfig, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err(), "unknown top-level field must fail to parse");
    }

    #[test]
    fn rejects_unknown_auth_kind() {
        let yaml = r#"
mode: ssh_tunnel
ssh:
  - host: bastion.example.invalid
    user: alice
    auth:
      kind: kerberos
target: { host: db.internal.invalid, port: 5432 }
"#;
        let res: Result<TransportConfig, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err(), "unknown auth kind must fail to parse");
    }
}
