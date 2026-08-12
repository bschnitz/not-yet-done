//! `create` action — a list-wide operation on the Jira root that POSTs a
//! brand-new issue from a small form. Unlike [`clone`](super::issue::clone),
//! which seeds every field from a source issue and opens the 3b editor, this
//! is a target-less create: the user names the project, summary, and the few
//! optional fields directly in the form, mirroring the cross-adapter `create`
//! convention (see `not_yet_done_local_adapter::projects`).
//!
//! The rich body / label-slug / assignee-slug treatment stays with the
//! per-issue editor actions (`edit`, `edit (markdown)`, `clone`): a freshly
//! created skeleton can be fleshed out there. The form's `description` is a
//! single line by design; multi-line bodies belong to the follow-up edit.

use std::collections::HashMap;
use std::sync::Arc;

use not_yet_done_content::*;

use crate::client::{CreateIssueFields, JiraClient};

use super::types::issue_node_type;
use super::util::other_err;

/// The root's `create` action: a form with a required project key + summary
/// and optional type / priority / assignee / labels / description.
pub(super) fn create_action() -> NodeAction {
    NodeAction::new("create", "New issue", create_input_spec())
}

/// Form for `create`. `project` and `summary` are required; everything else is
/// optional and, when left blank, lets Jira apply the project's defaults
/// (notably the workflow's initial status, which is never set on create).
fn create_input_spec() -> InputSpec {
    InputSpec::Form {
        fields: vec![
            FormFieldSpec::text("project", "Project key (e.g. PROJ)"),
            FormFieldSpec::text("summary", "Summary"),
            FormFieldSpec::text("type", "Issue type")
                .optional()
                .with_default("Task"),
            FormFieldSpec::text("priority", "Priority").optional(),
            FormFieldSpec::text("assignee", "Assignee (username)").optional(),
            FormFieldSpec::text("labels", "Labels (comma-separated)").optional(),
            FormFieldSpec::text("description", "Description").optional(),
        ],
    }
}

/// Trim a form value, returning `None` for a missing-or-blank field.
fn opt(values: &HashMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Trim a required form value, erroring when missing or blank.
fn required(values: &HashMap<String, String>, key: &str) -> Result<String> {
    opt(values, key).ok_or_else(|| other_err(format!("{key} is required")))
}

/// `execute("create")` — build [`CreateIssueFields`] from the form and POST a
/// new issue, then navigate to the freshly assigned key so the user lands on
/// the new ticket. A create failure surfaces the server error verbatim.
pub(super) async fn execute_create(
    client: &Arc<JiraClient>,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let project_key = required(values, "project")?;
    let summary = required(values, "summary")?;

    let labels = opt(values, "labels")
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let fields = CreateIssueFields {
        project_key,
        summary,
        description: opt(values, "description").unwrap_or_default(),
        issue_type: opt(values, "type").unwrap_or_default(),
        priority: opt(values, "priority").unwrap_or_default(),
        labels,
        assignee_key: opt(values, "assignee").unwrap_or_default(),
    };

    let new_key = client.create_issue(&fields).await.map_err(other_err)?;

    Ok(ActionOutcome::Navigate {
        node_id: new_key.clone(),
        node_type: issue_node_type(),
        message: Some(format!("Created {new_key}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn required_project_and_summary() {
        assert!(required(&form(&[("summary", "x")]), "project").is_err());
        assert!(required(&form(&[("project", "  ")]), "project").is_err());
        assert_eq!(
            required(&form(&[("project", " PROJ ")]), "project").unwrap(),
            "PROJ"
        );
    }

    #[test]
    fn opt_trims_and_blanks_to_none() {
        let f = form(&[("priority", "  High "), ("type", "   ")]);
        assert_eq!(opt(&f, "priority").as_deref(), Some("High"));
        assert_eq!(opt(&f, "type"), None);
        assert_eq!(opt(&f, "missing"), None);
    }

    #[test]
    fn create_action_form_has_required_and_optional_fields() {
        let action = create_action();
        match action.input {
            InputSpec::Form { fields } => {
                let req: Vec<&str> = fields
                    .iter()
                    .filter(|f| f.required)
                    .map(|f| f.key.as_str())
                    .collect();
                assert_eq!(req, ["project", "summary"]);
                assert!(fields.iter().any(|f| f.key == "labels" && !f.required));
                let type_field = fields.iter().find(|f| f.key == "type").unwrap();
                assert_eq!(type_field.default.as_deref(), Some("Task"));
            }
            other => panic!("expected Form input spec, got {other:?}"),
        }
    }
}
