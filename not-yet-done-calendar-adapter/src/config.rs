//! Adapter-level config: the set of calendar connections plus the polling /
//! time-window knobs. Each connection carries an opaque `config:` sub-tree the
//! adapter re-serialises and hands to the backend factory untouched — so the
//! adapter never learns any backend's config shape.

use chrono::{DateTime, Duration, Utc};
use fieldsmith::Buildable;
use serde::{Deserialize, Deserializer};

use not_yet_done_calendar_core::TimeRange;

/// Deserialise `reminder_lead_minutes` from either a bare scalar (`5`) or a
/// sequence (`[15, 5]`) into `Some(Vec)`. An empty sequence yields `Some([])`
/// (reminders off), distinct from an absent key (`None` → the default lead).
fn deserialize_leads<'de, D>(de: D) -> std::result::Result<Option<Vec<i64>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ScalarOrSeq {
        Scalar(i64),
        Seq(Vec<i64>),
    }
    Ok(match Option::<ScalarOrSeq>::deserialize(de)? {
        None => None,
        Some(ScalarOrSeq::Scalar(n)) => Some(vec![n]),
        Some(ScalarOrSeq::Seq(v)) => Some(v),
    })
}

/// How often the background task re-fetches every connection to detect
/// externally-made changes. Calendars change out-of-band and Graph offers no
/// usable push, so the adapter polls itself; 5 minutes trades freshness for a
/// gentle request rate.
pub(crate) const DEFAULT_POLL_INTERVAL_SECS: u64 = 300;
/// Default look-behind / look-ahead of the event window (days).
pub(crate) const DEFAULT_WINDOW_PAST_DAYS: i64 = 7;
pub(crate) const DEFAULT_WINDOW_FUTURE_DAYS: i64 = 30;
/// How many minutes before an event's start the adapter fires a
/// [`Reminder`](not_yet_done_content::Reminder), when the config names none.
/// One fire per lead, so `[15, 5]` reminds twice; the default is a single
/// 5-minute heads-up. The adapter owns *when* a reminder fires; the frontend
/// owns whether it wants one at all and what command runs. A future per-event
/// / filter policy can refine this.
pub(crate) fn default_reminder_leads() -> Vec<i64> {
    vec![5]
}

/// The adapter's `config:` block.
///
/// ```yaml
/// connections:
///   - id: work
///     backend: microsoft
///     config:
///       token: { type: command, script: get-work-token.sh }
///       name: "Work"
///   - id: side
///     backend: microsoft
///     config:
///       token: { type: command, script: get-side-token.sh }
///       name: "Side project"
/// poll_interval_secs: 300
/// window_past_days: 7
/// window_future_days: 30
/// ```
#[derive(Deserialize, Buildable, Debug)]
#[serde(deny_unknown_fields)]
pub struct CalendarConfig {
    /// One entry per calendar source. May mix backend types and multiple
    /// instances of the same backend — the whole point of this adapter.
    pub(crate) connections: Vec<ConnectionConfig>,
    #[serde(default)]
    pub(crate) poll_interval_secs: Option<u64>,
    #[serde(default)]
    pub(crate) window_past_days: Option<i64>,
    #[serde(default)]
    pub(crate) window_future_days: Option<i64>,
    /// Minutes before an event's start to fire a reminder — one entry per
    /// desired fire, so `[15, 5]` reminds fifteen *and* five minutes ahead.
    /// Accepts a bare scalar (`5`) or a sequence (`[15, 5]`). Whether reminders
    /// are *acted on* is the frontend's choice (its per-tab `reminder:` block);
    /// this only sets the adapter's lead times. Defaults to
    /// [`default_reminder_leads`].
    #[serde(default, deserialize_with = "deserialize_leads")]
    pub(crate) reminder_lead_minutes: Option<Vec<i64>>,
}

/// One connection entry. `config` is the backend-specific sub-tree, kept as an
/// opaque value so the adapter stays ignorant of it (re-serialised to YAML and
/// passed to [`CalendarBackendFactory::create`](not_yet_done_calendar_core::CalendarBackendFactory::create)).
#[derive(Deserialize, Buildable, Debug)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig {
    /// Stable id, unique within the adapter — namespaces this connection's
    /// event ids so ids from different connections never collide.
    pub(crate) id: String,
    /// Which backend speaks this connection's protocol (e.g. `microsoft`).
    pub(crate) backend: String,
    /// Opaque backend-specific sub-tree — the adapter never interprets it, so
    /// the schema treats it as a single leaf scalar rather than recursing.
    #[serde(default)]
    #[builder(leaf)]
    pub(crate) config: serde_yaml::Value,
}

/// The look-behind / look-ahead window, resolved to a concrete [`TimeRange`]
/// against a `now` instant at each listing / poll so it always tracks the
/// present.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Window {
    pub(crate) past_days: i64,
    pub(crate) future_days: i64,
}

impl Window {
    pub(crate) fn range(&self, now: DateTime<Utc>) -> TimeRange {
        TimeRange::new(
            now - Duration::days(self.past_days),
            now + Duration::days(self.future_days),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multi_connection_config() {
        let yaml = r#"
connections:
  - id: work
    backend: microsoft
    config:
      token: { type: command, script: a.sh }
      name: Work
  - id: side
    backend: microsoft
    config:
      token: { type: command, script: b.sh }
poll_interval_secs: 120
window_future_days: 14
"#;
        let cfg: CalendarConfig = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(cfg.connections.len(), 2);
        assert_eq!(cfg.connections[0].id, "work");
        assert_eq!(cfg.connections[1].backend, "microsoft");
        assert_eq!(cfg.poll_interval_secs, Some(120));
        assert_eq!(cfg.window_future_days, Some(14));
        assert_eq!(cfg.window_past_days, None);
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = "connections: []\nbogus: 1\n";
        assert!(serde_yaml::from_str::<CalendarConfig>(yaml).is_err());
    }
}
