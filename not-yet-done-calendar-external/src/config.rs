//! Backend config block for a `backend: external` connection.

use std::collections::BTreeMap;

use serde::Deserialize;

use not_yet_done_content::CredentialProvider;
use not_yet_done_content::auth::{CredentialPrompts, CredentialResolver};

/// The `config:` sub-tree of an `external` connection entry, e.g.
///
/// ```yaml
/// command: my-calendar-backend --verbose   # run under `sh -c`
/// name: "Work"                             # "Account" column label (optional)
/// share: work                              # same command + share → one process
/// secrets:                                 # resolved by nyd, handed over in `start`
///   password:
///     type: command
///     script: pass show work/calendar
/// config:                                  # passed to the backend verbatim
///   account: user@example.com
/// ```
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalConfig {
    /// The backend program, run under `sh -c`.
    pub(crate) command: String,
    /// Display label for the "Account" column. Defaults to the connection id.
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// Connections naming the same `command` and `share` key talk to one
    /// process — one sign-in for both. Default: the connection id, so every
    /// connection has its own.
    #[serde(default)]
    pub(crate) share: Option<String>,
    /// The secrets the backend needs, by the name it knows them by, and how
    /// nyd obtains each. Any credential provider works; a `script` provider
    /// may ask its passphrase through the frontend.
    #[serde(default)]
    pub(crate) secrets: BTreeMap<String, CredentialProvider>,
    /// Handed to the backend as-is in `start`.
    #[serde(default)]
    pub(crate) config: serde_yaml::Value,
    /// How long one call may take before nyd gives up on it. Long by default:
    /// a call may wait on an interactive sign-in.
    #[serde(default = "default_call_timeout_secs")]
    pub(crate) call_timeout_secs: u64,
}

fn default_call_timeout_secs() -> u64 {
    600
}

impl ExternalConfig {
    /// One credential resolver per configured secret.
    pub(crate) fn build_secret_resolvers(
        &self,
        prompts: &CredentialPrompts,
    ) -> Result<BTreeMap<String, Box<dyn CredentialResolver>>, String> {
        self.secrets
            .iter()
            .map(|(name, provider)| {
                provider
                    .build_resolver_with(Some(prompts))
                    .map(|resolver| (name.clone(), resolver))
            })
            .collect()
    }

    /// The `config:` block as the JSON the `start` line carries.
    pub(crate) fn backend_config(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(&self.config).map_err(|e| format!("config: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_all_it_takes() {
        let cfg: ExternalConfig = serde_yaml::from_str("command: my-backend\n").expect("parses");
        assert_eq!(cfg.command, "my-backend");
        assert_eq!(cfg.call_timeout_secs, 600);
        assert_eq!(cfg.backend_config().unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn the_config_block_reaches_the_backend_verbatim() {
        let yaml = r#"
command: my-backend
config:
  account: user@example.com
  flags: [a, b]
  headless: true
"#;
        let cfg: ExternalConfig = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(
            cfg.backend_config().unwrap(),
            serde_json::json!({
                "account": "user@example.com",
                "flags": ["a", "b"],
                "headless": true
            })
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(serde_yaml::from_str::<ExternalConfig>("command: x\nbogus: 1\n").is_err());
    }
}
