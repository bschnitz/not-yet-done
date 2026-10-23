//! The event form: the fields "New event" and "Edit" share, how their values
//! read back into event times, and — for an edit — how the form is seeded from
//! an event and how what the user changed becomes an [`EventPatch`].
//!
//! An edit sends only what changed. The form shows times to the minute and
//! notes as one line, so an untouched field must compare equal to the event it
//! was seeded from even then: otherwise saving an edit of the title would
//! rewrite the notes a server keeps as HTML, or the zone its times were
//! written in.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Local, NaiveDate, Timelike, Utc};
use natural_date::{resolve_date, resolve_datetime};
use not_yet_done_calendar_core::{CalEvent, EventPatch, EventTimes, ShowAs};
use not_yet_done_content::{ContentError, FormFieldSpec, Result};

fn other_err(msg: impl Into<String>) -> ContentError {
    ContentError::Other(msg.into().into())
}

/// The "Show as" choices offered by the form, paired with the [`ShowAs`] token
/// they map to. The label is what the user picks; the enum is what the write
/// path sends.
pub(crate) const SHOW_AS_CHOICES: &[(&str, ShowAs)] = &[
    ("Busy", ShowAs::Busy),
    ("Free", ShowAs::Free),
    ("Tentative", ShowAs::Tentative),
    ("Out of office", ShowAs::OutOfOffice),
    ("Working elsewhere", ShowAs::WorkingElsewhere),
];

/// The fields of an event, in form order. "New event" adds its calendar
/// picker to these; "Edit" uses them as they are (an event stays in its
/// calendar).
pub(crate) fn event_fields() -> Vec<FormFieldSpec> {
    let show_as: Vec<String> = SHOW_AS_CHOICES.iter().map(|(l, _)| l.to_string()).collect();
    vec![
        FormFieldSpec::text("title", "Title"),
        FormFieldSpec::datetime("start", "Start", true).with_default("today 9:00"),
        FormFieldSpec::datetime("end", "End", true).with_default("today 10:00"),
        FormFieldSpec::toggle("all_day", "All day"),
        FormFieldSpec::select("show_as", "Show as", show_as).with_default("Busy"),
        FormFieldSpec::text("location", "Location").optional(),
        FormFieldSpec::text("body", "Notes").optional(),
    ]
}

// -- Form-value helpers (the calendar counterpart of local-adapter's form.rs) --

/// Read a Form field, treating absent or whitespace-only values as `None`.
pub(crate) fn form_opt(values: &HashMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Read a required Form field; error if absent or empty.
pub(crate) fn form_required<'a>(values: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    match values.get(key).map(|v| v.trim()) {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(other_err(format!("field '{key}' is required"))),
    }
}

/// A Toggle delivers `"true"`/`"false"`; anything but `"true"` (incl. absent)
/// is off.
pub(crate) fn form_flag(values: &HashMap<String, String>, key: &str) -> bool {
    values.get(key).map(|v| v == "true").unwrap_or(false)
}

/// When the form says the event takes place. Start and end are natural
/// language, resolved against `now` ("tomorrow 14:00", "next mon 9am").
///
/// A timed event's end must come after its start. An all-day event reads the
/// dates only, and its end is the **last day** it covers — a one-day event
/// ends the day it starts; the exclusive end the backends store is the day
/// after that. The dates become UTC midnight, how the backends take a date.
pub(crate) fn form_times(
    values: &HashMap<String, String>,
    now: DateTime<Local>,
) -> Result<EventTimes> {
    let all_day = form_flag(values, "all_day");
    let start_raw = form_required(values, "start")?;
    let end_raw = form_required(values, "end")?;
    if all_day {
        let start = resolve_date(start_raw, now)
            .ok_or_else(|| other_err(format!("could not understand start date: '{start_raw}'")))?;
        let last = resolve_date(end_raw, now)
            .ok_or_else(|| other_err(format!("could not understand end date: '{end_raw}'")))?;
        if last < start {
            return Err(other_err("end must not be before start"));
        }
        return Ok(EventTimes {
            start: utc_midnight(start),
            end: utc_midnight(last + Duration::days(1)),
            all_day,
        });
    }
    let start = resolve_datetime(start_raw, now)
        .ok_or_else(|| other_err(format!("could not understand start time: '{start_raw}'")))?;
    let end = resolve_datetime(end_raw, now)
        .ok_or_else(|| other_err(format!("could not understand end time: '{end_raw}'")))?;
    if end <= start {
        return Err(other_err("end must be after start"));
    }
    Ok(EventTimes {
        start,
        end,
        all_day,
    })
}

/// The "Show as" the form picked; Busy when it picked nothing it knows.
pub(crate) fn form_show_as(values: &HashMap<String, String>) -> ShowAs {
    let label = form_opt(values, "show_as").unwrap_or_default();
    SHOW_AS_CHOICES
        .iter()
        .find(|(l, _)| *l == label)
        .map(|(_, s)| *s)
        .unwrap_or(ShowAs::Busy)
}

/// The form's label for `show_as`. An event whose busy state is unknown shows
/// as Busy, what the form would write for it.
pub(crate) fn show_as_label(show_as: ShowAs) -> &'static str {
    SHOW_AS_CHOICES
        .iter()
        .find(|(_, s)| *s == show_as)
        .map(|(l, _)| *l)
        .unwrap_or("Busy")
}

/// The form values an edit of `e` starts from.
pub(crate) fn prefill(e: &CalEvent) -> HashMap<String, String> {
    let (start, end) = if e.all_day {
        let (start, last) = all_day_dates(e);
        (start.to_string(), last.to_string())
    } else {
        let at = |t: DateTime<Utc>| t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string();
        (at(e.start), at(e.end))
    };
    let mut values = HashMap::from([
        ("title".to_string(), e.title.clone()),
        ("start".to_string(), start),
        ("end".to_string(), end),
        ("all_day".to_string(), e.all_day.to_string()),
        ("show_as".to_string(), show_as_label(e.show_as).to_string()),
    ]);
    if let Some(location) = &e.location {
        values.insert("location".into(), location.clone());
    }
    if let Some(body) = &e.body {
        values.insert("body".into(), body.clone());
    }
    values
}

/// What an edit form with `values` changes about `e`. A field that still says
/// what [`prefill`] put there is no change, even where the form showed less
/// than the event holds (seconds, line breaks).
pub(crate) fn patch(
    e: &CalEvent,
    values: &HashMap<String, String>,
    now: DateTime<Local>,
) -> Result<EventPatch> {
    let title = form_required(values, "title")?;
    let times = form_times(values, now)?;
    let location = form_opt(values, "location");
    let body = form_opt(values, "body");
    let show_as = form_show_as(values);
    Ok(EventPatch {
        title: (!same_text(Some(title), Some(&e.title))).then(|| title.to_string()),
        times: (!same_times(&times, e)).then_some(times),
        location: (!same_text(location.as_deref(), e.location.as_deref())).then_some(location),
        body: (!same_text(body.as_deref(), e.body.as_deref())).then_some(body),
        show_as: (show_as_label(show_as) != show_as_label(e.show_as)).then_some(show_as),
    })
}

/// The first and the last day of the all-day event `e`. Its bounds are dates
/// at UTC midnight, the end exclusive; an end not after the start is read as
/// a one-day event.
fn all_day_dates(e: &CalEvent) -> (NaiveDate, NaiveDate) {
    let start = e.start.date_naive();
    let last = (e.end.date_naive() - Duration::days(1)).max(start);
    (start, last)
}

/// Whether `times` is when `e` takes place, as closely as the form tells.
fn same_times(times: &EventTimes, e: &CalEvent) -> bool {
    if times.all_day != e.all_day {
        return false;
    }
    if e.all_day {
        let (start, last) = all_day_dates(e);
        return times.start.date_naive() == start
            && times.end.date_naive() - Duration::days(1) == last;
    }
    to_minute(times.start) == to_minute(e.start) && to_minute(times.end) == to_minute(e.end)
}

fn to_minute(t: DateTime<Utc>) -> DateTime<Utc> {
    t.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(t)
}

/// Whether two texts say the same, whatever the whitespace between the words
/// (the form edits the notes as one line). Blank is the same as none.
fn same_text(a: Option<&str>, b: Option<&str>) -> bool {
    let words = |s: Option<&str>| {
        s.unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    words(a) == words(b)
}

fn utc_midnight(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn event() -> CalEvent {
        CalEvent {
            uid: "u1".into(),
            calendar: "Work".into(),
            title: "Dentist".into(),
            start: Local
                .with_ymd_and_hms(2030, 1, 15, 9, 0, 0)
                .unwrap()
                .to_utc(),
            end: Local
                .with_ymd_and_hms(2030, 1, 15, 10, 30, 0)
                .unwrap()
                .to_utc(),
            all_day: false,
            location: Some("Main street".into()),
            organizer: None,
            show_as: ShowAs::Busy,
            body: Some("Bring the card.\nAnd the letter.".into()),
            url: None,
            online_meeting: None,
            attendees: Vec::new(),
            my_response: None,
            cancelled: false,
        }
    }

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2030, 1, 10, 12, 0, 0).unwrap()
    }

    #[test]
    fn an_untouched_form_changes_nothing() {
        let e = event();
        let values = prefill(&e);
        assert_eq!(values["start"], "2030-01-15 09:00");
        assert_eq!(values["end"], "2030-01-15 10:30");
        assert!(patch(&e, &values, now()).unwrap().is_empty());
    }

    #[test]
    fn notes_flattened_to_one_line_are_no_change() {
        let e = event();
        let mut values = prefill(&e);
        values.insert("body".into(), "Bring the card. And the letter.".into());
        assert!(patch(&e, &values, now()).unwrap().body.is_none());
    }

    #[test]
    fn only_the_changed_fields_travel() {
        let e = event();
        let mut values = prefill(&e);
        values.insert("title".into(), "Dentist (moved)".into());
        values.insert("start".into(), "2030-01-16 14:00".into());
        values.insert("end".into(), "2030-01-16 15:00".into());
        values.insert("location".into(), "  ".into());
        let p = patch(&e, &values, now()).unwrap();
        assert_eq!(p.title.as_deref(), Some("Dentist (moved)"));
        let times = p.times.unwrap();
        assert_eq!(
            times.start,
            Local
                .with_ymd_and_hms(2030, 1, 16, 14, 0, 0)
                .unwrap()
                .to_utc()
        );
        assert!(!times.all_day);
        assert_eq!(p.location, Some(None), "a blanked location is cleared");
        assert_eq!(p.body, None);
        assert_eq!(p.show_as, None);
    }

    #[test]
    fn an_all_day_event_edits_by_its_first_and_last_day() {
        let mut e = event();
        e.all_day = true;
        e.start = Utc.with_ymd_and_hms(2030, 1, 15, 0, 0, 0).unwrap();
        e.end = Utc.with_ymd_and_hms(2030, 1, 17, 0, 0, 0).unwrap();
        let mut values = prefill(&e);
        assert_eq!(
            (values["start"].as_str(), values["end"].as_str()),
            ("2030-01-15", "2030-01-16")
        );
        assert!(patch(&e, &values, now()).unwrap().is_empty());

        values.insert("end".into(), "2030-01-15".into());
        let times = patch(&e, &values, now()).unwrap().times.unwrap();
        assert_eq!(
            times.start,
            Utc.with_ymd_and_hms(2030, 1, 15, 0, 0, 0).unwrap()
        );
        assert_eq!(
            times.end,
            Utc.with_ymd_and_hms(2030, 1, 16, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn turning_a_timed_event_all_day_keeps_its_date() {
        let e = event();
        let mut values = prefill(&e);
        values.insert("all_day".into(), "true".into());
        let times = patch(&e, &values, now()).unwrap().times.unwrap();
        assert!(times.all_day);
        assert_eq!(
            times.start,
            Utc.with_ymd_and_hms(2030, 1, 15, 0, 0, 0).unwrap()
        );
        assert_eq!(
            times.end,
            Utc.with_ymd_and_hms(2030, 1, 16, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn an_end_before_the_start_is_refused() {
        let e = event();
        let mut values = prefill(&e);
        values.insert("end".into(), "2030-01-15 08:00".into());
        assert!(patch(&e, &values, now()).is_err());
        values.insert("all_day".into(), "true".into());
        values.insert("end".into(), "2030-01-14".into());
        assert!(patch(&e, &values, now()).is_err());
    }
}
