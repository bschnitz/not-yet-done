//! Live rows: the manager's own signals, turned into invalidations.
//!
//! Everything before this phase was a pull — a level is what the manager said
//! when the load asked. That is wrong for exactly the moment the user cares
//! about: they press `s` on a service and then watch the row, and a row that
//! only moves when something reloads it is a row that lies while they look at
//! it.
//!
//! # What the bus actually costs
//!
//! The fear this phase was planned around — "`PropertiesChanged` for every unit
//! is a lot of traffic for rows nobody is looking at" — did not survive being
//! measured. On a user manager with 421 loaded units and `Subscribe()` active,
//! an **idle** bus emits *nothing at all*: the traffic is proportional to state
//! *changes*, not to the number of units. One unit going up and down again
//! produced 23 signals over 7 object paths, of which 5 were `PropertiesChanged`
//! for the unit itself.
//!
//! So there is no narrow subscription to maintain and no on-screen bookkeeping
//! to keep in sync with the panes. There is one subscription, and the only
//! thing worth doing is **coalescing**: those 5 signals are one state change,
//! and a row that is rebuilt 5 times is 4 round trips and 4 repaints wasted.
//!
//! # Why the fire re-reads the unit
//!
//! Also measured: the `PropertiesChanged` payload carries `ActiveState`,
//! `SubState`, `MainPID`, `Result`, `NRestarts` and the state timestamps — but
//! **not** `Description`, `LoadState`, `UnitFileState` or `MemoryCurrent`. It
//! is half a row, and half a row cannot be pushed as [`Invalidation::Row`],
//! which is by contract the row's *complete* new state. Patching a cached row
//! instead would mean keeping a cache of every level's last listing and
//! answering "what did this row look like before" — state that can go stale in
//! ways nothing detects.
//!
//! Reading the one unit's two property maps costs two local round trips and
//! needs no memory at all, so that is what the fire does: **one D-Bus call per
//! real state change, and none while nothing happens.**
//!
//! # The three streams
//!
//! * `PropertiesChanged` under `/org/freedesktop/systemd1/unit/` → one
//!   [`Invalidation::Row`] per unit that settled. Rows only; the pane keeps its
//!   selection and its scroll position.
//! * `UnitNew` / `UnitRemoved` for a `.service` or `.timer` → the level gained
//!   or lost a row, which no `Row` can express, so the open levels are asked to
//!   reload. Held for a second first: a desktop session starts and collects
//!   units in bursts.
//!
//!   [`Invalidation::All`] rather than the narrower [`Invalidation::Node`],
//!   which would be the obvious fit: `Node` reaches a pane whose *parent* is
//!   that node, and the three unit levels sit at the root of their view, where
//!   the frontend has no parent frame to compare against. There is nothing to
//!   address them by that is narrower than "this adapter".
//! * `Reloading` → [`Invalidation::All`]. A `daemon-reload` can change every
//!   unit's `LoadState` and `UnitFileState` at once, and re-reading 400 units
//!   one signal at a time would be slower than the reload the user asked for.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::StreamExt;
use not_yet_done_content::{Invalidation, Result};
use tokio::sync::broadcast;
use tokio::time::Instant;
use zbus_systemd::systemd1::ManagerProxy;
use zbus_systemd::zbus;
use zbus_systemd::zvariant::OwnedObjectPath;

use crate::bus::{Bus, SERVICE_IFACE, TIMER_IFACE, UNIT_IFACE};
use crate::model::{ServiceRow, TimerRow, boot_instant, entry_from_properties};

/// How long a unit's signals are allowed to settle before its row is rebuilt.
///
/// One transition emits about five `PropertiesChanged` in a few milliseconds
/// (`activating` → `active`, then the PID, then the timestamps). 150 ms is
/// long enough that they arrive as one change and short enough that the row
/// still moves while the key the user pressed is under their finger.
const COALESCE: Duration = Duration::from_millis(150);

/// How long appearing and disappearing units are collected before the levels
/// are asked to reload.
///
/// Longer than [`COALESCE`], because this one costs a whole level rather than a
/// row, and because units arrive in bursts — a desktop session logging in
/// brings up dozens of them inside a second.
const RESTRUCTURE: Duration = Duration::from_secs(1);

/// How long the loop parks when there is nothing due. Not a poll interval —
/// nothing is checked when it expires; every real wake-up comes from a signal.
const IDLE_PARK: Duration = Duration::from_secs(3600);

/// Where every unit object lives. Matched as a namespace, so one rule covers
/// every unit the manager has.
const UNIT_PATH_NAMESPACE: &str = "/org/freedesktop/systemd1/unit";

/// The bus name the signals must come from. Without it the rule would also
/// match a signal any other client on the session bus chose to emit under that
/// path.
const SYSTEMD_SERVICE: &str = "org.freedesktop.systemd1";

/// Start the watcher, once, for the lifetime of the adapter.
///
/// Detached rather than owned: it has nothing to return, nothing to cancel it
/// from, and it ends by itself when the connection does. A watcher that cannot
/// start leaves the adapter exactly as it was before this phase — pull-only,
/// with `r` still reloading — so the failure is reported where it can be seen
/// (`NYD_DEBUG_SYSTEMD=1`) rather than turned into a failed adapter, which
/// would take the working half of the tab down with it.
pub fn spawn(bus: Arc<Bus>, inv: broadcast::Sender<Invalidation>) {
    tokio::spawn(async move {
        if let Err(e) = run(bus, inv).await {
            debug(format_args!("the live watcher stopped: {e}"));
        }
    });
}

fn debug(what: std::fmt::Arguments<'_>) {
    if std::env::var_os("NYD_DEBUG_SYSTEMD").is_some() {
        eprintln!("systemd: {what}");
    }
}

async fn run(bus: Arc<Bus>, inv: broadcast::Sender<Invalidation>) -> Result<()> {
    let conn = bus.connection_handle().await?;
    // Without this the manager stays quiet — it emits unit and job signals only
    // to clients that asked. Idempotent; `run_job` may have asked already.
    bus.subscribe_signals().await?;

    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(SYSTEMD_SERVICE)
        .and_then(|b| b.interface("org.freedesktop.DBus.Properties"))
        .and_then(|b| b.member("PropertiesChanged"))
        .and_then(|b| b.path_namespace(UNIT_PATH_NAMESPACE))
        .map_err(|e| other(format!("building the unit match rule: {e}")))?
        .build();
    let mut changed = zbus::MessageStream::for_match_rule(rule, &conn, Some(256))
        .await
        .map_err(|e| other(format!("watching unit properties: {e}")))?;

    let proxy = ManagerProxy::new(&conn)
        .await
        .map_err(|e| other(format!("opening the manager proxy: {e}")))?;
    let mut appeared = proxy
        .receive_unit_new()
        .await
        .map_err(|e| other(format!("watching new units: {e}")))?;
    let mut vanished = proxy
        .receive_unit_removed()
        .await
        .map_err(|e| other(format!("watching removed units: {e}")))?;
    let mut reloading = proxy
        .receive_reloading()
        .await
        .map_err(|e| other(format!("watching reloads: {e}")))?;

    // The unit paths whose rows are due to be rebuilt, and when.
    let mut pending: HashMap<OwnedObjectPath, Instant> = HashMap::new();
    // Set once a unit appeared or vanished; cleared when the levels are told.
    let mut restructure: Option<Instant> = None;

    loop {
        // Far enough out that the timer never fires on its own, but a plain
        // `sleep_until` rather than a branch that can be disabled — `select!`
        // needs a future in every arm, and an idle bus must simply park here.
        let wake = earliest(&pending, restructure).unwrap_or_else(|| Instant::now() + IDLE_PARK);
        tokio::select! {
            msg = changed.next() => match msg {
                Some(Ok(msg)) => {
                    let header = msg.header();
                    // Filter on the path, not on what the read turns up: the
                    // unit name is escaped into the path (`.` becomes `_2e`),
                    // so a `.scope` or a `.socket` — the two that churn on a
                    // desktop — is rejected before it costs a round trip.
                    if let Some(path) = header.path().filter(|p| interface_for(p.as_str()).is_some()) {
                        pending.insert(path.to_owned().into(), Instant::now() + COALESCE);
                    }
                }
                // A message we could not parse says nothing about the next one.
                Some(Err(e)) => debug(format_args!("a unit signal did not parse: {e}")),
                None => break,
            },
            signal = appeared.next() => match signal {
                Some(s) => {
                    if s.args().is_ok_and(|a| is_watched_unit(a.id())) {
                        restructure.get_or_insert(Instant::now() + RESTRUCTURE);
                    }
                }
                None => break,
            },
            signal = vanished.next() => match signal {
                Some(s) => {
                    if s.args().is_ok_and(|a| is_watched_unit(a.id())) {
                        restructure.get_or_insert(Instant::now() + RESTRUCTURE);
                    }
                }
                None => break,
            },
            signal = reloading.next() => match signal {
                Some(s) => {
                    // Fired twice per reload: `true` when it starts, `false`
                    // when it is done. Only the second one has a new picture
                    // to show.
                    if s.args().is_ok_and(|a| !a.active()) {
                        let _ = inv.send(Invalidation::All);
                        // Every level is about to be reloaded anyway.
                        pending.clear();
                        restructure = None;
                    }
                }
                None => break,
            },
            _ = tokio::time::sleep_until(wake) => {
                let now = Instant::now();
                if restructure.is_some_and(|at| at <= now) {
                    restructure = None;
                    let _ = inv.send(Invalidation::All);
                }
                let due: Vec<OwnedObjectPath> = pending
                    .iter()
                    .filter(|(_, at)| **at <= now)
                    .map(|(path, _)| path.clone())
                    .collect();
                for path in due {
                    pending.remove(&path);
                    // Nobody is listening in a CLI run, and a read whose result
                    // has nowhere to go is a round trip for nothing.
                    if inv.receiver_count() == 0 {
                        continue;
                    }
                    // Spawned, not awaited: the loop must keep draining the
                    // stream while this one unit is being read, or a burst
                    // would serialise behind it.
                    tokio::spawn(push_row(bus.clone(), inv.clone(), path));
                }
            }
        }
    }
    Ok(())
}

/// The earliest thing that wants doing, if anything does.
fn earliest(
    pending: &HashMap<OwnedObjectPath, Instant>,
    restructure: Option<Instant>,
) -> Option<Instant> {
    pending.values().copied().chain(restructure).min()
}

/// Which type-specific interface a unit object path belongs to — and, by
/// returning `None`, whether this adapter shows that kind of unit at all.
///
/// systemd escapes the unit name into the path, encoding every character
/// outside `[A-Za-z0-9_]` as `_xx`, so `sshd.service` lives at
/// `…/unit/sshd_2eservice`. That makes the suffix as reliable a type test as
/// the name is, and it is available without asking the bus anything.
fn interface_for(path: &str) -> Option<&'static str> {
    if path.ends_with("_2eservice") {
        Some(SERVICE_IFACE)
    } else if path.ends_with("_2etimer") {
        Some(TIMER_IFACE)
    } else {
        None
    }
}

/// Whether a unit *name* is one of the two kinds that have a level.
fn is_watched_unit(name: &str) -> bool {
    name.ends_with(".service") || name.ends_with(".timer")
}

/// Read one unit and push its row.
///
/// Two `GetAll` calls, the same pair the level's own load makes per unit, so a
/// row that arrives this way is built by exactly the code that built it the
/// first time — there is no second definition of what a service row is that
/// could drift from the first.
async fn push_row(bus: Arc<Bus>, inv: broadcast::Sender<Invalidation>, path: OwnedObjectPath) {
    let Some(iface) = interface_for(path.as_str()) else {
        return;
    };
    let unit = bus.properties(&path, UNIT_IFACE).await;
    // The unit is gone — it finished and the manager collected it between the
    // signal and this read. `UnitRemoved` is on its way and will reload the
    // level; a row built from an empty map would be a row of blanks.
    let Some(entry) = entry_from_properties(&path, &unit) else {
        return;
    };
    let own = bus.properties(&path, iface).await;
    let summary = if iface == SERVICE_IFACE {
        ServiceRow::build(&entry, &unit, &own).summary()
    } else {
        TimerRow::build(&entry, &unit, &own, boot_instant()).summary(Utc::now())
    };
    let _ = inv.send(Invalidation::Row(summary));
}

fn other(msg: String) -> not_yet_done_content::ContentError {
    not_yet_done_content::ContentError::Other(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Manager;

    #[test]
    fn the_path_suffix_tells_the_two_kinds_apart_from_everything_else() {
        let unit = "/org/freedesktop/systemd1/unit";
        assert_eq!(
            interface_for(&format!("{unit}/sshd_2eservice")),
            Some(SERVICE_IFACE)
        );
        assert_eq!(
            interface_for(&format!("{unit}/logrotate_2etimer")),
            Some(TIMER_IFACE)
        );
        // The two that churn on a desktop, and the one that never has a row.
        assert_eq!(interface_for(&format!("{unit}/app_2dfoo_2escope")), None);
        assert_eq!(interface_for(&format!("{unit}/dbus_2esocket")), None);
        assert_eq!(interface_for(&format!("{unit}/_2d_2emount")), None);
    }

    #[test]
    fn only_the_two_kinds_with_a_level_trigger_a_reload() {
        assert!(is_watched_unit("backup-photos.service"));
        assert!(is_watched_unit("backup-photos.timer"));
        assert!(!is_watched_unit("session-3.scope"));
        assert!(!is_watched_unit("dbus.socket"));
    }

    #[test]
    fn the_loop_wakes_for_whichever_is_due_first() {
        let now = Instant::now();
        let path = OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/a_2eservice").unwrap();
        let mut pending = HashMap::new();
        assert_eq!(earliest(&pending, None), None);

        pending.insert(path, now + Duration::from_millis(150));
        assert_eq!(
            earliest(&pending, None),
            Some(now + Duration::from_millis(150))
        );
        // A restructure a second out must not pull the row rebuild forward…
        assert_eq!(
            earliest(&pending, Some(now + Duration::from_secs(1))),
            Some(now + Duration::from_millis(150))
        );
        // …and an empty pending map must not hide it either.
        assert_eq!(
            earliest(&HashMap::new(), Some(now + Duration::from_secs(1))),
            Some(now + Duration::from_secs(1))
        );
    }

    /// The whole phase, against the real bus: start a transient unit, and check
    /// that its row arrives by itself — coalesced, not once per signal.
    ///
    /// Ignored by default because it needs a running `--user` manager and
    /// creates a unit. Run it by hand:
    /// `cargo test -p not-yet-done-systemd-adapter -- --ignored --nocapture live`
    #[tokio::test]
    #[ignore = "needs a running --user manager; starts a transient unit"]
    async fn a_transient_unit_pushes_its_row_without_anyone_asking() {
        let unit = "nyd-live-smoke.service";
        let bus = Arc::new(Bus::new(Manager::User, Some(Duration::from_secs(10))));
        let (tx, mut rx) = broadcast::channel(64);
        spawn(bus, tx);
        // Let the watcher get its match rule registered before the unit runs;
        // a rule added afterwards would miss the signals it is meant to catch.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let started = std::process::Command::new("systemd-run")
            .args(["--user", "--unit", unit, "--", "/bin/sleep", "1"])
            .status()
            .expect("systemd-run");
        assert!(started.success(), "could not start {unit}");

        let mut rows = 0usize;
        let collecting = async {
            while let Ok(inv) = rx.recv().await {
                if let Invalidation::Row(summary) = inv {
                    if summary.id == format!("{}{unit}", crate::SERVICE_PREFIX) {
                        rows += 1;
                    }
                }
            }
        };
        // Long enough for the unit to start, run its second and be collected.
        let _ = tokio::time::timeout(Duration::from_secs(6), collecting).await;

        eprintln!("{rows} row invalidations for one start/stop cycle");
        assert!(rows > 0, "the unit's row never arrived");
        // It went up and it came down: two settled states, and the roughly five
        // `PropertiesChanged` each transition emits must not have become five
        // rows.
        assert!(
            rows <= 4,
            "{rows} rows for two transitions — not coalescing"
        );
    }
}
