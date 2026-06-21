//! CLI configuration: aliases + the `config edit` subcommand (Block D, D3).
//!
//! The generic front-end ([`crate::adapter_cli`]) is deliberately verbose —
//! `nyd tasks do toggle-tracking <id>` spells out instance, verb, action and
//! target. **Aliases** give a short name to a fixed invocation shape, with
//! positional/named substitution, so day-to-day use stays terse without the
//! CLI having to hard-code any adapter knowledge:
//!
//! ```yaml
//! # ~/.config/not_yet_done/cli.yaml
//! aliases:
//!   track: [tasks, do, toggle-tracking, "{0}"]   # nyd track <id>
//!   find:  [tasks, ls, --query, "{@}"]            # nyd find status=open …
//!   new:   [tasks, do, add, --value, "{parent}"]  # nyd new --parent <id>
//! ```
//!
//! An alias maps a name to a list of expansion tokens. When you run
//! `nyd <alias> <args…>`, the trailing args are split into **positionals**
//! (bare tokens) and **named** values (`--key value` or `--key=value`), then
//! substituted into the template tokens:
//!
//! | placeholder | expands to                                            |
//! |-------------|-------------------------------------------------------|
//! | `{0}` `{1}` | the N-th positional arg (error if missing)            |
//! | `{@}`       | *all* positional args (spliced as separate tokens)    |
//! | `{name}`    | the `--name <value>` the caller passed (error if none)|
//!
//! The expansion's first token must name a configured adapter instance, so an
//! alias is just a shorthand for one of the generic verbs — it can't reach the
//! legacy `tusks` commands.
//!
//! A small set of [`DEFAULT_ALIASES`] ships compiled in (assuming the
//! conventional instance names `tasks`/`trackings`). A user `cli.yaml` is
//! *merged over* them — same-name keys win — so shipping new defaults in a
//! later release reaches everyone, while the file stays purely additive.
//!
//! `nyd config edit [cli|tui|<view>]` opens the relevant config file in
//! `$EDITOR`, seeding `cli.yaml` with a documented template on first use.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

/// Built-in aliases, always available unless overridden by `cli.yaml`. They
/// assume the conventional instance names `tasks`/`trackings`; if those
/// instances aren't configured the alias still expands, then fails at dispatch
/// with a clear "no adapter instance" error.
///
/// An alias name must not collide with a built-in `tusks` subcommand (`tag`,
/// `backup`, `help`) or a configured adapter instance — both are matched
/// *before* aliases, so a colliding name is unreachable. The former task-core
/// commands (`task`/`project`/`track`/`query`/`db`) were removed in D3b, so
/// those names are now free for aliases (e.g. the `track` toggle below).
pub const DEFAULT_ALIASES: &[(&str, &[&str])] = &[
    // Open the new-task editor buffer (or supply it inline with `-m`, where the
    // first line is the title and the rest the description — the same Markdown
    // buffer the TUI's add action uses).
    ("add", &["tasks", "do", "add"]),
    // Edit a task's buffer: `nyd edit <id>`.
    ("edit", &["tasks", "do", "edit", "{0}"]),
    // Delete a task (needs `--yes`): `nyd rm <id> --yes`.
    ("rm", &["tasks", "do", "delete", "{0}"]),
    // Toggle time tracking on a task: `nyd track <id>`. The adapter exposes a
    // single `toggle-tracking` action (start when stopped, stop when running),
    // so the legacy `track start`/`track stop` pair collapses into one verb.
    // `toggle` stays as a back-compat synonym.
    ("track", &["tasks", "do", "toggle-tracking", "{0}"]),
    ("toggle", &["tasks", "do", "toggle-tracking", "{0}"]),
    // The task forest as a tree: `nyd tree`.
    ("tree", &["tasks", "ls", "--tree"]),
    // Tracked time rolled up per task, bucketed by day (newest first) — the
    // generic stand-in for the legacy `track summary`. Add `--query` after it
    // (spliced) to time-box, e.g. `nyd summary --query 'started_at gt last week'`.
    (
        "summary",
        &[
            "trackings",
            "ls",
            "--tree",
            "--type",
            "tracking:tree-group",
            "--group-by",
            "started_at:day:desc",
        ],
    ),
];

/// Parsed `cli.yaml`. Only `aliases:` for now; the struct leaves room for
/// future CLI-wide settings (default output format, …) without a schema break.
#[derive(Debug, Default, Deserialize)]
struct CliConfigFile {
    #[serde(default)]
    aliases: BTreeMap<String, Vec<String>>,
}

/// The effective alias table: compiled-in defaults with the user file merged
/// over them.
#[derive(Debug, Default)]
pub struct CliConfig {
    aliases: BTreeMap<String, Vec<String>>,
}

impl CliConfig {
    /// Look up an alias's expansion template by name.
    pub fn alias(&self, name: &str) -> Option<&[String]> {
        self.aliases.get(name).map(Vec::as_slice)
    }
}

/// `~/.config/not_yet_done/cli.yaml`.
pub fn cli_config_path() -> PathBuf {
    not_yet_done_host::config_dir().join("cli.yaml")
}

/// Load the effective alias table: start from [`DEFAULT_ALIASES`], then merge a
/// `cli.yaml` over it (same-name keys replace the default). A missing file is
/// not an error — the defaults stand alone. A malformed file *is* an error, so
/// a typo surfaces loudly rather than silently dropping every alias.
pub fn load() -> Result<CliConfig> {
    let mut aliases: BTreeMap<String, Vec<String>> = DEFAULT_ALIASES
        .iter()
        .map(|(name, toks)| {
            (
                (*name).to_string(),
                toks.iter().map(|t| (*t).to_string()).collect(),
            )
        })
        .collect();

    let path = cli_config_path();
    if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let file: CliConfigFile = serde_yaml::from_str(&raw)
            .with_context(|| format!("parsing {}", path.display()))?;
        for (name, toks) in file.aliases {
            aliases.insert(name, toks);
        }
    }
    Ok(CliConfig { aliases })
}

// ---------------------------------------------------------------------------
// Alias expansion
// ---------------------------------------------------------------------------

/// Split the trailing args of an alias invocation into positionals (bare
/// tokens) and named values. `--key value` and `--key=value` are named; a
/// trailing `--key` with no value (end of args, or followed by another `--`)
/// becomes `key = ""`. Everything else is a positional, in order.
fn split_args(user_args: &[String]) -> (Vec<String>, HashMap<String, String>) {
    let mut positionals = Vec::new();
    let mut named = HashMap::new();
    let mut i = 0;
    while i < user_args.len() {
        let a = &user_args[i];
        if let Some(rest) = a.strip_prefix("--") {
            if let Some((k, v)) = rest.split_once('=') {
                named.insert(k.to_string(), v.to_string());
            } else if user_args
                .get(i + 1)
                .is_some_and(|n| !n.starts_with("--"))
            {
                named.insert(rest.to_string(), user_args[i + 1].clone());
                i += 1;
            } else {
                named.insert(rest.to_string(), String::new());
            }
        } else {
            positionals.push(a.clone());
        }
        i += 1;
    }
    (positionals, named)
}

/// Substitute placeholders in one template token. A token consisting solely of
/// `{@}` is signalled by returning every positional (the caller splices them);
/// any other token returns exactly one substituted string.
enum Expanded {
    One(String),
    Splice(Vec<String>),
}

fn expand_token(
    token: &str,
    positionals: &[String],
    named: &HashMap<String, String>,
) -> Result<Expanded> {
    if token == "{@}" {
        return Ok(Expanded::Splice(positionals.to_vec()));
    }
    let mut out = String::with_capacity(token.len());
    let mut rest = token;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| anyhow!("unterminated '{{' in alias token '{token}'"))?;
        let name = &after[..close];
        let value = if name == "@" {
            positionals.join(" ")
        } else if name.chars().all(|c| c.is_ascii_digit()) && !name.is_empty() {
            let idx: usize = name.parse().unwrap();
            positionals.get(idx).cloned().ok_or_else(|| {
                anyhow!(
                    "alias needs positional argument {{{idx}}} but only {} given",
                    positionals.len()
                )
            })?
        } else {
            named
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow!("alias needs named argument --{name} <value>"))?
        };
        out.push_str(&value);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(Expanded::One(out))
}

/// Expand an alias `template` with the caller's trailing `user_args` into a
/// flat token list (instance, verb, …) ready to feed back through the generic
/// dispatcher. Does *not* include the program name.
pub fn expand(template: &[String], user_args: &[String]) -> Result<Vec<String>> {
    let (positionals, named) = split_args(user_args);
    let mut out = Vec::with_capacity(template.len() + positionals.len());
    for token in template {
        match expand_token(token, &positionals, &named)? {
            Expanded::One(s) => out.push(s),
            Expanded::Splice(many) => out.extend(many),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// `nyd config edit [target]`
// ---------------------------------------------------------------------------

/// Handle `nyd config <sub> [target]`. The only subcommand is `edit` (the
/// default when omitted), which opens a config file in `$EDITOR`:
///
/// * `cli`            → `cli.yaml` (seeded with a documented template)
/// * `tui`            → `tui.yaml`
/// * `<name>`         → `views/<name>.yaml`
/// * *(no target)*    → `cli`
pub fn run_config(args: &[String]) -> Result<()> {
    // args = ["nyd", "config", <sub?>, <target?>]
    let sub = args.get(2).map(String::as_str).unwrap_or("edit");
    if sub != "edit" {
        return Err(anyhow!(
            "unknown config subcommand '{sub}' (only `edit` is supported)"
        ));
    }
    let target = args.get(3).map(String::as_str).unwrap_or("cli");
    let path = config_target_path(target)?;
    if !path.exists() {
        seed_config_file(&path, target)?;
    }
    launch_editor(&path)
}

/// Resolve a `config edit` target to a path under the config root.
fn config_target_path(target: &str) -> Result<PathBuf> {
    let root = not_yet_done_host::config_dir();
    Ok(match target {
        "cli" => root.join("cli.yaml"),
        "tui" => root.join("tui.yaml"),
        name => {
            // A view name → views/<name>.yaml. Must already exist; creating a
            // valid view config is not something we can seed blindly.
            let path = root.join("views").join(format!("{name}.yaml"));
            if !path.exists() {
                let known = list_view_names();
                return Err(anyhow!(
                    "no config target '{name}' (use `cli`, `tui`, or a view: {})",
                    if known.is_empty() {
                        "<none configured>".to_string()
                    } else {
                        known.join(", ")
                    }
                ));
            }
            path
        }
    })
}

/// View names (file stems) under the views dir, for the error hint.
fn list_view_names() -> Vec<String> {
    std::fs::read_dir(not_yet_done_host::views_dir())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let is_yaml = p
                .extension()
                .is_some_and(|x| x == "yaml" || x == "yml");
            is_yaml
                .then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .flatten()
        })
        .collect()
}

/// Create a config file's parent dir and seed initial content. `cli.yaml` gets
/// a fully documented template (substitution rules + the defaults shown as
/// commented reference); `tui.yaml` gets a one-line stub.
fn seed_config_file(path: &Path, target: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let content = match target {
        "cli" => cli_yaml_template(),
        _ => "# not-yet-done config\n".to_string(),
    };
    std::fs::write(path, content).with_context(|| format!("seeding {}", path.display()))
}

/// The seeded `cli.yaml`: documents substitution and lists the built-in
/// defaults as a commented reference, leaving an empty `aliases:` for the user.
/// The defaults stay active from code, so the commented block never goes stale
/// vs. a deletion (it is reference only — uncomment + edit to override).
fn cli_yaml_template() -> String {
    let mut s = String::new();
    s.push_str("# not-yet-done CLI config.\n");
    s.push_str("#\n");
    s.push_str("# `aliases` give a short name to a generic invocation. Run as\n");
    s.push_str("#   nyd <alias> <args…>\n");
    s.push_str("# args split into positionals (bare) and named (--key value);\n");
    s.push_str("# substituted into the template tokens:\n");
    s.push_str("#   {0} {1}  N-th positional      {@}  all positionals\n");
    s.push_str("#   {name}   value of --name <v>\n");
    s.push_str("#\n");
    s.push_str("# The first expanded token must be a configured adapter instance.\n");
    s.push_str("# These built-in defaults are active without being listed here;\n");
    s.push_str("# redefine a name below to override it.\n");
    s.push_str("#\n");
    for (name, toks) in DEFAULT_ALIASES {
        let rendered: Vec<String> = toks
            .iter()
            .map(|t| {
                if t.contains('{') || t.starts_with("--") {
                    format!("\"{t}\"")
                } else {
                    t.to_string()
                }
            })
            .collect();
        s.push_str(&format!("#   {name}: [{}]\n", rendered.join(", ")));
    }
    s.push_str("\naliases: {}\n");
    s
}

/// Launch `$EDITOR` (falling back to `$VISUAL`) on an existing file path,
/// inheriting the terminal. Shared by `config edit` and the `do`-verb editor
/// flow.
pub fn launch_editor(path: &Path) -> Result<()> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map_err(|_| anyhow!("no $EDITOR (or $VISUAL) set"))?;
    let mut parts = editor.split_whitespace();
    let program = parts.next().ok_or_else(|| anyhow!("$EDITOR is empty"))?;
    let status = std::process::Command::new(program)
        .args(parts)
        .arg(path)
        .status()
        .with_context(|| format!("launching editor '{editor}'"))?;
    if !status.success() {
        return Err(anyhow!("editor '{editor}' exited with {status}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn split_separates_positionals_and_named() {
        let (pos, named) = split_args(&v(&["abc", "--parent", "p1", "def", "--flag=on"]));
        assert_eq!(pos, v(&["abc", "def"]));
        assert_eq!(named.get("parent").map(String::as_str), Some("p1"));
        assert_eq!(named.get("flag").map(String::as_str), Some("on"));
    }

    #[test]
    fn trailing_named_without_value_is_empty() {
        let (pos, named) = split_args(&v(&["x", "--bare"]));
        assert_eq!(pos, v(&["x"]));
        assert_eq!(named.get("bare").map(String::as_str), Some(""));
    }

    #[test]
    fn expand_positional_index() {
        let out = expand(&v(&["tasks", "do", "edit", "{0}"]), &v(&["abc123"])).unwrap();
        assert_eq!(out, v(&["tasks", "do", "edit", "abc123"]));
    }

    #[test]
    fn expand_splices_all_positionals() {
        let out = expand(&v(&["tasks", "ls", "--query", "{@}"]), &v(&["status=open", "p=1"]))
            .unwrap();
        assert_eq!(out, v(&["tasks", "ls", "--query", "status=open", "p=1"]));
    }

    #[test]
    fn expand_embedded_at_joins_with_space() {
        let out = expand(&v(&["q", "find:{@}"]), &v(&["foo", "bar"])).unwrap();
        assert_eq!(out, v(&["q", "find:foo bar"]));
    }

    #[test]
    fn expand_named_placeholder() {
        let out = expand(&v(&["tasks", "do", "add", "--value", "{parent}"]),
            &v(&["--parent", "root1"]))
        .unwrap();
        assert_eq!(out, v(&["tasks", "do", "add", "--value", "root1"]));
    }

    #[test]
    fn expand_missing_positional_errors() {
        assert!(expand(&v(&["tasks", "do", "edit", "{0}"]), &[]).is_err());
    }

    #[test]
    fn expand_missing_named_errors() {
        assert!(expand(&v(&["x", "{parent}"]), &[]).is_err());
    }

    /// Look up a default alias's template by name (defaults are the source of
    /// truth; `load()` may be shadowed by a real `cli.yaml` in dev/CI).
    fn default_template(name: &str) -> Vec<String> {
        DEFAULT_ALIASES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, toks)| toks.iter().map(|t| t.to_string()).collect())
            .unwrap_or_else(|| panic!("no default alias '{name}'"))
    }

    #[test]
    fn default_track_alias_toggles_tracking() {
        let out = expand(&default_template("track"), &v(&["abc123"])).unwrap();
        assert_eq!(out, v(&["tasks", "do", "toggle-tracking", "abc123"]));
    }

    #[test]
    fn default_summary_alias_is_grouped_tracking_tree() {
        // No args → the bare grouped-tree invocation.
        let out = expand(&default_template("summary"), &[]).unwrap();
        assert_eq!(
            out,
            v(&[
                "trackings",
                "ls",
                "--tree",
                "--type",
                "tracking:tree-group",
                "--group-by",
                "started_at:day:desc",
            ])
        );
    }

    #[test]
    fn load_includes_compiled_defaults() {
        // No file in a clean env: defaults must still be present. (CI/dev may
        // have a real cli.yaml; only assert the default keys are reachable.)
        let cfg = load().unwrap();
        for (name, _) in DEFAULT_ALIASES {
            assert!(cfg.alias(name).is_some(), "default alias '{name}' missing");
        }
    }
}
