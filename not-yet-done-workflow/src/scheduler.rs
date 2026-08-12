//! The background **trigger scheduler** (Phase 6c).
//!
//! A workflow's frontmatter may declare [`Trigger`]s — reasons to start a run
//! without a person asking. This module spawns one Tokio task per adapter
//! instance that watches those triggers and, when one fires, starts a run and
//! drives it as far as it can go automatically (see
//! [`crate::adapter::trigger_run`]).
//!
//! Two kinds of trigger, watched in a single `select!` loop:
//!
//! * [`Trigger::Cron`] — a standard 5-field cron expression (via the
//!   [`croner`] crate), evaluated in **local** time. The loop sleeps until the
//!   soonest upcoming occurrence across all cron triggers (capped at
//!   [`MAX_SLEEP`] so a suspended/clock-jumped machine recovers), then fires
//!   every trigger whose occurrence has passed.
//! * [`Trigger::Event`] — fires when a [`BusEvent`] whose `topic` matches is
//!   seen on the host event bus.
//!
//! Definitions are re-read from disk each pass, so editing a `.md` changes the
//! schedule on the next wake. The task is only spawned when at least one trigger
//! exists at startup; adding the *first* trigger to an instance therefore needs
//! a restart, but changing existing ones does not.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local};
use croner::Cron;
use not_yet_done_content::{subscribe_events, BusEvent, HostEventBus};
use tokio::sync::broadcast::error::RecvError;

use crate::adapter::{collect_triggers, trigger_run, Ctx};
use crate::model::Trigger;

/// The longest the loop sleeps between cron re-evaluations, even when the next
/// occurrence is further off. Caps drift after suspend / clock jumps, mirroring
/// the calendar adapter's reminder scheduler.
const MAX_SLEEP: Duration = Duration::from_secs(60);

/// Spawn the trigger scheduler for one adapter instance. A no-op when there is
/// no Tokio runtime (a sync test harness) or the instance declares no triggers.
pub(crate) fn spawn(ctx: Ctx, event_bus: Arc<dyn HostEventBus>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if collect_triggers(&ctx).is_empty() {
        return;
    }
    handle.spawn(run_loop(ctx, event_bus));
}

/// The scheduler's `select!` loop: wake on the soonest cron occurrence or on a
/// bus event, fire the matching triggers, repeat until the bus closes.
async fn run_loop(ctx: Ctx, event_bus: Arc<dyn HostEventBus>) {
    let mut events = subscribe_events(&*event_bus);
    let mut last = Local::now();
    loop {
        let triggers = collect_triggers(&ctx);
        let now = Local::now();
        let sleep_for = match next_cron_fire(&triggers, last) {
            Some(dt) => (dt - now).to_std().unwrap_or(Duration::ZERO).min(MAX_SLEEP),
            None => MAX_SLEEP,
        };

        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => {
                let now = Local::now();
                fire_due_cron(&ctx, &triggers, last, now);
                last = now;
            }
            recv = events.recv() => match recv {
                Ok(event) => {
                    if let Some(bus_event) = BusEvent::from_host_event(&event) {
                        fire_event_matches(&ctx, &triggers, &bus_event.topic);
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
        }
    }
}

/// The soonest upcoming occurrence strictly after `after`, across all cron
/// triggers. `None` when there are no (parseable) cron triggers.
fn next_cron_fire(
    triggers: &[(String, Trigger)],
    after: DateTime<Local>,
) -> Option<DateTime<Local>> {
    triggers
        .iter()
        .filter_map(|(_, t)| cron_expr(t))
        .filter_map(|expr| next_occurrence(expr, after))
        .min()
}

/// Fire every cron trigger whose next occurrence after `last` has passed by
/// `now`. Each fires an independent run, spawned so a slow run never stalls the
/// loop; the run's own visit guard bounds any routed loop.
fn fire_due_cron(
    ctx: &Ctx,
    triggers: &[(String, Trigger)],
    last: DateTime<Local>,
    now: DateTime<Local>,
) {
    for (name, trigger) in triggers {
        let Some(expr) = cron_expr(trigger) else {
            continue;
        };
        if let Some(dt) = next_occurrence(expr, last) {
            if dt <= now {
                fire(ctx, name);
            }
        }
    }
}

/// Fire every event trigger whose topic matches `topic`.
fn fire_event_matches(ctx: &Ctx, triggers: &[(String, Trigger)], topic: &str) {
    for (name, trigger) in triggers {
        if matches!(trigger, Trigger::Event(t) if t == topic) {
            fire(ctx, name);
        }
    }
}

/// Start (and drive) a run of `name` on a detached task, so the scheduler loop
/// stays responsive while an `auto`/`ai` run executes. Errors are best-effort:
/// a run that fails to start is dropped rather than crashing the scheduler.
fn fire(ctx: &Ctx, name: &str) {
    let ctx = ctx.clone();
    let name = name.to_string();
    tokio::spawn(async move {
        let _ = trigger_run(&ctx, &name).await;
    });
}

/// The cron expression of a [`Trigger::Cron`], or `None` for an event trigger.
fn cron_expr(trigger: &Trigger) -> Option<&str> {
    match trigger {
        Trigger::Cron(expr) => Some(expr),
        Trigger::Event(_) => None,
    }
}

/// The next occurrence of a cron `expr` strictly after `after`, in local time.
/// A malformed expression yields `None` (skipped, not fatal).
fn next_occurrence(expr: &str, after: DateTime<Local>) -> Option<DateTime<Local>> {
    Cron::from_str(expr)
        .ok()?
        .find_next_occurrence(&after, false)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn cron(name: &str, expr: &str) -> (String, Trigger) {
        (name.to_string(), Trigger::Cron(expr.to_string()))
    }

    #[test]
    fn next_cron_fire_picks_the_soonest_valid_occurrence() {
        // 2026-07-20 10:30:00 local.
        let after = Local.with_ymd_and_hms(2026, 7, 20, 10, 30, 0).unwrap();
        let triggers = vec![
            cron("daily", "0 2 * * *"),   // next: tomorrow 02:00
            cron("hourly", "0 * * * *"),  // next: today 11:00
            cron("broken", "not a cron"), // ignored
        ];
        let next = next_cron_fire(&triggers, after).unwrap();
        assert_eq!(next, Local.with_ymd_and_hms(2026, 7, 20, 11, 0, 0).unwrap());
    }

    #[test]
    fn no_cron_triggers_yields_none() {
        let triggers = vec![("e".to_string(), Trigger::Event("ci:push".into()))];
        assert!(next_cron_fire(&triggers, Local::now()).is_none());
    }

    #[test]
    fn malformed_cron_is_skipped_not_fatal() {
        assert!(next_occurrence("wat", Local::now()).is_none());
        assert!(next_occurrence("0 2 * * *", Local::now()).is_some());
    }
}
