//! The rows the tab is made of, and the columns that describe them.
//!
//! One type per unit kind, per decision D3 in `docs/plan-systemd-adapter.md`: a
//! timer wants `next` / `left` / `last`, a service wants `pid` / `mem` /
//! `restarts`, and neither wants the other's empty cells.
//!
//! # Canonical values, not formatted ones
//!
//! Every cell leaves here machine-readable — bytes as bytes, a CPU time as
//! integer seconds, an instant as RFC 3339 — and the table engine's `kind:`
//! does the formatting, aligning and live ticking. That is the view spec's own
//! rule, and it is what keeps a column sortable and filterable: `[mem, gt,
//! 100000000]` means something, `[mem, gt, "95.4 MiB"]` does not.
//!
//! There is no exception any more. A timer's `left` used to be one: a
//! **countdown** — `field − now` — where the engine only knew `kind: elapsed`,
//! `now − field`. The engine now has `kind: countdown` with a `countdown_to:`
//! source field, so `left` is not a cell at all: the view derives it from
//! `next`, ticking live between loads, and `next` is what sorts.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use not_yet_done_content::{ColumnSchema, Metadata, MetadataField, NodeSummary, NodeType};
use zbus_systemd::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::bus::{UnitEntry, UnitFileEntry};
use crate::preset::Policy;
use crate::shadow::SearchPath;

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

/// Build one of this adapter's node types.
pub(crate) fn node_type(type_id: &str, display_name: &str) -> NodeType {
    NodeType {
        type_id: type_id.into(),
        mime_type: String::new(),
        syntax: None,
        file_extension: String::new(),
        display_name: display_name.into(),
    }
}

pub fn manager_type() -> NodeType {
    node_type("systemd:manager", "systemd")
}

pub fn service_type() -> NodeType {
    node_type("systemd:service", "Service")
}

pub fn timer_type() -> NodeType {
    node_type("systemd:timer", "Timer")
}

pub fn property_type() -> NodeType {
    node_type("systemd:property", "Property")
}

pub fn unit_file_type() -> NodeType {
    node_type("systemd:unitfile", "Unit file")
}

pub fn log_type() -> NodeType {
    node_type("systemd:log", "Journal")
}

pub fn security_type() -> NodeType {
    node_type("systemd:security", "Security")
}

// ---------------------------------------------------------------------------
// Reading D-Bus property maps
// ---------------------------------------------------------------------------

/// Systemd's "this value is not available" for a `u64` property — `MemoryCurrent`
/// on a unit without the accounting enabled, `NextElapseUSecRealtime` on a timer
/// with no realtime elapse.
const UNSET_U64: u64 = u64::MAX;

/// One string property, or the empty string where the unit does not carry it.
/// `pub(crate)` because the security level reads a single property of its own
/// — the unit a timer triggers — without building a row around it.
pub(crate) fn as_str(props: &HashMap<String, OwnedValue>, key: &str) -> String {
    match props.get(key).map(|v| &**v) {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    }
}

fn as_u64(props: &HashMap<String, OwnedValue>, key: &str) -> Option<u64> {
    let n = match props.get(key).map(|v| &**v)? {
        Value::U64(n) => *n,
        Value::U32(n) => u64::from(*n),
        Value::I64(n) => u64::try_from(*n).ok()?,
        Value::I32(n) => u64::try_from(*n).ok()?,
        _ => return None,
    };
    (n != UNSET_U64).then_some(n)
}

/// A property that is an array of strings — the shape every dependency
/// relation has (`Requires`, `After`, …). Anything else, including an absent
/// key, is no strings at all rather than an error: a unit that names nothing
/// and a unit that does not exist should both produce an empty level.
pub fn strings(props: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    match props.get(key).map(|v| &**v) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn as_bool(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
    matches!(props.get(key).map(|v| &**v), Some(Value::Bool(true)))
}

/// A systemd realtime timestamp (microseconds since the epoch) as an instant.
///
/// Both `0` and "unset" mean *never happened*, and a row says that with an empty
/// cell rather than with 1970.
fn realtime(props: &HashMap<String, OwnedValue>, key: &str) -> Option<DateTime<Utc>> {
    let usec = as_u64(props, key).filter(|n| *n > 0)?;
    DateTime::from_timestamp_micros(i64::try_from(usec).ok()?)
}

/// A systemd monotonic timestamp (microseconds since boot) as an instant,
/// against a boot instant the caller has already established.
///
/// A timer with `OnBootSec=` / `OnUnitActiveSec=` has only this — its realtime
/// elapse is unset — so without the conversion those timers would show an empty
/// `next` forever.
fn monotonic(
    props: &HashMap<String, OwnedValue>,
    key: &str,
    boot: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    let usec = as_u64(props, key).filter(|n| *n > 0)?;
    Some(boot? + chrono::TimeDelta::microseconds(i64::try_from(usec).ok()?))
}

/// How long the unit's last activation took, in microseconds — the number
/// `systemd-analyze blame` prints, computed from the unit's own properties.
///
/// # Why not run `systemd-analyze blame`
///
/// Because there is nothing to run. `blame` reads the same two timestamps
/// this does, and both of them arrive in the property map the row is already
/// built from (the adapter reads interfaces with `GetAll`), so the column
/// costs no call, no subprocess and no parsing. `blame` also has no
/// `--json=`: systemd names the verbs that do, and `blame` is not among
/// them, so the alternative would have been parsing a human-formatted table.
///
/// # The two timestamps
///
/// Activation starts when the unit leaves `inactive`. It ends either when
/// the unit becomes `active`, or — for a `Type=oneshot` without
/// `RemainAfterExit=`, which runs to completion and goes straight back to
/// `inactive` without ever being active — when it re-enters `inactive`.
/// Taking only the first pair would leave exactly those units blank, which
/// is how this was caught: measured against `systemd-analyze --user blame`
/// over every loaded unit, the first rule matched 27 of 28 and the odd one
/// out was the one oneshot.
///
/// `None` when the unit has not finished an activation — it never started,
/// or it is activating right now and the end timestamp still belongs to the
/// previous run. An empty cell is the honest answer there.
///
/// # Zero is a measurement
///
/// A `Type=simple` unit counts as active the instant its process is exec'd,
/// so both timestamps are the same number and the span is a real, measured
/// zero. That is why the end is accepted when it merely *equals* the start:
/// on this machine more than half the running services are `Type=simple`,
/// and requiring a strictly later end blanked every one of them. `blame`
/// omits those units from its output entirely; the column has a row for them
/// regardless, and `0` says "started instantly" where an empty cell would
/// have said "never started".
fn activation_usec(unit: &HashMap<String, OwnedValue>) -> Option<u64> {
    let start = as_u64(unit, "InactiveExitTimestampMonotonic").filter(|n| *n > 0)?;
    let end = [
        "ActiveEnterTimestampMonotonic",
        "InactiveEnterTimestampMonotonic",
    ]
    .iter()
    .filter_map(|key| as_u64(unit, key))
    .find(|end| *end >= start)?;
    Some(end - start)
}

/// Microseconds as a `kind: duration` cell — seconds, with the fraction kept.
///
/// Seconds because that is the canonical unit of a duration column, and the
/// fraction because these spans live below it: a service that took 15 ms
/// would be `0` in whole seconds. Formatted from integer arithmetic rather
/// than through a float, so the cell is exact and never picks up an
/// exponent.
fn secs_cell(usec: Option<u64>) -> String {
    match usec {
        Some(n) => format!("{}.{:06}", n / 1_000_000, n % 1_000_000),
        None => String::new(),
    }
}

/// A [`UnitEntry`] rebuilt from one unit's generic properties, the way
/// `ListUnits` would have reported it.
///
/// The listing tuple and the `Unit` interface carry the same five facts under
/// the same names, so a row can be built without the listing — which is what
/// [`crate::live`] needs: a `PropertiesChanged` signal names an object path,
/// not a listing row, and re-listing every unit to find one of them would make
/// a single state change cost what a whole level costs.
///
/// `None` when the map has no `Id`, which is how a unit that vanished between
/// the signal and the read announces itself (see [`crate::bus::Bus::properties`]).
pub fn entry_from_properties(
    path: &OwnedObjectPath,
    unit: &HashMap<String, OwnedValue>,
) -> Option<UnitEntry> {
    let name = as_str(unit, "Id");
    if name.is_empty() {
        return None;
    }
    Some(UnitEntry {
        name,
        description: as_str(unit, "Description"),
        load_state: as_str(unit, "LoadState"),
        active_state: as_str(unit, "ActiveState"),
        sub_state: as_str(unit, "SubState"),
        path: path.clone(),
    })
}

/// When this machine booted, read from `/proc/uptime`.
///
/// The reference point for the monotonic clock. Read once per listing rather
/// than per row: the answer drifts by the time the listing takes, which is
/// nothing against a countdown shown to the minute.
pub fn boot_instant() -> Option<DateTime<Utc>> {
    let raw = std::fs::read_to_string("/proc/uptime").ok()?;
    let secs: f64 = raw.split_whitespace().next()?.parse().ok()?;
    Some(Utc::now() - chrono::TimeDelta::microseconds((secs * 1_000_000.0) as i64))
}

// ---------------------------------------------------------------------------
// Cells
// ---------------------------------------------------------------------------

pub(crate) fn field(key: &str, label: &str, value: impl Into<String>) -> MetadataField {
    MetadataField {
        key: key.into(),
        value: value.into(),
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    }
}

/// An optional number as a cell: absent is an empty cell, not a zero.
fn num(value: Option<u64>) -> String {
    value.map(|n| n.to_string()).unwrap_or_default()
}

/// An instant as a cell, canonical RFC 3339 so `kind: datetime` and
/// `kind: elapsed` can both read it.
fn instant(value: Option<DateTime<Utc>>) -> String {
    value.map(|t| t.to_rfc3339()).unwrap_or_default()
}

fn flag(value: bool) -> String {
    if value { "yes".into() } else { String::new() }
}

/// A count whose zero is "nothing to say" rather than a number worth reading.
/// Keeps a column that is empty on almost every row actually empty.
fn count(value: usize) -> String {
    if value == 0 {
        String::new()
    } else {
        value.to_string()
    }
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

/// One `.service` unit the manager has loaded.
#[derive(Clone, Debug, Default)]
pub struct ServiceRow {
    pub name: String,
    pub description: String,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub enabled: String,
    pub since: Option<DateTime<Utc>>,
    pub pid: Option<u64>,
    pub mem: Option<u64>,
    /// The high-water mark of `mem` — what the service cost when it cost the
    /// most, which is the number that explains an OOM kill or a chosen
    /// `MemoryMax=`. Hidden by default in the shipped view: it is the same
    /// question as `mem` asked about the past, and a row that already carries
    /// eleven columns should not spend a twelfth on it unasked.
    pub mem_peak: Option<u64>,
    pub tasks: Option<u64>,
    /// CPU time consumed, in whole seconds (`kind: duration`).
    pub cpu: Option<u64>,
    /// How long the last activation took, in microseconds — see
    /// [`activation_usec`]. Microseconds and not seconds because that is the
    /// resolution systemd reports and the one these spans need; the cell is
    /// seconds, with the fraction kept.
    pub startup: Option<u64>,
    pub restarts: Option<u64>,
    pub needs_reload: bool,
    /// How many drop-in files modify this unit. A count rather than the paths:
    /// the column sorts and filters (`[dropins, gt, 0]` is the audit), and the
    /// paths themselves are one keystroke away on the property level.
    pub dropins: usize,
    pub fragment: String,
    /// The unit's overall `systemd-analyze security` exposure, `0.0`..=`10.0`,
    /// as the string systemd prints it.
    ///
    /// A string and not a number because it is one shared listing-wide read
    /// that either happened or did not, and an absent score must leave the
    /// cell blank rather than read as a perfect zero. The column is still
    /// `typed("number")`, so it sorts and compares numerically.
    pub exposure: String,
    /// Whether the unit needs anything — what decides whether its row offers to
    /// unfold into [the dependency tree](crate::deps). Read from the same
    /// property map as the rest of the row, so it costs nothing.
    pub has_deps: bool,
}

impl ServiceRow {
    /// Assemble a row from the listing entry plus the two property maps.
    pub fn build(
        entry: &UnitEntry,
        unit: &HashMap<String, OwnedValue>,
        service: &HashMap<String, OwnedValue>,
    ) -> Self {
        Self {
            name: entry.name.clone(),
            description: entry.description.clone(),
            load: entry.load_state.clone(),
            active: entry.active_state.clone(),
            sub: entry.sub_state.clone(),
            enabled: as_str(unit, "UnitFileState"),
            since: realtime(unit, "ActiveEnterTimestamp"),
            // A stopped service reports PID 0; that is "none", not a process.
            pid: as_u64(service, "MainPID").filter(|p| *p > 0),
            mem: as_u64(service, "MemoryCurrent"),
            mem_peak: as_u64(service, "MemoryPeak"),
            tasks: as_u64(service, "TasksCurrent"),
            cpu: as_u64(service, "CPUUsageNSec").map(|ns| ns / 1_000_000_000),
            startup: activation_usec(unit),
            restarts: as_u64(service, "NRestarts"),
            needs_reload: as_bool(unit, "NeedDaemonReload"),
            dropins: strings(unit, "DropInPaths").len(),
            fragment: as_str(unit, "FragmentPath"),
            // Filled in by the level, which reads every unit's score in one
            // call — see [`crate::security::overview`].
            exposure: String::new(),
            has_deps: crate::deps::has_needs(unit),
        }
    }

    pub fn summary(&self) -> NodeSummary {
        NodeSummary {
            id: format!("{}{}", crate::SERVICE_PREFIX, self.name),
            label: self.name.clone(),
            node_type: service_type(),
            metadata: Metadata {
                fields: vec![
                    field("name", "Name", self.name.clone()),
                    field("description", "Description", self.description.clone()),
                    field("load", "Load", self.load.clone()),
                    field("active", "Active", self.active.clone()),
                    field("sub", "Sub", self.sub.clone()),
                    field("enabled", "Enabled", self.enabled.clone()),
                    field("since", "Since", instant(self.since)),
                    field("pid", "PID", num(self.pid)),
                    field("mem", "Mem", num(self.mem)),
                    field("mem_peak", "Mem peak", num(self.mem_peak)),
                    field("tasks", "Tasks", num(self.tasks)),
                    field("cpu", "CPU", num(self.cpu)),
                    field("startup", "Startup", secs_cell(self.startup)),
                    field("restarts", "Restarts", num(self.restarts)),
                    field("needs_reload", "Reload?", flag(self.needs_reload)),
                    field("dropins", "Drop-ins", count(self.dropins)),
                    field("fragment", "Fragment", self.fragment.clone()),
                    field("exposure", "Exposure", self.exposure.clone()),
                ],
            },
            has_children: Some(self.has_deps),
        }
    }
}

pub fn service_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("name", "Name"),
        ColumnSchema::new("description", "Description"),
        ColumnSchema::new("load", "Load"),
        ColumnSchema::new("active", "Active"),
        ColumnSchema::new("sub", "Sub"),
        ColumnSchema::new("enabled", "Enabled"),
        ColumnSchema::new("since", "Since").typed("datetime"),
        ColumnSchema::new("pid", "PID").typed("number"),
        // Bytes. Raw, because a pre-formatted "95.4 MiB" sorts as text and
        // cannot be compared in a query; the view renders it with `kind: bytes`.
        ColumnSchema::new("mem", "Mem").typed("number"),
        ColumnSchema::new("mem_peak", "Mem peak").typed("number"),
        ColumnSchema::new("tasks", "Tasks").typed("number"),
        ColumnSchema::new("cpu", "CPU").typed("duration"),
        // Seconds, fraction kept — the span `systemd-analyze blame` reports,
        // read straight off the unit. The view renders it with
        // `format: precise`, without which a column of milliseconds would be
        // a column of `00`. See [`activation_usec`].
        ColumnSchema::new("startup", "Startup").typed("duration"),
        // Earns its place: a unit with `Restart=always` that crashes in a loop
        // reads as plain "active" everywhere else.
        ColumnSchema::new("restarts", "Restarts").typed("number"),
        ColumnSchema::new("needs_reload", "Reload?"),
        ColumnSchema::new("dropins", "Drop-ins").typed("number"),
        ColumnSchema::new("fragment", "Fragment"),
        // `systemd-analyze security`, 0 (locked down) to 10 (wide open). One
        // call for the whole level; see [`crate::security`] for why the number
        // alone says less than the level it opens onto.
        ColumnSchema::new("exposure", "Exposure").typed("number"),
    ]
}

// ---------------------------------------------------------------------------
// Timers
// ---------------------------------------------------------------------------

/// One `.timer` unit the manager has loaded.
#[derive(Clone, Debug, Default)]
pub struct TimerRow {
    pub name: String,
    pub description: String,
    pub active: String,
    pub sub: String,
    pub enabled: String,
    pub next: Option<DateTime<Utc>>,
    pub last: Option<DateTime<Utc>>,
    /// The unit this timer triggers.
    pub unit: String,
    pub result: String,
    pub persistent: bool,
    /// Whether the timer needs anything — see [`ServiceRow::has_deps`].
    pub has_deps: bool,
}

impl TimerRow {
    pub fn build(
        entry: &UnitEntry,
        unit: &HashMap<String, OwnedValue>,
        timer: &HashMap<String, OwnedValue>,
        boot: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            name: entry.name.clone(),
            description: entry.description.clone(),
            active: entry.active_state.clone(),
            sub: entry.sub_state.clone(),
            enabled: as_str(unit, "UnitFileState"),
            // A calendar timer has the realtime elapse; a monotonic one
            // (`OnBootSec=`, `OnUnitActiveSec=`) has only the other.
            next: realtime(timer, "NextElapseUSecRealtime")
                .or_else(|| monotonic(timer, "NextElapseUSecMonotonic", boot)),
            last: realtime(timer, "LastTriggerUSec"),
            unit: as_str(timer, "Unit"),
            result: as_str(timer, "Result"),
            persistent: as_bool(timer, "Persistent"),
            has_deps: crate::deps::has_needs(unit),
        }
    }

    pub fn summary(&self) -> NodeSummary {
        NodeSummary {
            id: format!("{}{}", crate::TIMER_PREFIX, self.name),
            label: self.name.clone(),
            node_type: timer_type(),
            metadata: Metadata {
                fields: vec![
                    field("name", "Name", self.name.clone()),
                    field("description", "Description", self.description.clone()),
                    field("active", "Active", self.active.clone()),
                    field("sub", "Sub", self.sub.clone()),
                    field("enabled", "Enabled", self.enabled.clone()),
                    field("next", "Next", instant(self.next)),
                    field("last", "Last", instant(self.last)),
                    field("unit", "Unit", self.unit.clone()),
                    field("result", "Result", self.result.clone()),
                    field("persistent", "Persistent", flag(self.persistent)),
                ],
            },
            has_children: Some(self.has_deps),
        }
    }
}

pub fn timer_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("name", "Name"),
        ColumnSchema::new("description", "Description"),
        ColumnSchema::new("active", "Active"),
        ColumnSchema::new("sub", "Sub"),
        ColumnSchema::new("enabled", "Enabled"),
        ColumnSchema::new("next", "Next").typed("datetime"),
        ColumnSchema::new("last", "Last").typed("datetime"),
        ColumnSchema::new("unit", "Unit"),
        ColumnSchema::new("result", "Result"),
        ColumnSchema::new("persistent", "Persistent"),
    ]
}

// ---------------------------------------------------------------------------
// Unit files
// ---------------------------------------------------------------------------

/// One unit file on disk — the half of the picture the loaded-unit levels
/// cannot show, because a unit that has never been started is not loaded.
#[derive(Clone, Debug, Default)]
pub struct UnitFileRow {
    pub name: String,
    pub state: String,
    pub path: String,
    /// Where the file comes from — see [`origin`].
    pub vendor: String,
    /// What the distribution's policy wants this unit's enablement to be —
    /// see [`crate::preset`]. Empty where a preset cannot apply.
    pub preset: String,
    /// How `state` disagrees with `preset`, named as the fix. Empty when they
    /// agree, which is the common case and the point of the column.
    pub drift: String,
    /// The unit file this one hides, if any — see [`crate::shadow`].
    pub shadows: String,
}

impl UnitFileRow {
    /// Assemble a row from the listing entry plus the two things systemd does
    /// not report: what the preset policy says, and what the file hides.
    pub fn build(entry: &UnitFileEntry, policy: &Policy, paths: &SearchPath) -> Self {
        let name = entry
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&entry.path)
            .to_string();
        let (preset, drift) = if crate::preset::preset_applies(&entry.state) {
            let p = policy.query(&name);
            (p.word().to_string(), crate::preset::drift(&entry.state, p))
        } else {
            (String::new(), "")
        };
        Self {
            shadows: paths.shadowed(&name, &entry.path),
            name,
            state: entry.state.clone(),
            path: entry.path.clone(),
            vendor: origin(&entry.path).into(),
            preset,
            drift: drift.into(),
        }
    }

    pub fn summary(&self) -> NodeSummary {
        NodeSummary {
            id: format!("{}{}", crate::UNIT_FILE_PREFIX, self.name),
            label: self.name.clone(),
            node_type: unit_file_type(),
            metadata: Metadata {
                fields: vec![
                    field("name", "Name", self.name.clone()),
                    field("state", "State", self.state.clone()),
                    field("path", "Path", self.path.clone()),
                    field("vendor", "Origin", self.vendor.clone()),
                    field("preset", "Preset", self.preset.clone()),
                    field("drift", "Drift", self.drift.clone()),
                    field("shadows", "Shadows", self.shadows.clone()),
                ],
            },
            has_children: Some(false),
        }
    }
}

/// Which of the unit-file search paths a file came from.
///
/// The single most useful thing about a unit file's path, and the one people
/// squint at the path to work out: is this mine, the distribution's, the
/// administrator's, or something generated for this boot only?
pub fn origin(path: &str) -> &'static str {
    match path {
        p if p.starts_with("/usr/") => "vendor",
        p if p.starts_with("/etc/") => "admin",
        p if p.starts_with("/run/") => "runtime",
        _ => "user",
    }
}

pub fn unit_file_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("name", "Name"),
        ColumnSchema::new("state", "State"),
        ColumnSchema::new("path", "Path"),
        ColumnSchema::new("vendor", "Origin"),
        ColumnSchema::new("preset", "Preset"),
        ColumnSchema::new("drift", "Drift"),
        ColumnSchema::new("shadows", "Shadows"),
    ]
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// One property of one unit — a row on the `systemd:property` level.
///
/// The level exists because `systemctl show` is two hundred lines of
/// `Key=value` and a terminal can only scroll it. As a table it can be sorted,
/// filtered and searched with everything the tab already has: `[name, like,
/// Timeout]` answers in one keystroke what grepping a wall of text answers in
/// three.
#[derive(Clone, Debug, Default)]
pub struct PropertyRow {
    /// The unit these properties belong to, carried so the row can address
    /// itself by id.
    pub unit: String,
    /// The property's name, as systemd spells it (`MainPID`, `TimeoutStopUSec`).
    pub name: String,
    /// Its value, rendered — see [`render`].
    pub value: String,
    /// The short interface name it came from: `Unit`, `Service`, `Timer`.
    pub interface: String,
}

impl PropertyRow {
    pub fn summary(&self) -> NodeSummary {
        NodeSummary {
            id: format!("{}{}:{}", crate::PROPERTY_PREFIX, self.unit, self.name),
            label: self.name.clone(),
            node_type: property_type(),
            metadata: Metadata {
                fields: vec![
                    field("name", "Property", self.name.clone()),
                    field("value", "Value", self.value.clone()),
                    field("interface", "Interface", self.interface.clone()),
                ],
            },
            has_children: Some(false),
        }
    }
}

pub fn property_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("name", "Property"),
        ColumnSchema::new("value", "Value"),
        ColumnSchema::new("interface", "Interface"),
    ]
}

/// Turn one interface's property map into rows.
///
/// `interface` is the short name the rows carry — the full
/// `org.freedesktop.systemd1.Service` is noise in a column when every row on
/// the level starts with it.
pub fn property_rows(
    unit: &str,
    interface: &str,
    props: &HashMap<String, OwnedValue>,
) -> Vec<PropertyRow> {
    props
        .iter()
        .map(|(name, value)| PropertyRow {
            unit: unit.to_string(),
            name: name.clone(),
            value: render(value),
            interface: interface.to_string(),
        })
        .collect()
}

/// A D-Bus value as one line of text.
///
/// Everything on this level is a string in the end, because the level's whole
/// population is heterogeneous — a `bool`, a `u64`, an array of exec commands —
/// and there is no column type that covers it. That is the trade the level
/// makes: `show`-fidelity instead of typed comparison. The typed cells are the
/// ones the service and timer levels lift out into real columns.
///
/// Containers are flattened rather than debug-printed: `Debug` for a zvariant
/// array spells out its signature, which is true and unreadable.
fn render(value: &OwnedValue) -> String {
    render_value(value)
}

fn render_value(value: &Value<'_>) -> String {
    match value {
        Value::Str(s) => s.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::U8(n) => n.to_string(),
        Value::I16(n) => n.to_string(),
        Value::U16(n) => n.to_string(),
        Value::I32(n) => n.to_string(),
        Value::U32(n) => n.to_string(),
        Value::I64(n) => n.to_string(),
        Value::U64(n) => {
            // The same "unset" sentinel the typed columns filter out. Here it
            // stays visible as the word, because on a properties level the fact
            // that systemd reports the maximum *is* the answer.
            if *n == UNSET_U64 {
                "(unset)".to_string()
            } else {
                n.to_string()
            }
        }
        Value::F64(n) => n.to_string(),
        Value::ObjectPath(p) => p.to_string(),
        Value::Signature(s) => s.to_string(),
        Value::Value(inner) => render_value(inner),
        Value::Array(a) => a.iter().map(render_value).collect::<Vec<_>>().join(", "),
        Value::Structure(s) => s
            .fields()
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Dict(d) => d
            .iter()
            .map(|(k, v)| format!("{}={}", render_value(k), render_value(v)))
            .collect::<Vec<_>>()
            .join(", "),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::children::check_rows;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    /// Build a `Unit` property map out of the three timestamps activation is
    /// read from. `0` is systemd's "never", so it is the default.
    fn timestamps(exit: u64, active_enter: u64, inactive_enter: u64) -> HashMap<String, OwnedValue> {
        [
            ("InactiveExitTimestampMonotonic", exit),
            ("ActiveEnterTimestampMonotonic", active_enter),
            ("InactiveEnterTimestampMonotonic", inactive_enter),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), OwnedValue::from(v)))
        .collect()
    }

    /// The ordinary case: the unit left `inactive` and became `active`.
    /// The numbers are a real reading — `systemd-analyze --user blame` said
    /// `103ms` for the same unit at the same moment.
    #[test]
    fn activation_is_the_span_from_leaving_inactive_to_becoming_active() {
        let unit = timestamps(136_670_234_541, 136_670_337_965, 0);
        assert_eq!(activation_usec(&unit), Some(103_424));
    }

    /// A `Type=oneshot` without `RemainAfterExit=` never becomes active: it
    /// runs and goes straight back to `inactive`. Reading only the active
    /// timestamp would leave exactly those units blank — this is the one unit
    /// of 28 that disagreed with the simpler rule when it was measured
    /// against `blame`, which reported `15ms` for it.
    #[test]
    fn a_oneshot_that_never_became_active_is_measured_to_where_it_ended() {
        let unit = timestamps(16_126_342, 0, 16_141_677);
        assert_eq!(activation_usec(&unit), Some(15_335));
    }

    /// Becoming active wins over going back to inactive, so a unit that has
    /// since been stopped still reports how long it took to *start*, not how
    /// long it ran.
    #[test]
    fn a_unit_that_ran_and_stopped_still_reports_its_start() {
        let unit = timestamps(1_000_000, 1_250_000, 9_000_000);
        assert_eq!(activation_usec(&unit), Some(250_000));
    }

    /// A `Type=simple` unit is active the moment it is exec'd, so both
    /// timestamps are the same instant and the span is a measured zero — not
    /// a missing one. Requiring a strictly later end blanked every such unit,
    /// which on a live user manager was most of the running services;
    /// `pipewire.service` is the reading below.
    #[test]
    fn an_instant_activation_is_a_zero_and_not_a_blank() {
        let unit = timestamps(16_401_924, 16_401_924, 0);
        assert_eq!(activation_usec(&unit), Some(0));
        assert_eq!(secs_cell(activation_usec(&unit)), "0.000000");
    }

    /// Nothing to report is an empty cell, never a zero: a unit that never
    /// started and a unit that started instantly must not look the same. The
    /// mid-activation case is the subtle one — the end timestamp is still the
    /// previous run's and lies *before* the start, so there is no span yet.
    #[test]
    fn an_unfinished_activation_has_no_span() {
        assert_eq!(activation_usec(&timestamps(0, 0, 0)), None);
        assert_eq!(activation_usec(&timestamps(0, 5_000_000, 0)), None);
        // Currently activating: started at 9s, last became active at 2s.
        assert_eq!(activation_usec(&timestamps(9_000_000, 2_000_000, 0)), None);
        assert_eq!(secs_cell(activation_usec(&timestamps(0, 0, 0))), "");
    }

    /// The cell is seconds with the fraction kept, because the column is a
    /// `duration` and a duration's canonical unit is seconds — and because
    /// whole seconds would render every one of these spans as zero.
    #[test]
    fn the_startup_cell_is_fractional_seconds() {
        assert_eq!(secs_cell(Some(103_424)), "0.103424");
        assert_eq!(secs_cell(Some(5_242_346)), "5.242346");
        assert_eq!(secs_cell(Some(31)), "0.000031");
        assert_eq!(secs_cell(Some(60_000_000)), "60.000000");
        // Parses back as the number the sort and the query compare on.
        assert_eq!(secs_cell(Some(103_424)).parse::<f64>().unwrap(), 0.103424);
    }

    /// The `in_rows` promise, per level: every declared column must be a field
    /// on every row. Checked here rather than after the first blank cell shows
    /// up on screen.
    #[test]
    fn every_level_keeps_the_in_rows_promise() {
        let service = ServiceRow {
            name: "backup-photos.service".into(),
            ..Default::default()
        };
        assert!(
            check_rows(&service_columns(), &[service.summary()]).is_empty(),
            "service row is missing declared columns: {:?}",
            check_rows(&service_columns(), &[service.summary()])
        );

        let timer = TimerRow {
            name: "backup-photos.timer".into(),
            ..Default::default()
        };
        assert!(
            check_rows(&timer_columns(), &[timer.summary()]).is_empty(),
            "timer row is missing declared columns: {:?}",
            check_rows(&timer_columns(), &[timer.summary()])
        );

        let file = UnitFileRow {
            name: "backup-photos.service".into(),
            ..Default::default()
        };
        assert!(
            check_rows(&unit_file_columns(), &[file.summary()]).is_empty(),
            "unit file row is missing declared columns: {:?}",
            check_rows(&unit_file_columns(), &[file.summary()])
        );
    }

    #[test]
    fn an_absent_number_is_an_empty_cell_not_a_zero() {
        assert_eq!(num(None), "");
        assert_eq!(num(Some(0)), "0");
    }

    #[test]
    fn origin_names_the_search_path() {
        assert_eq!(origin("/usr/lib/systemd/user/dbus.service"), "vendor");
        assert_eq!(origin("/etc/systemd/user/thing.service"), "admin");
        assert_eq!(origin("/run/systemd/user/gen.service"), "runtime");
        assert_eq!(
            origin("/home/someone/.config/systemd/user/a.service"),
            "user"
        );
    }

    fn unit_file(path: &str, state: &str, policy: &str) -> UnitFileRow {
        UnitFileRow::build(
            &UnitFileEntry {
                path: path.into(),
                state: state.into(),
            },
            &Policy::parse([policy.to_string()]),
            &SearchPath::default(),
        )
    }

    #[test]
    fn a_unit_file_row_takes_its_name_from_the_basename() {
        let row = unit_file("/usr/lib/systemd/user/backup-photos.timer", "static", "");
        assert_eq!(row.name, "backup-photos.timer");
        assert_eq!(row.vendor, "vendor");
    }

    /// A state with no `[Install]` section has nothing a preset could apply to,
    /// and an empty cell says that more quietly than a word would.
    #[test]
    fn a_static_unit_shows_no_preset_and_no_drift() {
        let row = unit_file("/usr/lib/systemd/user/dbus.socket", "static", "disable *\n");
        assert_eq!(row.preset, "");
        assert_eq!(row.drift, "");
    }

    #[test]
    fn drift_names_the_fix_and_is_empty_when_the_unit_agrees() {
        let off = unit_file("/usr/lib/systemd/user/a.service", "disabled", "disable *\n");
        assert_eq!(off.preset, "disabled");
        assert_eq!(off.drift, "", "disabled and meant to be: no drift");

        let on = unit_file("/usr/lib/systemd/user/a.service", "enabled", "disable *\n");
        assert_eq!(on.drift, "should-disable");

        // No matching rule at all means `enable` — systemd's own default, and
        // the reason a distribution without a default-off preset shows a long
        // list here.
        let unclaimed = unit_file("/usr/lib/systemd/user/b.service", "disabled", "");
        assert_eq!(unclaimed.preset, "enabled");
        assert_eq!(unclaimed.drift, "should-enable");
    }
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

/// One journal entry of one unit — a row on the `systemd:log` level.
///
/// Built from `journalctl --output=json` rather than from D-Bus, which is the
/// one level here that is: the journal has no bus interface. See
/// [`crate::journal`] for why the command is the supported interface and what
/// the JSON does to a message that is not plain text.
#[derive(Clone, Debug, Default)]
pub struct LogRow {
    /// The unit the entry belongs to, carried so the row can address itself.
    pub unit: String,
    /// journald's own address for this entry (`__CURSOR`) — stable across
    /// reboots and rotations, which is what makes it the row's id.
    pub cursor: String,
    pub time: Option<DateTime<Utc>>,
    /// The syslog severity as a number, `0`..=`7`. The queryable, sortable,
    /// comparable form: `[prio, lte, 3]` is the query this level is for.
    pub prio: Option<u64>,
    /// The same severity as the word systemd uses for it. Not a second source
    /// of truth — it is derived from [`LogRow::prio`] — but a column of bare
    /// digits is one nobody can read at a glance, and `highlights:` rules read
    /// better against `err` than against `3`.
    pub level: String,
    pub pid: Option<u64>,
    pub message: String,
}

impl LogRow {
    /// One `--output=json` line as a row. `None` for a line that is not an
    /// entry — journalctl prints the occasional note among them, and a note is
    /// not a row.
    pub fn parse(unit: &str, line: &str) -> Option<Self> {
        let fields = crate::journal::decode(line)?;
        let cursor = fields.get("__CURSOR")?.clone();
        let prio = fields.get("PRIORITY").and_then(|p| p.trim().parse().ok());
        Some(Self {
            unit: unit.to_string(),
            cursor,
            time: fields
                .get("__REALTIME_TIMESTAMP")
                .and_then(|t| crate::journal::instant(t)),
            prio,
            level: level_word(prio),
            pid: fields.get("_PID").and_then(|p| p.trim().parse().ok()),
            message: fields.get("MESSAGE").cloned().unwrap_or_default(),
        })
    }

    pub fn summary(&self) -> NodeSummary {
        NodeSummary {
            // The unit first and the cursor last, because a cursor never holds
            // a colon and a unit name may (`dbus-:1.19-….service`) — so the
            // *last* colon is the one that splits the pair.
            id: format!("{}{}:{}", crate::LOG_PREFIX, self.unit, self.cursor),
            label: self.message.lines().next().unwrap_or_default().to_string(),
            node_type: log_type(),
            metadata: Metadata {
                fields: vec![
                    field("time", "Time", instant(self.time)),
                    field("level", "Level", self.level.clone()),
                    field("prio", "Prio", num(self.prio)),
                    field("pid", "PID", num(self.pid)),
                    field("message", "Message", self.message.clone()),
                ],
            },
            has_children: Some(false),
        }
    }
}

pub fn log_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("time", "Time").typed("datetime"),
        ColumnSchema::new("level", "Level"),
        ColumnSchema::new("prio", "Prio").typed("number"),
        ColumnSchema::new("pid", "PID").typed("number"),
        ColumnSchema::new("message", "Message"),
    ]
}

/// The word systemd uses for a syslog severity. An absent or out-of-range
/// priority has no word — an empty cell, not a guess.
fn level_word(prio: Option<u64>) -> String {
    match prio {
        Some(0) => "emerg",
        Some(1) => "alert",
        Some(2) => "crit",
        Some(3) => "err",
        Some(4) => "warning",
        Some(5) => "notice",
        Some(6) => "info",
        Some(7) => "debug",
        _ => "",
    }
    .to_string()
}

/// One `systemd-analyze security` check of one unit — a row on the
/// `systemd:security` level.
///
/// Built from `--json=short`, which is the only machine-readable form of the
/// analysis; see [`crate::security`] for the two fields whose names mislead
/// (`set` is tri-state, `exposure` is a string that is absent exactly when the
/// check passes) and for why the fix is a curated table rather than the check's
/// own name.
#[derive(Clone, Debug, Default)]
pub struct SecurityRow {
    /// The unit the check was run against, carried so the row can address
    /// itself and so the harden action knows which drop-in to open.
    pub unit: String,
    /// systemd's stable identifier for the check (`json_field`) — the row's id
    /// and the key the fix table is looked up by.
    pub id: String,
    /// The check as systemd displays it (`CapabilityBoundingSet=~CAP_KILL`).
    /// A **label**: for the group checks it is not a directive that can be
    /// written anywhere.
    pub check: String,
    pub description: String,
    /// `ok`, `exposed` or `no-effect` — the three states of `set`, spelled out.
    /// A word rather than a boolean because the third state is the one a
    /// boolean would lose.
    pub status: String,
    /// What this check contributes to the unit's overall exposure. Absent
    /// exactly when the check passes.
    pub exposure: Option<f64>,
    /// The directives that would settle this check, as one cell — empty where
    /// no single directive does.
    pub fix: String,
}

impl SecurityRow {
    /// One element of the `--json=short` array as a row. `None` for an element
    /// that is missing the identifier the row is addressed by.
    pub fn parse(unit: &str, item: &serde_json::Value) -> Option<Self> {
        let id = item.get("json_field")?.as_str()?.to_string();
        // Tri-state: absent or JSON null is "has no effect here", which is a
        // different answer from "this unit fails the check".
        let status = match item.get("set").and_then(serde_json::Value::as_bool) {
            Some(true) => crate::security::OK,
            Some(false) => crate::security::EXPOSED,
            None => crate::security::NO_EFFECT,
        };
        Some(Self {
            unit: unit.to_string(),
            fix: crate::security::fix_cell(&id),
            id,
            check: item
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            description: item
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: status.to_string(),
            exposure: item
                .get("exposure")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| s.trim().parse().ok()),
        })
    }

    pub fn summary(&self) -> NodeSummary {
        NodeSummary {
            // The unit first and the check id last, for the same reason a
            // journal id is built that way: a `json_field` never holds a
            // colon, a unit name may.
            id: format!("{}{}:{}", crate::SECURITY_PREFIX, self.unit, self.id),
            label: self.check.clone(),
            node_type: security_type(),
            metadata: Metadata {
                fields: vec![
                    field("check", "Check", self.check.clone()),
                    field("status", "Status", self.status.clone()),
                    field("exposure", "Exposure", exposure(self.exposure)),
                    field("description", "Description", self.description.clone()),
                    field("fix", "Fix", self.fix.clone()),
                    field("id", "Id", self.id.clone()),
                ],
            },
            has_children: Some(false),
        }
    }
}

pub fn security_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("check", "Check"),
        ColumnSchema::new("status", "Status"),
        // Empty on a check that passes — which is what makes
        // `[exposure, gt, 0]` the same query as "what is still open".
        ColumnSchema::new("exposure", "Exposure").typed("number"),
        ColumnSchema::new("description", "Description"),
        ColumnSchema::new("fix", "Fix"),
        ColumnSchema::new("id", "Id"),
    ]
}

/// An exposure as a cell: one decimal, the way systemd prints it, and empty
/// rather than `0.0` when there is none.
fn exposure(value: Option<f64>) -> String {
    value.map(|n| format!("{n:.1}")).unwrap_or_default()
}
