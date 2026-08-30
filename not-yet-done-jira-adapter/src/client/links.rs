//! Issue links: list the available link types, read an issue's links, create
//! a link between two issues via `POST /rest/api/2/issueLink` and remove one
//! via `DELETE /rest/api/2/issueLink/{id}`.
//!
//! Jira models a link type as a name plus two directional phrasings, e.g.
//! `Blocks` → outward `"blocks"`, inward `"is blocked by"`. A concrete link
//! names the type and which issue sits on the outward vs. inward side:
//! `outwardIssue "blocks" inwardIssue`.

use serde::Deserialize;

use super::{Assignee, JiraClient, NameField};

/// A Jira issue-link type with its two directional phrasings.
#[derive(Debug, Clone)]
pub struct JiraLinkType {
    pub name: String,
    /// Phrasing when this side is the inward issue (e.g. `"is blocked by"`).
    pub inward: String,
    /// Phrasing when this side is the outward issue (e.g. `"blocks"`).
    pub outward: String,
}

/// One of an issue's links, already oriented from *that* issue's side: the
/// relation reads as `<queried issue> <relation> <key>` (e.g. `PROJ-1 blocks
/// PROJ-2`), and the ticket fields describe the issue at the other end.
#[derive(Debug, Clone)]
pub struct JiraIssueLink {
    /// Link id — the handle for [`JiraClient::delete_issue_link`].
    pub id: String,
    /// Canonical link-type name (`Blocks`), direction-free.
    pub type_name: String,
    /// Directional phrasing seen from the queried issue (`blocks` when it is
    /// the outward side, `is blocked by` when it is the inward one). Empty if
    /// the instance leaves the phrasing unconfigured.
    pub relation: String,
    /// `true` when the queried issue is the link's outward side.
    pub outward: bool,
    /// The issue at the other end.
    pub key: String,
    pub summary: String,
    pub status: String,
    pub priority: String,
    pub issue_type: String,
    pub assignee: String,
}

#[derive(Deserialize)]
struct LinkTypesResponse {
    #[serde(rename = "issueLinkTypes")]
    issue_link_types: Vec<RawLinkType>,
}

#[derive(Deserialize)]
struct RawLinkType {
    name: String,
    #[serde(default)]
    inward: String,
    #[serde(default)]
    outward: String,
}

/// `GET /issue/{key}?fields=issuelinks` — only the one field is requested,
/// so everything else on the issue is absent by design.
#[derive(Deserialize)]
struct IssueLinksResponse {
    #[serde(default)]
    fields: Option<IssueLinksFields>,
}

#[derive(Deserialize)]
struct IssueLinksFields {
    #[serde(default)]
    issuelinks: Vec<RawIssueLink>,
}

#[derive(Deserialize)]
struct RawIssueLink {
    id: String,
    #[serde(rename = "type")]
    link_type: Option<RawLinkType>,
    /// Present when the *other* issue is the outward side — i.e. the queried
    /// issue is the inward one. Exactly one of the two is set per link.
    #[serde(default, rename = "outwardIssue")]
    outward_issue: Option<RawLinkedIssue>,
    #[serde(default, rename = "inwardIssue")]
    inward_issue: Option<RawLinkedIssue>,
}

#[derive(Deserialize)]
struct RawLinkedIssue {
    key: String,
    #[serde(default)]
    fields: Option<RawLinkedIssueFields>,
}

#[derive(Deserialize)]
struct RawLinkedIssueFields {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    status: Option<NameField>,
    #[serde(default)]
    priority: Option<NameField>,
    #[serde(default)]
    issuetype: Option<NameField>,
    #[serde(default)]
    assignee: Option<Assignee>,
}

/// Orient one raw `issuelinks` entry from the queried issue's side.
///
/// Jira names the *other* end: an entry carrying `outwardIssue` means that
/// issue is the outward side, so the queried one is inward and reads with the
/// type's `inward` phrasing — and vice versa. An entry with neither end is a
/// link Jira could not resolve for this user (permission-filtered) and is
/// dropped rather than rendered as a blank row.
fn orient(raw: RawIssueLink) -> Option<JiraIssueLink> {
    let (type_name, inward_phrase, outward_phrase) = raw
        .link_type
        .map(|t| (t.name, t.inward, t.outward))
        .unwrap_or_default();

    // `outwardIssue` on the entry ⇒ the queried issue is the inward side.
    let (other, this_outward) = match (raw.outward_issue, raw.inward_issue) {
        (Some(other), _) => (other, false),
        (None, Some(other)) => (other, true),
        (None, None) => return None,
    };
    let relation = if this_outward {
        outward_phrase
    } else {
        inward_phrase
    };

    let fields = other.fields.unwrap_or(RawLinkedIssueFields {
        summary: None,
        status: None,
        priority: None,
        issuetype: None,
        assignee: None,
    });
    Some(JiraIssueLink {
        id: raw.id,
        type_name,
        relation,
        outward: this_outward,
        key: other.key,
        summary: fields.summary.unwrap_or_default(),
        status: fields.status.and_then(|s| s.name).unwrap_or_default(),
        priority: fields.priority.and_then(|p| p.name).unwrap_or_default(),
        issue_type: fields.issuetype.and_then(|t| t.name).unwrap_or_default(),
        assignee: fields
            .assignee
            .and_then(|a| a.display_name)
            .unwrap_or_default(),
    })
}

impl JiraClient {
    /// Fetch the instance's configured issue-link types.
    pub async fn get_issue_link_types(&self) -> Result<Vec<JiraLinkType>, String> {
        let url = format!("{}/rest/api/2/issueLinkType", self.base_url);

        let resp = self.send("GET", &url, self.http.get(&url)).await?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: LinkTypesResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse link types: {e}"))?;

        Ok(data
            .issue_link_types
            .into_iter()
            .map(|t| JiraLinkType {
                name: t.name,
                inward: t.inward,
                outward: t.outward,
            })
            .collect())
    }

    /// Read `key`'s issue links, oriented from `key`'s side (see [`orient`]).
    /// Only the `issuelinks` field is requested; the linked issues come back
    /// with the summary/status/priority/type/assignee Jira embeds per link,
    /// so the list needs no extra call per row.
    pub async fn get_issue_links(&self, key: &str) -> Result<Vec<JiraIssueLink>, String> {
        let url = format!(
            "{}/rest/api/2/issue/{}?fields=issuelinks",
            self.base_url, key
        );

        let resp = self.send("GET", &url, self.http.get(&url)).await?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: IssueLinksResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse issue links: {e}"))?;

        Ok(data
            .fields
            .map(|f| f.issuelinks)
            .unwrap_or_default()
            .into_iter()
            .filter_map(orient)
            .collect())
    }

    /// Remove a single issue link by its id. Irreversible — the caller owns
    /// the confirmation; the ticket on either end is untouched.
    pub async fn delete_issue_link(&self, link_id: &str) -> Result<(), String> {
        let url = format!("{}/rest/api/2/issueLink/{}", self.base_url, link_id);

        let resp = self.send("DELETE", &url, self.http.delete(&url)).await?;
        self.check_status("DELETE", &url, resp).await?;

        Ok(())
    }

    /// Create a link of `type_name` where `outward_key` is the outward issue
    /// and `inward_key` the inward one (`outward_key <outward-phrase>
    /// inward_key`, e.g. `PROJ-1 "blocks" PROJ-2`).
    pub async fn create_issue_link(
        &self,
        type_name: &str,
        outward_key: &str,
        inward_key: &str,
    ) -> Result<(), String> {
        let url = format!("{}/rest/api/2/issueLink", self.base_url);

        let body = serde_json::json!({
            "type": { "name": type_name },
            "outwardIssue": { "key": outward_key },
            "inwardIssue": { "key": inward_key },
        });

        let resp = self
            .send("POST", &url, self.http.post(&url).json(&body))
            .await?;
        self.check_status("POST", &url, resp).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `issuelinks` payload with one link per direction, shaped like the
    /// server's but with invented keys/names.
    fn payload() -> &'static str {
        r#"{
          "key": "ABC-1",
          "fields": {
            "issuelinks": [
              {
                "id": "40001",
                "type": { "name": "Blocks", "inward": "is blocked by", "outward": "blocks" },
                "outwardIssue": {
                  "key": "ABC-2",
                  "fields": {
                    "summary": "Downstream cleanup",
                    "status": { "name": "Open" },
                    "priority": { "name": "High" },
                    "issuetype": { "name": "Task" },
                    "assignee": { "displayName": "Ada Doe", "name": "ada" }
                  }
                }
              },
              {
                "id": "40002",
                "type": { "name": "Blocks", "inward": "is blocked by", "outward": "blocks" },
                "inwardIssue": {
                  "key": "ABC-3",
                  "fields": {
                    "summary": "Prerequisite work",
                    "status": { "name": "Done" },
                    "issuetype": { "name": "Bug" }
                  }
                }
              }
            ]
          }
        }"#
    }

    fn parse(json: &str) -> Vec<JiraIssueLink> {
        let data: IssueLinksResponse = serde_json::from_str(json).expect("parse payload");
        data.fields
            .map(|f| f.issuelinks)
            .unwrap_or_default()
            .into_iter()
            .filter_map(orient)
            .collect()
    }

    #[test]
    fn outward_entry_makes_the_queried_issue_the_inward_side() {
        // The entry names ABC-2 as the *outward* issue, so ABC-1 is inward and
        // the row must read "is blocked by ABC-2".
        let link = &parse(payload())[0];
        assert_eq!(link.id, "40001");
        assert_eq!(link.key, "ABC-2");
        assert_eq!(link.relation, "is blocked by");
        assert!(!link.outward);
        assert_eq!(link.type_name, "Blocks");
        assert_eq!(link.summary, "Downstream cleanup");
        assert_eq!(link.status, "Open");
        assert_eq!(link.priority, "High");
        assert_eq!(link.issue_type, "Task");
        assert_eq!(link.assignee, "Ada Doe");
    }

    #[test]
    fn inward_entry_makes_the_queried_issue_the_outward_side() {
        let link = &parse(payload())[1];
        assert_eq!(link.key, "ABC-3");
        assert_eq!(link.relation, "blocks");
        assert!(link.outward);
        // Fields the payload omits render as empty, not as a parse failure.
        assert_eq!(link.priority, "");
        assert_eq!(link.assignee, "");
    }

    #[test]
    fn a_link_without_either_end_is_dropped() {
        // Jira emits this when the user may see the link but not the issue on
        // the other side.
        let links = parse(
            r#"{ "fields": { "issuelinks": [
                 { "id": "40003", "type": { "name": "Relates" } }
               ] } }"#,
        );
        assert!(links.is_empty());
    }

    #[test]
    fn an_issue_without_links_yields_an_empty_list() {
        assert!(parse(r#"{ "key": "ABC-1", "fields": { "issuelinks": [] } }"#).is_empty());
        // `fields` itself absent (a response shape older deployments emit).
        assert!(parse(r#"{ "key": "ABC-1" }"#).is_empty());
    }
}
