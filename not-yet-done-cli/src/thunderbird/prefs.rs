//! Reading Thunderbird's `prefs.js`, and finding the profile it lives in.
//!
//! `prefs.js` is a JavaScript file only in spelling. Thunderbird rewrites it
//! whole on every shutdown as a flat list of `user_pref("key", value);` lines
//! with nothing between them — no conditionals, no computation, no includes.
//! Reading it as text is therefore not a shortcut around a parser; there is
//! nothing else in the file to miss.
//!
//! Only prefs are read. Thunderbird's password store (`logins.json` plus the
//! NSS-encrypted `key4.db`) is deliberately left alone — see the plan's §7:
//! decrypting it would mean linking NSS for a one-time convenience, and it
//! would copy every password into a second place on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

/// Every `user_pref` of one profile, by key.
#[derive(Debug, Default)]
pub(super) struct Prefs {
    values: BTreeMap<String, String>,
}

impl Prefs {
    /// Read every `user_pref("key", value);` line. A line that does not parse
    /// is skipped rather than fatal: a profile is allowed to carry a comment,
    /// a blank line or a pref written by an add-on we know nothing about, and
    /// none of that is a reason to refuse the accounts.
    pub(super) fn parse(text: &str) -> Self {
        let mut values = BTreeMap::new();
        for line in text.lines() {
            if let Some((key, value)) = parse_line(line.trim()) {
                values.insert(key, value);
            }
        }
        Self { values }
    }

    pub(super) fn read(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(Self::parse(&text))
    }

    pub(super) fn get(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// A pref holding a comma-separated list (`mail.accountmanager.accounts`).
    /// Empty entries are dropped — Thunderbird leaves a trailing comma behind
    /// when an account is removed.
    pub(super) fn list(&self, key: &str) -> Vec<String> {
        self.get(key)
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn get_u16(&self, key: &str) -> Option<u16> {
        self.get(key)?.parse().ok()
    }

    pub(super) fn get_i32(&self, key: &str) -> Option<i32> {
        self.get(key)?.parse().ok()
    }

    pub(super) fn get_bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }
}

/// `user_pref("<key>", <value>);` → the key and the value as written, with a
/// string value unquoted and unescaped.
fn parse_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("user_pref(")?;
    let (key, rest) = js_string(rest)?;
    let rest = rest.trim_start().strip_prefix(',')?.trim_start();
    let raw = rest.strip_suffix(';')?.trim_end().strip_suffix(')')?.trim();
    let value = if raw.starts_with('"') {
        js_string(raw)?.0
    } else {
        raw.to_string()
    };
    Some((key, value))
}

/// Read one double-quoted JavaScript string from the front of `s`, returning
/// its unescaped content and whatever follows the closing quote.
fn js_string(s: &str) -> Option<(String, &str)> {
    let body = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => match chars.next() {
                // Thunderbird escapes only these; anything else it writes
                // literally, so an unknown escape keeps its backslash rather
                // than silently losing a character.
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, esc @ ('"' | '\\' | '/'))) => out.push(esc),
                Some((_, other)) => {
                    out.push('\\');
                    out.push(other);
                }
                None => return None,
            },
            '"' => return Some((out, &body[i + 1..])),
            _ => out.push(c),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Finding the profile
// ---------------------------------------------------------------------------

/// Resolve the profile whose `prefs.js` to read.
///
/// `explicit` may name the profile directory or the `prefs.js` itself. With
/// nothing given, the standard roots are searched — including the Flatpak one,
/// which is a different directory rather than a different format.
pub(super) fn find_prefs(explicit: Option<&str>) -> Result<PathBuf> {
    if let Some(given) = explicit {
        let path = expand_tilde(given);
        return if path.is_dir() {
            let prefs = path.join("prefs.js");
            prefs
                .is_file()
                .then_some(prefs)
                .ok_or_else(|| anyhow!("{} holds no prefs.js", path.display()))
        } else if path.is_file() {
            Ok(path)
        } else {
            Err(anyhow!("no such profile: {}", path.display()))
        };
    }

    let mut tried = Vec::new();
    for root in profile_roots() {
        tried.push(root.display().to_string());
        if let Some(prefs) = profile_in(&root) {
            return Ok(prefs);
        }
    }
    Err(anyhow!(
        "no Thunderbird profile found (looked in {}) — name one with --profile <dir>",
        tried.join(", ")
    ))
}

fn profile_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    vec![
        home.join(".thunderbird"),
        home.join(".mozilla-thunderbird"),
        home.join(".var/app/org.mozilla.Thunderbird/.thunderbird"),
    ]
}

/// The profile to read under one root, in the order that answers "which
/// profile is Thunderbird actually using?".
///
/// `installs.ini` wins over `profiles.ini`. That is not a preference but the
/// answer to a real trap: a profile marked `Default=1` in `profiles.ini` can
/// be a leftover that the installed Thunderbird has not opened in years, while
/// the `[Install…]` section names the one it really starts. Reading the stale
/// one would import an empty or long-outdated account list and look like the
/// accounts had gone missing.
///
/// A profile with no `prefs.js` is passed over rather than reported: it has
/// never been opened, so it holds no accounts either way.
pub(super) fn profile_in(root: &Path) -> Option<PathBuf> {
    let installs = read_ini(&root.join("installs.ini"));
    let profiles = read_ini(&root.join("profiles.ini"));

    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut push = |raw: &str, relative: bool| {
        let path = if relative {
            root.join(raw)
        } else {
            PathBuf::from(raw)
        };
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    };

    for section in &installs {
        if let Some(default) = section.get("Default") {
            push(default, true);
        }
    }
    for section in &profiles {
        if section.get("Default").is_some_and(|d| d == "1") {
            if let Some(path) = section.get("Path") {
                push(path, section.get("IsRelative").is_none_or(|r| r == "1"));
            }
        }
    }
    for section in &profiles {
        if let Some(path) = section.get("Path") {
            push(path, section.get("IsRelative").is_none_or(|r| r == "1"));
        }
    }

    candidates
        .into_iter()
        .map(|dir| dir.join("prefs.js"))
        .find(|prefs| prefs.is_file())
}

/// The handful of INI that Mozilla writes: `[Section]` headers over `key=value`
/// lines. Returned in file order, because that is the order the caller's
/// precedence rules are written against.
fn read_ini(path: &Path) -> Vec<BTreeMap<String, String>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut sections: Vec<BTreeMap<String, String>> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            sections.push(BTreeMap::new());
        } else if let Some((k, v)) = line.split_once('=') {
            if let Some(current) = sections.last_mut() {
                current.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    sections
}

pub(super) fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pref_line_gives_up_its_key_and_value() {
        let prefs = Prefs::parse(
            r#"
// a comment Thunderbird itself writes
user_pref("mail.server.server1.hostname", "imap.example.org");
user_pref("mail.server.server1.port", 993);
user_pref("mail.server.server1.login_at_startup", true);
"#,
        );
        assert_eq!(
            prefs.get("mail.server.server1.hostname"),
            Some("imap.example.org")
        );
        assert_eq!(prefs.get_u16("mail.server.server1.port"), Some(993));
        assert_eq!(
            prefs.get_bool("mail.server.server1.login_at_startup"),
            Some(true)
        );
        assert_eq!(prefs.get("mail.server.server1.missing"), None);
    }

    #[test]
    fn an_escaped_quote_stays_inside_the_value() {
        // Thunderbird writes an empty IMAP namespace as an escaped empty
        // string, and a display name may carry quotes of its own.
        let prefs = Prefs::parse(
            r#"user_pref("mail.server.server1.namespace.personal", "\"\"");
user_pref("mail.identity.id1.fullName", "Ada \"Speedy\" Lovelace");"#,
        );
        assert_eq!(
            prefs.get("mail.server.server1.namespace.personal"),
            Some("\"\"")
        );
        assert_eq!(
            prefs.get("mail.identity.id1.fullName"),
            Some("Ada \"Speedy\" Lovelace")
        );
    }

    #[test]
    fn a_trailing_comma_does_not_invent_an_account() {
        let prefs =
            Prefs::parse(r#"user_pref("mail.accountmanager.accounts", "account1,account2,");"#);
        assert_eq!(
            prefs.list("mail.accountmanager.accounts"),
            ["account1", "account2"]
        );
    }

    #[test]
    fn the_install_beats_a_stale_default_profile() {
        // The trap this ordering exists for: `profiles.ini` marks an old
        // profile `Default=1`, but the installed Thunderbird runs the one
        // `installs.ini` names — and only that one has a `prefs.js`.
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        std::fs::create_dir_all(path.join("old.default")).unwrap();
        std::fs::create_dir_all(path.join("live.default-release")).unwrap();
        std::fs::write(path.join("live.default-release/prefs.js"), "").unwrap();
        std::fs::write(
            path.join("profiles.ini"),
            "[Profile0]\nName=old\nIsRelative=1\nPath=old.default\nDefault=1\n\
             \n[Profile1]\nName=live\nIsRelative=1\nPath=live.default-release\n",
        )
        .unwrap();
        std::fs::write(
            path.join("installs.ini"),
            "[Install01]\nDefault=live.default-release\nLocked=1\n",
        )
        .unwrap();

        assert_eq!(
            profile_in(path),
            Some(path.join("live.default-release/prefs.js"))
        );
    }

    #[test]
    fn a_profile_that_was_never_opened_is_passed_over() {
        // `Default=1` on a profile with no prefs.js at all: it holds no
        // accounts, so the next candidate is the honest answer.
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        std::fs::create_dir_all(path.join("empty")).unwrap();
        std::fs::create_dir_all(path.join("used")).unwrap();
        std::fs::write(path.join("used/prefs.js"), "").unwrap();
        std::fs::write(
            path.join("profiles.ini"),
            "[Profile0]\nIsRelative=1\nPath=empty\nDefault=1\n\
             \n[Profile1]\nIsRelative=1\nPath=used\n",
        )
        .unwrap();

        assert_eq!(profile_in(path), Some(path.join("used/prefs.js")));
    }
}
