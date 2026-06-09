//! `clone` action — POST a new Jira issue pre-filled from another issue's
//! fields. The editor opens with the source's summary/description/labels
//! carried over; assignee starts empty; issue type and priority are
//! read-only so the create call lands on the same project with sensible
//! defaults.
//!
//! Status is intentionally omitted from the POST — Jira assigns the
//! workflow's initial status, mirroring how a "create issue" through the
//! web UI would behave.

use not_yet_done_content::*;

use crate::client::{CreateIssueFields, JiraIssueDetail};

use super::JiraIssueNode;
use super::markers::{BODY_MARKER, CACHE_MARKER, EDITABLE_MARKER};
use super::slugs::{build_slug_tables, resolve_slugs_inplace};
use super::template::{FieldError, Parsed3b};

fn clone_fields() -> Vec<String> {
    vec!["summary".into(), "labels".into(), "assignee".into()]
}

/// Derive the project key from `PROJ-123` → `PROJ`. Returns the original
/// string unchanged if there's no `-` (lets the server complain instead of
/// silently posting to nowhere).
fn project_key_from_issue_key(key: &str) -> &str {
    match key.rsplit_once('-') {
        Some((proj, _)) => proj,
        None => key,
    }
}

impl JiraIssueNode {
    pub(super) async fn prepare_clone(&self) -> Result<EditorPrep> {
        let detail = self.detail().await?;
        let template = self.render_clone_template(detail);
        Ok(EditorPrep {
            template,
            // No version semantics — create has no optimistic-lock token.
            version: String::new(),
            suffix: ".jira".into(),
        })
    }

    pub(super) async fn execute_clone(&mut self, text: &str) -> Result<ActionOutcome> {
        let editable_fields = clone_fields();

        let mut parsed = match self.parse_3b(text) {
            Ok(p) => p,
            Err(errs) => {
                return Ok(ActionOutcome::Reopen {
                    content: self.render_with_errors(text, &errs),
                    new_version: None,
                });
            }
        };

        let tables = build_slug_tables(&self.cache);
        let mut errors = self.validate_3b(&parsed, &editable_fields);
        resolve_slugs_inplace(&mut parsed, &tables, &mut errors);
        if !errors.is_empty() {
            return Ok(ActionOutcome::Reopen {
                content: self.render_with_errors(text, &errors),
                new_version: None,
            });
        }

        let detail = self.detail().await?.clone();
        let (summary, assignee_key, labels) = extract_clone_inputs(&parsed);

        let fields = CreateIssueFields {
            project_key: project_key_from_issue_key(&detail.key).to_string(),
            summary,
            description: parsed.body.trim().to_string(),
            issue_type: detail.issue_type.clone(),
            priority: detail.priority.clone(),
            labels,
            assignee_key,
        };

        let new_key = match self.client.create_issue(&fields).await {
            Ok(k) => k,
            Err(e) => {
                let banner = FieldError {
                    message: format!("create failed: {e}"),
                };
                return Ok(ActionOutcome::Reopen {
                    content: self.render_with_errors(text, &[banner]),
                    new_version: None,
                });
            }
        };

        Ok(ActionOutcome::Done {
            message: Some(format!("Created {new_key}")),
        })
    }

    /// Build the clone-template buffer. Editable section starts with the
    /// source's summary, blank assignee, and source label slugs. Read-only
    /// section advertises `type` and `priority` so the user sees what the
    /// POST will use (those values are pulled from `self.detail`, not from
    /// the buffer). The completions block lists labels + users.
    fn render_clone_template(&self, detail: &JiraIssueDetail) -> String {
        let tables = build_slug_tables(&self.cache);
        let mut out = String::new();
        out.push_str(&format!(
            "# Clone of {} — edit summary/assignee/labels below and save to create a new issue\n",
            detail.key,
        ));

        out.push_str(&format!("summary: {}\n", detail.summary));
        let label_slugs: Vec<String> = detail
            .labels
            .iter()
            .filter_map(|l| tables.labels.slug_for(l).map(String::from))
            .collect();
        out.push_str(&format!("labels: {}\n", label_slugs.join(", ")));
        out.push_str("assignee: \n");

        out.push_str(EDITABLE_MARKER);
        out.push('\n');

        out.push_str(&format!("project: {}\n", project_key_from_issue_key(&detail.key)));
        out.push_str(&format!("type: {}\n", detail.issue_type));
        out.push_str(&format!("priority: {}\n", detail.priority));

        out.push_str(BODY_MARKER);
        out.push_str("\n\n");
        out.push_str(&detail.description);

        out.push_str(&render_clone_cache_section(&tables));
        out
    }
}

/// Pull just the inputs the clone POST cares about out of a parsed buffer.
/// `validate_3b` has already guaranteed `summary` is present + non-empty;
/// `resolve_slugs_inplace` has translated `ll-…`/`uu-…` slugs back to
/// original Jira names.
fn extract_clone_inputs(parsed: &Parsed3b) -> (String, String, Vec<String>) {
    let summary = parsed
        .editable
        .get("summary")
        .cloned()
        .unwrap_or_default();
    let assignee_key = parsed
        .editable
        .get("assignee")
        .cloned()
        .unwrap_or_default();
    let labels = parsed
        .editable
        .get("labels")
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    (summary, assignee_key, labels)
}

fn render_clone_cache_section(tables: &super::slugs::SlugTables) -> String {
    if tables.labels.is_empty() && tables.users.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push('\n');
    out.push_str(CACHE_MARKER);
    out.push('\n');
    if !tables.labels.is_empty() {
        out.push_str("# labels: ");
        out.push_str(&tables.labels.slugs().join(", "));
        out.push('\n');
    }
    if !tables.users.is_empty() {
        out.push_str("# users: ");
        out.push_str(&tables.users.slugs().join(", "));
        out.push('\n');
    }
    out
}
