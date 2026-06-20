//! Shared helpers for the "edit tag in YAML" flow used by both the
//! CLI (`tag new` command) and the TUI (tag menu CreateNew / Edit).
//!
//! The editor invocation itself differs per host (CLI spawns a child
//! process directly; the TUI must pause/resume ratatui first), so it
//! lives in the respective crate. Everything that is host-agnostic —
//! template generation, parsing, error annotation, normalization — is
//! collected here so both call sites stay in lockstep.

use serde::Deserialize;

use crate::entity::{global_tag, project_tag};

/// Raw form fields as read back from the YAML buffer. All optional —
/// the caller turns blanks into `None` via [`normalize`].
#[derive(Debug, Default, Deserialize)]
pub struct TagDraft {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub fg_color: Option<String>,
    #[serde(default)]
    pub bg_color: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

/// Template buffer for a brand-new tag.
pub fn new_tag_template() -> String {
    "\
# Create a new tag. Save & quit to commit.
# Empty `name:` aborts. Lines starting with `#` are ignored.

# Tag name (required).
name:

# Visual style — all optional, leave blank to omit.
# Colors as hex string, e.g. \"#FFFFFF\" or \"#f03\".
fg_color:
bg_color:

# Free-form, e.g. \"BUG\", \"★\", \"🐞\".
symbol:

# Scope: leave blank for a global tag, otherwise project name or UUID.
project:
"
    .to_string()
}

/// Render the existing global tag as a YAML form for editing.
/// `project:` is intentionally kept empty — the type of a tag does
/// not change via the edit form. The CLI / TUI layer enforces this.
pub fn edit_global_template(tag: &global_tag::Model) -> String {
    edit_template(
        &tag.name,
        tag.fg_color.as_deref(),
        tag.bg_color.as_deref(),
        tag.symbol.as_deref(),
        None,
    )
}

/// Render the existing project tag as a YAML form. The project name
/// is filled in pre-resolved; renaming the project field has no
/// effect on save (re-scope is not supported via edit).
pub fn edit_project_template(tag: &project_tag::Model, project_name: &str) -> String {
    edit_template(
        &tag.name,
        tag.fg_color.as_deref(),
        tag.bg_color.as_deref(),
        tag.symbol.as_deref(),
        Some(project_name),
    )
}

fn edit_template(
    name: &str,
    fg: Option<&str>,
    bg: Option<&str>,
    symbol: Option<&str>,
    project: Option<&str>,
) -> String {
    fn yaml_str(v: Option<&str>) -> String {
        match v {
            Some(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            None => String::new(),
        }
    }
    format!(
        "\
# Edit tag. Save & quit to commit.
# `name:` darf nicht leer werden. Lines starting with `#` are ignored.
# `project:` ist read-only — Re-scope wird hier nicht unterstützt.

name: \"{name}\"

fg_color: {fg}
bg_color: {bg}
symbol: {sym}

project: {project}
",
        name = name.replace('\\', "\\\\").replace('"', "\\\""),
        fg = yaml_str(fg),
        bg = yaml_str(bg),
        sym = yaml_str(symbol),
        project = match project {
            Some(p) => format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\"")),
            None => String::new(),
        },
    )
}

/// Parse the buffer back into a [`TagDraft`]. Strips the optional
/// error block written by [`annotate_error`] so previous failed
/// attempts round-trip.
pub fn parse_draft(text: &str) -> Result<TagDraft, String> {
    let body = strip_error_block(text);
    serde_yaml::from_str::<TagDraft>(&body).map_err(|e| format!("YAML parse: {e}"))
}

const ERROR_START: &str = "# <<< ERROR <<<";
const ERROR_END: &str = "# >>> end-error >>>";

/// Remove the leading error annotation block (if any) so the user's
/// original content is recoverable for the next edit pass.
pub fn strip_error_block(text: &str) -> String {
    if let Some(idx) = text.find(ERROR_END) {
        let rest = &text[idx + ERROR_END.len()..];
        return rest.trim_start_matches('\n').to_string();
    }
    text.to_string()
}

/// Prepend an error block above the user's previous content so the
/// editor reopens with both the message and their input visible.
pub fn annotate_error(previous: &str, err: &str) -> String {
    let body = strip_error_block(previous);
    let mut out = String::new();
    out.push_str(ERROR_START);
    out.push('\n');
    for line in err.lines() {
        out.push_str("# ERROR: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("# Korrigiere die Eingabe und speichere erneut.\n");
    out.push_str(ERROR_END);
    out.push('\n');
    out.push_str(&body);
    out
}

/// Trim, then treat an empty string as `None`. Used uniformly on
/// every form field before handing the draft to the service layer.
pub fn normalize(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
