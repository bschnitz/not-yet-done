//! Calendar domain API — one of the typed "outward interfaces" a session
//! exposes. Obtained via [`SessionHandle::calendar`](crate::SessionHandle).
//!
//! The contract with the flow lives here too: what the run is told about the
//! range, and what it is expected to yield.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::browser::Run;
use crate::dto::{MsCalEvent, MsTimeRange};
use crate::error::MsOfficeError;
use crate::session::SessionInner;

/// The data names the flow reads the range under: `{{data.from}}` and `{{data.until}}`,
/// each an RFC 3339 instant. The flow reads them at day granularity — the day `from` falls
/// on is the first, the day `until` falls on is the first one *not* listed.
pub(crate) const FROM: &str = "from";
pub(crate) const UNTIL: &str = "until";

/// The name the flow yields the calendar under. An object whose `events` is the list of
/// events, each in the shape [`MsCalEvent`] deserializes.
pub(crate) const YIELD: &str = "calendar";

/// Read access to the account's calendar over the Office 365 web surface.
pub struct CalendarApi {
    inner: Arc<SessionInner>,
}

impl CalendarApi {
    pub(crate) fn new(inner: Arc<SessionInner>) -> Self {
        Self { inner }
    }

    /// All events overlapping `range`. Runs the session's flow for it; on a
    /// profile that is not signed in, the run signs on first, telling and
    /// asking whoever attends the session (see
    /// [`SessionHandle::take_prompts`](crate::SessionHandle::take_prompts))
    /// what the second factor needs.
    pub async fn get_view(&self, range: MsTimeRange) -> Result<Vec<MsCalEvent>, MsOfficeError> {
        self.inner.get_calendar_view(range).await
    }
}

/// What a run for `range` is about, as `flow_run` carries it.
pub(crate) fn data_for(range: &MsTimeRange) -> BTreeMap<String, String> {
    BTreeMap::from([
        (FROM.to_string(), range.start.to_rfc3339()),
        (UNTIL.to_string(), range.end.to_rfc3339()),
    ])
}

/// The events out of a finished run — or why there are none.
pub(crate) fn events_of(run: Run) -> Result<Vec<MsCalEvent>, MsOfficeError> {
    if run.stopped {
        return Err(MsOfficeError::Run("somebody stopped the run".into()));
    }
    if !run.tally.clean() {
        return Err(MsOfficeError::Run(run.trouble.unwrap_or_else(|| {
            format!(
                "{} of {} steps failed",
                run.tally.failed + run.tally.broke,
                run.tally.ran
            )
        })));
    }
    let calendar = run
        .yielded
        .get(YIELD)
        .ok_or_else(|| MsOfficeError::Protocol(format!("the run yielded no `{YIELD}`")))?;
    let events = calendar
        .get("events")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    serde_json::from_value(events)
        .map_err(|e| MsOfficeError::Protocol(format!("bad calendar events: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::Tally;
    use serde_json::json;

    fn run(yielded: Value) -> Run {
        Run {
            tally: Tally {
                ran: 3,
                failed: 0,
                broke: 0,
            },
            stopped: false,
            yielded: serde_json::from_value(yielded).unwrap(),
            trouble: None,
        }
    }

    #[test]
    fn events_come_out_of_the_calendar_yield() {
        let events = events_of(run(json!({
            "calendar": { "asked": 1, "events": [
                { "id": "a", "subject": "Standup", "start": "2026-09-07T07:00:00Z",
                  "end": "2026-09-07T07:15:00Z", "showAs": "busy" }
            ] }
        })))
        .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].subject.as_deref(), Some("Standup"));
        assert_eq!(events[0].show_as, crate::MsShowAs::Busy);
    }

    #[test]
    fn a_run_that_failed_is_its_own_sentence() {
        let mut failed = run(json!({}));
        failed.tally.failed = 1;
        failed.trouble = Some("give up: the password was refused".into());
        match events_of(failed) {
            Err(MsOfficeError::Run(why)) => assert!(why.contains("password"), "{why}"),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn a_clean_run_without_the_yield_is_a_protocol_error() {
        assert!(matches!(
            events_of(run(json!({}))),
            Err(MsOfficeError::Protocol(_))
        ));
    }

    #[test]
    fn the_range_travels_as_from_and_until() {
        let range = MsTimeRange::new(
            "2026-09-07T00:00:00Z".parse().unwrap(),
            "2026-09-28T00:00:00Z".parse().unwrap(),
        );
        let data = data_for(&range);
        assert_eq!(data["from"], "2026-09-07T00:00:00+00:00");
        assert_eq!(data["until"], "2026-09-28T00:00:00+00:00");
    }
}
