//! The critical chain of a unit: what its start actually waited for.
//!
//! `systemd-analyze critical-chain foo.service` answers the question a slow
//! start raises — *what held it up* — by walking backwards through the
//! ordering graph, one hop at a time, always to the dependency that finished
//! last. This module computes that walk itself.
//!
//! # Why compute it rather than parse it
//!
//! [`crate::security`] parses `systemd-analyze` output because there is a
//! `--json=` for it. `critical-chain` has none: its only output is a
//! box-drawing tree, with the unit names escaped the way systemd escapes them
//! (`dev-disk-by\x2duuid-….device`), and a parser for it would break on
//! the first unit name that happens to hold a `└`. Computing the chain instead
//! costs one `ListUnits`, one manager property read and one `GetAll` per unit
//! the walk touches — all of which the adapter already knows how to do — and
//! it buys three things a parser could not have:
//!
//! * the same answer on both managers, from the same code, where the text
//!   output differs in what it omits;
//! * cells with types — `at` and `took` are durations a column sorts, not
//!   `+4.525s` in a string;
//! * the ability to mark a row whose timestamp is not from this startup at
//!   all, which is the one thing `systemd-analyze` gets quietly wrong (see
//!   *What the tree does not say*).
//!
//! # The rule, measured
//!
//! The obvious rule — *of everything this unit is `After=`, take the one that
//! became active last* — is wrong, and wrong in a way that looks right. On
//! this machine `sysinit.target` has 57 `After=` dependencies, and the latest
//! of them by activation is `systemd-journald.service`: not because it held
//! anything up, but because it was restarted 91 hours into the uptime. Its
//! timestamp is later than `sysinit.target`'s own.
//!
//! The rule that reproduces systemd's answer adds one word: of everything this
//! unit is `After=`, take the one that became active last **before this unit
//! did**. Against the same `sysinit.target` that picks `cryptsetup.target` at
//! 13.325160 s, against the target's own 13.325544 s — which is exactly the
//! line `systemd-analyze` prints. The filter is what keeps a later restart out
//! of a chain about the boot.
//!
//! A chain built that way cannot loop: every hop is strictly earlier than the
//! one before it. The visited set and [`MAX_DEPTH`] are there for a graph with
//! timestamps that are not strictly ordered, not for a cycle anyone has seen.
//!
//! `systemd-analyze` filters differently, and the difference is visible in its
//! output: it keeps the dependencies that became active before the *manager*
//! finished starting up, and of those takes the latest — never comparing them
//! against the unit they are a dependency of. That catches the restart case
//! above, because a restart hours later falls outside the window. It does not
//! catch a dependency that became active after its own parent did but still
//! inside it. Measured: `local-fs.target` came up at 7.969 s, and the line
//! `systemd-analyze` prints under it is `run-user-1000.mount` at 13.116 s —
//! a mount that happened five seconds after the target it is shown as having
//! held up. The same walk here picks `boot-efi.mount` at 7.967 s, the latest
//! dependency that was actually in place when the target became active.
//!
//! # What `at` is an offset from
//!
//! The manager's own `UserspaceTimestampMonotonic` — the same zero
//! `systemd-analyze time` counts from, so a row's `at` is the number the tree
//! prints after `@`, and `at + took` is the moment the unit became active.
//! `FinishTimestampMonotonic` is the other end of that window, and
//! `Finish − Userspace` is the figure `systemd-analyze time` calls the
//! userspace boot.
//!
//! # What the tree does not say, and this level does
//!
//! Three divergences, all found by reading systemd's own output against the
//! numbers behind it:
//!
//! * **A timestamp from outside the startup is printed without comment.** A
//!   unit restarted hours after the boot appears in a boot tree with a `+`
//!   like any other line. Here every row carries an `origin` cell, and a
//!   restart says `after` in it.
//! * **The root loses its `@`.** `systemd-analyze` prints the unit the chain
//!   was asked about through a different code path than its children, and that
//!   path prints either the duration or the timestamp, never both:
//!   `sshd.service +16ms`, while every line under it gets `@6.979s +26ms`. The
//!   root's own timestamp — the one number that says when the thing you asked
//!   about actually came up — is the one it drops. Here the first row is a row
//!   like the others.
//! * **A chain that leaves the startup window is quietly rerouted.** Because
//!   `systemd-analyze` will not look at a dependency that became active after
//!   the manager finished, a unit started later gets a chain assembled from
//!   whatever was still inside the window, and the tree says nothing about the
//!   substitution. On the user manager, which finishes in under 200 ms, that
//!   is most of the session. Measured: the manager finished at 185 ms and
//!   `session.slice` came up at 202 ms, seventeen milliseconds too late to be
//!   looked at — and on the strength of those seventeen milliseconds 22 of 28
//!   running user services are shown a chain through `dbus.socket` and the
//!   slices below it instead of the slice they really waited for. Here the
//!   walk follows the real predecessor, and every row outside the window says
//!   so in `origin`.
//!
//! # A flat list, not a tree
//!
//! The level exists to *find* the biggest `+`, and finding it is a sort. A
//! tree cannot be sorted without ceasing to be the chain, so the chain is a
//! list in chain order with a `depth` column — the indentation as data. Sort
//! by `took` and the culprit is the first row; sort by `depth` and the chain
//! is back.

use std::collections::{HashMap, HashSet};

use futures::stream::{self, StreamExt};
use not_yet_done_content::{ColumnSchema, ContentError, Metadata, NodeSummary, NodeType, Result};

use crate::bus::{Bus, Props, UNIT_IFACE, UnitEntry};
use crate::model::{as_u64, field, node_type, secs_cell, strings};

/// What separates the unit a chain was opened on from the unit a row stands
/// for, in a node id. The same character [`crate::deps`] uses, for the same
/// reason: a unit name may hold a colon, never a `>`.
const SEP: char = '>';

/// How many hops the walk follows before it gives up.
///
/// Not a limit anyone reaches: the longest chain on this machine is eleven
/// rows, and each hop is strictly earlier than the last, so the walk ends by
/// itself. It bounds a graph whose timestamps are not strictly ordered.
pub const MAX_DEPTH: usize = 64;

/// The unit activated inside the manager's own startup window — the rows the
/// level is about.
pub const STARTUP: &str = "startup";
/// The unit activated *after* the startup finished: a restart, or something
/// started by hand. Its `at` is not a startup offset, and saying so is the
/// point of the column.
pub const AFTER: &str = "after";
/// The unit activated *before* the startup window began — on the system
/// manager that is the initrd. There is no offset to print, so `at` is empty.
pub const BEFORE: &str = "before";

pub fn chain_type() -> NodeType {
    node_type("systemd:chain", "Critical chain")
}

/// One unit on the critical chain of another.
#[derive(Clone, Debug, Default)]
pub struct ChainRow {
    /// The unit the chain was opened on. Part of the row's identity, because
    /// the same unit sits on many chains and a row is addressed by id.
    pub root: String,
    /// The unit this row stands for.
    pub unit: String,
    /// `0` for the unit the chain was opened on, one more for each hop back —
    /// the tree's indentation, as a number a column can sort.
    pub depth: usize,
    pub description: String,
    /// When this unit's activation began, as an offset from the start of the
    /// manager's startup. Empty where the unit activated before that window —
    /// see [`BEFORE`].
    pub at: Option<u64>,
    /// How long the activation took — the number after `+`.
    pub took: Option<u64>,
    /// [`STARTUP`], [`AFTER`] or [`BEFORE`]; empty for a unit that has never
    /// activated at all.
    pub origin: String,
}

impl ChainRow {
    pub fn summary(&self) -> NodeSummary {
        NodeSummary {
            // Root first, unit last, split on a character no unit name can
            // hold — the same shape a dependency id has.
            id: format!("{}{}{SEP}{}", crate::CHAIN_PREFIX, self.root, self.unit),
            label: self.unit.clone(),
            node_type: chain_type(),
            metadata: Metadata {
                fields: vec![
                    field("unit", "Unit", self.unit.clone()),
                    field("depth", "Depth", self.depth.to_string()),
                    field("at", "At", secs_cell(self.at)),
                    field("took", "Took", secs_cell(self.took)),
                    field("origin", "Origin", self.origin.clone()),
                    field("description", "Description", self.description.clone()),
                ],
            },
            has_children: Some(false),
        }
    }
}

pub fn chain_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("unit", "Unit"),
        // The level's own order, and therefore its default sort. A number
        // rather than leading spaces: indentation that is data can be sorted
        // away and sorted back.
        ColumnSchema::new("depth", "Depth").typed("number"),
        ColumnSchema::new("at", "At").typed("duration"),
        ColumnSchema::new("took", "Took").typed("duration"),
        ColumnSchema::new("origin", "Origin"),
        ColumnSchema::new("description", "Description"),
    ]
}

/// The unit a chain row stands for, read straight out of its id — what lets
/// the verbs reach the row, exactly as they reach a dependency row.
pub fn unit_of(id: &str) -> Option<&str> {
    let rest = id.strip_prefix(crate::CHAIN_PREFIX)?;
    rest.rsplit(SEP).next().filter(|u| !u.is_empty())
}

/// The pair a chain id carries: the unit the chain was opened on, and the unit
/// this row is.
pub fn parse(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix(crate::CHAIN_PREFIX)?;
    let (root, unit) = rest.split_once(SEP)?;
    (!root.is_empty() && !unit.is_empty()).then_some((root, unit))
}

/// One unit's activation timestamps, as the walk needs them.
#[derive(Clone, Copy, Debug, Default)]
struct Times {
    /// `InactiveExitTimestampMonotonic` — when the activation began.
    began: Option<u64>,
    /// When it ended: `ActiveEnterTimestampMonotonic`, or — for a
    /// `Type=oneshot` that runs to completion and never becomes active —
    /// `InactiveEnterTimestampMonotonic`. The same pair
    /// [`crate::model`]'s startup column is built from, and for the same
    /// reason: taking only the first leaves every oneshot blank.
    entered: Option<u64>,
}

impl Times {
    fn read(props: &Props) -> Self {
        let began = as_u64(props, "InactiveExitTimestampMonotonic").filter(|n| *n > 0);
        let entered = [
            "ActiveEnterTimestampMonotonic",
            "InactiveEnterTimestampMonotonic",
        ]
        .iter()
        .filter_map(|key| as_u64(props, key))
        .filter(|n| *n > 0)
        .find(|n| began.is_none_or(|b| *n >= b));
        Self { began, entered }
    }

    /// The moment the walk orders units by: when the unit finished coming up.
    ///
    /// Falling back to the start is what keeps a unit that is still activating
    /// on the chain — it is holding something up right now, which is the most
    /// interesting state a row can be in.
    fn activated(&self) -> Option<u64> {
        self.entered.or(self.began)
    }

    /// The moment the row's `at` is measured from — `systemd-analyze` prints
    /// the start of the activation where there is one, so that `@` and `+`
    /// meet at the moment the unit became active.
    fn start(&self) -> Option<u64> {
        self.began.or(self.entered)
    }

    fn took(&self) -> Option<u64> {
        match (self.began, self.entered) {
            (Some(b), Some(e)) if e >= b => Some(e - b),
            _ => None,
        }
    }
}

/// The critical chain of `root`, from the unit itself back to what it waited
/// for first.
///
/// Errors only when the unit is not loaded: a chain is a fact about a startup
/// that happened, and a unit the manager has never loaded had none.
pub async fn level(bus: &Bus, root: &str) -> Result<Vec<ChainRow>> {
    let (userspace, finish) = bus.startup_window().await?;
    let units = bus.list_units().await?;
    let by_name: HashMap<&str, &UnitEntry> = units.iter().map(|u| (u.name.as_str(), u)).collect();
    if !by_name.contains_key(root) {
        return Err(ContentError::NotFound(format!("no loaded unit {root}")));
    }

    let mut rows: Vec<ChainRow> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut current = root.to_string();
    let mut props = read(bus, &by_name, &current).await;

    loop {
        let times = Times::read(&props);
        seen.insert(current.clone());
        rows.push(ChainRow {
            root: root.to_string(),
            unit: current.clone(),
            depth: rows.len(),
            description: crate::model::as_str(&props, "Description"),
            // An offset only where there is a window to offset from; a unit
            // that came up before the manager's startup began has none.
            at: times.start().filter(|s| *s >= userspace).map(|s| s - userspace),
            took: times.took(),
            origin: origin(times.activated(), userspace, finish),
        });
        if rows.len() >= MAX_DEPTH {
            break;
        }
        // Nothing to compare against: a unit that never activated cannot say
        // which of its dependencies it waited for.
        let Some(mine) = times.activated() else { break };
        let Some((next, next_props)) = predecessor(bus, &by_name, &props, mine, &seen).await else {
            break;
        };
        current = next;
        props = next_props;
    }
    Ok(rows)
}

/// The dependency this unit actually waited for: of everything it is `After=`,
/// the one that became active last *before it did*.
///
/// See the module docs for why the second half of that sentence is not
/// optional. Ties — two dependencies with the same microsecond — go to the
/// first by name, so that reloading the level twice gives the same chain
/// rather than whichever read came back first.
async fn predecessor(
    bus: &Bus,
    by_name: &HashMap<&str, &UnitEntry>,
    props: &Props,
    mine: u64,
    seen: &HashSet<String>,
) -> Option<(String, Props)> {
    let candidates: Vec<String> = strings(props, "After")
        .into_iter()
        .filter(|name| !seen.contains(name))
        .collect();
    stream::iter(candidates)
        .map(|name| async move {
            let props = read(bus, by_name, &name).await;
            let activated = Times::read(&props).activated();
            (name, props, activated)
        })
        .buffer_unordered(crate::bus::MAX_INFLIGHT)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter(|(_, _, activated)| activated.is_some_and(|a| a < mine))
        .max_by(|a, b| a.2.cmp(&b.2).then_with(|| b.0.cmp(&a.0)))
        .map(|(name, props, _)| (name, props))
}

/// One unit's generic properties, or an empty map where the manager has not
/// loaded it.
///
/// An `After=` may name a unit that does not exist on this machine at all, and
/// an empty map answers the only question the walk asks of it — when did you
/// activate — with "never", which is the truth.
async fn read(bus: &Bus, by_name: &HashMap<&str, &UnitEntry>, name: &str) -> Props {
    match by_name.get(name) {
        Some(entry) => bus.properties(&entry.path, UNIT_IFACE).await,
        None => Props::new(),
    }
}

/// Where a unit's activation falls relative to the manager's own startup.
fn origin(activated: Option<u64>, userspace: u64, finish: Option<u64>) -> String {
    match activated {
        None => "",
        Some(a) if a < userspace => BEFORE,
        // A manager that is still starting up has no end yet, and nothing can
        // be after an end that has not arrived.
        Some(a) if finish.is_some_and(|f| a > f) => AFTER,
        Some(_) => STARTUP,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_id_carries_the_chain_it_was_reached_on_and_the_unit_it_is() {
        let row = ChainRow {
            root: "sshd.service".into(),
            unit: "network.target".into(),
            ..Default::default()
        };
        let id = row.summary().id;
        assert_eq!(parse(&id), Some(("sshd.service", "network.target")));
        // The verbs act on a name, and the name is the second half.
        assert_eq!(unit_of(&id), Some("network.target"));
    }

    /// A unit name may hold a colon — `dbus-:1.19-org.a11y….service` is one
    /// this machine really runs — so the id is split on `>`, which a unit name
    /// cannot hold at all.
    #[test]
    fn a_colon_in_a_unit_name_does_not_split_the_id() {
        let row = ChainRow {
            root: "dbus-:1.19-org.a11y.atspi.Registry@0.service".into(),
            unit: "dbus-:1.19-org.a11y.atspi.Registry@0.socket".into(),
            ..Default::default()
        };
        let id = row.summary().id;
        assert_eq!(
            parse(&id),
            Some((
                "dbus-:1.19-org.a11y.atspi.Registry@0.service",
                "dbus-:1.19-org.a11y.atspi.Registry@0.socket"
            ))
        );
    }

    /// The three states of `origin`, and the fourth that is an empty cell.
    ///
    /// The `after` row is the one this column exists for: a unit restarted
    /// long after the boot appears on a boot chain with a timestamp that is
    /// not a boot offset, and `systemd-analyze` prints it without a word.
    #[test]
    fn a_timestamp_from_outside_the_startup_is_named_rather_than_printed_plain() {
        let (userspace, finish) = (7_056_331, Some(14_703_209));
        assert_eq!(origin(Some(13_325_544), userspace, finish), STARTUP);
        assert_eq!(origin(Some(334_000_000_000), userspace, finish), AFTER);
        assert_eq!(origin(Some(1_733_000), userspace, finish), BEFORE);
        assert_eq!(origin(None, userspace, finish), "");
        // A boot that has not finished has no "after" — nothing is later than
        // an end that has not arrived.
        assert_eq!(origin(Some(334_000_000_000), userspace, None), STARTUP);
    }

    /// The pair the row's two duration cells are built from, including the
    /// oneshot that never becomes active.
    #[test]
    fn a_oneshot_that_never_becomes_active_still_has_a_duration() {
        let mut props = Props::new();
        let put = |props: &mut Props, key: &str, value: u64| {
            props.insert(key.into(), zbus_systemd::zvariant::Value::U64(value).try_into().unwrap());
        };
        put(&mut props, "InactiveExitTimestampMonotonic", 1_000_000);
        put(&mut props, "ActiveEnterTimestampMonotonic", 0);
        put(&mut props, "InactiveEnterTimestampMonotonic", 1_250_000);
        let times = Times::read(&props);
        assert_eq!(times.took(), Some(250_000));
        assert_eq!(times.activated(), Some(1_250_000));
        assert_eq!(times.start(), Some(1_000_000));
    }

    /// A unit that is still activating has no end yet — and belongs on the
    /// chain more than anything else does, because it is holding something up
    /// right now.
    #[test]
    fn a_unit_that_is_still_activating_stays_on_the_chain() {
        let mut props = Props::new();
        props.insert(
            "InactiveExitTimestampMonotonic".into(),
            zbus_systemd::zvariant::Value::U64(2_000_000).try_into().unwrap(),
        );
        let times = Times::read(&props);
        assert_eq!(times.activated(), Some(2_000_000));
        // No end, so no span — an empty cell rather than a made-up zero.
        assert_eq!(times.took(), None);
    }
}
