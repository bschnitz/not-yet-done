//! `link` action — connect this issue to another via a Jira issue link.
//!
//! The form takes a target issue key and a free-text *relation* phrase. Rather
//! than force the user to know the link-type name and pick a direction, the
//! phrase is resolved against the instance's configured link types: typing an
//! outward phrasing (`"blocks"`) links this issue as the outward side, an
//! inward phrasing (`"is blocked by"`) as the inward side, and a bare type
//! name (`"Blocks"`) defaults to outward. An unrecognised phrase produces an
//! error listing the available relations, which keeps the free-text field
//! discoverable without needing a dynamically-populated select.

use std::collections::HashMap;

use not_yet_done_content::*;

use crate::client::JiraLinkType;

use super::super::util::other_err;
use super::JiraIssueNode;

/// Resolve a user-typed relation phrase against the instance's link types.
/// Matches (case-insensitively) a type's outward phrase, inward phrase, or
/// bare type name. Returns the canonical type name and whether *this* issue
/// is the outward side of the link.
fn resolve_relation(types: &[JiraLinkType], phrase: &str) -> Option<(String, bool)> {
    let want = phrase.trim().to_lowercase();
    // Prefer an exact directional-phrase match so "is blocked by" picks the
    // inward side even though it contains the word "blocked".
    for t in types {
        if t.outward.to_lowercase() == want {
            return Some((t.name.clone(), true));
        }
        if t.inward.to_lowercase() == want {
            return Some((t.name.clone(), false));
        }
    }
    // Fall back to a bare type name, defaulting to the outward direction.
    types
        .iter()
        .find(|t| t.name.to_lowercase() == want)
        .map(|t| (t.name.clone(), true))
}

/// The available relation phrases, sorted and de-duplicated, for the
/// "unknown relation" error message.
fn available_relations(types: &[JiraLinkType]) -> Vec<String> {
    let mut phrases: Vec<String> = types
        .iter()
        .flat_map(|t| [t.outward.clone(), t.inward.clone()])
        .filter(|s| !s.is_empty())
        .collect();
    phrases.sort();
    phrases.dedup();
    phrases
}

impl JiraIssueNode {
    pub(super) async fn execute_link(
        &self,
        values: &HashMap<String, String>,
    ) -> Result<ActionOutcome> {
        let target = values
            .get("target")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| other_err("target issue key is required".to_string()))?;
        let relation = values
            .get("relation")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| other_err("relation is required".to_string()))?;

        let types = self
            .client
            .get_issue_link_types()
            .await
            .map_err(other_err)?;

        let (type_name, this_outward) = resolve_relation(&types, relation).ok_or_else(|| {
            other_err(format!(
                "unknown relation '{relation}'. Available: {}",
                available_relations(&types).join(", ")
            ))
        })?;

        // `outwardIssue <outward-phrase> inwardIssue`; put this issue on the
        // side the resolved phrase dictates.
        let (outward, inward) = if this_outward {
            (self.key.as_str(), target)
        } else {
            (target, self.key.as_str())
        };

        self.client
            .create_issue_link(&type_name, outward, inward)
            .await
            .map_err(other_err)?;

        Ok(ActionOutcome::Done {
            message: Some(format!("{}: linked ({relation} {target})", self.key)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_types() -> Vec<JiraLinkType> {
        vec![
            JiraLinkType {
                name: "Blocks".into(),
                inward: "is blocked by".into(),
                outward: "blocks".into(),
            },
            JiraLinkType {
                name: "Relates".into(),
                inward: "relates to".into(),
                outward: "relates to".into(),
            },
        ]
    }

    #[test]
    fn outward_phrase_puts_this_issue_outward() {
        let (name, this_outward) = resolve_relation(&link_types(), "blocks").unwrap();
        assert_eq!(name, "Blocks");
        assert!(this_outward);
    }

    #[test]
    fn inward_phrase_puts_this_issue_inward() {
        let (name, this_outward) = resolve_relation(&link_types(), "is blocked by").unwrap();
        assert_eq!(name, "Blocks");
        assert!(!this_outward);
    }

    #[test]
    fn matching_is_case_insensitive() {
        let (name, this_outward) = resolve_relation(&link_types(), "BLOCKS").unwrap();
        assert_eq!(name, "Blocks");
        assert!(this_outward);
    }

    #[test]
    fn bare_type_name_defaults_to_outward() {
        let (name, this_outward) = resolve_relation(&link_types(), "blocks").unwrap();
        assert_eq!(name, "Blocks");
        assert!(this_outward);
        // The type name itself also resolves (defaulting to outward) even when
        // it doesn't equal either phrase.
        let (name, this_outward) = resolve_relation(&link_types(), "Relates").unwrap();
        assert_eq!(name, "Relates");
        assert!(this_outward);
    }

    #[test]
    fn unknown_relation_returns_none() {
        assert!(resolve_relation(&link_types(), "supersedes").is_none());
    }

    #[test]
    fn available_relations_are_sorted_and_deduped() {
        // "relates to" is both the inward and outward phrase of Relates.
        let rels = available_relations(&link_types());
        assert_eq!(rels, ["blocks", "is blocked by", "relates to"]);
    }
}
