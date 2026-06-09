//! Structured query layer.
//!
//! A Taiga query body is a YAML mapping with three top-level keys:
//!
//! ```yaml
//! queries:
//!   - { type: [task, issue, epic], project: 3, assigned_to: $me }
//!   - { type: userstory, project: 3, assigned_to: $me, is_archived: false }
//! sort:                # optional — list of {column, direction} pairs
//!   - { column: type, direction: asc }
//!   - { column: modified, direction: desc }
//! page:                # optional — default page size for the YAML view
//!   size: 50
//! ```
//!
//! `queries` is the only required field. Each entry fans out into the
//! cartesian product of its list-valued fields and one `QuerySpec` is
//! emitted per `(item_type, filter-set)` tuple. Specs run in parallel
//! and the merged result is sliced to the caller's effective page after
//! a global sort.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use not_yet_done_content::http_log;
use not_yet_done_content::{SortDirection, SortKey};
use serde::Deserialize;

use super::TaigaClient;
use super::project_meta::TaigaMember;

/// Sentinel emitted by the TUI substitution layer when an
/// `<input_if_numeric>` placeholder didn't match. Adapter drops any spec
/// containing it.
pub const OMIT_SENTINEL: &str = "__OMIT__";

/// `$me` placeholder. Resolved against the cached `/users/me` response.
const ME_PLACEHOLDER: &str = "$me";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemType {
    Task,
    Issue,
    Epic,
    UserStory,
}

impl ItemType {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemType::Task => "task",
            ItemType::Issue => "issue",
            ItemType::Epic => "epic",
            ItemType::UserStory => "userstory",
        }
    }

    /// Sort priority: task < issue < epic < userstory.
    pub fn sort_priority(self) -> u8 {
        match self {
            ItemType::Task => 0,
            ItemType::Issue => 1,
            ItemType::Epic => 2,
            ItemType::UserStory => 3,
        }
    }

    /// Path segment under `/api/v1/`: `tasks`, `issues`, `epics`,
    /// `userstories`. Used for list, detail, watch/unwatch and attachment
    /// endpoints (all of which share this prefix).
    pub fn url_segment(self) -> &'static str {
        match self {
            ItemType::Task => "tasks",
            ItemType::Issue => "issues",
            ItemType::Epic => "epics",
            ItemType::UserStory => "userstories",
        }
    }

    /// Path segment used in the human-facing web URL
    /// (`<base>/project/<slug>/<seg>/<ref>`). Diverges from
    /// [`Self::url_segment`] for userstories: the API uses `userstories`,
    /// the UI route uses `us`.
    pub fn web_segment(self) -> &'static str {
        match self {
            ItemType::Task => "task",
            ItemType::Issue => "issue",
            ItemType::Epic => "epic",
            ItemType::UserStory => "us",
        }
    }

    fn endpoint(self) -> String {
        format!("/api/v1/{}", self.url_segment())
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "task" => Ok(ItemType::Task),
            "issue" => Ok(ItemType::Issue),
            "epic" => Ok(ItemType::Epic),
            "userstory" | "user_story" => Ok(ItemType::UserStory),
            other => Err(format!("unknown query type: {other}")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct QuerySpec {
    pub item_type: ItemType,
    /// Raw filter pairs, sent as `?key=value` query params. Multi-value
    /// fields (arrays in YAML) become repeated params (e.g. `?watchers=1&watchers=2`).
    pub params: Vec<(String, String)>,
}

/// Result of parsing a Taiga query body. The TUI may override `sort` and
/// `page_size` per call via [`not_yet_done_content::ListParams`]; the
/// parsed values act as YAML-level defaults.
#[derive(Clone, Debug, Default)]
pub struct ParsedTaigaQuery {
    pub queries: Vec<QuerySpec>,
    pub sort: Vec<SortKey>,
    pub page_size: Option<u32>,
}

/// Parse the YAML query string from view-config into a structured query.
///
/// Top level is a mapping with `queries`, optional `sort`, optional
/// `page`:
///
/// ```yaml
/// queries:
///   - { type: [task, issue], project: [2, 3], assigned_to: $me }
/// sort:
///   - { column: modified, direction: desc }
/// page:
///   size: 50
/// ```
///
/// Each entry of `queries` fans out into the cartesian product of its
/// list-valued fields. This is needed because Taiga's API rejects
/// multi-value filters (`?project=2,3` → 400, `?project__in=2,3` is
/// silently ignored, repeated `?project=2&project=3` only keeps the last).
///
/// Blocks containing the [`OMIT_SENTINEL`] in any value are dropped
/// silently — that's how the TUI signals a non-applicable conditional
/// placeholder (e.g. `<input_if_numeric>` with non-numeric input).
pub fn parse_taiga_query(yaml: &str) -> Result<ParsedTaigaQuery, String> {
    let raw: RawTaigaQuery = serde_yaml::from_str(yaml.trim())
        .map_err(|e| format!("parse taiga query: {e}"))?;
    let queries = expand_queries(raw.queries)?;
    let sort = raw
        .sort
        .into_iter()
        .map(parse_sort_key)
        .collect::<Result<Vec<_>, _>>()?;
    let page_size = raw.page.and_then(|p| p.size);
    Ok(ParsedTaigaQuery { queries, sort, page_size })
}

/// Compatibility shim so existing call sites keep compiling. Returns just
/// the expanded specs from a full parsed query.
pub fn parse_query_yaml(yaml: &str) -> Result<Vec<QuerySpec>, String> {
    Ok(parse_taiga_query(yaml)?.queries)
}

fn expand_queries(raw: Vec<RawQuerySpec>) -> Result<Vec<QuerySpec>, String> {
    let mut specs = Vec::new();
    'next: for raw_spec in raw {
        let types = parse_types(&raw_spec.r#type)?;
        let mut dimensions: Vec<(String, Vec<String>)> = Vec::with_capacity(raw_spec.extra.len());
        for (key, value) in raw_spec.extra {
            let key_str = match key.as_str() {
                Some(s) => s.to_string(),
                None => return Err(format!("non-string filter key: {key:?}")),
            };
            let values = flatten_value(&value);
            if values.iter().any(|v| v == OMIT_SENTINEL) {
                continue 'next;
            }
            dimensions.push((key_str, values));
        }
        for combo in cartesian(&dimensions) {
            for &t in &types {
                specs.push(QuerySpec {
                    item_type: t,
                    params: combo.clone(),
                });
            }
        }
    }
    Ok(specs)
}

fn parse_sort_key(raw: RawSortKey) -> Result<SortKey, String> {
    let direction = match raw.direction.to_ascii_lowercase().as_str() {
        "asc" | "ascending" => SortDirection::Asc,
        "desc" | "descending" => SortDirection::Desc,
        other => return Err(format!("unknown sort direction: {other}")),
    };
    Ok(SortKey { column: raw.column, direction })
}

fn parse_types(v: &serde_yaml::Value) -> Result<Vec<ItemType>, String> {
    match v {
        serde_yaml::Value::String(s) => Ok(vec![ItemType::parse(s)?]),
        serde_yaml::Value::Sequence(seq) => {
            if seq.is_empty() {
                return Err("type list must not be empty".into());
            }
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                let s = item
                    .as_str()
                    .ok_or_else(|| format!("type entries must be strings, got: {item:?}"))?;
                out.push(ItemType::parse(s)?);
            }
            Ok(out)
        }
        other => Err(format!("type must be string or list, got: {other:?}")),
    }
}

/// Cartesian product across dimensions. Empty `dimensions` yields one
/// empty combination (so a spec with only a `type` still emits).
fn cartesian(dimensions: &[(String, Vec<String>)]) -> Vec<Vec<(String, String)>> {
    let mut result: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for (key, values) in dimensions {
        let mut next = Vec::with_capacity(result.len() * values.len().max(1));
        for prefix in &result {
            for v in values {
                let mut p = prefix.clone();
                p.push((key.clone(), v.clone()));
                next.push(p);
            }
        }
        result = next;
    }
    result
}

#[derive(Deserialize)]
struct RawTaigaQuery {
    queries: Vec<RawQuerySpec>,
    #[serde(default)]
    sort: Vec<RawSortKey>,
    #[serde(default)]
    page: Option<RawPage>,
}

#[derive(Deserialize)]
struct RawQuerySpec {
    r#type: serde_yaml::Value,
    #[serde(flatten)]
    extra: serde_yaml::Mapping,
}

#[derive(Deserialize)]
struct RawSortKey {
    column: String,
    direction: String,
}

#[derive(Deserialize)]
struct RawPage {
    #[serde(default)]
    size: Option<u32>,
}

/// Render YAML scalar / sequence into one or more URL-param string values.
/// Mappings inside filter params are not meaningful for Taiga's API and are
/// flattened to their JSON-string form (caller probably has a typo).
fn flatten_value(v: &serde_yaml::Value) -> Vec<String> {
    match v {
        serde_yaml::Value::Null => vec![String::new()],
        serde_yaml::Value::Bool(b) => vec![b.to_string()],
        serde_yaml::Value::Number(n) => vec![n.to_string()],
        serde_yaml::Value::String(s) => vec![s.clone()],
        serde_yaml::Value::Sequence(seq) => {
            seq.iter().flat_map(flatten_value).collect()
        }
        serde_yaml::Value::Mapping(_) | serde_yaml::Value::Tagged(_) => {
            vec![serde_yaml::to_string(v).unwrap_or_default()]
        }
    }
}

/// Resolve `$me` → user-id-string. Other values pass through.
async fn resolve_param(client: &TaigaClient, value: &str) -> Result<String, String> {
    if value == ME_PLACEHOLDER {
        let id = client.current_user_id().await?;
        Ok(id.to_string())
    } else {
        Ok(value.to_string())
    }
}

/// One row in the merged result list.
#[derive(Clone, Debug)]
pub struct ItemSummary {
    pub item_type: ItemType,
    pub id: u64,
    pub r#ref: u64,
    pub project_id: u64,
    pub project_slug: Option<String>,
    pub subject: String,
    pub status: String,
    /// All assignee user IDs from `assigned_users`. After
    /// `resolve_assignees`, sorted current-user-first then alphabetical.
    pub assignee_ids: Vec<u64>,
    /// Display names parallel to `assignee_ids`. Empty until
    /// `resolve_assignees` runs.
    pub assignees: Vec<String>,
    /// Canonical usernames parallel to `assignee_ids`. Empty until
    /// `resolve_assignees` runs.
    pub assignee_usernames: Vec<String>,
    pub modified: Option<String>,
    pub total_attachments: u64,
}

/// Run all specs in parallel and merge the results, deduplicating by
/// `(item_type, id)`. The returned vector is in unspecified order — the
/// caller is responsible for applying the desired sort.
pub async fn run_queries(
    client: Arc<TaigaClient>,
    specs: Vec<QuerySpec>,
) -> Result<Vec<ItemSummary>, String> {
    let mut handles = Vec::with_capacity(specs.len());
    for spec in specs {
        let client = Arc::clone(&client);
        handles.push(tokio::spawn(
            async move { run_one(&client, spec).await },
        ));
    }
    let mut all = Vec::new();
    for h in handles {
        let chunk = h.await.map_err(|e| format!("join: {e}"))??;
        all.extend(chunk);
    }
    // Dedup by (item_type, id) — multiple specs can pull the same row
    // (e.g. /tasks?q=foo + /tasks?ref=42 if input is numeric).
    all.sort_by(|a, b| {
        a.item_type
            .cmp(&b.item_type)
            .then_with(|| a.id.cmp(&b.id))
    });
    all.dedup_by(|a, b| a.item_type == b.item_type && a.id == b.id);
    resolve_assignees(&client, &mut all).await;
    Ok(all)
}

/// Fill `assignees` / `assignee_usernames` on each summary by looking up
/// `assignee_ids` against the per-project members cache, then re-order
/// the IDs (and parallel display/username vectors) so the current user
/// appears first, with the remaining members sorted alphabetically by
/// display name. Missing members fall back to `user-<id>`.
async fn resolve_assignees(client: &TaigaClient, items: &mut [ItemSummary]) {
    let current_user_id = client.current_user_id().await.unwrap_or(0);
    let needed_projects: HashSet<u64> = items
        .iter()
        .filter(|it| !it.assignee_ids.is_empty())
        .map(|it| it.project_id)
        .collect();
    let mut member_by_id: HashMap<u64, TaigaMember> = HashMap::new();
    for project_id in needed_projects {
        if let Ok(members) = client.ensure_members(project_id).await {
            for m in members {
                member_by_id.entry(m.id).or_insert(m);
            }
        }
    }
    for it in items.iter_mut() {
        if it.assignee_ids.is_empty() {
            continue;
        }
        let mut named: Vec<(u64, String, String)> = it
            .assignee_ids
            .iter()
            .map(|id| {
                let m = member_by_id.get(id);
                let display = m
                    .map(|m| {
                        if m.full_name.is_empty() {
                            m.username.clone()
                        } else {
                            m.full_name.clone()
                        }
                    })
                    .unwrap_or_else(|| format!("user-{id}"));
                let username = m
                    .map(|m| m.username.clone())
                    .unwrap_or_else(|| format!("user-{id}"));
                (*id, display, username)
            })
            .collect();
        named.sort_by(|a, b| {
            let a_cur = a.0 == current_user_id;
            let b_cur = b.0 == current_user_id;
            match (a_cur, b_cur) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.1.to_lowercase().cmp(&b.1.to_lowercase()),
            }
        });
        it.assignee_ids = named.iter().map(|(id, _, _)| *id).collect();
        it.assignees = named.iter().map(|(_, d, _)| d.clone()).collect();
        it.assignee_usernames = named.into_iter().map(|(_, _, u)| u).collect();
    }
}

/// Default sort when neither runtime params nor YAML provide one:
/// type-priority asc, modified desc. Mirrors the legacy behaviour
/// pre-sort feature.
pub fn default_sort() -> Vec<SortKey> {
    vec![
        SortKey { column: "type".into(), direction: SortDirection::Asc },
        SortKey { column: "modified".into(), direction: SortDirection::Desc },
    ]
}

/// Sortable column keys advertised by [`Node::sortable_columns`] for
/// `taiga:item` and the per-type variants.
pub fn sortable_column_keys() -> &'static [&'static str] {
    &["ref", "type", "status", "assignee", "subject", "modified", "project"]
}

/// Apply the requested sort to `items` in place, dropping keys that map
/// to no known field. Returns the subset that was actually honoured.
pub fn apply_sort(items: &mut [ItemSummary], sort: &[SortKey]) -> Vec<SortKey> {
    let applied: Vec<SortKey> = sort
        .iter()
        .filter(|k| sortable_column_keys().contains(&k.column.as_str()))
        .cloned()
        .collect();
    if applied.is_empty() {
        return applied;
    }
    items.sort_by(|a, b| {
        for key in &applied {
            let ord = compare_on_column(&key.column, a, b);
            let ord = match key.direction {
                SortDirection::Asc => ord,
                SortDirection::Desc => ord.reverse(),
            };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
    applied
}

fn compare_on_column(
    column: &str,
    a: &ItemSummary,
    b: &ItemSummary,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match column {
        "ref" => a.r#ref.cmp(&b.r#ref),
        "type" => a.item_type.sort_priority().cmp(&b.item_type.sort_priority()),
        "status" => a.status.cmp(&b.status),
        "assignee" => {
            // Compare by first display name (which is current-user-first
            // for the logged-in user); empty assignee lists sort last.
            let a_key = a.assignees.first().map(String::as_str).unwrap_or("");
            let b_key = b.assignees.first().map(String::as_str).unwrap_or("");
            match (a_key.is_empty(), b_key.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                _ => a_key.to_lowercase().cmp(&b_key.to_lowercase()),
            }
        }
        "subject" => a.subject.cmp(&b.subject),
        "modified" => a.modified.cmp(&b.modified),
        "project" => a
            .project_slug
            .cmp(&b.project_slug)
            .then_with(|| a.project_id.cmp(&b.project_id)),
        _ => Ordering::Equal,
    }
}

/// Page size used for the underlying Taiga API calls. We aggregate all
/// pages of every spec before slicing locally — this size is purely a
/// fetch-batch tuning, not the user-visible page size (see
/// [`ListResult::page`]).
const FETCH_PAGE_SIZE: u32 = 100;

/// Safety cap on the number of fetch-batch pages per spec. Prevents an
/// infinite loop if Taiga ever stops shrinking the page on the last
/// request. With `FETCH_PAGE_SIZE = 100` this caps a single spec at
/// 10 000 items, which is more than enough for an interactive view.
const FETCH_MAX_PAGES: u32 = 100;

async fn run_one(
    client: &TaigaClient,
    spec: QuerySpec,
) -> Result<Vec<ItemSummary>, String> {
    let endpoint = format!("{}{}", client.base_url, spec.item_type.endpoint());

    // Resolve `$me` once per filter — it's stable for the duration of
    // the call and doesn't depend on the page index.
    let mut filter_parts: Vec<String> = Vec::with_capacity(spec.params.len());
    for (k, v) in &spec.params {
        let resolved = resolve_param(client, v).await?;
        filter_parts.push(format!("{}={}", urlencode(k), urlencode(&resolved)));
    }

    let mut all = Vec::new();
    let mut page_n: u32 = 1;
    loop {
        let mut parts = filter_parts.clone();
        parts.push(format!("page={page_n}"));
        parts.push(format!("page_size={FETCH_PAGE_SIZE}"));
        let url = format!("{endpoint}?{}", parts.join("&"));

        let headers = client.auth_headers()?;
        http_log::log_request("GET", &url);
        let resp = client
            .send_retrying("GET", &url, || client.http.get(&url).headers(headers.clone()))
            .await?;

        // Some Taiga deployments respond with 404 when paging past the
        // last page (rather than returning an empty array). Treat as
        // end-of-list rather than a hard error.
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            http_log::log_response("GET", &url, 404);
            break;
        }
        let resp = http_log::check_status("GET", &url, resp).await?;

        let raw: Vec<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| format!("{} parse: {e}", spec.item_type.as_str()))?;

        let count = raw.len();
        all.extend(raw.into_iter().map(|item| parse_item(spec.item_type, &item)));

        if count < FETCH_PAGE_SIZE as usize {
            break;
        }
        page_n += 1;
        if page_n > FETCH_MAX_PAGES {
            break;
        }
    }
    Ok(all)
}

fn parse_item(item_type: ItemType, v: &serde_json::Value) -> ItemSummary {
    let s = |key: &str| {
        v.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let u = |key: &str| v.get(key).and_then(|x| x.as_u64()).unwrap_or(0);

    // Multi-assignee: `assigned_users` is the array of all assignees.
    // `assigned_to_extra_info` (singular) is only used as a fallback if
    // the array is empty — its display fields would otherwise be lost.
    let mut assignee_ids: Vec<u64> = v
        .get("assigned_users")
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_u64()).collect())
        .unwrap_or_default();
    if assignee_ids.is_empty() {
        if let Some(id) = v.get("assigned_to").and_then(|x| x.as_u64()) {
            assignee_ids.push(id);
        }
    }

    let status = v
        .get("status_extra_info")
        .and_then(|e| e.get("name"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    let project_slug = v
        .get("project_extra_info")
        .and_then(|e| e.get("slug"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());

    ItemSummary {
        item_type,
        id: u("id"),
        r#ref: u("ref"),
        project_id: u("project"),
        project_slug,
        subject: s("subject"),
        status,
        assignee_ids,
        assignees: Vec::new(),
        assignee_usernames: Vec::new(),
        modified: v
            .get("modified_date")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        total_attachments: u("total_attachments"),
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_query_list() {
        let yaml = r#"
queries:
  - { type: task, assigned_to: "$me" }
  - { type: issue, watchers: ["$me"] }
  - { type: epic, q: "foo" }
"#;
        let specs = parse_query_yaml(yaml).unwrap();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].item_type, ItemType::Task);
        assert_eq!(specs[0].params, vec![("assigned_to".into(), "$me".into())]);
        assert_eq!(specs[1].item_type, ItemType::Issue);
        assert_eq!(specs[1].params, vec![("watchers".into(), "$me".into())]);
        assert_eq!(specs[2].params, vec![("q".into(), "foo".into())]);
    }

    #[test]
    fn parse_drops_omit_specs() {
        let yaml = r#"
queries:
  - { type: task, q: "foo" }
  - { type: task, ref: "__OMIT__" }
"#;
        let specs = parse_query_yaml(yaml).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].params[0].0, "q");
    }

    #[test]
    fn parse_list_value_fans_out() {
        let yaml = r#"
queries:
  - { type: task, watchers: [1, 2, 3] }
"#;
        let specs = parse_query_yaml(yaml).unwrap();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].params, vec![("watchers".into(), "1".into())]);
        assert_eq!(specs[1].params, vec![("watchers".into(), "2".into())]);
        assert_eq!(specs[2].params, vec![("watchers".into(), "3".into())]);
    }

    #[test]
    fn parse_type_list_fans_out() {
        let yaml = r#"
queries:
  - { type: [task, issue, epic], project: 3 }
"#;
        let specs = parse_query_yaml(yaml).unwrap();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].item_type, ItemType::Task);
        assert_eq!(specs[1].item_type, ItemType::Issue);
        assert_eq!(specs[2].item_type, ItemType::Epic);
        for s in &specs {
            assert_eq!(s.params, vec![("project".into(), "3".into())]);
        }
    }

    #[test]
    fn parse_cartesian_type_x_field() {
        let yaml = r#"
queries:
  - { type: [task, issue], project: [2, 3], assigned_to: $me }
"#;
        let specs = parse_query_yaml(yaml).unwrap();
        // 2 types × 2 projects = 4 specs, each carrying assigned_to.
        assert_eq!(specs.len(), 4);
        let pairs: Vec<_> = specs
            .iter()
            .map(|s| {
                let project = s
                    .params
                    .iter()
                    .find(|(k, _)| k == "project")
                    .unwrap()
                    .1
                    .clone();
                (s.item_type, project)
            })
            .collect();
        assert!(pairs.contains(&(ItemType::Task, "2".to_string())));
        assert!(pairs.contains(&(ItemType::Task, "3".to_string())));
        assert!(pairs.contains(&(ItemType::Issue, "2".to_string())));
        assert!(pairs.contains(&(ItemType::Issue, "3".to_string())));
        for s in &specs {
            assert!(s.params.iter().any(|(k, v)| k == "assigned_to" && v == "$me"));
        }
    }

    #[test]
    fn parse_single_element_list_equals_scalar() {
        let yaml_scalar = "queries:\n  - { type: task, watchers: $me }\n";
        let yaml_list = "queries:\n  - { type: task, watchers: [$me] }\n";
        let s_scalar = parse_query_yaml(yaml_scalar).unwrap();
        let s_list = parse_query_yaml(yaml_list).unwrap();
        assert_eq!(s_scalar.len(), 1);
        assert_eq!(s_list.len(), 1);
        assert_eq!(s_scalar[0].params, s_list[0].params);
    }

    #[test]
    fn parse_rejects_unknown_type() {
        let yaml = "queries:\n  - { type: lol, q: x }\n";
        let err = parse_query_yaml(yaml).unwrap_err();
        assert!(err.contains("lol"), "{err}");
    }

    #[test]
    fn item_type_sort_priority() {
        assert!(ItemType::Task.sort_priority() < ItemType::Issue.sort_priority());
        assert!(ItemType::Issue.sort_priority() < ItemType::Epic.sort_priority());
    }

    #[test]
    fn parse_top_level_mapping_with_queries_and_sort() {
        let yaml = r#"
queries:
  - { type: task, assigned_to: $me }
sort:
  - { column: modified, direction: desc }
page:
  size: 25
"#;
        let parsed = parse_taiga_query(yaml).unwrap();
        assert_eq!(parsed.queries.len(), 1);
        assert_eq!(parsed.sort.len(), 1);
        assert_eq!(parsed.sort[0].column, "modified");
        assert_eq!(parsed.sort[0].direction, SortDirection::Desc);
        assert_eq!(parsed.page_size, Some(25));
    }

    #[test]
    fn parse_rejects_legacy_bare_sequence() {
        // The pre-Phase-2 YAML body was a bare sequence. After the
        // breaking migration this must fail loudly so users see they
        // need to wrap it in `queries:`.
        let yaml = r#"
- { type: task, assigned_to: $me }
"#;
        let err = parse_taiga_query(yaml).unwrap_err();
        assert!(err.contains("parse taiga query"), "{err}");
    }

    #[test]
    fn parse_omitted_sort_and_page_default_to_empty() {
        let yaml = r#"
queries:
  - { type: task }
"#;
        let parsed = parse_taiga_query(yaml).unwrap();
        assert!(parsed.sort.is_empty());
        assert_eq!(parsed.page_size, None);
    }

    #[test]
    fn parse_rejects_unknown_sort_direction() {
        let yaml = r#"
queries:
  - { type: task }
sort:
  - { column: modified, direction: sideways }
"#;
        let err = parse_taiga_query(yaml).unwrap_err();
        assert!(err.contains("sideways"), "{err}");
    }

    fn item(item_type: ItemType, r#ref: u64, status: &str, modified: Option<&str>) -> ItemSummary {
        ItemSummary {
            item_type,
            id: r#ref,
            r#ref,
            project_id: 1,
            project_slug: None,
            subject: format!("subject-{ref}"),
            status: status.into(),
            assignee_ids: Vec::new(),
            assignees: Vec::new(),
            assignee_usernames: Vec::new(),
            modified: modified.map(|s| s.into()),
            total_attachments: 0,
        }
    }

    #[test]
    fn apply_sort_drops_unknown_columns() {
        let mut items = vec![
            item(ItemType::Task, 2, "Open", Some("2026-01-02")),
            item(ItemType::Task, 1, "Open", Some("2026-01-01")),
        ];
        let applied = apply_sort(
            &mut items,
            &[
                SortKey { column: "nonsense".into(), direction: SortDirection::Asc },
                SortKey { column: "ref".into(), direction: SortDirection::Asc },
            ],
        );
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].column, "ref");
        assert_eq!(items[0].r#ref, 1);
        assert_eq!(items[1].r#ref, 2);
    }

    #[test]
    fn apply_sort_modified_desc() {
        let mut items = vec![
            item(ItemType::Task, 1, "Open", Some("2026-01-01")),
            item(ItemType::Task, 2, "Open", Some("2026-02-01")),
            item(ItemType::Task, 3, "Open", None),
        ];
        apply_sort(
            &mut items,
            &[SortKey { column: "modified".into(), direction: SortDirection::Desc }],
        );
        // None sorts low under Option<T>, so DESC puts Some(latest) first
        // and None last.
        assert_eq!(items[0].r#ref, 2);
        assert_eq!(items[1].r#ref, 1);
        assert_eq!(items[2].r#ref, 3);
    }

    #[test]
    fn default_sort_matches_legacy_behaviour() {
        let keys = default_sort();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].column, "type");
        assert_eq!(keys[0].direction, SortDirection::Asc);
        assert_eq!(keys[1].column, "modified");
        assert_eq!(keys[1].direction, SortDirection::Desc);
    }

    #[test]
    fn apply_default_sort_groups_by_type_then_modified_desc() {
        let mut items = vec![
            item(ItemType::Issue, 9, "Open", Some("2026-02-01")),
            item(ItemType::Task, 8, "Open", Some("2026-01-01")),
            item(ItemType::Task, 7, "Open", Some("2026-03-01")),
        ];
        apply_sort(&mut items, &default_sort());
        assert_eq!(items[0].item_type, ItemType::Task);
        assert_eq!(items[0].r#ref, 7); // newer task first
        assert_eq!(items[1].item_type, ItemType::Task);
        assert_eq!(items[1].r#ref, 8);
        assert_eq!(items[2].item_type, ItemType::Issue);
    }
}
