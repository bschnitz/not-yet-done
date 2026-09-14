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

// ---------------------------------------------------------------------------
// Reading D-Bus property maps
// ---------------------------------------------------------------------------

/// Systemd's "this value is not available" for a `u64` property — `MemoryCurrent`
/// on a unit without the accounting enabled, `NextElapseUSecRealtime` on a timer
/// with no realtime elapse.
const UNSET_U64: u64 = u64::MAX;

fn as_str(props: &HashMap<String, OwnedValue>, key: &str) -> String {
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
    pub tasks: Option<u64>,
    /// CPU time consumed, in whole seconds (`kind: duration`).
    pub cpu: Option<u64>,
    pub restarts: Option<u64>,
    pub needs_reload: bool,
    /// How many drop-in files modify this unit. A count rather than the paths:
    /// the column sorts and filters (`[dropins, gt, 0]` is the audit), and the
    /// paths themselves are one keystroke away on the property level.
    pub dropins: usize,
    pub fragment: String,
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
            tasks: as_u64(service, "TasksCurrent"),
            cpu: as_u64(service, "CPUUsageNSec").map(|ns| ns / 1_000_000_000),
            restarts: as_u64(service, "NRestarts"),
            needs_reload: as_bool(unit, "NeedDaemonReload"),
            dropins: strings(unit, "DropInPaths").len(),
            fragment: as_str(unit, "FragmentPath"),
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
                    field("tasks", "Tasks", num(self.tasks)),
                    field("cpu", "CPU", num(self.cpu)),
                    field("restarts", "Restarts", num(self.restarts)),
                    field("needs_reload", "Reload?", flag(self.needs_reload)),
                    field("dropins", "Drop-ins", count(self.dropins)),
                    field("fragment", "Fragment", self.fragment.clone()),
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
        ColumnSchema::new("tasks", "Tasks").typed("number"),
        ColumnSchema::new("cpu", "CPU").typed("duration"),
        // Earns its place: a unit with `Restart=always` that crashes in a loop
        // reads as plain "active" everywhere else.
        ColumnSchema::new("restarts", "Restarts").typed("number"),
        ColumnSchema::new("needs_reload", "Reload?"),
        ColumnSchema::new("dropins", "Drop-ins").typed("number"),
        ColumnSchema::new("fragment", "Fragment"),
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
