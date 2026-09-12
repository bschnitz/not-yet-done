//! `project_types` action — the issue types a project offers, listed on the
//! Jira root.
//!
//! [`create`](super::create) names its type (`issuetype: { name }`), which
//! is friendly to write and unforgiving to guess: a misspelt name comes back
//! as a server-side rejection after the POST, with no hint of what would
//! have worked. This action is the lookup that makes the name checkable
//! first — and, listed rather than validated, it also answers "what can this
//! project even hold?" for a caller building a menu.

use std::collections::HashMap;
use std::sync::Arc;

use not_yet_done_content::*;

use crate::client::JiraClient;

use super::util::other_err;

/// The root's `project_types` action: one required project key.
pub(super) fn project_types_action() -> NodeAction {
    NodeAction::new(
        "project_types",
        "Issue types of a project",
        InputSpec::Form {
            fields: vec![FormFieldSpec::text("project", "Project key (e.g. PROJ)")],
        },
    )
}

/// `execute("project_types")` — a `# project <id>` header line, then one
/// type per line as `id<TAB>name<TAB>kind`, `kind` being `standard` or
/// `subtask`. Tab-separated because the answer is read by scripts as often
/// as by people, and a type name may contain spaces; the order is Jira's
/// own. The header carries the project's numeric id, which a caller that
/// goes on to address the project (a create-issue URL, say) needs and would
/// otherwise have to fetch a second time; `#` keeps it out of the way of a
/// reader that only wants the rows.
pub(super) async fn execute_project_types(
    client: &Arc<JiraClient>,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let project = values
        .get("project")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| other_err("project is required".to_string()))?;

    let (project_id, types) = client
        .get_project_issue_types(project)
        .await
        .map_err(other_err)?;

    let mut lines = vec![format!("# project {project_id}")];
    lines.extend(types.iter().map(|t| {
        let kind = if t.subtask { "subtask" } else { "standard" };
        format!("{}\t{}\t{}", t.id, t.name, kind)
    }));
    let listing = lines.join("\n");

    Ok(ActionOutcome::Done {
        message: Some(listing),
    })
}
