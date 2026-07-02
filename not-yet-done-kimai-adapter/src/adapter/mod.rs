//! Kimai ContentAdapter implementation.
//!
//! Read-only, flat: the root lists `kimai:timesheet` rows for the
//! configured lookback window. Grouping (day/week/month) happens
//! engine-side via the view's `group_by` — the adapter only has to emit
//! RFC3339 `begin`/`end` values and a numeric `duration` (seconds), like
//! the local trackings adapter. Project and activity arrive as numeric
//! ids from `/api/timesheets` and are resolved to names via the
//! `/api/projects` + `/api/activities` lookups on every listing.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Days, Local};

use not_yet_done_content::*;

use crate::client::{KimaiClient, KimaiProject, KimaiTimesheet};

mod auth_bridge;
mod config;
mod factory;

pub use factory::KimaiAdapterFactory;

use auth_bridge::AuthBridge;

fn timesheet_node_type() -> NodeType {
    NodeType {
        type_id: "kimai:timesheet".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Timesheet".into(),
    }
}

fn other_err(msg: impl Into<String>) -> ContentError {
    ContentError::Other(msg.into().into())
}

pub struct KimaiAdapter {
    auth: Arc<AuthBridge>,
    connection_name: String,
    instance_id: String,
    lookback_days: u32,
}

impl KimaiAdapter {
    /// Build from a pre-built [`AuthBridge`], produced by the factory.
    pub(in crate::adapter) fn from_parts(
        auth: Arc<AuthBridge>,
        connection_name: String,
        instance_id: String,
        lookback_days: u32,
    ) -> Self {
        Self {
            auth,
            connection_name,
            instance_id,
            lookback_days,
        }
    }
}

#[async_trait]
impl ContentAdapter for KimaiAdapter {
    fn adapter_type(&self) -> &str {
        "kimai"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        let client = self.auth.get_client().await.map_err(other_err)?;
        Ok(Box::new(KimaiRoot {
            client,
            connection_name: self.connection_name.clone(),
            lookback_days: self.lookback_days,
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        let client = self.auth.get_client().await.map_err(other_err)?;
        fetch_timesheet_node(&client, id).await
    }

    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.auth.subscribe_status()
    }

    async fn invalidate_session(&self) -> Result<()> {
        self.auth.invalidate_session().await;
        Ok(())
    }

    async fn invalidate_credentials(&self) -> Result<()> {
        self.auth.invalidate_credentials().await;
        Ok(())
    }
}

/// Fetch one timesheet plus the lookup tables and build a leaf node.
/// Only used for the rare `get_by_id`/`get_child` path — the listing
/// resolves names in bulk instead.
async fn fetch_timesheet_node(client: &Arc<KimaiClient>, id: &str) -> Result<Box<dyn Node>> {
    let numeric: u64 = id
        .parse()
        .map_err(|_| other_err(format!("invalid timesheet id: {id}")))?;
    let (ts, projects, activities) = tokio::try_join!(
        client.timesheet(numeric),
        client.projects(),
        client.activities()
    )
    .map_err(other_err)?;
    let projects: HashMap<u64, KimaiProject> =
        projects.into_iter().map(|p| (p.id, p)).collect();
    let activities: HashMap<u64, String> =
        activities.into_iter().map(|a| (a.id, a.name)).collect();
    let summary = timesheet_summary(ts, &projects, &activities);
    Ok(Box::new(KimaiTimesheetNode::from_summary(summary)))
}

struct KimaiRoot {
    client: Arc<KimaiClient>,
    connection_name: String,
    lookback_days: u32,
}

#[async_trait]
impl Node for KimaiRoot {
    fn id(&self) -> &str {
        "root"
    }

    fn label(&self) -> &str {
        &self.connection_name
    }

    fn node_type(&self) -> &NodeType {
        static ROOT_TYPE: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(|| NodeType {
            type_id: "kimai:root".into(),
            mime_type: "".into(),
            syntax: None,
            file_extension: "".into(),
            display_name: "Kimai Root".into(),
        });
        &ROOT_TYPE
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![timesheet_node_type()]
    }

    fn sortable_columns(&self, node_type: &NodeType) -> Vec<SortableColumn> {
        match node_type.type_id.as_str() {
            "kimai:timesheet" => timesheet_sortable_columns(),
            _ => Vec::new(),
        }
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        match params.node_type.type_id.as_str() {
            "kimai:timesheet" => self.list_timesheets(params).await,
            other => Err(ContentError::NotSupported(format!(
                "Unknown node type: {other}"
            ))),
        }
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        fetch_timesheet_node(&self.client, id).await
    }
}

impl KimaiRoot {
    /// One bulk listing: timesheets since the lookback boundary plus both
    /// lookup tables, fetched concurrently. Sorting is always local —
    /// Kimai's server order (begin desc) is just the input order.
    async fn list_timesheets(&self, params: ListParams) -> Result<ListResult> {
        let begin_local = Local::now()
            .checked_sub_days(Days::new(self.lookback_days as u64))
            .unwrap_or_else(Local::now)
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();

        let (timesheets, projects, activities) = tokio::try_join!(
            self.client.timesheets_since(&begin_local),
            self.client.projects(),
            self.client.activities()
        )
        .map_err(other_err)?;

        let projects: HashMap<u64, KimaiProject> =
            projects.into_iter().map(|p| (p.id, p)).collect();
        let activities: HashMap<u64, String> =
            activities.into_iter().map(|a| (a.id, a.name)).collect();

        let mut items: Vec<NodeSummary> = timesheets
            .into_iter()
            .map(|ts| timesheet_summary(ts, &projects, &activities))
            .collect();

        let applied_sort = apply_sort(&mut items, &params.sort, &timesheet_sortable_columns());

        Ok(ListResult {
            items,
            applied_sort,
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        })
    }
}

fn timesheet_sortable_columns() -> Vec<SortableColumn> {
    [
        ("project", "Project", SortKind::Text),
        ("customer", "Customer", SortKind::Text),
        ("activity", "Activity", SortKind::Text),
        ("duration", "Duration", SortKind::Number),
        ("begin", "Begin", SortKind::DateTime),
        ("end", "End", SortKind::DateTime),
    ]
    .into_iter()
    .map(|(key, label, kind)| SortableColumn {
        key: key.into(),
        label: label.into(),
        kind,
    })
    .collect()
}

/// Insert the missing colon into an ISO offset (`+0200` → `+02:00`) so the
/// value parses as RFC3339 — that's what `SortKind::DateTime` and the
/// engine's `group_by` bucketing expect. Values that already carry a
/// colon, end in `Z`, or have no offset at all pass through unchanged.
fn normalize_iso_offset(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 5 {
        let tail = &bytes[bytes.len() - 5..];
        if (tail[0] == b'+' || tail[0] == b'-') && tail[1..].iter().all(u8::is_ascii_digit) {
            let (head, offset) = value.split_at(value.len() - 4);
            return format!("{head}{}:{}", &offset[..2], &offset[2..]);
        }
    }
    value.to_string()
}

/// Map one API record to a list row. `project`/`activity` ids resolve via
/// the lookup maps; an id missing from its map (deleted or invisible
/// entity) renders as `#<id>`. `customer` is the project's `parentTitle`.
/// `duration` stays in raw seconds — the view formats it
/// (`kind: duration`) and the engine sums it per group.
fn timesheet_summary(
    ts: KimaiTimesheet,
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, String>,
) -> NodeSummary {
    let (project, customer) = projects
        .get(&ts.project)
        .map(|p| (p.name.clone(), p.parent_title.clone().unwrap_or_default()))
        .unwrap_or_else(|| (format!("#{}", ts.project), String::new()));
    let activity = activities
        .get(&ts.activity)
        .cloned()
        .unwrap_or_else(|| format!("#{}", ts.activity));

    let description = ts.description.unwrap_or_default();
    let label = description
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| activity.clone());

    let field = |key: &str, value: String, display: &str| MetadataField {
        key: key.into(),
        value,
        display_label: display.into(),
        editable: false,
        allowed_values: None,
    };

    let fields = vec![
        field("project", project, "Project"),
        field("customer", customer, "Customer"),
        field("activity", activity, "Activity"),
        field(
            "duration",
            ts.duration.unwrap_or(0).to_string(),
            "Duration",
        ),
        field("begin", normalize_iso_offset(&ts.begin), "Begin"),
        field(
            "end",
            ts.end.as_deref().map(normalize_iso_offset).unwrap_or_default(),
            "End",
        ),
        field("description", description, "Description"),
        field("tags", ts.tags.join(", "), "Tags"),
    ];

    NodeSummary {
        id: ts.id.to_string(),
        label,
        node_type: timesheet_node_type(),
        metadata: Metadata { fields },
        has_children: Some(false),
    }
}

/// Leaf node for a single timesheet (detail / `get_by_id` path). Carries
/// the same fields as the list row.
struct KimaiTimesheetNode {
    id: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
}

impl KimaiTimesheetNode {
    fn from_summary(summary: NodeSummary) -> Self {
        Self {
            id: summary.id,
            label: summary.label,
            node_type: summary.node_type,
            metadata: summary.metadata,
        }
    }
}

#[async_trait]
impl Node for KimaiTimesheetNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn node_type(&self) -> &NodeType {
        &self.node_type
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_offset_without_colon() {
        assert_eq!(
            normalize_iso_offset("2030-01-15T08:30:00+0200"),
            "2030-01-15T08:30:00+02:00"
        );
        assert_eq!(
            normalize_iso_offset("2030-01-15T08:30:00-0530"),
            "2030-01-15T08:30:00-05:30"
        );
    }

    #[test]
    fn leaves_normalized_values_alone() {
        assert_eq!(
            normalize_iso_offset("2030-01-15T08:30:00+02:00"),
            "2030-01-15T08:30:00+02:00"
        );
        assert_eq!(
            normalize_iso_offset("2030-01-15T08:30:00Z"),
            "2030-01-15T08:30:00Z"
        );
        assert_eq!(
            normalize_iso_offset("2030-01-15T08:30:00"),
            "2030-01-15T08:30:00"
        );
    }

    fn sample_timesheet() -> KimaiTimesheet {
        KimaiTimesheet {
            id: 4711,
            project: 7,
            activity: 3,
            begin: "2030-01-15T09:00:00+0100".into(),
            end: Some("2030-01-15T10:30:00+0100".into()),
            duration: Some(5400),
            description: Some("\n  \nRefactor login form\nsecond line".into()),
            tags: vec!["backend".into(), "sprint-9".into()],
        }
    }

    fn lookups() -> (HashMap<u64, KimaiProject>, HashMap<u64, String>) {
        let projects = HashMap::from([(
            7,
            KimaiProject {
                id: 7,
                name: "Website Relaunch".to_string(),
                parent_title: Some("Acme Corp".to_string()),
            },
        )]);
        let activities = HashMap::from([(3, "Development".to_string())]);
        (projects, activities)
    }

    #[test]
    fn maps_timesheet_row() {
        let (projects, activities) = lookups();
        let row = timesheet_summary(sample_timesheet(), &projects, &activities);

        assert_eq!(row.id, "4711");
        assert_eq!(row.label, "Refactor login form");
        assert_eq!(row.node_type.type_id, "kimai:timesheet");
        assert_eq!(row.has_children, Some(false));

        let get = |key: &str| {
            row.metadata
                .fields
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.value.clone())
        };
        assert_eq!(get("project").as_deref(), Some("Website Relaunch"));
        assert_eq!(get("customer").as_deref(), Some("Acme Corp"));
        assert_eq!(get("activity").as_deref(), Some("Development"));
        assert_eq!(get("duration").as_deref(), Some("5400"));
        assert_eq!(get("begin").as_deref(), Some("2030-01-15T09:00:00+01:00"));
        assert_eq!(get("end").as_deref(), Some("2030-01-15T10:30:00+01:00"));
        assert_eq!(get("tags").as_deref(), Some("backend, sprint-9"));
    }

    #[test]
    fn falls_back_for_unknown_ids_and_empty_description() {
        let ts = KimaiTimesheet {
            description: None,
            end: None,
            duration: None,
            ..sample_timesheet()
        };
        let row = timesheet_summary(ts, &HashMap::new(), &HashMap::new());

        let get = |key: &str| {
            row.metadata
                .fields
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.value.clone())
        };
        assert_eq!(get("project").as_deref(), Some("#7"));
        assert_eq!(get("customer").as_deref(), Some(""));
        assert_eq!(get("activity").as_deref(), Some("#3"));
        assert_eq!(get("duration").as_deref(), Some("0"));
        assert_eq!(get("end").as_deref(), Some(""));
        // Empty description → activity name (fallback chain) as the label.
        assert_eq!(row.label, "#3");
    }
}
