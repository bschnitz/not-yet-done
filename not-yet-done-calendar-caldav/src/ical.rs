//! Minimal iCalendar (RFC 5545) reader — just enough to turn the `VEVENT`s a
//! CalDAV `calendar-query` returns into [`CalEvent`]s.
//!
//! Scope is deliberately narrow: we ask the server to **expand** recurrences
//! (see [`crate::client`]), so every event arrives as a concrete instance with
//! its `DTSTART`/`DTEND` already resolved to UTC — no `RRULE`, no `VTIMEZONE`
//! math to do here. We therefore parse only the handful of properties the
//! event list needs and accept UTC (`…Z`), floating (treated as UTC), and
//! date-only (`VALUE=DATE`, all-day) date forms.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};

use not_yet_done_calendar_core::{CalEvent, EventDraft, ShowAs};

/// Serialise an [`EventDraft`] into a one-`VEVENT` iCalendar document for a
/// CalDAV `PUT`. `uid` is the event's UID (also the basis of its resource
/// filename). `now` stamps `DTSTAMP` (RFC 5545 requires it). Output uses CRLF
/// line endings and folds long content lines, so it round-trips back through
/// [`parse_events`].
pub(crate) fn to_ics(draft: &EventDraft, uid: &str, now: DateTime<Utc>) -> String {
    let mut lines: Vec<String> = vec![
        "BEGIN:VCALENDAR".into(),
        "VERSION:2.0".into(),
        "PRODID:-//not-yet-done//caldav//EN".into(),
        "CALSCALE:GREGORIAN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{uid}"),
        format!("DTSTAMP:{}", fmt_utc(now)),
    ];

    if draft.all_day {
        // All-day: DATE values, DTEND exclusive (at least the day after DTSTART).
        let start_date = draft.start.date_naive();
        let mut end_date = draft.end.date_naive();
        if end_date <= start_date {
            end_date = start_date + Duration::days(1);
        }
        lines.push(format!(
            "DTSTART;VALUE=DATE:{}",
            start_date.format("%Y%m%d")
        ));
        lines.push(format!("DTEND;VALUE=DATE:{}", end_date.format("%Y%m%d")));
    } else {
        lines.push(format!("DTSTART:{}", fmt_utc(draft.start)));
        lines.push(format!("DTEND:{}", fmt_utc(draft.end)));
    }

    lines.push(format!("SUMMARY:{}", escape_text(&draft.title)));
    if let Some(loc) = draft
        .location
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        lines.push(format!("LOCATION:{}", escape_text(loc)));
    }
    if let Some(body) = draft
        .body
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        lines.push(format!("DESCRIPTION:{}", escape_text(body)));
    }

    let (transp, busy) = show_as_ical(draft.show_as);
    lines.push(format!("TRANSP:{transp}"));
    lines.push(format!("X-MICROSOFT-CDO-BUSYSTATUS:{busy}"));

    lines.push("END:VEVENT".into());
    lines.push("END:VCALENDAR".into());

    let mut out = String::new();
    for line in lines {
        out.push_str(&fold_line(&line));
        out.push_str("\r\n");
    }
    out
}

/// iCalendar UTC timestamp `20240115T090000Z`.
fn fmt_utc(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
        .replace(['-', ':'], "")
}

/// The `(TRANSP, X-MICROSOFT-CDO-BUSYSTATUS)` pair for a [`ShowAs`] — the write
/// mirror of the hints [`apply_property`] reads back. Only `Free` is transparent
/// (doesn't occupy the slot); everything else is opaque with its busy token.
fn show_as_ical(show_as: ShowAs) -> (&'static str, &'static str) {
    match show_as {
        ShowAs::Free => ("TRANSPARENT", "FREE"),
        ShowAs::Tentative => ("OPAQUE", "TENTATIVE"),
        ShowAs::Busy => ("OPAQUE", "BUSY"),
        ShowAs::OutOfOffice => ("OPAQUE", "OOF"),
        ShowAs::WorkingElsewhere => ("OPAQUE", "WORKINGELSEWHERE"),
        ShowAs::Unknown => ("OPAQUE", "BUSY"),
    }
}

/// Escape an RFC 5545 TEXT value — the inverse of [`unescape`]: backslash,
/// newline, comma and semicolon become their escaped forms.
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            _ => out.push(c),
        }
    }
    out
}

/// Fold a content line to the RFC 5545 75-octet limit: continuation lines start
/// with a single space. We fold conservatively on char boundaries at 73 chars
/// (well under 75 octets for ASCII, safe for multi-byte too).
fn fold_line(line: &str) -> String {
    const LIMIT: usize = 73;
    if line.chars().count() <= LIMIT {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + line.len() / LIMIT);
    for (i, c) in line.chars().enumerate() {
        if i > 0 && i % LIMIT == 0 {
            out.push_str("\r\n ");
        }
        out.push(c);
    }
    out
}

/// One parsed `VEVENT`, before the connection label is attached.
#[derive(Debug, Clone)]
pub(crate) struct ParsedEvent {
    pub(crate) uid: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) start: Option<DateTime<Utc>>,
    pub(crate) end: Option<DateTime<Utc>>,
    pub(crate) all_day: bool,
    pub(crate) location: Option<String>,
    pub(crate) organizer: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) show_as: ShowAs,
    /// Duration parsed from a `DURATION` property, used to derive `end` when the
    /// event carries `DTSTART` + `DURATION` instead of `DTEND` (RFC 5545 §3.6.1).
    duration: Option<Duration>,
}

impl Default for ParsedEvent {
    fn default() -> Self {
        Self {
            uid: None,
            summary: None,
            start: None,
            end: None,
            all_day: false,
            location: None,
            organizer: None,
            description: None,
            url: None,
            // `ShowAs` has no Default; an event with no busy hint is Unknown.
            show_as: ShowAs::Unknown,
            duration: None,
        }
    }
}

impl ParsedEvent {
    /// Turn this into a `CalEvent` for `calendar`, if it has the minimum
    /// viable shape (a uid and a start). Events without a start are skipped by
    /// the caller (they cannot be placed on a timeline).
    pub(crate) fn into_cal_event(self, calendar: &str) -> Option<CalEvent> {
        let start = self.start?;
        // Derive the end: explicit DTEND wins; else DTSTART+DURATION; else a
        // zero-length event (all-day with no DTEND spans the single start day).
        let end = self
            .end
            .or_else(|| self.duration.map(|d| start + d))
            .unwrap_or_else(|| {
                if self.all_day {
                    start + Duration::days(1)
                } else {
                    start
                }
            });
        Some(CalEvent {
            uid: self.uid.unwrap_or_default(),
            calendar: calendar.to_string(),
            title: self.summary.unwrap_or_default(),
            start,
            end,
            all_day: self.all_day,
            location: self.location.filter(|s| !s.trim().is_empty()),
            organizer: self.organizer.filter(|s| !s.trim().is_empty()),
            show_as: self.show_as,
            body: self.description.filter(|s| !s.trim().is_empty()),
            url: self.url.filter(|s| !s.trim().is_empty()),
        })
    }
}

/// Parse every `VEVENT` in one iCalendar document. Malformed individual
/// properties are skipped rather than failing the whole parse — a partial
/// event is more useful than none, and calendars in the wild are messy.
pub(crate) fn parse_events(ics: &str) -> Vec<ParsedEvent> {
    let lines = unfold(ics);
    let mut out = Vec::new();
    let mut cur: Option<ParsedEvent> = None;
    // Track nesting so an alarm's own properties (VALARM inside VEVENT) don't
    // clobber the event's — we only apply properties at VEVENT depth.
    let mut in_alarm = false;

    for line in lines {
        let (name, params, value) = match split_line(&line) {
            Some(parts) => parts,
            None => continue,
        };
        let name_up = name.to_ascii_uppercase();
        match name_up.as_str() {
            "BEGIN" => match value.to_ascii_uppercase().as_str() {
                "VEVENT" => cur = Some(ParsedEvent::default()),
                "VALARM" => in_alarm = true,
                _ => {}
            },
            "END" => match value.to_ascii_uppercase().as_str() {
                "VEVENT" => {
                    if let Some(ev) = cur.take() {
                        out.push(ev);
                    }
                }
                "VALARM" => in_alarm = false,
                _ => {}
            },
            _ => {
                if in_alarm {
                    continue;
                }
                if let Some(ev) = cur.as_mut() {
                    apply_property(ev, &name_up, &params, &value);
                }
            }
        }
    }
    out
}

/// Apply one property line to the event being built.
fn apply_property(ev: &mut ParsedEvent, name: &str, params: &[(String, String)], value: &str) {
    match name {
        "UID" => ev.uid = Some(value.to_string()),
        "SUMMARY" => ev.summary = Some(unescape(value)),
        "LOCATION" => ev.location = Some(unescape(value)),
        "DESCRIPTION" => ev.description = Some(unescape(value)),
        "URL" => ev.url = Some(value.to_string()),
        "ORGANIZER" => ev.organizer = Some(organizer_display(params, value)),
        "DTSTART" => {
            if let Some((dt, all_day)) = parse_ical_datetime(params, value) {
                ev.start = Some(dt);
                ev.all_day = all_day;
            }
        }
        "DTEND" => {
            if let Some((dt, _)) = parse_ical_datetime(params, value) {
                ev.end = Some(dt);
            }
        }
        "DURATION" => ev.duration = parse_duration(value),
        // Prefer the Microsoft busy-status hint (maps cleanly onto ShowAs);
        // fall back to TRANSP. Never downgrade a known value to Unknown.
        "X-MICROSOFT-CDO-BUSYSTATUS" => {
            let s = map_busystatus(value);
            if s != ShowAs::Unknown {
                ev.show_as = s;
            }
        }
        "TRANSP" => {
            if ev.show_as == ShowAs::Unknown {
                ev.show_as = match value.to_ascii_uppercase().as_str() {
                    "TRANSPARENT" => ShowAs::Free,
                    "OPAQUE" => ShowAs::Busy,
                    _ => ShowAs::Unknown,
                };
            }
        }
        _ => {}
    }
}

fn map_busystatus(v: &str) -> ShowAs {
    match v.to_ascii_uppercase().as_str() {
        "FREE" => ShowAs::Free,
        "TENTATIVE" => ShowAs::Tentative,
        "BUSY" => ShowAs::Busy,
        "OOF" => ShowAs::OutOfOffice,
        "WORKINGELSEWHERE" => ShowAs::WorkingElsewhere,
        _ => ShowAs::Unknown,
    }
}

/// `ORGANIZER;CN=Jane Doe:mailto:jane@example.com` → `Jane Doe`; without a CN,
/// the address with any `mailto:` scheme stripped.
fn organizer_display(params: &[(String, String)], value: &str) -> String {
    for (k, v) in params {
        if k.eq_ignore_ascii_case("CN") && !v.trim().is_empty() {
            return unescape(v);
        }
    }
    value
        .strip_prefix("mailto:")
        .or_else(|| value.strip_prefix("MAILTO:"))
        .unwrap_or(value)
        .to_string()
}

/// Parse an iCalendar `DATE` or `DATE-TIME` value. Returns the instant plus
/// whether it was date-only (all-day). With server-side expansion timed values
/// arrive in UTC (`…Z`); a floating value (no `Z`, no `TZID` we honour) is
/// taken as UTC best-effort.
fn parse_ical_datetime(params: &[(String, String)], value: &str) -> Option<(DateTime<Utc>, bool)> {
    let is_date = params
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("VALUE") && v.eq_ignore_ascii_case("DATE"));
    let v = value.trim();
    if is_date || (v.len() == 8 && v.bytes().all(|b| b.is_ascii_digit())) {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        let dt = d.and_hms_opt(0, 0, 0)?;
        return Some((Utc.from_utc_datetime(&dt), true));
    }
    // DATE-TIME, optionally UTC-suffixed with `Z`.
    let (core, _utc) = match v.strip_suffix('Z') {
        Some(rest) => (rest, true),
        None => (v, false),
    };
    let ndt = NaiveDateTime::parse_from_str(core, "%Y%m%dT%H%M%S").ok()?;
    Some((Utc.from_utc_datetime(&ndt), false))
}

/// Parse an RFC 5545 `DURATION` (e.g. `PT1H`, `P1DT2H30M`, `-PT15M`). Weeks
/// (`P2W`) are supported; the sign prefix is honoured.
fn parse_duration(value: &str) -> Option<Duration> {
    let v = value.trim();
    let (neg, rest) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let rest = rest.strip_prefix('P')?;
    let mut secs: i64 = 0;
    let mut num = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            'W' => {
                secs += num.parse::<i64>().ok()? * 7 * 86400;
                num.clear();
            }
            'D' => {
                secs += num.parse::<i64>().ok()? * 86400;
                num.clear();
            }
            'H' if in_time => {
                secs += num.parse::<i64>().ok()? * 3600;
                num.clear();
            }
            'M' if in_time => {
                secs += num.parse::<i64>().ok()? * 60;
                num.clear();
            }
            'S' if in_time => {
                secs += num.parse::<i64>().ok()?;
                num.clear();
            }
            _ => return None,
        }
    }
    Some(Duration::seconds(if neg { -secs } else { secs }))
}

/// Undo RFC 5545 line folding: a line beginning with a space or tab is a
/// continuation of the previous one (the leading whitespace is removed).
/// Handles both CRLF and bare-LF inputs.
fn unfold(ics: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in ics.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        out.push(line.to_string());
    }
    out
}

/// Split a content line into `(name, params, value)`. The value begins after
/// the first `:` that is not inside a double-quoted parameter value. Returns
/// `None` for a line with no `:` at all.
fn split_line(line: &str) -> Option<(String, Vec<(String, String)>, String)> {
    let mut in_quote = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (left, right) = line.split_at(colon);
    let value = right[1..].to_string();

    let mut parts = split_params(left);
    let name = parts.remove(0);
    Some((name, parts.into_iter().map(parse_param).collect(), value))
}

/// Split the `name;PARAM=a;PARAM2="x;y"` left side on unquoted semicolons.
fn split_params(left: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in left.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            ';' if !in_quote => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// `KEY="quoted value"` or `KEY=value` → `(KEY, value)` (quotes stripped).
fn parse_param(token: String) -> (String, String) {
    match token.split_once('=') {
        Some((k, v)) => {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(v);
            (k.to_string(), v.to_string())
        }
        None => (token, String::new()),
    }
}

/// Unescape RFC 5545 TEXT: `\n`/`\N` → newline, `\,` `\;` `\\` → literals.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // SUMMARY is folded mid-word ("Sprint pla" + fold + "nning"): RFC 5545
    // unfolding drops the CRLF and the one leading space, so it rejoins to
    // "Sprint planning" (a fold never inserts a space).
    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:abc-123\r\nSUMMARY:Sprint pla\r\n nning\r\nDTSTART:20240115T090000Z\r\nDTEND:20240115T100000Z\r\nLOCATION:Room 1\r\nDESCRIPTION:Agenda\\nsecond line\r\nORGANIZER;CN=Jane Doe:mailto:jane@example.invalid\r\nX-MICROSOFT-CDO-BUSYSTATUS:BUSY\r\nURL:https://example.invalid/e/1\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:reminder\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_folded_event_with_all_fields() {
        let evs = parse_events(SAMPLE);
        assert_eq!(evs.len(), 1);
        let ev = evs[0].clone().into_cal_event("Personal").unwrap();
        assert_eq!(ev.uid, "abc-123");
        // Folded SUMMARY was rejoined without the fold whitespace.
        assert_eq!(ev.title, "Sprint planning");
        assert_eq!(ev.calendar, "Personal");
        assert_eq!(
            ev.start.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "2024-01-15T09:00:00Z"
        );
        assert_eq!(
            ev.end.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "2024-01-15T10:00:00Z"
        );
        assert_eq!(ev.location.as_deref(), Some("Room 1"));
        // VALARM DESCRIPTION must not clobber the event DESCRIPTION.
        assert_eq!(ev.body.as_deref(), Some("Agenda\nsecond line"));
        assert_eq!(ev.organizer.as_deref(), Some("Jane Doe"));
        assert_eq!(ev.show_as, ShowAs::Busy);
        assert_eq!(ev.url.as_deref(), Some("https://example.invalid/e/1"));
        assert!(!ev.all_day);
    }

    #[test]
    fn all_day_date_value_spans_one_day() {
        let ics =
            "BEGIN:VEVENT\nUID:d1\nSUMMARY:Holiday\nDTSTART;VALUE=DATE:20240301\nEND:VEVENT\n";
        let ev = parse_events(ics).remove(0).into_cal_event("c").unwrap();
        assert!(ev.all_day);
        assert_eq!(
            ev.start.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "2024-03-01T00:00:00Z"
        );
        // No DTEND on an all-day event → spans the single start day.
        assert_eq!(
            ev.end.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "2024-03-02T00:00:00Z"
        );
    }

    #[test]
    fn derives_end_from_duration() {
        let ics = "BEGIN:VEVENT\nUID:d2\nDTSTART:20240115T090000Z\nDURATION:PT1H30M\nEND:VEVENT\n";
        let ev = parse_events(ics).remove(0).into_cal_event("c").unwrap();
        assert_eq!(ev.end.format("%H:%M").to_string(), "10:30");
    }

    #[test]
    fn transp_falls_back_when_no_busystatus() {
        let ics =
            "BEGIN:VEVENT\nUID:d3\nDTSTART:20240115T090000Z\nTRANSP:TRANSPARENT\nEND:VEVENT\n";
        let ev = parse_events(ics).remove(0).into_cal_event("c").unwrap();
        assert_eq!(ev.show_as, ShowAs::Free);
    }

    #[test]
    fn organizer_without_cn_strips_mailto() {
        let (_n, p, v) = split_line("ORGANIZER:mailto:bob@example.invalid").unwrap();
        assert_eq!(organizer_display(&p, &v), "bob@example.invalid");
    }

    #[test]
    fn event_without_start_is_dropped() {
        let ics = "BEGIN:VEVENT\nUID:no-start\nSUMMARY:x\nEND:VEVENT\n";
        assert!(parse_events(ics).remove(0).into_cal_event("c").is_none());
    }

    #[test]
    fn parse_duration_handles_weeks_and_sign() {
        assert_eq!(parse_duration("P1W"), Some(Duration::days(7)));
        assert_eq!(parse_duration("-PT15M"), Some(Duration::minutes(-15)));
        assert_eq!(parse_duration("P1DT2H"), Some(Duration::hours(26)));
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    // The serializer's output must survive our own reader unchanged: the whole
    // point is that a PUT we write reads back as the same event.
    #[test]
    fn timed_draft_round_trips_through_parser() {
        let draft = EventDraft {
            title: "Sprint, planning; review".into(),
            start: utc("2024-01-15T09:00:00Z"),
            end: utc("2024-01-15T10:30:00Z"),
            all_day: false,
            location: Some("Room 1".into()),
            body: Some("line one\nline two".into()),
            show_as: ShowAs::Busy,
        };
        let ics = to_ics(&draft, "uid-1@nyd", utc("2024-01-10T08:00:00Z"));
        let ev = parse_events(&ics)
            .remove(0)
            .into_cal_event("Personal")
            .unwrap();
        assert_eq!(ev.uid, "uid-1@nyd");
        // TEXT special chars round-trip (escaped on write, unescaped on read).
        assert_eq!(ev.title, "Sprint, planning; review");
        assert_eq!(ev.body.as_deref(), Some("line one\nline two"));
        assert_eq!(ev.location.as_deref(), Some("Room 1"));
        assert_eq!(ev.start, draft.start);
        assert_eq!(ev.end, draft.end);
        assert_eq!(ev.show_as, ShowAs::Busy);
        assert!(!ev.all_day);
    }

    #[test]
    fn all_day_draft_serialises_as_date_values() {
        let draft = EventDraft {
            title: "Holiday".into(),
            start: utc("2024-03-01T00:00:00Z"),
            end: utc("2024-03-01T00:00:00Z"),
            all_day: true,
            location: None,
            body: None,
            show_as: ShowAs::Free,
        };
        let ics = to_ics(&draft, "d1", utc("2024-02-01T00:00:00Z"));
        assert!(ics.contains("DTSTART;VALUE=DATE:20240301"));
        // End defaults to the following day when not after the start date.
        assert!(ics.contains("DTEND;VALUE=DATE:20240302"));
        let ev = parse_events(&ics).remove(0).into_cal_event("c").unwrap();
        assert!(ev.all_day);
        assert_eq!(ev.show_as, ShowAs::Free);
    }

    #[test]
    fn long_summary_is_folded_under_the_octet_limit() {
        let draft = EventDraft {
            title: "x".repeat(200),
            start: utc("2024-01-15T09:00:00Z"),
            end: utc("2024-01-15T10:00:00Z"),
            all_day: false,
            location: None,
            body: None,
            show_as: ShowAs::Busy,
        };
        let ics = to_ics(&draft, "long", utc("2024-01-10T08:00:00Z"));
        assert!(
            ics.lines().all(|l| l.chars().count() <= 74),
            "every line folds under 75"
        );
        // And it still reads back whole.
        let ev = parse_events(&ics).remove(0).into_cal_event("c").unwrap();
        assert_eq!(ev.title, "x".repeat(200));
    }
}
