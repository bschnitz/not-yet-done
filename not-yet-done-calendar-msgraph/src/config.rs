//! Backend config block for a Microsoft Graph connection.

use serde::Deserialize;

use not_yet_done_content::CredentialProvider;

/// Default per-request timeout (seconds) — long enough for a slow but healthy
/// Graph response, short enough that a dead connection surfaces instead of
/// freezing the poll loop.
pub(crate) const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 20;

/// The `config:` sub-tree of a `backend: microsoft` connection entry, e.g.
///
/// ```yaml
/// token:
///   type: command
///   script: >-
///     az account get-access-token --resource https://graph.microsoft.com
///     --tenant <TENANT_ID> --query accessToken -o tsv
/// name: "Work (contoso)"
/// ```
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct MsGraphConfig {
    /// How to obtain a Graph bearer token. Any credential provider works
    /// (`command`, `env`, `file`, `keyring`, `literal`); `command` wrapping
    /// the Azure CLI is the intended default. Resolved lazily and re-resolved
    /// on a 401 so an expired token refreshes transparently.
    pub(crate) token: CredentialProvider,
    /// Display label for this connection (the "Account" column). Defaults to
    /// the connection id.
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// Override the Graph base URL — only useful for tests / sovereign clouds.
    /// Defaults to the public `https://graph.microsoft.com`.
    #[serde(default)]
    pub(crate) base_url: Option<String>,
    #[serde(default)]
    pub(crate) request_timeout_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_token_config() {
        let yaml = r#"
token:
  type: command
  script: get-graph-token.sh
name: Work
"#;
        let cfg: MsGraphConfig = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(cfg.name.as_deref(), Some("Work"));
        assert!(matches!(cfg.token, CredentialProvider::Command { .. }));
        assert_eq!(cfg.request_timeout_secs, None);
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = "token:\n  type: literal\n  value: x\nbogus: 1\n";
        assert!(serde_yaml::from_str::<MsGraphConfig>(yaml).is_err());
    }
}
