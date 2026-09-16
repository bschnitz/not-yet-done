//! What is broken right now, as one sentence.
//!
//! # Why this is not a query on the Services level
//!
//! The Services level is the units whose name ends in `.service`, and that is
//! the right population for a level: a mount and a socket have nothing in
//! common with a service except that both can fail. But *failing* is exactly
//! what they have in common, and an indicator that only counted services would
//! be quietly wrong on the one failure people actually hit — a mount that did
//! not come up. So this reads the same population `systemctl --failed` does:
//! every loaded unit, of every type, whose `ActiveState` is `failed`.
//!
//! # Why it costs one call
//!
//! `ListUnits` already carries `ActiveState` for every loaded unit, so the
//! whole answer is one D-Bus round trip with no per-unit `GetAll` behind it.
//! That is what makes it cheap enough to hang off a lifecycle hook, which runs
//! on a program start whether or not anyone ever opens the tab.
//!
//! # Why it says nothing when nothing is wrong
//!
//! [`message`] returns `None` for an empty list, and the action turns that into
//! a `Noop`. A notification that appears on every launch to say "all good" is a
//! notification people learn to dismiss without reading, and then the one that
//! matters is dismissed with it.

use not_yet_done_content::{
    ColumnSchema, InputSpec, Metadata, NodeAction, NodeSummary, NodeType, Result,
};

use crate::bus::{Bus, Props, UnitEntry};
use crate::config::Manager;
use crate::model::{field, instant, node_type, realtime};

/// Report which units are in a failed state.
///
/// Hung off the **manager**, not off a row: the question is about the machine,
/// not about a unit, and the answer exists before any level has been loaded.
/// That is also what lets a `connected` hook bind it (see the `hooks:` block in
/// `docs/examples/views/systemd.yaml`) — a hook invokes an action on the
/// adapter root.
pub const REPORT: &str = "report-failed";

/// How many names the message spells out before it just counts the rest. A
/// notification is one line in a bar; past a handful of names it stops being
/// readable and the count is the part that still informs.
const NAMED: usize = 5;

/// The action the manager offers. Only the manager: asking a single unit which
/// units have failed is a question about the wrong subject.
pub fn actions_for(type_id: &str) -> Vec<NodeAction> {
    if type_id != crate::model::manager_type().type_id {
        return Vec::new();
    }
    vec![NodeAction::new(
        REPORT,
        "Report the failed units",
        InputSpec::None,
    )]
}

/// Every loaded unit whose `ActiveState` is `failed`, by name, sorted.
///
/// Sorted because the message names the first few: an unsorted list would name
/// a different few on every call and read like the set had changed when only
/// the bus's ordering had.
pub async fn units(bus: &Bus) -> Result<Vec<String>> {
    Ok(entries(bus).await?.into_iter().map(|u| u.name).collect())
}

/// Whether this listing entry is one of the failed ones.
pub fn is_failed(unit: &UnitEntry) -> bool {
    unit.active_state == "failed"
}

/// The listing entries that are in a failed state, in name order.
pub async fn entries(bus: &Bus) -> Result<Vec<UnitEntry>> {
    let mut failed: Vec<UnitEntry> = bus.list_units().await?.into_iter().filter(is_failed).collect();
    failed.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(failed)
}

// ---------------------------------------------------------------------------
// The level
// ---------------------------------------------------------------------------

/// The level's node type.
///
/// It is its own type rather than a query on the Services level because its
/// population is a different one: every loaded unit of every kind. A failed
/// mount is the failure people actually hit, and it has no row on a level that
/// selects by `.service`.
pub fn failed_type() -> NodeType {
    node_type("systemd:failed", "Failed unit")
}

pub fn failed_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("name", "Name"),
        ColumnSchema::new("kind", "Kind"),
        ColumnSchema::new("description", "Description"),
        ColumnSchema::new("load", "Load"),
        ColumnSchema::new("sub", "Sub"),
        ColumnSchema::new("since", "Since").typed("datetime"),
    ]
}

/// One failed unit, of whatever kind.
///
/// No `active` column: every row on this level is `failed`, and a column with
/// one value in it is a column that only takes width away from the ones that
/// differ. `sub` is kept because it does differ — `failed` and `dead` are two
/// different endings.
#[derive(Clone, Debug, Default)]
pub struct FailedRow {
    pub name: String,
    /// The unit suffix without the dot: `service`, `mount`, `socket`. The
    /// column exists because this is the one level where the kind varies, and
    /// it is the first thing that tells you which manual page to open.
    pub kind: String,
    pub description: String,
    pub load: String,
    pub sub: String,
    /// When the unit last changed state — on this level, when it failed.
    ///
    /// `StateChangeTimestamp` rather than `ActiveEnterTimestamp`: the latter is
    /// when it last came *up*, which for a unit that has been failing since
    /// boot is either long ago or never, and neither answers "since when is
    /// this broken".
    pub since: Option<chrono::DateTime<chrono::Utc>>,
}

impl FailedRow {
    pub fn build(entry: &UnitEntry, unit: &Props) -> Self {
        Self {
            name: entry.name.clone(),
            kind: kind_of(&entry.name).to_string(),
            description: entry.description.clone(),
            load: entry.load_state.clone(),
            sub: entry.sub_state.clone(),
            since: realtime(unit, "StateChangeTimestamp"),
        }
    }

    pub fn summary(&self) -> NodeSummary {
        NodeSummary {
            id: format!("{}{}", crate::FAILED_PREFIX, self.name),
            label: self.name.clone(),
            node_type: failed_type(),
            metadata: Metadata {
                fields: vec![
                    field("name", "Name", self.name.clone()),
                    field("kind", "Kind", self.kind.clone()),
                    field("description", "Description", self.description.clone()),
                    field("load", "Load", self.load.clone()),
                    field("sub", "Sub", self.sub.clone()),
                    field("since", "Since", instant(self.since)),
                ],
            },
            has_children: Some(false),
        }
    }
}

/// The unit kind from its name — the suffix without the dot.
///
/// A name with no dot is not a unit systemd would have loaded, so the fallback
/// is only ever reached by a bug or a very odd manager; it returns the whole
/// name rather than an empty cell so the row still says something.
pub fn kind_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, k)| k).unwrap_or(name)
}

/// The unit a failed row is about — the whole row, which is what makes the
/// control verbs work on this level without a line of their own.
pub fn unit_of(id: &str) -> Option<&str> {
    id.strip_prefix(crate::FAILED_PREFIX)
}

/// The sentence for a set of failed units, or `None` when the set is empty.
///
/// The manager is named because the answer differs by manager and a bare "2
/// units failed" sends the reader to the wrong `systemctl`.
pub fn message(manager: Manager, names: &[String]) -> Option<String> {
    let (first, rest) = match names.len() {
        0 => return None,
        n if n <= NAMED => (names, 0),
        n => (&names[..NAMED], n - NAMED),
    };
    let unit = match names.len() {
        1 => "unit",
        _ => "units",
    };
    let listed = first.join(", ");
    Some(match rest {
        0 => format!(
            "{} {} {} failed: {listed}",
            names.len(),
            manager.as_str(),
            unit
        ),
        more => format!(
            "{} {} {} failed: {listed} and {more} more",
            names.len(),
            manager.as_str(),
            unit
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_manager_offers_the_report_and_a_unit_does_not() {
        assert_eq!(actions_for("systemd:manager").len(), 1);
        assert_eq!(actions_for("systemd:manager")[0].id, REPORT);
        assert!(actions_for("systemd:service").is_empty());
        assert!(actions_for("systemd:unitfile").is_empty());
    }

    #[test]
    fn nothing_failed_is_nothing_to_say() {
        assert_eq!(message(Manager::User, &[]), None);
    }

    #[test]
    fn one_failure_is_singular_and_names_the_manager() {
        assert_eq!(
            message(Manager::User, &names(&["backup.service"])).unwrap(),
            "1 user unit failed: backup.service"
        );
        assert_eq!(
            message(Manager::System, &names(&["data.mount"])).unwrap(),
            "1 system unit failed: data.mount"
        );
    }

    #[test]
    fn a_handful_is_named_in_full() {
        assert_eq!(
            message(Manager::User, &names(&["a.service", "b.mount"])).unwrap(),
            "2 user units failed: a.service, b.mount"
        );
    }

    #[test]
    fn past_a_handful_the_rest_is_counted() {
        let many = names(&["a", "b", "c", "d", "e", "f", "g"]);
        assert_eq!(
            message(Manager::System, &many).unwrap(),
            "7 system units failed: a, b, c, d, e and 2 more"
        );
    }

    #[test]
    fn only_a_failed_active_state_counts() {
        let entry = |name: &str, state: &str| UnitEntry {
            name: name.to_string(),
            description: String::new(),
            load_state: "loaded".to_string(),
            active_state: state.to_string(),
            sub_state: String::new(),
            path: zbus_systemd::zvariant::OwnedObjectPath::try_from("/x").unwrap(),
        };
        assert!(is_failed(&entry("a.service", "failed")));
        // `activating` is the state a unit passes through on its way to
        // failing, and a unit that is still trying has not failed yet.
        assert!(!is_failed(&entry("b.service", "activating")));
        assert!(!is_failed(&entry("c.service", "inactive")));
        assert!(!is_failed(&entry("d.service", "active")));
    }
}
