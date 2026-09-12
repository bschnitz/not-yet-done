//! The `systemd` [`ContentAdapter`] — one manager, browsed as three tables.
//!
//! # Node hierarchy
//!
//! ```text
//! root → service
//!      → timer
//!      → unitfile
//! ```
//!
//! * **root** (`systemd:manager`, id `root`) — the manager itself. It carries
//!   no cells; it exists so the three levels have somewhere to hang.
//! * **service** (`systemd:service`, id `service:<name>`) — a `.service` unit
//!   the manager has **loaded**, with its runtime state (active/sub, main PID,
//!   memory, CPU, restarts).
//! * **timer** (`systemd:timer`, id `timer:<name>`) — a loaded `.timer` unit
//!   with its schedule (next/last elapse, the unit it triggers).
//! * **unitfile** (`systemd:unitfile`, id `unitfile:<name>`) — a unit file on
//!   disk, loaded or not. This is the level that can see a unit the manager has
//!   never touched; the other two cannot, by construction.
//!
//! The levels are siblings rather than a hierarchy on purpose: a service is not
//! *inside* a timer, and a unit file is not the parent of the unit it describes
//! — they are three questions about the same manager, and each view asks one.
//!
//! # What a level shows
//!
//! Everything the manager reports, filtered by the level's [`query`](crate::query).
//! There is no configured include/exclude: which units are interesting changes
//! by the minute ("what is failing", "what did I write myself"), and that is a
//! query the user switches at runtime, not a setting they edit and restart.
//! See decision D2 in `docs/plan-systemd-adapter.md`.
//!
//! # Phase 0 is read-only
//!
//! No actions, no writes. `start`/`stop`/`enable`, editing a unit file and the
//! journal are later phases that build on exactly these rows.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, StreamExt};

use not_yet_done_content::*;

use crate::bus::{Bus, Props, SERVICE_IFACE, TIMER_IFACE, UNIT_IFACE, UnitEntry};
use crate::config::SystemdConfig;
use crate::model::{
    ServiceRow, TimerRow, UnitFileRow, boot_instant, manager_type, service_columns, service_type,
    timer_columns, timer_type, unit_file_columns, unit_file_type,
};
use crate::query::{self, UnitQuery};

/// Id of the adapter root, addressable via [`ContentAdapter::get_by_id`].
const ROOT_ID: &str = "root";

/// A systemd manager as a content tree.
pub struct SystemdAdapter {
    instance_id: String,
    /// Shared with every in-flight list: the levels borrow it, the node the
    /// root hands out does not need it.
    bus: Arc<Bus>,
    /// The deadline a single call runs under, for the busy line. `0` = none,
    /// which is also what the status channel means by "no deadline".
    timeout_secs: u64,
    status: StatusReporter,
    saved_queries: FsQueryStore,
}

impl SystemdAdapter {
    pub fn new(instance_id: String, cfg: &SystemdConfig) -> Self {
        let manager = cfg.manager();
        let bus = Bus::new(manager, cfg.timeout());
        let queries_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("systemd")
            .join(&instance_id)
            .join("queries");
        Self {
            instance_id,
            bus: Arc::new(bus),
            timeout_secs: cfg.timeout().map(|d| d.as_secs()).unwrap_or(0),
            // The connection is opened by the first load, not here — so the
            // reporter starts out ready and the first level is what reports
            // a manager that is not running.
            status: StatusReporter::new(),
            saved_queries: FsQueryStore::new(queries_root, ".yaml"),
        }
    }

    fn root_node(&self) -> SystemdRoot {
        SystemdRoot {
            label: format!("systemd ({})", self.bus.manager().as_str()),
        }
    }
}

#[async_trait]
impl ContentAdapter for SystemdAdapter {
    fn adapter_type(&self) -> &str {
        "systemd"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(self.root_node()))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        if id == ROOT_ID {
            return Ok(Box::new(self.root_node()));
        }
        if let Some(name) = id.strip_prefix(crate::SERVICE_PREFIX) {
            let entry = self.find_unit(name).await?;
            let unit = self.bus.properties(&entry.path, UNIT_IFACE).await;
            let service = self.bus.properties(&entry.path, SERVICE_IFACE).await;
            let row = ServiceRow::build(&entry, &unit, &service);
            return Ok(Box::new(UnitNode::from(row.summary())));
        }
        if let Some(name) = id.strip_prefix(crate::TIMER_PREFIX) {
            let entry = self.find_unit(name).await?;
            let unit = self.bus.properties(&entry.path, UNIT_IFACE).await;
            let timer = self.bus.properties(&entry.path, TIMER_IFACE).await;
            let row = TimerRow::build(&entry, &unit, &timer, boot_instant());
            return Ok(Box::new(UnitNode::from(row.summary(Utc::now()))));
        }
        if let Some(name) = id.strip_prefix(crate::UNIT_FILE_PREFIX) {
            let entry = self
                .bus
                .list_unit_files()
                .await?
                .into_iter()
                .find(|e| e.path.rsplit('/').next().unwrap_or(&e.path) == name)
                .ok_or_else(|| ContentError::NotFound(format!("no unit file {name}")))?;
            return Ok(Box::new(UnitNode::from(UnitFileRow::build(&entry).summary())));
        }
        Err(ContentError::NotFound(format!("unknown systemd id {id}")))
    }

    /// The single source of truth about what lives under a systemd node: the
    /// root lists the three unit levels, each row is a leaf. Every `list`
    /// closure borrows the adapter's own bus, so a level fetches lazily and
    /// nothing has to be cloned into the node the root handed out.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        if node.node_type().type_id != manager_type().type_id {
            return Vec::new();
        }
        vec![
            Child {
                node_type: service_type(),
                columns: service_columns(),
                list: Box::new(move |params| Box::pin(self.list_services(params))),
            },
            Child {
                node_type: timer_type(),
                columns: timer_columns(),
                list: Box::new(move |params| Box::pin(self.list_timers(params))),
            },
            Child {
                node_type: unit_file_type(),
                columns: unit_file_columns(),
                list: Box::new(move |params| Box::pin(self.list_unit_files(params))),
            },
        ]
    }

    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.status.subscribe()
    }

    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        Some(&self.saved_queries)
    }
}

impl SystemdAdapter {
    /// The loaded unit by name — the lookup both `get_by_id` arms share.
    ///
    /// `ListUnits` reports only what the manager has *loaded*, which is exactly
    /// the population the service and timer levels show, so a name that is not
    /// in it is genuinely not addressable as a unit (it may still exist as a
    /// unit file, which is a different level with a different id prefix).
    async fn find_unit(&self, name: &str) -> Result<UnitEntry> {
        self.bus
            .list_units()
            .await?
            .into_iter()
            .find(|u| u.name == name)
            .ok_or_else(|| ContentError::NotFound(format!("no loaded unit {name}")))
    }

    async fn list_services(&self, params: ListParams) -> Result<ListResult> {
        let query = compile(params.query.as_deref(), query::SERVICE_COLUMNS)?;
        let _busy = self.status.busy("Reading services", self.timeout_secs);
        let units = self.units_with_suffix(".service").await?;
        let rows = self
            .with_properties(units, SERVICE_IFACE, |entry, unit, own| {
                ServiceRow::build(entry, unit, own)
            })
            .await;
        Ok(finish(
            query::retain(rows, &query)
                .iter()
                .map(ServiceRow::summary)
                .collect(),
            &params.sort,
            &service_columns(),
        ))
    }

    async fn list_timers(&self, params: ListParams) -> Result<ListResult> {
        let query = compile(params.query.as_deref(), query::TIMER_COLUMNS)?;
        let _busy = self.status.busy("Reading timers", self.timeout_secs);
        let units = self.units_with_suffix(".timer").await?;
        // Read once for the whole level: a monotonic timer's next elapse is an
        // offset from this boot, and re-deriving it per row would let rows
        // disagree about when the machine started.
        let boot = boot_instant();
        let rows = self
            .with_properties(units, TIMER_IFACE, move |entry, unit, own| {
                TimerRow::build(entry, unit, own, boot)
            })
            .await;
        let now = Utc::now();
        Ok(finish(
            query::retain(rows, &query)
                .iter()
                .map(|r| r.summary(now))
                .collect(),
            &params.sort,
            &timer_columns(),
        ))
    }

    async fn list_unit_files(&self, params: ListParams) -> Result<ListResult> {
        let query = compile(params.query.as_deref(), query::UNIT_FILE_COLUMNS)?;
        let _busy = self.status.busy("Reading unit files", self.timeout_secs);
        let rows: Vec<UnitFileRow> = self
            .bus
            .list_unit_files()
            .await?
            .iter()
            .map(UnitFileRow::build)
            .collect();
        Ok(finish(
            query::retain(rows, &query)
                .iter()
                .map(UnitFileRow::summary)
                .collect(),
            &params.sort,
            &unit_file_columns(),
        ))
    }

    /// The loaded units whose name ends in `suffix` — how a level picks its
    /// own population out of the one listing the manager offers.
    async fn units_with_suffix(&self, suffix: &str) -> Result<Vec<UnitEntry>> {
        let units = self.bus.list_units().await?;
        self.status.connected();
        Ok(units
            .into_iter()
            .filter(|u| u.name.ends_with(suffix))
            .collect())
    }

    /// Read each unit's generic and type-specific properties and fold both into
    /// a row.
    ///
    /// A unit's cells live behind two `GetAll` round trips, and doing a few
    /// hundred of those one after another is what makes a level feel slow. They
    /// go out concurrently, bounded by [`bus::MAX_INFLIGHT`](crate::bus::MAX_INFLIGHT)
    /// so the number of open futures stays a property of this code rather than
    /// of how many units the machine happens to have.
    async fn with_properties<R, F>(&self, units: Vec<UnitEntry>, iface: &str, build: F) -> Vec<R>
    where
        F: Fn(&UnitEntry, &Props, &Props) -> R + Copy + Send + Sync,
        R: Send,
    {
        stream::iter(units)
            .map(|entry| async move {
                let unit = self.bus.properties(&entry.path, UNIT_IFACE).await;
                let own = self.bus.properties(&entry.path, iface).await;
                build(&entry, &unit, &own)
            })
            .buffer_unordered(crate::bus::MAX_INFLIGHT)
            .collect()
            .await
    }
}

/// Compile the pane's query against the columns of *this* level.
///
/// Each level validates against its own column set, so a query written for
/// services (`restarts > 3`) is rejected on timers instead of silently matching
/// nothing — a typo and an inapplicable field should not look the same.
fn compile(raw: Option<&str>, columns: &[&str]) -> Result<Option<UnitQuery>> {
    UnitQuery::compile(raw, columns).map_err(|e| ContentError::Other(e.into()))
}

/// Sort the rows and wrap them as a level's result.
///
/// Without a requested sort the rows arrive in whatever order the concurrent
/// property reads finished, which would reshuffle the table on every load; by
/// name is the order the unit lists have everywhere else.
fn finish(
    mut items: Vec<NodeSummary>,
    sort: &[SortKey],
    columns: &[ColumnSchema],
) -> ListResult {
    let requested: Vec<SortKey> = if sort.is_empty() {
        vec![SortKey {
            column: "name".into(),
            direction: SortDirection::Asc,
        }]
    } else {
        sort.to_vec()
    };
    let applied_sort = apply_sort(&mut items, &requested, columns);
    ListResult {
        items,
        applied_sort,
        page: None,
        batch_download_available: false,
        downloaded: vec![],
    }
}

/// The manager node: the root every level hangs off.
struct SystemdRoot {
    label: String,
}

#[async_trait]
impl Node for SystemdRoot {
    fn id(&self) -> &str {
        ROOT_ID
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn node_type(&self) -> &NodeType {
        static ROOT_TYPE: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(manager_type);
        &ROOT_TYPE
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }
}

/// A single unit row addressed on its own — the same cells the level showed.
///
/// One type for all three levels: what distinguishes a service from a timer is
/// its `node_type` and its cells, both of which the summary already carries, so
/// three near-identical structs would only be three places to forget an edit.
struct UnitNode {
    id: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
}

impl From<NodeSummary> for UnitNode {
    fn from(s: NodeSummary) -> Self {
        Self {
            id: s.id,
            label: s.label,
            node_type: s.node_type,
            metadata: s.metadata,
        }
    }
}

#[async_trait]
impl Node for UnitNode {
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

    fn adapter() -> SystemdAdapter {
        SystemdAdapter::new("systemd".into(), &SystemdConfig::default())
    }

    #[test]
    fn the_root_offers_exactly_the_three_unit_levels() {
        let a = adapter();
        let root = a.root_node();
        let types: Vec<String> = a
            .childs(&root)
            .into_iter()
            .map(|c| c.node_type.type_id)
            .collect();
        assert_eq!(
            types,
            vec![
                "systemd:service".to_string(),
                "systemd:timer".to_string(),
                "systemd:unitfile".to_string()
            ]
        );
    }

    #[test]
    fn a_row_is_a_leaf() {
        let a = adapter();
        let node = UnitNode::from(ServiceRow::default().summary());
        assert!(a.childs(&node).is_empty());
    }

    #[test]
    fn an_unknown_column_is_rejected_by_the_level_that_lacks_it() {
        // `restarts` is a service column; on timers it must not silently
        // match nothing.
        let q = "query:\n  [restarts, gt, 3]";
        assert!(compile(Some(q), query::SERVICE_COLUMNS).is_ok());
        assert!(compile(Some(q), query::TIMER_COLUMNS).is_err());
    }

    #[test]
    fn rows_without_a_requested_sort_come_back_by_name() {
        let mut rows = vec![
            ServiceRow {
                name: "zeta.service".into(),
                ..Default::default()
            },
            ServiceRow {
                name: "alpha.service".into(),
                ..Default::default()
            },
        ];
        rows.sort_by(|_, _| std::cmp::Ordering::Equal);
        let result = finish(
            rows.iter().map(ServiceRow::summary).collect(),
            &[],
            &service_columns(),
        );
        let names: Vec<&str> = result.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(names, vec!["alpha.service", "zeta.service"]);
    }
}
