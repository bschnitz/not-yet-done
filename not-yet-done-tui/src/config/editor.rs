use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// EditorConfig
// ---------------------------------------------------------------------------

/// Configuration for **one** external-editor profile used by the TUI.
///
/// Profiles are defined under the top-level [`EditorsConfig`] (`editors:`)
/// block — see its docs for the `default` + named-profile layout. A single
/// profile looks like:
///
/// ```yaml
/// editors:
///   default:
///     command: "nvim {file}"
///     inline: true
///     pause_tui: false
/// ```
///
/// `{file}` is replaced with the path to the temporary file.
/// If `{file}` is absent the path is appended as the last argument.
///
/// When `inline` is `true` (default) ratatui pauses (leaves the alternate
/// screen) and the editor runs in the same terminal.  When `false` the
/// command is expected to open its own window (e.g. via tmux, a GUI
/// editor, etc.).
///
/// `pause_tui` (default `false`): when `true` and `inline` is `false`,
/// ratatui is briefly paused while the launch command executes.  Required
/// for commands like `kitty @` that need clean terminal access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorConfig {
    #[serde(default = "default_command")]
    pub command: String,

    #[serde(default = "default_inline")]
    pub inline: bool,

    #[serde(default = "default_pause_tui")]
    pub pause_tui: bool,

    #[serde(default = "default_indent")]
    pub indent: usize,
}

fn default_indent() -> usize {
    4
}

fn default_command() -> String {
    String::new() // empty → fall back to $EDITOR → vi
}

fn default_inline() -> bool {
    true
}

fn default_pause_tui() -> bool {
    false
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            command: default_command(),
            inline: default_inline(),
            pause_tui: default_pause_tui(),
            indent: default_indent(),
        }
    }
}

// ---------------------------------------------------------------------------
// EditorsConfig — named profiles
// ---------------------------------------------------------------------------

/// The `editors:` block: one mandatory `default` profile plus any number of
/// named profiles. An action in a view config selects a profile by name via
/// its `editor:` field; absence falls back to `default`.
///
/// ```yaml
/// editors:
///   default:            # used everywhere unless an action overrides it
///     command: "kitty @ launch --location=vsplit sh -c '{env}nvim {file}; mv {file} {file}.done'"
///     inline: false
///     pause_tui: true
///   compose-below:      # a second, differently-shaped editor
///     command: "kitty @ launch --location=hsplit sh -c '{env}nvim {file}; mv {file} {file}.done'"
///     inline: false
///     pause_tui: true
/// ```
///
/// Why named profiles: different tasks want different editor geometries (a
/// short chat compose fits a slim split below; a long ticket edit wants a
/// full vsplit). The editor is always a *separate process* (your `$EDITOR`,
/// e.g. via Kitty) — a TUI pane cannot host it (no PTY embedding) — so the
/// split is realised by the *terminal*, configured in `command`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorsConfig {
    /// Profile used whenever an action does not name one. Mandatory.
    pub default: EditorConfig,
    /// Additional profiles keyed by name. `flatten` lets them sit as
    /// sibling keys of `default:` in the YAML.
    #[serde(flatten, default)]
    pub named: HashMap<String, EditorConfig>,
}

impl Default for EditorsConfig {
    fn default() -> Self {
        Self {
            default: EditorConfig::default(),
            named: HashMap::new(),
        }
    }
}

impl EditorsConfig {
    /// Resolve a profile name to its [`EditorConfig`]. `None` and
    /// `"default"` both map to [`Self::default`]. Unknown names fall back
    /// to `default` defensively — the config validator already rejects
    /// unknown names at load time (see
    /// [`crate::config::view_config::ViewFileConfig::validate`]).
    pub fn resolve(&self, profile: Option<&str>) -> &EditorConfig {
        match profile {
            None | Some("default") => &self.default,
            Some(name) => self.named.get(name).unwrap_or(&self.default),
        }
    }

    /// Whether `name` resolves to a real profile (`"default"` or a key in
    /// [`Self::named`]). Used by the validator to reject typos.
    pub fn contains(&self, name: &str) -> bool {
        name == "default" || self.named.contains_key(name)
    }

    /// Names of all defined profiles, for error messages. `default` first.
    pub fn profile_names(&self) -> Vec<String> {
        let mut names = vec!["default".to_string()];
        let mut rest: Vec<String> = self.named.keys().cloned().collect();
        rest.sort();
        names.extend(rest);
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_default_only() {
        let cfg: EditorsConfig =
            serde_yaml::from_str("default:\n  command: \"vi {file}\"").unwrap();
        assert_eq!(cfg.default.command, "vi {file}");
        assert!(cfg.named.is_empty());
    }

    #[test]
    fn deserializes_default_plus_named_siblings() {
        let cfg: EditorsConfig = serde_yaml::from_str(
            "default:\n  command: \"a\"\ncompose-below:\n  command: \"b\"\n  pause_tui: true",
        )
        .unwrap();
        assert_eq!(cfg.default.command, "a");
        let named = cfg.named.get("compose-below").expect("named profile present");
        assert_eq!(named.command, "b");
        assert!(named.pause_tui);
    }

    #[test]
    fn resolve_maps_none_and_default_to_default() {
        let cfg: EditorsConfig =
            serde_yaml::from_str("default:\n  command: \"d\"\nother:\n  command: \"o\"").unwrap();
        assert_eq!(cfg.resolve(None).command, "d");
        assert_eq!(cfg.resolve(Some("default")).command, "d");
        assert_eq!(cfg.resolve(Some("other")).command, "o");
    }

    #[test]
    fn resolve_unknown_falls_back_to_default() {
        let cfg = EditorsConfig::default();
        // Defensive fallback — the validator rejects unknown names earlier.
        assert_eq!(
            cfg.resolve(Some("missing")).command,
            EditorConfig::default().command
        );
    }

    #[test]
    fn contains_and_profile_names() {
        let cfg: EditorsConfig =
            serde_yaml::from_str("default: {}\nzeta: {}\nalpha: {}").unwrap();
        assert!(cfg.contains("default"));
        assert!(cfg.contains("alpha"));
        assert!(!cfg.contains("nope"));
        // default first, rest sorted.
        assert_eq!(cfg.profile_names(), vec!["default", "alpha", "zeta"]);
    }
}
