//! The dependency graph as levels: what a unit needs, and what it waits for.
//!
//! systemd keeps both answers as ordinary properties on
//! `org.freedesktop.systemd1.Unit`, so this module needs no transport of its
//! own — it reads the same `GetAll` a unit row already reads and turns two
//! groups of those properties into rows.
//!
//! # Two questions, two levels
//!
//! "What does this unit need?" and "what has to run before it?" are different
//! questions, and a tree that answers both at once answers neither — a chain
//! that walked from a `Requires=` into an `After=` and back would be a path
//! through the graph that means nothing. So:
//!
//! * [`Axis::Needs`] — `Requires`, `Requisite`, `Wants`, `BindsTo`, `PartOf`,
//!   `Upholds`: what `systemctl list-dependencies` shows, and like it a
//!   **tree**, walked as deep as the user opens it.
//! * [`Axis::Order`] — `After` and `Before`, one hop and **flat**. Ordering is
//!   read one step at a time ("what has to be up before this, and who waits for
//!   it"), and recursing it is not useful: a user unit's `After=basic.target`
//!   expands into the whole session start.
//!
//! # A row is addressed by its whole chain, not by its unit
//!
//! The same unit turns up all over a dependency tree.  Measured on this
//! project's development machine: `systemctl --user list-dependencies --all
//! pipewire.service` prints **120 rows made of 27 distinct units** — a factor
//! of four and a half. A row id of `dep:<unit>` would therefore name four or
//! five different positions in the tree at once, and the frontend addresses
//! rows *by id*: it patches a live row by id and keys its tree caches by id.
//!
//! So the id carries the path it was reached by — `dep:a.service>b.target>c.socket`
//! — and only its last element is displayed. That also makes the walk itself
//! stateless: a row knows the chain it sits in, so [`level`] can tell a cycle
//! from a repeat without carrying a visited set between calls.
//!
//! `>` separates the units because a unit name cannot contain one: systemd
//! restricts names to alphanumerics plus `:-_.\@` and escapes everything else
//! as `\xNN`.
//!
//! # Where the walk stops
//!
//! * **A cycle** — the unit is already in its own chain. The row is *shown*,
//!   with `(cycle)` on its relation, and offers nothing below it. Hiding it
//!   would be the worse answer: a dependency cycle is a thing you want to see.
//! * **Depth** — [`MAX_DEPTH`], a guard rather than a policy. Nothing sane
//!   reaches it; a generated unit graph might.
//! * **A unit the manager has not loaded** — it has no D-Bus object, so there
//!   is nothing to ask. Its row shows the name and empty state cells, which is
//!   exactly what a broken `Requires=` looks like.
//!
//! # What a level costs
//!
//! One `ListUnits` for the whole level — that is where the child rows' state
//! and object paths come from, one round trip no matter how wide the level is
//! — plus one `GetAll` on the parent for the relation lists themselves.
//!
//! On the needs axis there is a third part: one `GetAll` per child, to know
//! whether that child has anything below it. That is a deliberate purchase.
//! `has_children` is what decides whether a row shows an expand arrow, and an
//! arrow that opens onto nothing is the kind of lie a tree should not tell; the
//! reads go out concurrently under [`MAX_INFLIGHT`](crate::bus::MAX_INFLIGHT),
//! and a level is ten or twenty rows wide where the services level does the
//! same thing four hundred times.

use std::collections::HashSet;

use futures::stream::{self, StreamExt};
use not_yet_done_content::{ColumnSchema, ContentError, Metadata, NodeSummary, NodeType, Result};
use zbus_systemd::zvariant::OwnedObjectPath;

use crate::bus::{Bus, Props, UNIT_IFACE, UnitEntry};
use crate::model::{field, node_type, strings};

/// The relations that answer "what does this unit need in order to run?".
///
/// The order is also the priority: a unit named by two of these — `Wants=` in
/// the unit file and `Requires=` from a drop-in — takes the first word, and the
/// strongest relation is the one worth reading.
pub const NEEDS: &[(&str, &str)] = &[
    ("Requires", "requires"),
    ("Requisite", "requisite"),
    ("BindsTo", "binds-to"),
    ("PartOf", "part-of"),
    ("Upholds", "upholds"),
    ("Wants", "wants"),
];

/// The ordering relations — what runs before this unit, and what waits for it.
pub const ORDER: &[(&str, &str)] = &[("After", "after"), ("Before", "before")];

/// How deep the needs tree offers to go. A guard against a pathological graph,
/// not a limit anyone reaches by hand.
pub const MAX_DEPTH: usize = 10;

/// What separates two units of a chain in a node id — see the module docs.
const SEP: char = '>';

/// Which question a dependency level answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// What the unit needs — recursive.
    Needs,
    /// What it is ordered against — one hop.
    Order,
}

impl Axis {
    /// The node-id prefix rows of this axis carry.
    pub fn prefix(self) -> &'static str {
        match self {
            Axis::Needs => crate::DEP_PREFIX,
            Axis::Order => crate::ORDER_PREFIX,
        }
    }

    /// The properties this axis reads, paired with the word a row from each
    /// one calls its relation.
    pub fn sources(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Axis::Needs => NEEDS,
            Axis::Order => ORDER,
        }
    }

    /// Whether a row on this axis is asked what is under it.
    fn recurses(self) -> bool {
        self == Axis::Needs
    }

    fn node_type(self) -> NodeType {
        match self {
            Axis::Needs => dep_type(),
            Axis::Order => order_type(),
        }
    }
}

pub fn dep_type() -> NodeType {
    node_type("systemd:dep", "Dependency")
}

pub fn order_type() -> NodeType {
    node_type("systemd:order", "Ordering")
}

/// One unit named by another unit's relations — a row on either dependency
/// level.
///
/// The two levels share one row type and one column set because they are the
/// same shape of answer: a unit, the relation that led to it, and what it is
/// doing. Only the word in `relation` and the recursion differ, and both of
/// those the row carries in its [`Axis`].
#[derive(Clone, Debug)]
pub struct DepRow {
    /// The path from the unit the level was opened on down to this row,
    /// inclusive — its last element is the unit this row stands for. See the
    /// module docs for why the whole chain, and not just the name, is the row's
    /// identity.
    pub chain: Vec<String>,
    pub axis: Axis,
    /// `requires`, `wants`, `after`, … — with ` (cycle)` appended where the
    /// walk stopped because the unit is already in its own chain.
    pub relation: String,
    pub active: String,
    pub sub: String,
    pub load: String,
    pub description: String,
    /// Whether this row has anything below it — see the module docs on cost.
    pub has_children: bool,
}

impl DepRow {
    /// The unit this row stands for.
    pub fn name(&self) -> &str {
        self.chain.last().map(String::as_str).unwrap_or_default()
    }

    pub fn summary(&self) -> NodeSummary {
        NodeSummary {
            id: encode(self.axis, &self.chain),
            label: self.name().to_string(),
            node_type: self.axis.node_type(),
            metadata: Metadata {
                fields: vec![
                    field("name", "Unit", self.name().to_string()),
                    field("relation", "Relation", self.relation.clone()),
                    field("active", "Active", self.active.clone()),
                    field("sub", "Sub", self.sub.clone()),
                    field("load", "Load", self.load.clone()),
                    field("description", "Description", self.description.clone()),
                ],
            },
            has_children: Some(self.has_children),
        }
    }
}

/// The columns of both dependency levels.
pub fn dep_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("name", "Unit"),
        ColumnSchema::new("relation", "Relation"),
        ColumnSchema::new("active", "Active"),
        ColumnSchema::new("sub", "Sub"),
        ColumnSchema::new("load", "Load"),
        ColumnSchema::new("description", "Description"),
    ]
}

// ---------------------------------------------------------------------------
// Ids
// ---------------------------------------------------------------------------

/// A chain as a node id.
pub fn encode(axis: Axis, chain: &[String]) -> String {
    format!("{}{}", axis.prefix(), chain.join(&SEP.to_string()))
}

/// A node id back into the axis and the chain it names, or `None` if it is not
/// a dependency id at all.
pub fn parse(id: &str) -> Option<(Axis, Vec<String>)> {
    for axis in [Axis::Needs, Axis::Order] {
        if let Some(rest) = id.strip_prefix(axis.prefix()) {
            let chain: Vec<String> = rest.split(SEP).map(str::to_string).collect();
            return chain.iter().all(|u| !u.is_empty()).then_some((axis, chain));
        }
    }
    None
}

/// The unit a dependency row stands for, read straight out of its id.
///
/// What lets the verbs reach a dependency row: they act on a unit *name*, and
/// the last element of the chain is one.
pub fn unit_of(id: &str) -> Option<&str> {
    let rest = id
        .strip_prefix(crate::DEP_PREFIX)
        .or_else(|| id.strip_prefix(crate::ORDER_PREFIX))?;
    rest.rsplit(SEP).next().filter(|u| !u.is_empty())
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

/// The rows under `chain` on `axis` — one level of the dependency graph.
///
/// `chain` is the path already walked; its last element is the unit whose
/// relations are read. A chain of one is the level directly under a unit row.
pub async fn level(bus: &Bus, axis: Axis, chain: &[String]) -> Result<Vec<DepRow>> {
    let units = bus.list_units().await?;
    let Some(parent) = units.iter().find(|u| Some(&u.name) == chain.last()) else {
        // The unit the level was opened on is not loaded — it has no object to
        // read relations from, so the level is empty rather than an error.
        return Ok(Vec::new());
    };
    let props = bus.properties(&parent.path, UNIT_IFACE).await;
    // Owned words, not the static ones: a closure that captured a borrow here
    // would have to be generic over its lifetime, which a boxed level future
    // cannot be.
    let named: Vec<(String, String)> = related(&props, axis.sources())
        .into_iter()
        .map(|(word, name)| (word.to_string(), name))
        .collect();
    let deeper = axis.recurses() && chain.len() < MAX_DEPTH;

    let rows = stream::iter(named)
        .map(|(word, name)| {
            let entry = units.iter().find(|u| u.name == name);
            async move {
                let cycle = chain.contains(&name);
                let has_children = match entry {
                    Some(e) if deeper && !cycle => has_any(bus, &e.path, axis).await,
                    _ => false,
                };
                row(chain, axis, &word, name, entry, cycle, has_children)
            }
        })
        .buffer_unordered(crate::bus::MAX_INFLIGHT)
        .collect::<Vec<DepRow>>()
        .await;
    Ok(rows)
}

/// One dependency row by its id — the `get_by_id` half.
///
/// Rebuilt by listing the level it belongs to rather than by reading the unit
/// alone: a row's relation is a fact about its *parent*, not about itself, and
/// so is the chain it hangs in.
pub async fn row_by_id(bus: &Bus, id: &str) -> Result<DepRow> {
    let (axis, chain) = parse(id)
        .filter(|(_, chain)| chain.len() > 1)
        .ok_or_else(|| ContentError::NotFound(format!("malformed dependency id {id}")))?;
    let parent = &chain[..chain.len() - 1];
    level(bus, axis, parent)
        .await?
        .into_iter()
        .find(|r| r.chain == chain)
        .ok_or_else(|| ContentError::NotFound(format!("no dependency {id}")))
}

/// Whether a unit's own properties name anything it needs.
///
/// The unit levels answer this for their own rows out of the property map they
/// already hold, which is what puts an expand arrow on a service row — or
/// keeps it off one that depends on nothing.
pub fn has_needs(unit: &Props) -> bool {
    NEEDS
        .iter()
        .any(|(prop, _)| !strings(unit, prop).is_empty())
}

/// Whether a unit names anything on this axis — the one question the
/// lookahead asks.
async fn has_any(bus: &Bus, path: &OwnedObjectPath, axis: Axis) -> bool {
    let props = bus.properties(path, UNIT_IFACE).await;
    axis.sources()
        .iter()
        .any(|(prop, _)| !strings(&props, prop).is_empty())
}

/// The units one axis's properties name, each with its relation word, the
/// strongest relation winning where a unit appears in more than one.
fn related(
    props: &Props,
    sources: &'static [(&'static str, &'static str)],
) -> Vec<(&'static str, String)> {
    let named: Vec<(&'static str, Vec<String>)> = sources
        .iter()
        .map(|(prop, word)| (*word, strings(props, prop)))
        .collect();
    dedup(&named)
}

/// Flatten the per-relation lists into rows, dropping a unit that a stronger
/// relation already claimed.
fn dedup(named: &[(&'static str, Vec<String>)]) -> Vec<(&'static str, String)> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for (word, names) in named {
        for name in names {
            if seen.insert(name.as_str()) {
                out.push((*word, name.clone()));
            }
        }
    }
    out
}

/// Assemble one row from what the listing knows about the named unit.
fn row(
    chain: &[String],
    axis: Axis,
    word: &str,
    name: String,
    entry: Option<&UnitEntry>,
    cycle: bool,
    has_children: bool,
) -> DepRow {
    let mut own = chain.to_vec();
    own.push(name);
    DepRow {
        chain: own,
        axis,
        relation: if cycle {
            format!("{word} (cycle)")
        } else {
            word.to_string()
        },
        active: entry.map(|e| e.active_state.clone()).unwrap_or_default(),
        sub: entry.map(|e| e.sub_state.clone()).unwrap_or_default(),
        load: entry.map(|e| e.load_state.clone()).unwrap_or_default(),
        description: entry.map(|e| e.description.clone()).unwrap_or_default(),
        has_children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(units: &[&str]) -> Vec<String> {
        units.iter().map(|u| u.to_string()).collect()
    }

    #[test]
    fn an_id_carries_the_whole_path_and_comes_back_as_one() {
        let c = chain(&["a.service", "b.target", "c.socket"]);
        let id = encode(Axis::Needs, &c);
        assert_eq!(id, "dep:a.service>b.target>c.socket");
        assert_eq!(parse(&id), Some((Axis::Needs, c)));
        assert_eq!(unit_of(&id), Some("c.socket"));
    }

    #[test]
    fn the_two_axes_do_not_answer_to_each_others_ids() {
        let c = chain(&["a.service", "b.target"]);
        let ordered = encode(Axis::Order, &c);
        assert_eq!(ordered, "order:a.service>b.target");
        assert_eq!(parse(&ordered).map(|(a, _)| a), Some(Axis::Order));
        assert_eq!(parse("service:a.service"), None);
        assert_eq!(unit_of("service:a.service"), None);
    }

    #[test]
    fn a_unit_named_twice_takes_its_strongest_relation() {
        let named = vec![
            ("requires", chain(&["b.target"])),
            ("wants", chain(&["b.target", "c.socket"])),
        ];
        assert_eq!(
            dedup(&named),
            vec![
                ("requires", "b.target".to_string()),
                ("wants", "c.socket".to_string()),
            ]
        );
    }

    #[test]
    fn a_unit_that_is_already_in_its_own_chain_is_shown_and_marked() {
        let c = chain(&["a.service", "b.target"]);
        let looped = row(
            &c,
            Axis::Needs,
            "requires",
            "a.service".into(),
            None,
            true,
            false,
        );
        assert_eq!(looped.relation, "requires (cycle)");
        assert!(!looped.has_children, "a cycle must not offer to go deeper");
        assert_eq!(looped.name(), "a.service");
        assert_eq!(looped.summary().has_children, Some(false));
    }

    #[test]
    fn a_unit_the_manager_never_loaded_keeps_its_name_and_nothing_else() {
        let c = chain(&["a.service"]);
        let missing = row(
            &c,
            Axis::Needs,
            "requires",
            "gone.service".into(),
            None,
            false,
            false,
        );
        assert_eq!(missing.name(), "gone.service");
        assert!(missing.active.is_empty() && missing.load.is_empty());
    }
}
