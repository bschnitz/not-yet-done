//! Own timesheets the listing leaves out.
//!
//! `GET /api/timesheets` drops records whose project or activity the user's
//! teams do not reach (a closed project, typically), even the user's own;
//! `GET /api/timesheets/{id}` still serves them. The [`index`](super::index)
//! remembers every own record a listing returned, so one that drops out later
//! is still known by id: [`complete_listing`] adds such records back to each
//! listing, and [`scan`] finds the ones that were never listed by trying the
//! ids between the listed ones.

use std::collections::HashSet;
use std::sync::Arc;

use futures::stream::{self, StreamExt};

use super::index::IndexStore;
use crate::client::{KimaiClient, KimaiTimesheet, TimesheetProbe};

/// Requests in flight at once while probing ids. Kimai takes one to two
/// seconds per request (the token check dominates); more in parallel started
/// stalling single requests.
const PROBE_CONCURRENCY: usize = 8;

/// Ids per round of a scan; the index is written after each, so an
/// interrupted scan keeps what it found.
const SCAN_CHUNK: usize = 200;

/// The project and activity ids the API's lookups reach. A record on one
/// outside them is a record the listing leaves out.
pub(super) struct Reach {
    projects: HashSet<u64>,
    activities: HashSet<u64>,
}

impl Reach {
    pub(super) fn new(
        projects: impl IntoIterator<Item = u64>,
        activities: impl IntoIterator<Item = u64>,
    ) -> Self {
        Self {
            projects: projects.into_iter().collect(),
            activities: activities.into_iter().collect(),
        }
    }

    pub(super) fn has_project(&self, id: u64) -> bool {
        self.projects.contains(&id)
    }

    pub(super) fn has_activity(&self, id: u64) -> bool {
        self.activities.contains(&id)
    }

    pub(super) fn hides(&self, ts: &KimaiTimesheet) -> bool {
        !self.has_project(ts.project) || !self.has_activity(ts.activity)
    }
}

/// Record a listing in the index and return the own records it left out that
/// `wanted` (the query's server filters, judged locally) would have kept.
///
/// A known record missing from the listing is either hidden — then it comes
/// from the index as is — or was visible when last seen; that one is fetched
/// once on its own to tell whether it became hidden, changed, or is gone.
pub(super) async fn complete_listing(
    client: &Arc<KimaiClient>,
    store: &IndexStore,
    visible: &[KimaiTimesheet],
    reach: &Reach,
    wanted: impl Fn(&KimaiTimesheet) -> bool,
) -> Result<Vec<KimaiTimesheet>, String> {
    let index = store.read().await?;
    let listed: HashSet<u64> = visible.iter().map(|ts| ts.id).collect();

    let mut added = Vec::new();
    let mut recheck = Vec::new();
    for ts in index.entries.values() {
        if listed.contains(&ts.id) || !wanted(ts) {
            continue;
        }
        if reach.hides(ts) {
            added.push(ts.clone());
        } else {
            recheck.push(ts.id);
        }
    }

    let mut refreshed = Vec::new();
    let mut gone = Vec::new();
    for (id, probe) in probe_all(client, recheck).await {
        match probe {
            Ok(TimesheetProbe::Found(ts)) => {
                if reach.hides(&ts) && wanted(&ts) {
                    added.push(ts.clone());
                }
                refreshed.push(ts);
            }
            Ok(TimesheetProbe::Forbidden | TimesheetProbe::Missing) => gone.push(id),
            // Unanswered: keep the record for the next listing to try again.
            Err(_) => {}
        }
    }

    let mut seen = visible.to_vec();
    seen.extend(refreshed);
    store
        .update(move |index| {
            for id in gone {
                index.entries.remove(&id);
            }
            for ts in seen {
                index.entries.insert(ts.id, ts);
            }
        })
        .await?;
    Ok(added)
}

/// What a [`scan`] did.
#[derive(Debug, Default)]
pub(super) struct ScanReport {
    /// Ids asked for one by one.
    pub probed: usize,
    /// Of those, the ones that got no answer; the next scan tries them again.
    pub failed: usize,
    /// Own records the listing leaves out, from `from` on, after the scan.
    pub hidden: usize,
    /// Their summed duration, in seconds.
    pub hidden_secs: i64,
}

/// Find the own records from `from` on that the listing leaves out, by
/// fetching every id between the listed ones that the index does not account
/// for yet, and re-fetch the known ones.
///
/// `listing` is the own listing from somewhat before `from` on: the last
/// record listed before `from` bounds the ids from below (records are
/// numbered as they are created), the newest one from above. Ids that turn
/// out to be someone else's or deleted are remembered, so a second scan only
/// tries what is new or went unanswered.
pub(super) async fn scan(
    client: &Arc<KimaiClient>,
    store: &IndexStore,
    reach: &Reach,
    from: &str,
    listing: Vec<KimaiTimesheet>,
) -> Result<ScanReport, String> {
    let me = client.me().await?;
    let begins_before = |ts: &KimaiTimesheet| local(&ts.begin) < from;
    let floor = listing
        .iter()
        .filter(|ts| begins_before(ts))
        .map(|ts| ts.id + 1)
        .max()
        .or_else(|| listing.iter().map(|ts| ts.id).min());
    let ceiling = listing.iter().map(|ts| ts.id).max();
    let listed: HashSet<u64> = listing.iter().map(|ts| ts.id).collect();
    store
        .update(move |index| {
            for ts in listing {
                index.entries.insert(ts.id, ts);
            }
        })
        .await?;

    let index = store.read().await?;
    let mut ids: Vec<u64> = match (floor, ceiling) {
        (Some(lo), Some(hi)) => (lo..=hi)
            .filter(|id| !index.entries.contains_key(id) && !index.foreign.contains(id))
            .collect(),
        _ => Vec::new(),
    };
    ids.extend(
        index
            .entries
            .values()
            .filter(|ts| !listed.contains(&ts.id) && !begins_before(ts))
            .map(|ts| ts.id),
    );
    ids.sort_unstable();
    ids.dedup();

    let mut report = ScanReport {
        probed: ids.len(),
        ..ScanReport::default()
    };
    for chunk in ids.chunks(SCAN_CHUNK) {
        let mut found = Vec::new();
        let mut foreign = Vec::new();
        for (id, probe) in probe_all(client, chunk.to_vec()).await {
            match probe {
                Ok(TimesheetProbe::Found(ts)) if ts.user.is_none_or(|u| u == me) => found.push(ts),
                Ok(_) => foreign.push(id),
                Err(_) => report.failed += 1,
            }
        }
        store
            .update(move |index| {
                for id in foreign {
                    index.entries.remove(&id);
                    index.foreign.insert(id);
                }
                for ts in found {
                    index.entries.insert(ts.id, ts);
                }
            })
            .await?;
    }

    let index = store.read().await?;
    for ts in index.entries.values() {
        if !begins_before(ts) && reach.hides(ts) {
            report.hidden += 1;
            report.hidden_secs += ts.duration.unwrap_or(0);
        }
    }
    Ok(report)
}

/// Fetch every id on its own, a few at a time.
async fn probe_all(
    client: &Arc<KimaiClient>,
    ids: Vec<u64>,
) -> Vec<(u64, Result<TimesheetProbe, String>)> {
    stream::iter(ids)
        .map(|id| async move { (id, client.probe_timesheet(id).await) })
        .buffer_unordered(PROBE_CONCURRENCY)
        .collect()
        .await
}

/// The local `YYYY-MM-DDTHH:MM:SS` part of a Kimai timestamp, comparable as a
/// string with the query bounds.
fn local(begin: &str) -> &str {
    begin.get(..19).unwrap_or(begin)
}
