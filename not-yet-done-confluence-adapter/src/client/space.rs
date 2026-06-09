//! `GET /rest/api/space` — paginated list of Confluence spaces.
//!
//! Server's response carries `start` / `limit` / `size`, but no total count —
//! pagination is detected via the presence of a `_links.next` field. The
//! adapter exposes that via [`SpacePage::has_next`] so callers can issue a
//! follow-up request without re-parsing the link.

use serde::Deserialize;

use not_yet_done_content::http_log;

use super::ConfluenceClient;

/// Subset of the JSON record returned by `/rest/api/space`. Confluence
/// returns more (description excerpts, icons, …); we only consume what
/// the adapter needs for the listing + the `webui` link used by the
/// open-in-browser action.
#[derive(Deserialize, Clone, Debug)]
pub struct SpaceMeta {
    /// Numeric internal ID. Stable across renames; not user-visible.
    pub id: i64,
    /// Short alphanumeric key (e.g. `DEMO`). User-visible and appears in
    /// every URL — we use this as the node id.
    pub key: String,
    /// Human-readable name.
    pub name: String,
    /// `"global"` for normal spaces or `"personal"` for user-spaces.
    /// Stored on the node for column-rendering / filtering downstream.
    #[serde(rename = "type", default)]
    pub space_type: String,
    /// `_links.webui` value — a path relative to `base_url` like
    /// `/spaces/DEMO`. Empty when Confluence omitted the field.
    #[serde(default, rename = "_links", deserialize_with = "deserialize_webui")]
    pub webui: String,
    /// Id of the space's homepage (`homepage.id`) — populated when
    /// callers request `?expand=homepage`. Empty when the server didn't
    /// return it (older instances, or expand omitted by caller). The
    /// adapter uses this to list "top-level pages" as the children of
    /// the homepage, matching what the web UI's tree browser shows.
    ///
    /// The JSON field name is `homepage` (a full Page object); the
    /// custom deserializer pulls just the `id` out so callers don't
    /// have to carry the rest of the nested record.
    #[serde(
        default,
        rename = "homepage",
        deserialize_with = "deserialize_homepage_id"
    )]
    pub homepage_id: String,
}

fn deserialize_webui<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Links {
        #[serde(default)]
        webui: Option<String>,
    }
    let links = Links::deserialize(deserializer)?;
    Ok(links.webui.unwrap_or_default())
}

fn deserialize_homepage_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Confluence returns `homepage` as a full object when `expand=homepage`
    // is passed; absent otherwise. The wrapping `Option` covers the
    // present-but-null case some legacy instances emit. We land in a
    // `serde_json::Value` first so we can branch on null/object without
    // committing to one shape too early.
    let value: serde_json::Value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(String::new());
    }
    #[derive(Deserialize)]
    struct Homepage {
        #[serde(default)]
        id: String,
    }
    let hp: Homepage =
        serde_json::from_value(value).map_err(serde::de::Error::custom)?;
    Ok(hp.id)
}

#[derive(Deserialize, Debug)]
struct SpaceEnvelope {
    #[serde(default)]
    results: Vec<SpaceMeta>,
    #[serde(default)]
    start: u32,
    #[serde(default)]
    limit: u32,
    #[serde(default)]
    size: u32,
    #[serde(default, rename = "_links")]
    links: EnvelopeLinks,
}

#[derive(Deserialize, Debug, Default)]
struct EnvelopeLinks {
    #[serde(default)]
    next: Option<String>,
}

/// One page of [`SpaceMeta`] plus the pagination state needed to render
/// next/prev affordances. `total` is `None` because the Confluence Server
/// endpoint does not report a global count for `/space`.
#[derive(Debug)]
pub struct SpacePage {
    pub spaces: Vec<SpaceMeta>,
    pub start: u32,
    pub limit: u32,
    /// Number of records returned on this page (may be `< limit` on the
    /// last page).
    pub size: u32,
    /// True iff Confluence included a `_links.next` field on the envelope.
    pub has_next: bool,
}

impl ConfluenceClient {
    /// `GET /rest/api/space?start={start}&limit={limit}`. Defaults are
    /// the server's: `start=0`, `limit=25`. Returns the envelope as
    /// [`SpacePage`] so callers can paginate without re-parsing.
    pub async fn list_spaces(&self, start: u32, limit: u32) -> Result<SpacePage, String> {
        self.list_spaces_filtered(start, limit, &[]).await
    }

    /// CF-16 — same as [`Self::list_spaces`] but with an optional
    /// server-side whitelist via repeated `spaceKey=X` query params.
    /// Confluence Server documents this filter on `/rest/api/space`;
    /// passing an empty slice keeps the historic "all spaces" behaviour.
    /// Result order from the server is unspecified — the caller is
    /// responsible for reordering by the desired sequence.
    pub async fn list_spaces_filtered(
        &self,
        start: u32,
        limit: u32,
        space_keys: &[String],
    ) -> Result<SpacePage, String> {
        let url_for_log = format!("{}/rest/api/space", self.base_url());
        http_log::log_request("GET", &url_for_log);
        let start_s = start.to_string();
        let limit_s = limit.to_string();
        let mut query: Vec<(&str, &str)> = vec![
            ("start", &start_s),
            ("limit", &limit_s),
            // Pull each space's homepage id alongside the metadata.
            // Confluence's `/rest/api/space/{KEY}/content/page` endpoint
            // returns *every* page in a space regardless of nesting; the
            // web UI's "Tree browser" shows just the children of the
            // homepage. We replicate that by listing the homepage's
            // direct children, which requires knowing the homepage id
            // upfront — `?expand=homepage` is the cheapest way to get
            // it without a per-space second round-trip.
            ("expand", "homepage"),
        ];
        for key in space_keys {
            query.push(("spaceKey", key.as_str()));
        }
        let resp = self
            .inner_http()
            .get(&url_for_log)
            .query(&query)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url_for_log, e))?;
        let resp = http_log::check_status("GET", &url_for_log, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;
        let env: SpaceEnvelope = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse /space response: {e}"))?;
        Ok(SpacePage {
            spaces: env.results,
            start: env.start,
            limit: env.limit,
            size: env.size,
            has_next: env.links.next.is_some(),
        })
    }

    /// `GET /rest/api/space/{KEY}?expand=homepage` — fetch a single space
    /// with its homepage metadata. Used for the lookup-path (direct
    /// `get_by_id` navigation) where the bulk list didn't run and
    /// `homepage_id` would otherwise stay empty. Same parse path as the
    /// list endpoint, so callers get the same [`SpaceMeta`] shape.
    pub async fn get_space(&self, key: &str) -> Result<SpaceMeta, String> {
        let url = format!("{}/rest/api/space/{key}?expand=homepage", self.base_url());
        http_log::log_request("GET", &url);
        let resp = self
            .inner_http()
            .get(&url)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;
        let meta: SpaceMeta = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse /space/{key} response: {e}"))?;
        Ok(meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_envelope_with_next_link() {
        let body = r#"{
            "results": [
                {
                    "id": 100,
                    "key": "DEMO",
                    "name": "Demo Space",
                    "type": "global",
                    "_links": { "webui": "/spaces/DEMO" }
                },
                {
                    "id": 101,
                    "key": "MX",
                    "name": "Mixed",
                    "type": "personal",
                    "_links": { "webui": "/spaces/~mx" }
                }
            ],
            "start": 0,
            "limit": 25,
            "size": 2,
            "_links": { "next": "/rest/api/space?start=25&limit=25" }
        }"#;
        let env: SpaceEnvelope = serde_json::from_str(body).expect("parses");
        assert_eq!(env.results.len(), 2);
        assert_eq!(env.results[0].key, "DEMO");
        assert_eq!(env.results[0].space_type, "global");
        assert_eq!(env.results[0].webui, "/spaces/DEMO");
        assert_eq!(env.results[1].space_type, "personal");
        assert_eq!(env.results[1].webui, "/spaces/~mx");
        assert_eq!(env.start, 0);
        assert_eq!(env.size, 2);
        assert!(env.links.next.is_some());
    }

    #[test]
    fn parses_minimal_envelope_without_next() {
        // Last page of a result set — no `_links.next`, no webui on one row.
        let body = r#"{
            "results": [
                { "id": 200, "key": "ZZ", "name": "Tail" }
            ],
            "start": 25,
            "limit": 25,
            "size": 1,
            "_links": {}
        }"#;
        let env: SpaceEnvelope = serde_json::from_str(body).expect("parses");
        assert_eq!(env.results.len(), 1);
        assert_eq!(env.results[0].webui, "", "missing webui defaults to empty");
        assert_eq!(env.results[0].space_type, "", "missing type defaults to empty");
        assert_eq!(
            env.results[0].homepage_id, "",
            "missing homepage defaults to empty"
        );
        assert!(env.links.next.is_none());
    }

    #[test]
    fn parses_homepage_id_when_expanded() {
        // `?expand=homepage` returns the full homepage object alongside
        // the space record. We only need its id (used as the
        // tree-browser root for child page listing).
        let body = r#"{
            "id": 17,
            "key": "DEMO",
            "name": "Demo Space",
            "type": "global",
            "_links": { "webui": "/spaces/DEMO" },
            "homepage": {
                "id": "98765",
                "type": "page",
                "title": "Demo Home"
            }
        }"#;
        let meta: SpaceMeta = serde_json::from_str(body).expect("parses");
        assert_eq!(meta.homepage_id, "98765");
    }

    #[test]
    fn tolerates_present_but_null_homepage() {
        // Some legacy / restricted setups return `"homepage": null` even
        // when expanded. The parser must accept this without erroring.
        let body = r#"{
            "id": 17,
            "key": "DEMO",
            "name": "Demo Space",
            "type": "global",
            "_links": { "webui": "/spaces/DEMO" },
            "homepage": null
        }"#;
        let meta: SpaceMeta = serde_json::from_str(body).expect("parses");
        assert_eq!(meta.homepage_id, "");
    }
}
