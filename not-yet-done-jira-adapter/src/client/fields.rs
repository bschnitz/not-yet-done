//! Custom-field discovery.
//!
//! Story Points is not a built-in Jira field. Every instance carries it as
//! a *custom* field whose id was assigned when the field was created, so
//! `customfield_10006` on one server names something else on the next. The
//! only portable handle is the field's **name**, and the field catalogue
//! (`/rest/api/2/field`) is what maps that name onto both the REST id and
//! the JQL clause. We resolve it once per client and cache the answer —
//! including a negative one, so an instance without the field pays a
//! single call rather than one per listing.

use not_yet_done_content::http_log;
use serde::Deserialize;

use super::JiraClient;

/// How a discovered custom field is addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CustomField {
    /// REST id — what a `fields=` request asks for and what keys the field
    /// inside the returned issue JSON (`customfield_10006`).
    pub(super) id: String,
    /// JQL clause naming the same field (`cf[10006]`). The numeric form is
    /// used rather than the display name because a name containing spaces
    /// has to be quoted and stops being unambiguous the moment a second
    /// field shares it — which is exactly the case here, since several
    /// plugins ship their own "… story points".
    pub(super) clause: String,
}

/// Field names that mean "the story-points estimate", most specific first.
/// `story points` is what Jira Server / Data Center calls it, `story point
/// estimate` is the Cloud name. Matching is case-insensitive and exact, so
/// neighbours like "Original story points" (Portfolio) never win.
const STORY_POINT_NAMES: &[&str] = &["story points", "story point estimate"];

/// Custom-field *type* keys that identify a story-points field when no name
/// matched — a localised instance renames the field but keeps the type.
/// `original-story-points` is excluded by the `!contains` guard below.
const STORY_POINT_TYPE_MARKER: &str = "story-points";

#[derive(Deserialize)]
struct FieldEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    schema: Option<FieldSchema>,
}

#[derive(Deserialize)]
struct FieldSchema {
    #[serde(default)]
    custom: Option<String>,
    #[serde(default, rename = "customId")]
    custom_id: Option<i64>,
}

/// Build the addressing pair from a raw field id. Accepts what a user is
/// likely to write in the config — `customfield_10006` or the bare `10006`
/// — and returns `None` for anything without a numeric id, because the JQL
/// clause cannot be formed without it.
pub(super) fn custom_field_from_id(raw: &str) -> Option<CustomField> {
    let raw = raw.trim();
    let digits = raw.rsplit('_').next().unwrap_or(raw);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(CustomField {
        id: format!("customfield_{digits}"),
        clause: format!("cf[{digits}]"),
    })
}

/// Pick the story-points field out of a field catalogue. Name match first
/// (it is what a human configured the board with), field type second.
fn pick_story_points(fields: &[FieldEntry]) -> Option<CustomField> {
    let entry_to_field = |e: &FieldEntry| -> Option<CustomField> {
        match e.schema.as_ref().and_then(|s| s.custom_id) {
            Some(id) => Some(CustomField {
                id: e.id.clone(),
                clause: format!("cf[{id}]"),
            }),
            // No `customId` in the catalogue — derive it from the id, which
            // for a custom field always carries the number.
            None => custom_field_from_id(&e.id),
        }
    };

    for wanted in STORY_POINT_NAMES {
        if let Some(e) = fields.iter().find(|e| {
            e.name
                .as_deref()
                .is_some_and(|n| n.trim().eq_ignore_ascii_case(wanted))
        }) {
            return entry_to_field(e);
        }
    }

    fields
        .iter()
        .find(|e| {
            e.schema
                .as_ref()
                .and_then(|s| s.custom.as_deref())
                .is_some_and(|c| {
                    c.contains(STORY_POINT_TYPE_MARKER) && !c.contains("original-story-points")
                })
        })
        .and_then(entry_to_field)
}

/// Render a numeric custom-field value as a table cell. An absent or `null`
/// value is the empty cell (most issues carry no estimate), a whole number
/// drops the `.0` serde hands back so the common estimate reads `5` rather
/// than `5.0`, and a fractional one keeps its digits (`2.5`).
pub(super) fn number_cell(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::Number(n)) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
            Some(f) => format!("{f}"),
            None => n.to_string(),
        },
        // Some deployments back the field with a text or select field.
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

impl JiraClient {
    /// The instance's story-points field, or `None` when it has none.
    ///
    /// Resolved once per client: a configured override short-circuits the
    /// lookup entirely, otherwise the field catalogue is fetched and
    /// searched. A *transport* failure is not cached — the column simply
    /// stays blank for that listing and the next one retries — while a
    /// catalogue that genuinely has no story-points field caches the
    /// negative answer.
    pub(super) async fn story_points_field(&self) -> Option<&CustomField> {
        if let Some(configured) = &self.story_points_override {
            return Some(configured);
        }
        self.story_points
            .get_or_try_init(|| async {
                let url = format!("{}/rest/api/2/field", self.base_url);
                http_log::log_request("GET", &url);
                let resp = self
                    .http
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| http_log::network_error("GET", &url, e))?;
                let resp = self.check_status("GET", &url, resp).await?;
                let body_text = resp
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read response: {e}"))?;
                let fields: Vec<FieldEntry> = serde_json::from_str(&body_text)
                    .map_err(|e| format!("Failed to parse Jira field catalogue: {e}"))?;
                Ok::<_, String>(pick_story_points(&fields))
            })
            .await
            .ok()
            .and_then(|f| f.as_ref())
    }

    /// JQL clause ordering by story points (`cf[10006]`), or `None` when the
    /// instance has no such field. Owned so the caller can hold it across
    /// the `await` that issues the search.
    pub async fn story_points_jql_clause(&self) -> Option<String> {
        self.story_points_field().await.map(|f| f.clause.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, custom: &str, custom_id: Option<i64>) -> FieldEntry {
        FieldEntry {
            id: id.into(),
            name: Some(name.into()),
            schema: Some(FieldSchema {
                custom: Some(custom.into()),
                custom_id,
            }),
        }
    }

    /// The catalogue of a real Server instance carries several fields whose
    /// name contains "story points". Only the exact name may win — picking
    /// the Portfolio field would show a second, unrelated estimate.
    #[test]
    fn exact_name_beats_a_neighbouring_original_story_points() {
        let fields = vec![
            entry(
                "customfield_10500",
                "Original story points",
                "com.example.jpo:jpo-custom-field-original-story-points",
                Some(10500),
            ),
            entry(
                "customfield_10006",
                "Story Points",
                "com.atlassian.jira.plugin.system.customfieldtypes:float",
                Some(10006),
            ),
        ];
        let picked = pick_story_points(&fields).expect("story points found");
        assert_eq!(picked.id, "customfield_10006");
        assert_eq!(picked.clause, "cf[10006]");
    }

    /// Jira Cloud names the same thing "Story point estimate".
    #[test]
    fn cloud_field_name_is_recognised() {
        let fields = vec![entry(
            "customfield_10016",
            "Story point estimate",
            "com.atlassian.jira.plugin.system.customfieldtypes:float",
            Some(10016),
        )];
        assert_eq!(
            pick_story_points(&fields).expect("found").id,
            "customfield_10016"
        );
    }

    /// A localised instance renames the field but keeps the plugin type, so
    /// the type marker is the fallback — and it must still not fall for the
    /// Portfolio "original story points" type.
    #[test]
    fn falls_back_to_the_field_type_when_no_name_matches() {
        let fields = vec![
            entry(
                "customfield_10500",
                "Urspruengliche Story Punkte",
                "com.example.jpo:jpo-custom-field-original-story-points",
                Some(10500),
            ),
            entry(
                "customfield_10007",
                "Story Punkte",
                "com.pyxis.greenhopper.jira:gh-story-points",
                Some(10007),
            ),
        ];
        assert_eq!(
            pick_story_points(&fields).expect("found").id,
            "customfield_10007"
        );
    }

    /// An instance without the field must resolve to a cached `None` rather
    /// than to some arbitrary float column.
    #[test]
    fn an_instance_without_the_field_yields_none() {
        let fields = vec![entry(
            "customfield_10001",
            "Business Value",
            "com.atlassian.jira.plugin.system.customfieldtypes:float",
            Some(10001),
        )];
        assert!(pick_story_points(&fields).is_none());
    }

    /// The config override is what a user types, so both spellings parse
    /// and anything without a numeric id is rejected (no clause could be
    /// built from it).
    #[test]
    fn override_accepts_both_spellings_and_rejects_the_rest() {
        let expected = CustomField {
            id: "customfield_10006".into(),
            clause: "cf[10006]".into(),
        };
        assert_eq!(custom_field_from_id("customfield_10006"), Some(expected));
        assert_eq!(
            custom_field_from_id(" 10006 ").map(|f| f.clause),
            Some("cf[10006]".into())
        );
        assert!(custom_field_from_id("summary").is_none());
        assert!(custom_field_from_id("").is_none());
    }

    /// Estimates are entered as whole numbers but come back as floats; the
    /// column must not read `5.0` where the board reads `5`.
    #[test]
    fn number_cell_drops_a_trailing_zero_but_keeps_halves() {
        let n = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(number_cell(Some(&n("5.0"))), "5");
        assert_eq!(number_cell(Some(&n("13"))), "13");
        assert_eq!(number_cell(Some(&n("2.5"))), "2.5");
        assert_eq!(number_cell(Some(&n("null"))), "");
        assert_eq!(number_cell(None), "");
    }
}
