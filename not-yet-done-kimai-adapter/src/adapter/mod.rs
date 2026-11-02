//! Kimai ContentAdapter implementation.
//!
//! Flat: the root lists `kimai:timesheet` rows for the configured lookback
//! window; each row can be edited (`edit`, PATCH) and the root can create a
//! new entry (`create`, POST). Grouping (day/week/month) happens
//! engine-side via the view's `group_by` — the adapter only has to emit
//! RFC3339 `begin`/`end` values and a numeric `duration` (seconds), like
//! the local trackings adapter. Project and activity arrive as numeric
//! ids from `/api/timesheets` and are resolved to names via the
//! `/api/projects` + `/api/activities` lookups on every listing.
//!
//! Saved queries (`<data>/not_yet_done/kimai/<id>/queries/*.yaml`) narrow
//! the listing: server filters go to `/api/timesheets`, a `where:` clause
//! runs on the fetched rows — see [`query`].
//!
//! The listing leaves out own records on projects the user's teams no longer
//! reach; the timesheet index (`<data>/…/timesheet_index.json`) brings them
//! back — see [`hidden`]. Their project and activity names come from the
//! config's `names:`, since the lookups refuse them too.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Days, Local, NaiveDateTime};

use not_yet_done_content::*;

use crate::client::{KimaiActivity, KimaiClient, KimaiProject, KimaiTimesheet};

mod auth_bridge;
pub mod config;
mod factory;
mod hidden;
mod index;
pub mod query;
mod template;

pub use factory::KimaiAdapterFactory;

use auth_bridge::AuthBridge;
use config::KnownNames;
use hidden::Reach;
use index::IndexStore;
use query::{KimaiQuery, TimesheetRow};

/// File name of the timesheet index inside the instance's data directory.
const INDEX_FILE: &str = "timesheet_index.json";

/// How far before a scan's `from` the listing that bounds its ids reaches —
/// far enough to hold the last record listed before `from`.
const SCAN_LEAD_IN_DAYS: u64 = 31;

/// Action set for a timesheet row. Shared between [`Node::actions`] and
/// the adapter's instance-free [`ContentAdapter::actions_for_type`].
fn timesheet_actions() -> Vec<NodeAction> {
    vec![NodeAction::new("edit", "edit", InputSpec::Editor)]
}

/// Form fields for the root `create` action — a brand-new timesheet. `entry`
/// is the same `customer_project_activity` token the edit buffer/`entry_combos`
/// expose (the `#<pid>_#<aid>` escape works too); `duration` accepts `H:MM:SS`
/// or plain seconds and fixes `end = begin + duration`.
fn create_form_fields() -> Vec<FormFieldSpec> {
    vec![
        FormFieldSpec::text("entry", "Entry (customer_project_activity token)"),
        FormFieldSpec::text("begin", "Begin (YYYY-MM-DDTHH:MM[:SS])"),
        FormFieldSpec::text("duration", "Duration (H:MM:SS or seconds)"),
        FormFieldSpec::text("description", "Description").optional(),
    ]
}

/// Action set for the root: create a new timesheet, and scan for own
/// timesheets the listing leaves out. Shared between [`KimaiRoot::actions`]
/// and [`ContentAdapter::actions_for_type`].
fn root_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new(
            "create",
            "create",
            InputSpec::Form {
                fields: create_form_fields(),
            },
        ),
        NodeAction::new(
            "scan-hidden",
            "scan for hidden timesheets",
            InputSpec::Form {
                fields: vec![
                    FormFieldSpec::text(
                        "from",
                        "From (date or phrase like 'start of year'; default: the lookback window)",
                    )
                    .optional(),
                ],
            },
        ),
    ]
}

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
    /// What a listing needs besides the client.
    listing: ListingCtx,
    /// Saved queries, one `<name>.yaml` per query under the instance's data
    /// directory; the bodies are [`KimaiQuery`] documents.
    saved_queries: FsQueryStore,
}

/// The adapter state a listing (and the root's actions) works with.
#[derive(Clone)]
struct ListingCtx {
    lookback_days: u32,
    index: IndexStore,
    names: KnownNames,
}

impl ListingCtx {
    /// The listing's lower bound when the query names none.
    fn default_begin(&self) -> String {
        Local::now()
            .checked_sub_days(Days::new(self.lookback_days as u64))
            .unwrap_or_else(Local::now)
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string()
    }
}

impl KimaiAdapter {
    /// Build from a pre-built [`AuthBridge`], produced by the factory.
    pub(in crate::adapter) fn from_parts(
        auth: Arc<AuthBridge>,
        connection_name: String,
        instance_id: String,
        lookback_days: u32,
        names: KnownNames,
    ) -> Self {
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("kimai")
            .join(&instance_id);
        Self {
            auth,
            connection_name,
            instance_id,
            listing: ListingCtx {
                lookback_days,
                index: IndexStore::new(data_dir.join(INDEX_FILE)),
                names,
            },
            saved_queries: FsQueryStore::new(data_dir.join("queries"), ".yaml"),
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
            listing: self.listing.clone(),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        let client = self.auth.get_client().await.map_err(other_err)?;
        fetch_timesheet_node(&client, &self.listing.names, id).await
    }

    /// The single source of truth about what lives under a Kimai node: the
    /// root lists `kimai:timesheet` rows; a timesheet is a leaf. The `list`
    /// callback fetches lazily through the same [`list_timesheets`] the root's
    /// legacy `list` delegates to, so type set, sort columns, and fetcher can
    /// never disagree.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        match node.node_type().type_id.as_str() {
            "kimai:root" => vec![Child {
                node_type: timesheet_node_type(),
                columns: timesheet_columns(),
                list: Box::new(move |params| {
                    Box::pin(async move {
                        let client = self.auth.get_client().await.map_err(other_err)?;
                        list_timesheets(&client, &self.listing, params).await
                    })
                }),
            }],
            _ => Vec::new(),
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "kimai:timesheet" => timesheet_actions(),
            "kimai:root" => root_actions(),
            _ => Vec::new(),
        }
    }

    /// Reference data surfaced to any frontend (menu or CLI `values`).
    /// `entry_combos` yields every bookable project+activity slug — the same
    /// completion set edit mode offers — as `value` = the slug token,
    /// `label` = "Customer / Project / Activity", and `extra` carrying the
    /// `project` / `activity` clear names separately (so consumers can group
    /// by them). Used to build a local task-path → Kimai-slug mapping outside
    /// the editor.
    async fn list_values(&self, source: &str) -> Result<Vec<ValueOption>> {
        match source {
            "entry_combos" => {
                let client = self.auth.get_client().await.map_err(other_err)?;
                let (projects, activities) =
                    tokio::try_join!(client.projects(), client.activities()).map_err(other_err)?;
                let projects: HashMap<u64, KimaiProject> =
                    projects.into_iter().map(|p| (p.id, p)).collect();
                let activities: HashMap<u64, KimaiActivity> =
                    activities.into_iter().map(|a| (a.id, a)).collect();
                Ok(template::entry_slug_options(&projects, &activities)
                    .into_iter()
                    .map(|e| ValueOption {
                        value: e.token,
                        label: e.label,
                        extra: [
                            ("project".to_string(), e.project),
                            ("activity".to_string(), e.activity),
                        ]
                        .into_iter()
                        .collect(),
                    })
                    .collect())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        Some(&self.saved_queries)
    }

    /// Bodies may carry `${name:default}` placeholders wherever a value goes
    /// (`begin: ${from}`); they are filled in before the body is parsed.
    fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
        not_yet_done_content::query_vars::parse_variables(query)
    }

    fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
        not_yet_done_content::query_vars::render(query, vars)
    }

    /// One period, `begin` (or the lookback start) to `end` — the same bounds
    /// [`list_timesheets`] fetches with. `None` for a body that does not parse
    /// or whose `where:` narrows `begin`/`end` as well.
    fn query_periods(&self, query: &str) -> Option<Vec<QueryPeriod>> {
        let query = KimaiQuery::parse(query).ok()?;
        Some(vec![query.period(&self.listing.default_begin())?])
    }

    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.auth.subscribe_status()
    }

    async fn submit_credentials(
        &self,
        fields: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        self.auth
            .submit_credentials(fields)
            .await
            .map_err(|e| ContentError::Other(e.into()))
    }

    async fn cancel_credentials(&self) -> Result<()> {
        self.auth
            .cancel_credentials()
            .await
            .map_err(|e| ContentError::Other(e.into()))
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

/// The project and activity lookups by id, with the configured `names:`
/// filling in the ids the API refuses, plus the ids the API itself reaches.
struct Lookups {
    projects: HashMap<u64, KimaiProject>,
    activities: HashMap<u64, KimaiActivity>,
    reach: Reach,
}

async fn fetch_lookups(client: &KimaiClient, names: &KnownNames) -> Result<Lookups> {
    let (projects, activities) =
        tokio::try_join!(client.projects(), client.activities()).map_err(other_err)?;
    let mut projects: HashMap<u64, KimaiProject> =
        projects.into_iter().map(|p| (p.id, p)).collect();
    let mut activities: HashMap<u64, KimaiActivity> =
        activities.into_iter().map(|a| (a.id, a)).collect();
    let reach = Reach::new(projects.keys().copied(), activities.keys().copied());
    for p in &names.projects {
        projects.entry(p.id).or_insert_with(|| KimaiProject {
            id: p.id,
            name: p.name.clone(),
            parent_title: p.customer.clone(),
        });
    }
    for a in &names.activities {
        activities.entry(a.id).or_insert_with(|| KimaiActivity {
            id: a.id,
            name: a.name.clone(),
            project: None,
            parent_title: None,
        });
    }
    Ok(Lookups {
        projects,
        activities,
        reach,
    })
}

/// Fetch one timesheet plus the lookup tables and build a leaf node.
/// Only used for the rare `get_by_id`/`get_child` path — the listing
/// resolves names in bulk instead.
async fn fetch_timesheet_node(
    client: &Arc<KimaiClient>,
    names: &KnownNames,
    id: &str,
) -> Result<Box<dyn Node>> {
    let numeric: u64 = id
        .parse()
        .map_err(|_| other_err(format!("invalid timesheet id: {id}")))?;
    let (ts, Lookups {
        projects,
        activities,
        ..
    }) = tokio::try_join!(
        async { client.timesheet(numeric).await.map_err(other_err) },
        fetch_lookups(client, names)
    )?;
    Ok(Box::new(KimaiTimesheetNode::new(
        client.clone(),
        ts,
        projects,
        activities,
    )))
}

struct KimaiRoot {
    client: Arc<KimaiClient>,
    connection_name: String,
    listing: ListingCtx,
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

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        fetch_timesheet_node(&self.client, &self.listing.names, id).await
    }
    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
        _args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("create", ActionInput::Form(fields)) => self.execute_create(fields).await,
            ("scan-hidden", ActionInput::Form(fields)) => self.execute_scan_hidden(fields).await,
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unknown action {other}"
            ))),
        }
    }
}

impl KimaiRoot {
    /// Look for own timesheets the listing leaves out, from `from` on (a
    /// date or phrase; the lookback window when empty), and record them in
    /// the index — see [`hidden::scan`]. Takes minutes for months of ids:
    /// every id between the listed records is fetched on its own.
    async fn execute_scan_hidden(
        &self,
        fields: std::collections::HashMap<String, String>,
    ) -> Result<ActionOutcome> {
        let from = match fields.get("from").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(phrase) => KimaiQuery::parse(&format!("begin: {phrase}"))
                .map_err(other_err)?
                .begin
                .unwrap_or_else(|| self.listing.default_begin()),
            None => self.listing.default_begin(),
        };
        let lead_in = NaiveDateTime::parse_from_str(&from, "%Y-%m-%dT%H:%M:%S")
            .ok()
            .and_then(|t| t.checked_sub_days(Days::new(SCAN_LEAD_IN_DAYS)))
            .map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string())
            .unwrap_or_else(|| from.clone());

        let (listing, lookups) = tokio::try_join!(
            async {
                self.client
                    .timesheets_since(&lead_in, &[])
                    .await
                    .map_err(other_err)
            },
            fetch_lookups(&self.client, &self.listing.names)
        )?;
        let report = hidden::scan(
            &self.client,
            &self.listing.index,
            &lookups.reach,
            &from,
            listing,
        )
        .await
        .map_err(other_err)?;

        let secs = report.hidden_secs;
        let mut message = format!(
            "fetched {} ids one by one: {} own timesheets since {} are left out of the listing \
             ({}:{:02} h)",
            report.probed,
            report.hidden,
            &from[..10.min(from.len())],
            secs / 3600,
            secs % 3600 / 60
        );
        if report.failed > 0 {
            message.push_str(&format!(
                "; {} ids went unanswered — run the scan again to retry them",
                report.failed
            ));
        }
        Ok(ActionOutcome::Done {
            message: Some(message),
        })
    }

    /// Resolve the create form, POST a new timesheet, and navigate to it.
    /// Fetches the project/activity lookups (needed to resolve the `entry`
    /// token) then delegates to [`template::build_create_body`]; field errors
    /// surface as a single `ContentError` (a `Form` action has no editor to
    /// reopen). The created record's id drives the `Navigate` outcome.
    async fn execute_create(
        &self,
        fields: std::collections::HashMap<String, String>,
    ) -> Result<ActionOutcome> {
        let get = |key: &str| fields.get(key).map(String::as_str).unwrap_or("");

        let (projects, activities) =
            tokio::try_join!(self.client.projects(), self.client.activities())
                .map_err(other_err)?;
        let projects: HashMap<u64, KimaiProject> =
            projects.into_iter().map(|p| (p.id, p)).collect();
        let activities: HashMap<u64, KimaiActivity> =
            activities.into_iter().map(|a| (a.id, a)).collect();

        let body = template::build_create_body(
            get("entry"),
            get("begin"),
            get("duration"),
            get("description"),
            &projects,
            &activities,
        )
        .map_err(|errors| other_err(errors.join("; ")))?;

        let created = self
            .client
            .create_timesheet(&body)
            .await
            .map_err(other_err)?;

        Ok(ActionOutcome::Navigate {
            node_id: created.id.to_string(),
            node_type: timesheet_node_type(),
            message: Some(format!("timesheet {}: created", created.id)),
        })
    }
}

/// List the timesheet rows the pane's query selects — the single fetch source
/// behind both the root's `list` and the adapter's [`ContentAdapter::childs`]
/// declaration. Without a query, or one without `begin`, the window starts
/// `lookback_days` ago. Sorting is always local; Kimai's server order (begin
/// desc) is just the input order.
///
/// The listing goes into the timesheet index, and the index adds the own
/// records the listing leaves out (see [`hidden::complete_listing`]).
async fn list_timesheets(
    client: &Arc<KimaiClient>,
    ctx: &ListingCtx,
    params: ListParams,
) -> Result<ListResult> {
    let query = KimaiQuery::parse(params.query.as_deref().unwrap_or("")).map_err(other_err)?;
    let begin_local = query.begin.clone().unwrap_or_else(|| ctx.default_begin());

    // The names in the query resolve against the lookups, so they come first.
    let Lookups {
        projects,
        activities,
        reach,
    } = fetch_lookups(client, &ctx.names).await?;
    let customers = if query.needs_customers() {
        client.customers().await.map_err(other_err)?
    } else {
        Vec::new()
    };

    let filters = query
        .server_params(&customers, &projects, &activities)
        .map_err(other_err)?;
    let visible = match reachable_params(filters, &reach) {
        Some(filters) => client
            .timesheets_since(&begin_local, &filters)
            .await
            .map_err(other_err)?,
        None => Vec::new(),
    };
    let left_out = hidden::complete_listing(client, &ctx.index, &visible, &reach, |ts| {
        let names = entry_names(ts, &projects, &activities);
        let row = TimesheetRow {
            ts,
            project: &names.project,
            customer: &names.customer,
            activity: &names.activity,
        };
        query.passes_server_filters(&row, &begin_local, &projects, &activities)
    })
    .await
    .map_err(other_err)?;

    let mut items: Vec<NodeSummary> = visible
        .into_iter()
        .chain(left_out)
        .filter(|ts| {
            let names = entry_names(ts, &projects, &activities);
            query.keeps(&TimesheetRow {
                ts,
                project: &names.project,
                customer: &names.customer,
                activity: &names.activity,
            })
        })
        .map(|ts| timesheet_summary(ts, &projects, &activities))
        .collect();

    let applied_sort = apply_sort(&mut items, &params.sort, &timesheet_columns());

    Ok(ListResult {
        items,
        applied_sort,
        page: None,
        batch_download_available: false,
        downloaded: vec![],
    })
}

/// The server filters without the project and activity ids the API refuses
/// (asking for one is a 403) — `None` when a filter named only such ids, so
/// nothing the server could list would pass it.
fn reachable_params(params: Vec<(String, String)>, reach: &Reach) -> Option<Vec<(String, String)>> {
    let mut asked = (false, false);
    let mut kept = (false, false);
    let params: Vec<(String, String)> = params
        .into_iter()
        .filter(|(key, value)| {
            let id = value.parse().unwrap_or(0);
            match key.as_str() {
                "projects[]" => {
                    asked.0 = true;
                    let ok = reach.has_project(id);
                    kept.0 |= ok;
                    ok
                }
                "activities[]" => {
                    asked.1 = true;
                    let ok = reach.has_activity(id);
                    kept.1 |= ok;
                    ok
                }
                _ => true,
            }
        })
        .collect();
    (asked.0 == kept.0 && asked.1 == kept.1).then_some(params)
}

fn timesheet_columns() -> Vec<ColumnSchema> {
    [
        ("project", "Project", "text"),
        ("customer", "Customer", "text"),
        ("activity", "Activity", "text"),
        ("duration", "Duration", "number"),
        ("begin", "Begin", "datetime"),
        ("end", "End", "datetime"),
    ]
    .into_iter()
    .map(|(key, label, value_type)| ColumnSchema::new(key, label).typed(value_type))
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

/// The names a timesheet's ids stand for.
struct EntryNames {
    project: String,
    customer: String,
    activity: String,
}

/// Resolve a timesheet's project, customer and activity names; an id missing
/// from its map renders as `#<id>`, an unknown customer as empty.
fn entry_names(
    ts: &KimaiTimesheet,
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, KimaiActivity>,
) -> EntryNames {
    let (project, customer) = projects
        .get(&ts.project)
        .map(|p| (p.name.clone(), p.parent_title.clone().unwrap_or_default()))
        .unwrap_or_else(|| (format!("#{}", ts.project), String::new()));
    let activity = activities
        .get(&ts.activity)
        .map(|a| a.name.clone())
        .unwrap_or_else(|| format!("#{}", ts.activity));
    EntryNames {
        project,
        customer,
        activity,
    }
}

/// Map one API record to a list row. `project`/`activity` ids resolve via
/// the lookup maps; an id missing from its map (deleted or invisible
/// entity) renders as `#<id>`. `customer` is the project's `parentTitle`.
/// `duration` stays in raw seconds — the view formats it
/// (`kind: duration`) and the engine sums it per group.
fn timesheet_summary(
    ts: KimaiTimesheet,
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, KimaiActivity>,
) -> NodeSummary {
    let EntryNames {
        project,
        customer,
        activity,
    } = entry_names(&ts, projects, activities);

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
        field("duration", ts.duration.unwrap_or(0).to_string(), "Duration"),
        field("begin", normalize_iso_offset(&ts.begin), "Begin"),
        field(
            "end",
            ts.end
                .as_deref()
                .map(normalize_iso_offset)
                .unwrap_or_default(),
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
/// the same fields as the list row, plus the client, the raw API record
/// and the lookup tables so the `edit` action can render its template and
/// PATCH changes back.
struct KimaiTimesheetNode {
    id: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
    client: Arc<KimaiClient>,
    ts: KimaiTimesheet,
    projects: HashMap<u64, KimaiProject>,
    activities: HashMap<u64, KimaiActivity>,
}

impl KimaiTimesheetNode {
    fn new(
        client: Arc<KimaiClient>,
        ts: KimaiTimesheet,
        projects: HashMap<u64, KimaiProject>,
        activities: HashMap<u64, KimaiActivity>,
    ) -> Self {
        let summary = timesheet_summary(ts.clone(), &projects, &activities);
        Self {
            id: summary.id,
            label: summary.label,
            node_type: summary.node_type,
            metadata: summary.metadata,
            client,
            ts,
            projects,
            activities,
        }
    }

    /// Parse the edited buffer, diff against the fresh upstream record and
    /// PATCH the changed fields. Parse/validation problems reopen the
    /// editor with an error banner; an upstream change while editing
    /// reopens with a conflict banner and a renewed version token.
    async fn execute_edit(&self, text: &str, version: &str) -> Result<ActionOutcome> {
        let parsed = match template::parse_edit(text) {
            Ok(parsed) => parsed,
            Err(errors) => {
                return Ok(ActionOutcome::Reopen {
                    content: template::render_with_errors(text, &errors),
                    new_version: None,
                });
            }
        };

        let fresh = self.client.timesheet(self.ts.id).await.map_err(other_err)?;
        let fresh_version = template::version_token(&fresh);
        if fresh_version != version {
            return Ok(ActionOutcome::Reopen {
                content: template::render_with_conflict(text),
                new_version: Some(fresh_version),
            });
        }

        let plan =
            match template::build_edit_plan(&parsed, &fresh, &self.projects, &self.activities) {
                Ok(Some(plan)) => plan,
                Ok(None) => return Ok(ActionOutcome::NoChanges),
                Err(errors) => {
                    return Ok(ActionOutcome::Reopen {
                        content: template::render_with_errors(text, &errors),
                        new_version: None,
                    });
                }
            };

        self.client
            .patch_timesheet(self.ts.id, &plan.patch)
            .await
            .map_err(other_err)?;

        Ok(ActionOutcome::Done {
            message: Some(format!(
                "timesheet {}: updated {}",
                self.ts.id,
                plan.changed.join(", ")
            )),
        })
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
    async fn prepare(&self, action_id: &str, _args: &ActionArgs) -> Result<EditorPrep> {
        match action_id {
            "edit" => Ok(EditorPrep {
                cursor_line: None,
                template: template::render_edit_template(
                    &self.ts,
                    &self.projects,
                    &self.activities,
                ),
                version: template::version_token(&self.ts),
                suffix: ".md".into(),
                file_path: None,
                args: Default::default(),
            }),
            other => Err(ContentError::NotSupported(format!(
                "prepare: unknown action {other}"
            ))),
        }
    }

    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
        _args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("edit", ActionInput::Edited { text, version, .. }) => {
                self.execute_edit(&text, &version).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unknown action {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_create_action_is_a_four_field_form() {
        let actions = root_actions();
        let ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["create", "scan-hidden"]);
        let create = &actions[0];
        assert_eq!(create.id, "create");
        match &create.input {
            InputSpec::Form { fields } => {
                let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
                assert_eq!(keys, vec!["entry", "begin", "duration", "description"]);
                // description is the only optional field.
                let desc = fields.iter().find(|f| f.key == "description").unwrap();
                assert!(!desc.required);
                assert!(fields.iter().filter(|f| f.required).count() == 3);
            }
            other => panic!("expected a Form input, got {other:?}"),
        }
    }

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
            ..Default::default()
        }
    }

    fn lookups() -> (HashMap<u64, KimaiProject>, HashMap<u64, KimaiActivity>) {
        let projects = HashMap::from([(
            7,
            KimaiProject {
                id: 7,
                name: "Website Relaunch".to_string(),
                parent_title: Some("Acme Corp".to_string()),
            },
        )]);
        let activities = HashMap::from([(
            3,
            KimaiActivity {
                id: 3,
                name: "Development".to_string(),
                project: None,
                parent_title: None,
            },
        )]);
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

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn refused_ids_leave_the_server_filters() {
        let reach = Reach::new([7, 8], [3]);
        assert_eq!(
            reachable_params(
                pairs(&[("projects[]", "7"), ("projects[]", "4711"), ("term", "x")]),
                &reach
            ),
            Some(pairs(&[("projects[]", "7"), ("term", "x")]))
        );
        // Only refused ids asked for: the server could list nothing that passes.
        assert_eq!(
            reachable_params(pairs(&[("activities[]", "42")]), &reach),
            None
        );
        assert_eq!(
            reachable_params(pairs(&[("begin", "x")]), &reach),
            Some(pairs(&[("begin", "x")]))
        );
    }
}
