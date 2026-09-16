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
//! Under a service and a timer hang three further levels:
//!
//! * **property** (`systemd:property`, id `property:<unit>:<Name>`) — one row
//!   per property the manager reports, from the generic `Unit` interface and
//!   from the unit's own. Not under **unitfile**: a unit the manager has not
//!   loaded has no D-Bus object, and therefore no properties to read.
//! * **dep** (`systemd:dep`, id `dep:<unit>><unit>>…`) — what the unit needs,
//!   as a tree: the level hangs under itself, so a row unfolds into its own
//!   dependencies. The one level of this adapter that is a tree rather than a
//!   table, which is why the unit levels above it carry a `tree_label`.
//! * **order** (`systemd:order`, id `order:<unit>><unit>`) — what the unit is
//!   ordered against, flat and one hop deep.
//!
//! Both dependency levels are [`crate::deps`], which explains why the first is
//! recursive and the second is not.
//!
//! # Acting on a unit
//!
//! The verbs live in [`control`](crate::control), which owns the table; this
//! file only routes. A shortcut reaches [`Node::invoke_action`], which checks
//! the protection list, asks for confirmation where the verb declares one, and
//! then runs it. `kill` is the exception: it needs a signal, so it is an
//! `InputSpec::Picker` and travels the `picker_options` → `execute` road
//! instead.

use std::sync::Arc;

use tokio::sync::{OnceCell, broadcast};

use async_trait::async_trait;
use futures::stream::{self, StreamExt};

use not_yet_done_content::*;

use std::collections::HashMap;

use crate::chain::{ChainRow, chain_columns, chain_type};
use crate::bus::{Bus, JOB_WAIT_SECS, Props, SERVICE_IFACE, TIMER_IFACE, UNIT_IFACE, UnitEntry};
use crate::config::SystemdConfig;
use crate::control;
use crate::create;
use crate::deps::{self, Axis, DepRow};
use crate::edit;
use crate::journal;
use crate::live;
use crate::model::{
    LogRow, PropertyRow, SecurityRow, ServiceRow, TimerRow, UnitFileRow, boot_instant, log_columns,
    log_type, manager_type, property_columns, property_rows, property_type, security_columns,
    security_type, service_columns, service_type, timer_columns, timer_type, unit_file_columns,
    unit_file_type,
};
use crate::protect::Protection;
use crate::query::{self, UnitQuery};
use crate::security;

/// Id of the adapter root, addressable via [`ContentAdapter::get_by_id`].
const ROOT_ID: &str = "root";

/// Everything a node needs in order to *do* something, in one handle.
///
/// A row reached through `get_by_id` is handed one of these, which is the whole
/// reason it exists: a [`UnitNode`] built only from its summary knows its name
/// and its cells but has no way to reach the manager, and a verb it cannot
/// execute is a verb that should not be offered. The three parts are the three
/// questions every verb asks — *whom do I talk to*, *am I allowed*, and *who
/// says a call is in flight*.
struct Shared {
    bus: Arc<Bus>,
    status: StatusReporter,
    protect: Protection,
    /// The deadline a single call runs under, for the busy line. `0` = none,
    /// which is also what the status channel means by "no deadline".
    timeout_secs: u64,
    /// The command line the journal-follow key opens — see
    /// [`SystemdConfig::terminal`].
    terminal: String,
    /// Out-of-band content changes, fanned out to every view bound to this
    /// instance. Held as the sender so the channel survives having no
    /// subscriber; each `subscribe_invalidations` call takes a fresh receiver.
    inv_tx: broadcast::Sender<Invalidation>,
    /// Latches once [`crate::live`] is running — see
    /// [`SystemdAdapter::ensure_watcher`].
    watcher: OnceCell<()>,
}

/// A systemd manager as a content tree.
pub struct SystemdAdapter {
    instance_id: String,
    /// Shared with every in-flight list and with every node that acts.
    shared: Arc<Shared>,
    saved_queries: FsQueryStore,
}

impl SystemdAdapter {
    pub fn new(instance_id: String, cfg: &SystemdConfig) -> Self {
        let manager = cfg.manager();
        let bus = Bus::new(manager, cfg.timeout(), cfg.auth_timeout());
        let queries_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("systemd")
            .join(&instance_id)
            .join("queries");
        Self {
            instance_id,
            shared: Arc::new(Shared {
                bus: Arc::new(bus),
                // The connection is opened by the first load, not here — so the
                // reporter starts out ready and the first level is what reports
                // a manager that is not running.
                status: StatusReporter::new(),
                protect: cfg.protection(),
                timeout_secs: cfg.timeout().map(|d| d.as_secs()).unwrap_or(0),
                terminal: cfg.terminal(),
                // 64, like the other pushing adapters: a burst is a handful of
                // rows, and a subscriber that falls this far behind has a
                // bigger problem than a dropped repaint.
                inv_tx: broadcast::channel(64).0,
                watcher: OnceCell::new(),
            }),
            saved_queries: FsQueryStore::new(queries_root, ".yaml"),
        }
    }

    fn bus(&self) -> &Bus {
        &self.shared.bus
    }

    fn status(&self) -> &StatusReporter {
        &self.shared.status
    }

    fn root_node(&self) -> SystemdRoot {
        SystemdRoot {
            label: format!("systemd ({})", self.bus().manager().as_str()),
            shared: Arc::clone(&self.shared),
        }
    }

    /// A row, carrying the handle it needs to act on itself.
    fn node(&self, summary: NodeSummary) -> UnitNode {
        UnitNode::new(summary, Arc::clone(&self.shared))
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
            let unit = self.bus().properties(&entry.path, UNIT_IFACE).await;
            let service = self.bus().properties(&entry.path, SERVICE_IFACE).await;
            let row = ServiceRow::build(&entry, &unit, &service);
            return Ok(Box::new(self.node(row.summary())));
        }
        if let Some(name) = id.strip_prefix(crate::TIMER_PREFIX) {
            let entry = self.find_unit(name).await?;
            let unit = self.bus().properties(&entry.path, UNIT_IFACE).await;
            let timer = self.bus().properties(&entry.path, TIMER_IFACE).await;
            let row = TimerRow::build(&entry, &unit, &timer, boot_instant());
            return Ok(Box::new(self.node(row.summary())));
        }
        if let Some(name) = id.strip_prefix(crate::UNIT_FILE_PREFIX) {
            let entry = self
                .bus()
                .list_unit_files()
                .await?
                .into_iter()
                .find(|e| e.path.rsplit('/').next().unwrap_or(&e.path) == name)
                .ok_or_else(|| ContentError::NotFound(format!("no unit file {name}")))?;
            let policy = crate::preset::Policy::load_for(self.bus().manager());
            let paths = crate::shadow::SearchPath::load_for(self.bus().manager()).await;
            let row = UnitFileRow::build(&entry, &policy, &paths);
            return Ok(Box::new(self.node(row.summary())));
        }
        if let Some(rest) = id.strip_prefix(crate::LOG_PREFIX) {
            // A cursor never holds a colon, a unit name may — so split from the
            // right, the same way a property id is split.
            let (unit, cursor) = rest
                .rsplit_once(':')
                .ok_or_else(|| ContentError::NotFound(format!("malformed journal id {id}")))?;
            let row = journal::entry(self.bus().manager(), unit, cursor).await?;
            return Ok(Box::new(self.node(row.summary())));
        }
        if let Some((unit, field)) = security::parse_id(id) {
            let row = security::check(self.bus().manager(), unit, field).await?;
            return Ok(Box::new(self.node(row.summary())));
        }
        if let Some((root, unit)) = crate::chain::parse(id) {
            let row = crate::chain::level(self.bus(), root)
                .await?
                .into_iter()
                .find(|r| r.unit == unit)
                .ok_or_else(|| {
                    ContentError::NotFound(format!("{unit} is not on the critical chain of {root}"))
                })?;
            return Ok(Box::new(self.node(row.summary())));
        }
        if deps::parse(id).is_some() {
            let row = deps::row_by_id(self.bus(), id).await?;
            return Ok(Box::new(self.node(row.summary())));
        }
        if let Some(rest) = id.strip_prefix(crate::PROPERTY_PREFIX) {
            // A unit name may hold dots but never a colon, so the *last* colon
            // is the one that separates the unit from the property name.
            let (unit, prop) = rest
                .rsplit_once(':')
                .ok_or_else(|| ContentError::NotFound(format!("malformed property id {id}")))?;
            let row = self
                .read_properties(unit)
                .await?
                .into_iter()
                .find(|r| r.name == prop)
                .ok_or_else(|| ContentError::NotFound(format!("{unit} has no property {prop}")))?;
            return Ok(Box::new(self.node(row.summary())));
        }
        Err(ContentError::NotFound(format!("unknown systemd id {id}")))
    }

    /// The single source of truth about what lives under a systemd node: the
    /// root lists the three unit levels, each row is a leaf. Every `list`
    /// closure borrows the adapter's own bus, so a level fetches lazily and
    /// nothing has to be cloned into the node the root handed out.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        let type_id = node.node_type().type_id.as_str();
        // A loaded unit has a D-Bus object, so it can be asked what it is; a
        // unit file has none, which is why the third level stays a leaf.
        // The journal hangs off every unit level, including unit files: reading
        // a unit's log needs no D-Bus object, only a name, so a unit the
        // manager has never loaded still has a journal from the last time it
        // ran. Properties are the opposite and stay where the object is.
        // The security level hangs off both for the same reason as the
        // journal: `systemd-analyze security` reads files, not bus objects, so
        // a unit the manager never loaded still answers. Under a timer it
        // answers about the service the timer triggers — see
        // [`SystemdAdapter::triggered_unit`].
        // The critical chain hangs off the loaded units only, and not off a
        // unit file: a chain is a fact about a startup that happened, and a
        // unit the manager has never loaded had none.
        // A dependency row hangs under itself: that is what makes the needs
        // level a tree. The ordering level does not — see [`crate::deps`].
        if type_id == deps::dep_type().type_id {
            let id = node.id();
            return vec![Child {
                node_type: deps::dep_type(),
                columns: deps::dep_columns(),
                list: Box::new(move |params| Box::pin(self.list_deps(id, Axis::Needs, params))),
            }];
        }
        if type_id == service_type().type_id || type_id == timer_type().type_id {
            let id = node.id();
            return vec![
                Child {
                    node_type: deps::dep_type(),
                    columns: deps::dep_columns(),
                    list: Box::new(move |params| Box::pin(self.list_deps(id, Axis::Needs, params))),
                },
                Child {
                    node_type: deps::order_type(),
                    columns: deps::dep_columns(),
                    list: Box::new(move |params| Box::pin(self.list_deps(id, Axis::Order, params))),
                },
                Child {
                    node_type: property_type(),
                    columns: property_columns(),
                    list: Box::new(move |params| Box::pin(self.list_properties(id, params))),
                },
                Child {
                    node_type: log_type(),
                    columns: log_columns(),
                    list: Box::new(move |params| Box::pin(self.list_journal(id, params))),
                },
                Child {
                    node_type: security_type(),
                    columns: security_columns(),
                    list: Box::new(move |params| Box::pin(self.list_security(id, params))),
                },
                Child {
                    node_type: chain_type(),
                    columns: chain_columns(),
                    list: Box::new(move |params| Box::pin(self.list_chain(id, params))),
                },
            ];
        }
        if type_id == unit_file_type().type_id {
            let id = node.id();
            return vec![
                Child {
                    node_type: log_type(),
                    columns: log_columns(),
                    list: Box::new(move |params| Box::pin(self.list_journal(id, params))),
                },
                Child {
                    node_type: security_type(),
                    columns: security_columns(),
                    list: Box::new(move |params| Box::pin(self.list_security(id, params))),
                },
            ];
        }
        if type_id != manager_type().type_id {
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
        self.status().subscribe()
    }

    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        Some(&self.saved_queries)
    }

    /// What each level may do: the verb table, then the editing actions, then
    /// what the manager itself can make.
    ///
    /// A level is offered a verb when the verb says it works there — a timer
    /// has no processes and so is never offered `kill`, a unit file is not
    /// loaded and so is never offered `reset-failed`. The editing actions ask
    /// a second question on top of the level: they write to
    /// `~/.config/systemd/user`, so against the **system** manager there are
    /// none of them (see [`crate::edit::actions_for`]).
    ///
    /// Creating hangs off the **manager** type rather than off a row, because
    /// a unit that does not exist yet has no row to hang off
    /// (see [`crate::create::actions_for`]).
    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        let manager = self.shared.bus.manager();
        let mut actions = control::actions_for(&node_type.type_id, manager);
        actions.extend(edit::actions_for(&node_type.type_id, manager));
        actions.extend(create::actions_for(&node_type.type_id, manager));
        actions.extend(journal::actions_for(&node_type.type_id));
        actions.extend(security::actions_for(&node_type.type_id, manager));
        actions
    }

    /// The stream the live watcher pushes into — see [`crate::live`] for what
    /// it carries and why. The channel exists from construction even though
    /// nothing writes to it until a level has been loaded, so a view can
    /// subscribe before the first load without racing the watcher's start.
    fn subscribe_invalidations(&self) -> broadcast::Receiver<Invalidation> {
        self.shared.inv_tx.subscribe()
    }

    /// The one named value list this adapter serves: the signals `kill` sends.
    async fn list_values(&self, source: &str) -> Result<Vec<ValueOption>> {
        match source {
            "signals" => Ok(control::signal_options()),
            other => Err(ContentError::NotFound(format!("no value list {other}"))),
        }
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
        self.bus()
            .list_units()
            .await?
            .into_iter()
            .find(|u| u.name == name)
            .ok_or_else(|| ContentError::NotFound(format!("no loaded unit {name}")))
    }

    /// Start the live watcher, once, the first time a level that has live rows
    /// is loaded.
    ///
    /// Not in `new()`: constructing an adapter must not open a connection (a
    /// `--user` manager may not be running, and a tab exists long before anyone
    /// looks at it), and `Subscribe()` needs one. Not in `root()` either —
    /// tying it to the *levels* means a systemd tab that is never opened never
    /// makes the manager emit a single signal.
    async fn ensure_watcher(&self) {
        let shared = self.shared.clone();
        self.shared
            .watcher
            .get_or_init(|| async {
                live::spawn(shared.bus.clone(), shared.inv_tx.clone());
            })
            .await;
    }

    async fn list_services(&self, params: ListParams) -> Result<ListResult> {
        self.ensure_watcher().await;
        let query = compile(params.query.as_deref(), query::SERVICE_COLUMNS)?;
        let _busy = self
            .status()
            .busy("Reading services", self.shared.timeout_secs);
        let units = self.units_with_suffix(".service").await?;
        let mut rows = self
            .with_properties(units, SERVICE_IFACE, |entry, unit, own| {
                ServiceRow::build(entry, unit, own)
            })
            .await;
        // One `systemd-analyze security` call for the whole level rather than
        // one per row: 53 ms for every loaded service against 8 ms for a
        // single one, so the shared read is what makes the column affordable
        // at all. A unit the overview does not name keeps an empty cell.
        let exposure = security::overview(self.bus().manager()).await;
        for row in &mut rows {
            if let Some(score) = exposure.get(&row.name) {
                row.exposure = score.clone();
            }
        }
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
        self.ensure_watcher().await;
        let query = compile(params.query.as_deref(), query::TIMER_COLUMNS)?;
        let _busy = self
            .status()
            .busy("Reading timers", self.shared.timeout_secs);
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
        Ok(finish(
            query::retain(rows, &query)
                .iter()
                .map(|r| r.summary())
                .collect(),
            &params.sort,
            &timer_columns(),
        ))
    }

    async fn list_unit_files(&self, params: ListParams) -> Result<ListResult> {
        let query = compile(params.query.as_deref(), query::UNIT_FILE_COLUMNS)?;
        let _busy = self
            .status()
            .busy("Reading unit files", self.shared.timeout_secs);
        // The two answers systemd does not give: what the preset policy wants,
        // and which file each one hides. Both are read once per listing and
        // shared across every row — the policy is the same document for all of
        // them, and the search path is the same walk.
        let policy = crate::preset::Policy::load_for(self.bus().manager());
        let paths = crate::shadow::SearchPath::load_for(self.bus().manager()).await;
        let rows: Vec<UnitFileRow> = self
            .bus()
            .list_unit_files()
            .await?
            .iter()
            .map(|e| UnitFileRow::build(e, &policy, &paths))
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

    /// Every property of one loaded unit, from both interfaces that have
    /// something to say about it.
    ///
    /// Two `GetAll` calls, not one per property: `systemctl show` is a single
    /// round trip and this level should not be slower than the command it
    /// replaces. The generic `Unit` interface comes first because that is where
    /// the questions usually start (`ActiveState`, `LoadState`, the
    /// dependencies); the type-specific one follows.
    async fn read_properties(&self, unit: &str) -> Result<Vec<PropertyRow>> {
        let (iface, label) = if unit.ends_with(".timer") {
            (TIMER_IFACE, "Timer")
        } else {
            (SERVICE_IFACE, "Service")
        };
        let entry = self.find_unit(unit).await?;
        let generic = self.bus().properties(&entry.path, UNIT_IFACE).await;
        let own = self.bus().properties(&entry.path, iface).await;
        let mut rows = property_rows(unit, "Unit", &generic);
        rows.extend(property_rows(unit, label, &own));
        Ok(rows)
    }

    /// The property level of the row the user drilled into.
    async fn list_properties(&self, node_id: &str, params: ListParams) -> Result<ListResult> {
        let query = compile(params.query.as_deref(), query::PROPERTY_COLUMNS)?;
        let unit = node_id
            .strip_prefix(crate::SERVICE_PREFIX)
            .or_else(|| node_id.strip_prefix(crate::TIMER_PREFIX))
            .ok_or_else(|| ContentError::NotFound(format!("{node_id} is not a loaded unit")))?;
        let _busy = self
            .status()
            .busy(&format!("Reading {unit}"), self.shared.timeout_secs);
        let rows = self.read_properties(unit).await?;
        Ok(finish(
            query::retain(rows, &query)
                .iter()
                .map(PropertyRow::summary)
                .collect(),
            &params.sort,
            &property_columns(),
        ))
    }

    /// One level of a unit's dependency graph — see [`crate::deps`] for the
    /// shape and the cost.
    ///
    /// What it hangs under is either a unit row, where the chain starts, or
    /// another dependency row, which already carries both its chain and its
    /// axis in its id. `axis` is what the *level* asks for and settles only the
    /// first case; an id that names an axis always wins, or a needs row would
    /// unfold into ordering rows.
    async fn list_deps(&self, node_id: &str, axis: Axis, params: ListParams) -> Result<ListResult> {
        let query = compile(params.query.as_deref(), query::DEP_COLUMNS)?;
        let (axis, chain) = match deps::parse(node_id) {
            Some(found) => found,
            None => {
                let unit = node_id
                    .strip_prefix(crate::SERVICE_PREFIX)
                    .or_else(|| node_id.strip_prefix(crate::TIMER_PREFIX))
                    .ok_or_else(|| {
                        ContentError::NotFound(format!("{node_id} is not a loaded unit"))
                    })?;
                (axis, vec![unit.to_string()])
            }
        };
        let of = chain.last().cloned().unwrap_or_default();
        let what = match axis {
            Axis::Needs => format!("Reading what {of} needs"),
            Axis::Order => format!("Reading what {of} is ordered against"),
        };
        let _busy = self.status().busy(&what, self.shared.timeout_secs);
        let rows = deps::level(self.bus(), axis, &chain).await?;
        Ok(finish(
            query::retain(rows, &query)
                .iter()
                .map(DepRow::summary)
                .collect(),
            &params.sort,
            &deps::dep_columns(),
        ))
    }

    /// The journal of the unit the user drilled into.
    ///
    /// The one level that pages. Every other one holds a complete D-Bus listing
    /// and is done; a journal has no end, so what a level shows is a window
    /// into it — newest first, `[offset, offset + limit)`, and what is on
    /// screen decides how far back the read goes.
    async fn list_journal(&self, node_id: &str, params: ListParams) -> Result<ListResult> {
        let query = compile(params.query.as_deref(), query::LOG_COLUMNS)?;
        let unit = node_id
            .strip_prefix(crate::SERVICE_PREFIX)
            .or_else(|| node_id.strip_prefix(crate::TIMER_PREFIX))
            .or_else(|| node_id.strip_prefix(crate::UNIT_FILE_PREFIX))
            .ok_or_else(|| ContentError::NotFound(format!("{node_id} is not a unit")))?;
        let window = params.page.unwrap_or(PageRequest {
            offset: 0,
            limit: journal::DEFAULT_LIMIT,
        });
        let _busy = self.shared.status.busy(
            &format!("Reading the journal of {unit}"),
            self.shared.timeout_secs,
        );
        let page = journal::page(
            self.bus().manager(),
            unit,
            query.as_ref().map(UnitQuery::expr),
            window.offset,
            window.limit,
        )
        .await?;
        let items: Vec<NodeSummary> = query::retain(page.rows, &query)
            .iter()
            .map(LogRow::summary)
            .collect();
        Ok(sorted_page(items, &params.sort, window, page.has_more))
    }

    /// What `systemd-analyze security` says about the unit the user drilled
    /// into.
    ///
    /// Every check is a row, passing ones included: a level that showed only
    /// the failures would answer "what is wrong" and never "what is already
    /// covered", and which of the two is wanted is what a query decides
    /// (`[status, =, exposed]`).
    async fn list_security(&self, node_id: &str, params: ListParams) -> Result<ListResult> {
        let query = compile(params.query.as_deref(), query::SECURITY_COLUMNS)?;
        // A timer is answered for by the service it triggers — see
        // [`Self::triggered_unit`]. Everywhere else the row is the unit.
        let unit = match node_id.strip_prefix(crate::TIMER_PREFIX) {
            Some(timer) => self.triggered_unit(timer).await?,
            None => node_id
                .strip_prefix(crate::SERVICE_PREFIX)
                .or_else(|| node_id.strip_prefix(crate::UNIT_FILE_PREFIX))
                .ok_or_else(|| ContentError::NotFound(format!("{node_id} is not a unit")))?
                .to_string(),
        };
        let _busy = self
            .shared
            .status
            .busy(&format!("Analysing {unit}"), self.shared.timeout_secs);
        let rows = security::checks(self.bus().manager(), &unit).await?;
        Ok(finish(
            query::retain(rows, &query)
                .iter()
                .map(SecurityRow::summary)
                .collect(),
            &params.sort,
            &security_columns(),
        ))
    }

    /// What the unit the user drilled into actually waited for, as a flat
    /// list in chain order.
    ///
    /// Computed rather than parsed out of `systemd-analyze critical-chain` —
    /// see [`crate::chain`] for the rule, for what the text output leaves out,
    /// and for why the level is a list with a `depth` column instead of a
    /// tree.
    async fn list_chain(&self, node_id: &str, params: ListParams) -> Result<ListResult> {
        let query = compile(params.query.as_deref(), query::CHAIN_COLUMNS)?;
        let unit = node_id
            .strip_prefix(crate::SERVICE_PREFIX)
            .or_else(|| node_id.strip_prefix(crate::TIMER_PREFIX))
            .ok_or_else(|| ContentError::NotFound(format!("{node_id} is not a unit")))?;
        let _busy = self.shared.status.busy(
            &format!("Walking back from {unit}"),
            self.shared.timeout_secs,
        );
        let rows = crate::chain::level(self.bus(), unit).await?;
        Ok(finish_by(
            query::retain(rows, &query)
                .iter()
                .map(ChainRow::summary)
                .collect(),
            &params.sort,
            &chain_columns(),
            "depth",
        ))
    }

    /// The unit a timer triggers — what a timer's security level is about.
    ///
    /// `systemd-analyze security` refuses a `.timer` outright ("is not a
    /// service unit"), and it is right to: a timer starts no processes of its
    /// own, so it has no sandbox to describe. What it does have is a service
    /// it starts, and that service's sandbox is the question the level is
    /// opened to answer. So the level under a timer analyses `Unit=` —
    /// `<name>.service` unless the timer names something else — and the rows
    /// say so, because each one carries the unit it was measured on and the
    /// harden action writes into *that* unit's drop-in.
    ///
    /// A timer without `Unit=` is not something systemd produces, but an empty
    /// name would analyse whatever came next in the command line. Falling back
    /// to the timer itself means systemd refuses in its own words instead.
    async fn triggered_unit(&self, timer: &str) -> Result<String> {
        let entry = self.find_unit(timer).await?;
        let props = self.bus().properties(&entry.path, TIMER_IFACE).await;
        let unit = crate::model::as_str(&props, "Unit");
        Ok(if unit.is_empty() {
            timer.to_string()
        } else {
            unit
        })
    }

    /// The loaded units whose name ends in `suffix` — how a level picks its
    /// own population out of the one listing the manager offers.
    async fn units_with_suffix(&self, suffix: &str) -> Result<Vec<UnitEntry>> {
        let units = self.bus().list_units().await?;
        self.status().connected();
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
                let unit = self.bus().properties(&entry.path, UNIT_IFACE).await;
                let own = self.bus().properties(&entry.path, iface).await;
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
fn finish(items: Vec<NodeSummary>, sort: &[SortKey], columns: &[ColumnSchema]) -> ListResult {
    finish_by(items, sort, columns, "name")
}

/// The same, for a level whose own order is not alphabetical.
///
/// The critical chain is the one so far: its rows are a path, and the path is
/// the answer. `depth` is that order as a column, so the level opens in chain
/// order and a user who wants the biggest `+` first sorts by `took` and gets
/// the chain back by sorting on `depth` again.
fn finish_by(
    mut items: Vec<NodeSummary>,
    sort: &[SortKey],
    columns: &[ColumnSchema],
    default: &str,
) -> ListResult {
    let requested: Vec<SortKey> = if sort.is_empty() {
        vec![SortKey {
            column: default.into(),
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

/// A journal window as a level result.
///
/// Sorted newest first unless the user asked otherwise — and the sort is over
/// the *window*, not over the journal, which is the honest thing a paging level
/// can offer: sorting the page by `pid` reorders what is on screen, it does not
/// go looking for the loudest process in yesterday's log.
fn sorted_page(
    mut items: Vec<NodeSummary>,
    sort: &[SortKey],
    window: PageRequest,
    has_more: bool,
) -> ListResult {
    let requested: Vec<SortKey> = if sort.is_empty() {
        vec![SortKey {
            column: "time".into(),
            direction: SortDirection::Desc,
        }]
    } else {
        sort.to_vec()
    };
    let applied_sort = apply_sort(&mut items, &requested, &log_columns());
    ListResult {
        items,
        applied_sort,
        page: Some(PageInfo {
            offset: window.offset,
            limit: window.limit,
            // journalctl does not count, and asking it to would mean reading
            // the whole journal to say how much of it there is.
            total: None,
            has_next: has_more,
            has_prev: window.offset > 0,
        }),
        batch_download_available: false,
        downloaded: vec![],
    }
}

/// The manager node: the root every level hangs off — and the only place a
/// unit that does not exist yet can be made from, since it has no row.
struct SystemdRoot {
    label: String,
    shared: Arc<Shared>,
}

impl SystemdRoot {
    /// A submitted creating form: build the files, check them, write them,
    /// and only then start anything.
    async fn create_units(
        &self,
        action_id: &str,
        values: &HashMap<String, String>,
    ) -> Result<ActionOutcome> {
        let draft = create::draft(action_id, values).await?;
        let mut message = {
            let _busy = self
                .shared
                .status
                .busy("writing the unit files", self.shared.timeout_secs);
            create::write(&self.shared.bus, &draft).await?
        };
        if let Some(unit) = &draft.enable {
            let _busy = self
                .shared
                .status
                .busy(&format!("enabling {unit}"), JOB_WAIT_SECS);
            message.push_str(&format!(
                ". {}",
                create::enable(&self.shared.bus, unit).await?
            ));
        }
        Ok(create::outcome(message))
    }

    /// A saved [`create::NEW_FILE`] buffer. The name comes out of the buffer's
    /// own header, so a buffer that was never named goes back to the editor
    /// saying so rather than landing under a name nobody chose.
    /// The editor road: the buffer names itself, and anything that stops it
    /// from being written comes back *in* the buffer rather than as a
    /// notification over an editor that has already closed. Nothing else holds
    /// a copy of what the user typed.
    async fn create_file(&self, text: &str) -> Result<ActionOutcome> {
        let Some(unit) = create::name_from_buffer(text) else {
            return Ok(ActionOutcome::Reopen {
                content: create::reopen(
                    text,
                    &["the unit line still carries the placeholder name".to_string()],
                ),
                new_version: None,
            });
        };
        let draft = create::Draft {
            files: vec![(unit, edit::strip_header(text).to_string())],
            enable: None,
            schedule: None,
        };
        let _busy = self
            .shared
            .status
            .busy("writing the unit file", self.shared.timeout_secs);
        // Refused before anything was touched (the name is taken, systemd will
        // not have the file) — hand the text back with the reason on top. A
        // failure *after* that point is a failure to write a file that has
        // already passed every check, and reopening the buffer would suggest
        // the text is at fault when it is not.
        let staged = match create::stage(&draft).await {
            Ok(staged) => staged,
            Err(e) => {
                return Ok(ActionOutcome::Reopen {
                    content: create::reopen(text, &[e.to_string()]),
                    new_version: None,
                });
            }
        };
        Ok(create::outcome(
            create::commit(&self.shared.bus, &draft, staged).await?,
        ))
    }
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

    /// The skeleton [`create::NEW_FILE`] opens on.
    async fn prepare(&self, action_id: &str, _args: &ActionArgs) -> Result<EditorPrep> {
        if action_id != create::NEW_FILE {
            return Err(ContentError::NotSupported(format!(
                "the manager has no editor action {action_id}"
            )));
        }
        Ok(EditorPrep {
            template: create::file_template(),
            // Nothing on disk to be out of date with: the file is new.
            version: String::new(),
            suffix: ".service".into(),
            ..Default::default()
        })
    }

    /// The two roads into creating: a form, or a buffer that names itself.
    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
        _args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            (create::NEW_FILE, ActionInput::Edited { text, .. }) => self.create_file(&text).await,
            (create::NEW_SERVICE | create::NEW_TIMER, ActionInput::Form(values)) => {
                self.create_units(action_id, &values).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "the manager has no action {other}"
            ))),
        }
    }
}

/// A single unit row addressed on its own — the same cells the level showed.
///
/// One type for all three levels (and for a property row): what distinguishes a
/// service from a timer is its `node_type` and its cells, both of which the
/// summary already carries, so three near-identical structs would only be three
/// places to forget an edit.
struct UnitNode {
    id: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
    shared: Arc<Shared>,
}

impl UnitNode {
    fn new(s: NodeSummary, shared: Arc<Shared>) -> Self {
        Self {
            id: s.id,
            label: s.label,
            node_type: s.node_type,
            metadata: s.metadata,
            shared,
        }
    }

    /// The unit this row stands for, stripped of the level's id prefix.
    ///
    /// `None` on a property row: a property is not something you can start, and
    /// answering with the unit name would let a verb through on a level that
    /// never offered it.
    ///
    /// A dependency row answers with the last unit of its chain — the whole
    /// point of a row on that level is the unit it names, and the verbs act on
    /// a name.
    fn unit(&self) -> Option<&str> {
        self.id
            .strip_prefix(crate::SERVICE_PREFIX)
            .or_else(|| self.id.strip_prefix(crate::TIMER_PREFIX))
            .or_else(|| self.id.strip_prefix(crate::UNIT_FILE_PREFIX))
            .or_else(|| deps::unit_of(&self.id))
            // A security row is about one unit too, and the drop-in the harden
            // action opens is that unit's.
            .or_else(|| security::unit_of(&self.id))
            // And a chain row is the unit it names, not the one the chain was
            // opened on — the verbs act on the culprit the level found.
            .or_else(|| crate::chain::unit_of(&self.id))
    }

    /// The checks every verb passes before it reaches the manager, in the order
    /// they matter: is this unit off-limits, and does the verb want a yes first.
    ///
    /// Protection comes first and is a refusal, not a question. A prompt asks
    /// "did you mean it", which is no answer to "this would take the session
    /// down" — the muscle memory that pressed the key presses `y` too.
    ///
    /// Both questions are asked *of this manager*. A verb that only warns about
    /// the next login on the user bus warns about the next boot of the machine
    /// on the system bus, and the same list of units means something different
    /// there too — see [`control::Verb::confirm_on`] and [`crate::protect`].
    fn gate(
        &self,
        verb: &control::Verb,
        unit: &str,
        ctx: &ActionContext,
    ) -> Result<Option<String>> {
        if verb.protected() && self.shared.protect.covers(unit) {
            return Err(ContentError::PermissionDenied(
                self.shared.protect.refusal(unit, verb.label),
            ));
        }
        if let Some(prompt) = verb.confirm_on(self.shared.bus.manager())
            && !ctx.confirmed
        {
            return Ok(Some(prompt.replace("{unit}", unit)));
        }
        Ok(None)
    }

    /// The unit this row stands for, or the reason it is not one.
    fn unit_or_err(&self) -> Result<&str> {
        self.unit().ok_or_else(|| {
            ContentError::NotSupported("this row is not a unit — there is nothing to edit".into())
        })
    }

    /// Which file an editing action means, refusing the units the protection
    /// list covers.
    ///
    /// Writing a unit's configuration is as final as stopping it, only later:
    /// the file decides what the unit comes back as. So the list that refuses
    /// `stop` refuses this too — and it refuses *before* the editor opens,
    /// rather than after a buffer has been filled in.
    async fn edit_target(&self, action_id: &str) -> Result<edit::Target> {
        let layer = match action_id {
            // Hardening asks for a drop-in: the point is to add directives to
            // a unit somebody else ships, not to take a copy of it. `resolve`
            // still has the last word — a unit that already loads from the
            // user's own tree is edited in place, because there is nothing
            // underneath to preserve.
            edit::EDIT | security::HARDEN => edit::Layer::DropIn,
            edit::EDIT_FULL => edit::Layer::Full,
            other => {
                return Err(ContentError::NotSupported(format!(
                    "systemd has no editor action {other}"
                )));
            }
        };
        let unit = self.unit_or_err()?;
        if self.shared.protect.covers(unit) {
            return Err(ContentError::PermissionDenied(
                self.shared.protect.refusal(unit, "Edit"),
            ));
        }
        edit::resolve(&self.shared.bus, unit, layer).await
    }

    /// Hand the buffer back with the reason it did not land at the top of it.
    async fn reopen(
        &self,
        target: &edit::Target,
        body: &str,
        notice: Vec<String>,
    ) -> ActionOutcome {
        let state = edit::state(&self.shared.bus, &target.unit).await;
        ActionOutcome::Reopen {
            content: edit::template(target, &state, body, &notice),
            new_version: Some(edit::version(target)),
        }
    }

    /// The write path: check, back up, write, reload — and only then ask.
    ///
    /// The order is the whole point. A file that does not survive
    /// [`edit::verify`] never reaches `~/.config`, so a rejected buffer cannot
    /// be picked up by somebody else's `daemon-reload` later; and the manager
    /// re-reads the file before anyone is asked what to do about the process,
    /// so the question is about a configuration that already exists.
    async fn save_unit_file(&self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        let ActionInput::Edited { text, version, .. } = input else {
            return Err(ContentError::NotSupported(format!(
                "{action_id} is an editor action"
            )));
        };
        let target = self.edit_target(action_id).await?;
        let body = edit::strip_header(&text).to_string();

        // The buffer says what the file already says.
        if std::fs::read_to_string(&target.path).is_ok_and(|on_disk| on_disk == body) {
            return Ok(ActionOutcome::NoChanges);
        }
        // A drop-in that was opened and closed without typing anything: the
        // buffer is still the bare `[Service]` heading the template offers, and
        // writing it would leave an inert file behind that systemd reads on
        // every reload and that `systemctl revert` then has to clean up. The
        // whole-file layer is deliberately not included — saving the vendor
        // text verbatim there *is* the act, because the copy stops following
        // the package.
        if target.layer == edit::Layer::DropIn
            && !target.path.exists()
            && body == edit::body(&target)
        {
            return Ok(ActionOutcome::NoChanges);
        }
        // The file moved while the editor was open. Not a merge — the plain
        // fact, plus a fresh token so the next save is the user's decision
        // rather than a second refusal.
        if edit::version(&target) != version {
            return Ok(self
                .reopen(
                    &target,
                    &body,
                    vec![format!(
                        "{} changed on disk while you were editing it. Saving again overwrites \
                         that change; the backup keeps whatever is there now.",
                        target.path.display()
                    )],
                )
                .await);
        }

        let findings = edit::verify(&target, &body).await?;
        if findings.fatal {
            return Ok(self.reopen(&target, &body, findings.lines).await);
        }

        let backed_up = edit::backup(&target)?;
        edit::write_file(&target.path, &body)?;
        {
            let _busy = self
                .shared
                .status
                .busy("reloading the manager", self.shared.timeout_secs);
            self.shared.bus.daemon_reload().await?;
        }

        let mut message = format!("Wrote {} and reloaded the manager", target.path.display());
        if let Some(path) = backed_up {
            message.push_str(&format!("; the previous version is in {}", path.display()));
        }
        // Not fatal, but systemd will quietly ignore it, and quietly is how a
        // directive that never took effect goes unnoticed for weeks.
        if !findings.is_clean() {
            message.push_str(&format!(". systemd had a note: {}", findings.summary()));
        }

        // The file is configuration now; the running process is not. Ask only
        // where the two can actually differ.
        let state = edit::state(&self.shared.bus, &target.unit).await;
        if edit::needs_applying(&state) {
            return Ok(ActionOutcome::OpenPicker {
                action_id: edit::APPLY.into(),
                message: Some(message),
            });
        }
        Ok(ActionOutcome::Done {
            message: Some(message),
        })
    }

    /// The answer to the question a write leaves behind.
    ///
    /// `restart` and `reload-or-restart` are looked up in the verb table
    /// rather than reimplemented here: the follow-up must be the same act as
    /// the `a` leader's, protection list included.
    async fn apply(&self, input: ActionInput) -> Result<ActionOutcome> {
        let ActionInput::Picked(choice) = input else {
            return Err(ContentError::NotSupported(
                "apply needs one of its choices".into(),
            ));
        };
        let unit = self.unit_or_err()?;
        if choice == edit::NOTHING {
            return Ok(ActionOutcome::Done {
                message: Some(format!(
                    "{unit} keeps running the configuration it started with"
                )),
            });
        }
        let verb = control::verb_on(self.shared.bus.manager(), &choice).ok_or_else(|| {
            ContentError::NotSupported(format!("{choice} is not one of the apply choices"))
        })?;
        if verb.disruptive && self.shared.protect.covers(unit) {
            return Err(ContentError::PermissionDenied(
                self.shared.protect.refusal(unit, verb.label),
            ));
        }
        let _busy = self
            .shared
            .status
            .busy(&format!("{} {unit}", verb.label), JOB_WAIT_SECS);
        let message = control::run(&self.shared.bus, verb, unit, None).await?;
        Ok(ActionOutcome::Done {
            message: Some(message),
        })
    }

    /// The phase-1 road: a verb that takes a value, now that it has one.
    async fn run_verb(&self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        let (Some(verb), Some(unit)) = (
            control::verb_on(self.shared.bus.manager(), action_id),
            self.unit(),
        ) else {
            return Err(ContentError::NotSupported(format!(
                "systemd has no action {action_id} here"
            )));
        };
        let ActionInput::Picked(value) = input else {
            return Err(ContentError::NotSupported(format!(
                "{action_id} needs a value"
            )));
        };
        if verb.disruptive && self.shared.protect.covers(unit) {
            return Err(ContentError::PermissionDenied(
                self.shared.protect.refusal(unit, verb.label),
            ));
        }
        let _busy = self
            .shared
            .status
            .busy(&format!("{} {unit}", verb.label), JOB_WAIT_SECS);
        let message = control::run(&self.shared.bus, verb, unit, Some(&value)).await?;
        Ok(ActionOutcome::Done {
            message: Some(message),
        })
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

    /// Run a verb on this unit.
    ///
    /// An unknown name answers `Noop` rather than an error: the frontend routes
    /// every unhandled shortcut here, and a view that binds something this
    /// adapter does not serve should stay quiet instead of accusing the user.
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        if name == journal::FOLLOW {
            let unit = self.unit_or_err()?;
            let ActionOutcome::Done { message } =
                journal::follow(&self.shared.terminal, self.shared.bus.manager(), unit)?
            else {
                return Ok(ActionDispatch::Noop);
            };
            return Ok(ActionDispatch::Done { message });
        }
        let (Some(verb), Some(unit)) = (
            control::verb_on(self.shared.bus.manager(), name),
            self.unit(),
        ) else {
            return Ok(ActionDispatch::Noop);
        };
        // `kill` needs a signal, so it travels the picker road instead; landing
        // here would mean firing one without ever having been asked which.
        if verb.takes_value {
            return Ok(ActionDispatch::Noop);
        }
        if let Some(prompt) = self.gate(verb, unit, ctx)? {
            return Ok(ActionDispatch::Confirm { prompt });
        }
        // The busy line runs on the job's clock, not the D-Bus call's: the call
        // returns as soon as the job is enqueued, and what the user is waiting
        // for is the job.
        let _busy = self
            .shared
            .status
            .busy(&format!("{} {unit}", verb.label), JOB_WAIT_SECS);
        let message = control::run(&self.shared.bus, verb, unit, ctx.value.as_deref()).await?;
        // Both halves: the state that just changed *and* what the manager said
        // about it.
        Ok(ActionDispatch::Done {
            message: Some(message),
        })
    }

    /// Render the buffer for an editing action.
    async fn prepare(&self, action_id: &str, _args: &ActionArgs) -> Result<EditorPrep> {
        let target = self.edit_target(action_id).await?;
        let state = edit::state(&self.shared.bus, &target.unit).await;
        let mut body = edit::body(&target);
        // Hardening opens the same drop-in as `edit`, with one check's answer
        // already typed into it — at the end, appended, so whatever the file
        // already says is still there and still first.
        if action_id == security::HARDEN {
            let (_, field) = security::parse_id(&self.id).ok_or_else(|| {
                ContentError::NotSupported("this row is not a security check".into())
            })?;
            let row = security::check(self.shared.bus.manager(), &target.unit, field).await?;
            body.push_str(&security::drop_in(&row));
        }
        Ok(EditorPrep {
            template: edit::template(&target, &state, &body, &[]),
            version: edit::version(&target),
            // The unit's own extension, not the drop-in's `.conf`: `.service`
            // is what an editor recognises.
            suffix: edit::suffix(&target.unit),
            ..Default::default()
        })
    }

    /// The two menus: the signals `kill` can send, and what to do with a unit
    /// that is still running the configuration it started with.
    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        if action_id == edit::APPLY {
            return Ok(edit::apply_options());
        }
        if control::verb_on(self.shared.bus.manager(), action_id).is_none_or(|v| !v.takes_value) {
            return Ok(Vec::new());
        }
        Ok(control::signal_options()
            .into_iter()
            .map(|o| ActionOption {
                label: o.label,
                value: o.value,
            })
            .collect())
    }

    /// Three roads, told apart by the action's own id: a saved buffer, an
    /// answer to the follow-up question, or a verb that was waiting for a
    /// value.
    ///
    /// Picking an entry out of a menu is itself the deliberate act, which is
    /// why none of these carries a second `(y/n)` on top of it. The protection
    /// list still applies — that one is not a question.
    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
        _args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        match action_id {
            edit::EDIT | edit::EDIT_FULL | security::HARDEN => {
                self.save_unit_file(action_id, input).await
            }
            edit::APPLY => self.apply(input).await,
            _ => self.run_verb(action_id, input).await,
        }
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

    /// The child levels answer six different questions about a unit, and only
    /// the ones that read the manager's object need it to have loaded the
    /// unit. The critical chain needs more than an object: it needs a startup
    /// that happened, which is why it is not under a unit file either.
    #[test]
    fn properties_need_a_loaded_unit_and_the_journal_does_not() {
        let a = adapter();
        // A service is loaded, so the manager holds an object that can be
        // asked what it needs, what it is ordered against and what it is — and
        // it has a journal, an analysis, and a chain of what it waited for.
        let service = a.node(ServiceRow::default().summary());
        let under: Vec<String> = a
            .childs(&service)
            .into_iter()
            .map(|c| c.node_type.type_id)
            .collect();
        assert_eq!(
            under,
            vec![
                "systemd:dep",
                "systemd:order",
                "systemd:property",
                "systemd:log",
                "systemd:security",
                "systemd:chain"
            ]
        );

        // The needs level hangs under itself — that is the tree — while the
        // ordering level is one hop and stops.
        let dep = a.node(
            DepRow {
                chain: vec!["a.service".into(), "b.target".into()],
                axis: Axis::Needs,
                relation: "requires".into(),
                active: String::new(),
                sub: String::new(),
                load: String::new(),
                description: String::new(),
                has_children: true,
            }
            .summary(),
        );
        let under: Vec<String> = a
            .childs(&dep)
            .into_iter()
            .map(|c| c.node_type.type_id)
            .collect();
        assert_eq!(under, vec!["systemd:dep"]);

        // A unit file may never have been loaded — there is no object behind
        // it and therefore no properties. Its journal and its security analysis
        // are still readable: both take a name, not an object.
        let file = a.node(UnitFileRow::default().summary());
        let under: Vec<String> = a
            .childs(&file)
            .into_iter()
            .map(|c| c.node_type.type_id)
            .collect();
        assert_eq!(under, vec!["systemd:log", "systemd:security"]);

        // And a property is where the drilling stops.
        let prop = a.node(PropertyRow::default().summary());
        assert!(a.childs(&prop).is_empty());
    }

    #[test]
    fn a_property_row_resolves_back_to_its_unit_and_name() {
        let row = PropertyRow {
            unit: "foo.service".into(),
            name: "MainPID".into(),
            ..Default::default()
        };
        let id = row.summary().id;
        let rest = id.strip_prefix(crate::PROPERTY_PREFIX).unwrap();
        // The unit name carries dots, so only splitting at the *last* colon
        // gets both halves back out.
        assert_eq!(rest.rsplit_once(':'), Some(("foo.service", "MainPID")));
    }

    #[test]
    fn a_protected_unit_refuses_the_disruptive_verbs_and_keeps_the_rest() {
        let a = adapter();
        let node = a.node(
            ServiceRow {
                name: "dbus.service".into(),
                ..Default::default()
            }
            .summary(),
        );
        let ctx = ActionContext::default();
        // `stop` would take the session down with it.
        let stop = node.gate(control::verb("stop").unwrap(), "dbus.service", &ctx);
        assert!(matches!(stop, Err(ContentError::PermissionDenied(_))));
        // `start` on something already running is not a way to lose anything,
        // so protection has nothing to say about it.
        assert!(
            node.gate(control::verb("start").unwrap(), "dbus.service", &ctx)
                .is_ok()
        );
    }

    #[test]
    fn a_verb_that_asks_asks_once() {
        let a = adapter();
        let node = a.node(
            ServiceRow {
                name: "backup.service".into(),
                ..Default::default()
            }
            .summary(),
        );
        let stop = control::verb("stop").unwrap();
        // First time round the adapter wants a yes, and the prompt names the
        // unit rather than leaving the user to remember which row they were on.
        let prompt = node
            .gate(stop, "backup.service", &ActionContext::default())
            .unwrap()
            .expect("stop asks");
        assert!(prompt.contains("backup.service"));
        // With the yes in hand it does not ask again.
        let confirmed = ActionContext {
            confirmed: true,
            ..Default::default()
        };
        assert_eq!(node.gate(stop, "backup.service", &confirmed).unwrap(), None);
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
