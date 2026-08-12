//! Backend config block for a CalDAV connection.

use serde::Deserialize;

use not_yet_done_content::CredentialProvider;

/// Default per-request timeout (seconds). Generous enough for a slow PROPFIND
/// over a fat calendar, short enough that a dead server surfaces instead of
/// wedging the poll loop.
pub(crate) const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// The `config:` sub-tree of a `backend: caldav` connection entry, e.g.
///
/// ```yaml
/// url: https://kalender.mail.de/principals/jane@mail.de
/// username:
///   type: literal
///   value: jane@mail.de
/// password:
///   type: command
///   script: pass show communication/e-mail/jane@mail.de/pass
/// name: "mail.de (jane)"
/// ```
///
/// `url` may point at the principal, the calendar-home collection, or a single
/// calendar collection — the client discovers the calendars from whichever it
/// is (see [`crate::client`]). Set `calendars` to skip discovery and query an
/// explicit set of collection paths.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct CalDavConfig {
    /// Entry point: a principal URL, a calendar-home URL, or a single calendar
    /// collection URL. Discovery walks from here to the actual calendars.
    pub(crate) url: String,
    /// Login name for HTTP Basic auth. Any credential provider works; for
    /// mail.de this is the full address (`jane@mail.de`), commonly a `literal`.
    pub(crate) username: CredentialProvider,
    /// Password for HTTP Basic auth. Resolved lazily and re-resolved on a 401.
    /// Use a `command` provider wrapping `pass`/`op`/… — never a literal.
    pub(crate) password: CredentialProvider,
    /// Display label for this connection (the "Account" column). Defaults to
    /// the connection id.
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// Explicit calendar collection URLs (absolute, or paths resolved against
    /// the server root of `url`). When set, discovery is skipped and exactly
    /// these collections are queried — handy to pin one of several calendars.
    #[serde(default)]
    pub(crate) calendars: Vec<String>,
    /// Skip TLS certificate verification. For a private server with a
    /// self-signed cert only; leave off for public providers like mail.de.
    #[serde(default)]
    pub(crate) danger_accept_invalid_certs: bool,
    #[serde(default)]
    pub(crate) request_timeout_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let yaml = r#"
url: https://kalender.mail.de/principals/jane@mail.de
username:
  type: literal
  value: jane@mail.de
password:
  type: command
  script: pass show x
name: mail.de
"#;
        let cfg: CalDavConfig = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(cfg.url, "https://kalender.mail.de/principals/jane@mail.de");
        assert_eq!(cfg.name.as_deref(), Some("mail.de"));
        assert!(cfg.calendars.is_empty());
        assert!(!cfg.danger_accept_invalid_certs);
        assert!(matches!(cfg.username, CredentialProvider::Literal { .. }));
        assert!(matches!(cfg.password, CredentialProvider::Command { .. }));
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = "url: x\nusername:\n  type: literal\n  value: u\npassword:\n  type: literal\n  value: p\nbogus: 1\n";
        assert!(serde_yaml::from_str::<CalDavConfig>(yaml).is_err());
    }
}
