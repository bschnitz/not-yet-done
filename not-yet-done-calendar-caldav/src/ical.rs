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

use not_yet_done_calendar_core::{
    Attendee, CalEvent, EventDraft, EventPatch, EventTimes, OnlineMeeting, Response, ShowAs,
};

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

    lines.extend(time_lines(&EventTimes {
        start: draft.start,
        end: draft.end,
        all_day: draft.all_day,
    }));
    lines.push(format!("SUMMARY:{}", escape_text(&draft.title)));
    lines.extend(text_line("LOCATION", draft.location.as_deref()));
    lines.extend(text_line("DESCRIPTION", draft.body.as_deref()));
    lines.extend(show_as_lines(draft.show_as));

    lines.push("END:VEVENT".into());
    lines.push("END:VCALENDAR".into());
    join_folded(lines)
}

/// Why an event's resource cannot be edited in place.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PatchRefusal {
    /// The resource holds a series (`RRULE`/`RDATE`, or overridden
    /// occurrences): its listed events are expanded instances, and writing one
    /// instance's times into the series would move every occurrence.
    Recurring,
    /// No `VEVENT` in the resource.
    NoEvent,
}

/// Rewrite the one `VEVENT` of the iCalendar resource `ics` as `patch` says.
/// Only the properties the patch changes are replaced; everything else —
/// attendees, alarms, the time zone definitions, unknown vendor properties —
/// is kept as the server had it. `DTSTAMP`/`LAST-MODIFIED` are restamped with
/// `now` and `SEQUENCE` goes up by one, as RFC 5545 asks of a changed event.
pub(crate) fn apply_patch(
    ics: &str,
    patch: &EventPatch,
    now: DateTime<Utc>,
) -> Result<String, PatchRefusal> {
    let lines = unfold(ics);

    // One pass to learn the shape: how many events, is it a series, and the
    // current SEQUENCE (only properties of the VEVENT itself count, not those
    // of an alarm inside it).
    let mut events = 0;
    let mut sequence = 0u32;
    let mut stack: Vec<String> = Vec::new();
    for line in &lines {
        let Some((name, _, value)) = split_line(line) else {
            continue;
        };
        match name.to_ascii_uppercase().as_str() {
            "BEGIN" => {
                let comp = value.trim().to_ascii_uppercase();
                if comp == "VEVENT" {
                    events += 1;
                }
                stack.push(comp);
            }
            "END" => {
                stack.pop();
            }
            prop if stack.last().map(String::as_str) == Some("VEVENT") => match prop {
                "RRULE" | "RDATE" | "RECURRENCE-ID" => return Err(PatchRefusal::Recurring),
                "SEQUENCE" => sequence = value.trim().parse().unwrap_or(0),
                _ => {}
            },
            _ => {}
        }
    }
    match events {
        0 => return Err(PatchRefusal::NoEvent),
        1 => {}
        _ => return Err(PatchRefusal::Recurring),
    }

    let mut replaced: Vec<&str> = vec!["DTSTAMP", "LAST-MODIFIED", "SEQUENCE"];
    let mut fresh: Vec<String> = vec![
        format!("DTSTAMP:{}", fmt_utc(now)),
        format!("LAST-MODIFIED:{}", fmt_utc(now)),
        format!("SEQUENCE:{}", sequence + 1),
    ];
    if let Some(title) = &patch.title {
        replaced.push("SUMMARY");
        fresh.push(format!("SUMMARY:{}", escape_text(title)));
    }
    if let Some(times) = &patch.times {
        replaced.extend(["DTSTART", "DTEND", "DURATION"]);
        fresh.extend(time_lines(times));
    }
    if let Some(location) = &patch.location {
        replaced.push("LOCATION");
        fresh.extend(text_line("LOCATION", location.as_deref()));
    }
    if let Some(body) = &patch.body {
        replaced.push("DESCRIPTION");
        fresh.extend(text_line("DESCRIPTION", body.as_deref()));
    }
    if let Some(show_as) = patch.show_as {
        replaced.extend(["TRANSP", "X-MICROSOFT-CDO-BUSYSTATUS"]);
        fresh.extend(show_as_lines(show_as));
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + fresh.len());
    let mut stack: Vec<String> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, _, value)) = split_line(&line) else {
            out.push(line);
            continue;
        };
        let name = name.to_ascii_uppercase();
        match name.as_str() {
            "BEGIN" => {
                let comp = value.trim().to_ascii_uppercase();
                let opens_event = comp == "VEVENT";
                stack.push(comp);
                out.push(line);
                // The event's own properties come before its alarms, so the
                // new ones go right after BEGIN:VEVENT.
                if opens_event {
                    out.append(&mut fresh);
                }
            }
            "END" => {
                stack.pop();
                out.push(line);
            }
            prop if stack.last().map(String::as_str) == Some("VEVENT")
                && replaced.contains(&prop) => {}
            _ => out.push(line),
        }
    }
    Ok(join_folded(out))
}

/// `DTSTART`/`DTEND` for `times`: DATE values for an all-day event (the end
/// exclusive, at least the day after the start), UTC date-times otherwise.
fn time_lines(times: &EventTimes) -> Vec<String> {
    if times.all_day {
        let start_date = times.start.date_naive();
        let mut end_date = times.end.date_naive();
        if end_date <= start_date {
            end_date = start_date + Duration::days(1);
        }
        vec![
            format!("DTSTART;VALUE=DATE:{}", start_date.format("%Y%m%d")),
            format!("DTEND;VALUE=DATE:{}", end_date.format("%Y%m%d")),
        ]
    } else {
        vec![
            format!("DTSTART:{}", fmt_utc(times.start)),
            format!("DTEND:{}", fmt_utc(times.end)),
        ]
    }
}

/// A TEXT property line, or none for a blank value.
fn text_line(name: &str, value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|v| format!("{name}:{}", escape_text(v)))
}

/// `TRANSP` and `X-MICROSOFT-CDO-BUSYSTATUS` for `show_as`.
fn show_as_lines(show_as: ShowAs) -> [String; 2] {
    let (transp, busy) = show_as_ical(show_as);
    [
        format!("TRANSP:{transp}"),
        format!("X-MICROSOFT-CDO-BUSYSTATUS:{busy}"),
    ]
}

/// Content lines as one document: each folded, each ended by CRLF.
fn join_folded(lines: Vec<String>) -> String {
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
    /// The meeting's join link, from `CONFERENCE` (RFC 7986) or a vendor
    /// property (`X-MICROSOFT-SKYPETEAMSMEETINGURL`, `X-GOOGLE-CONFERENCE`).
    join_url: Option<String>,
    /// The organizer's address, to tell whether the account itself invited.
    organizer_address: Option<String>,
    attendees: Vec<ParsedAttendee>,
    /// `STATUS:CANCELLED`.
    cancelled: bool,
}

/// One `ATTENDEE` line: the name shown, the address it is reached at, and
/// its `PARTSTAT`.
#[derive(Debug, Clone)]
struct ParsedAttendee {
    name: String,
    address: String,
    response: Response,
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
            join_url: None,
            organizer_address: None,
            attendees: Vec::new(),
            cancelled: false,
        }
    }
}

impl ParsedEvent {
    /// Turn this into a `CalEvent` for `calendar`, if it has the minimum
    /// viable shape (a uid and a start). Events without a start are skipped by
    /// the caller (they cannot be placed on a timeline). `me` is the
    /// connection's own address, which finds the account's own answer among
    /// the attendees.
    pub(crate) fn into_cal_event(self, calendar: &str, me: Option<&str>) -> Option<CalEvent> {
        let is_me = |address: &str| me.is_some_and(|me| address.eq_ignore_ascii_case(me.trim()));
        let my_response = if self.organizer_address.as_deref().is_some_and(is_me) {
            Some(Response::Organizer)
        } else {
            self.attendees
                .iter()
                .find(|a| is_me(&a.address))
                .map(|a| a.response)
        };
        let attendees = self
            .attendees
            .into_iter()
            .map(|a| Attendee {
                name: a.name,
                response: a.response,
            })
            .collect();
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
            online_meeting: self.join_url.as_deref().map(OnlineMeeting::from_join_url),
            attendees,
            my_response,
            cancelled: self.cancelled,
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
        "ORGANIZER" => {
            ev.organizer = Some(organizer_display(params, value));
            ev.organizer_address = Some(address_of(value));
        }
        "ATTENDEE" => {
            if let Some(attendee) = attendee(params, value) {
                ev.attendees.push(attendee);
            }
        }
        "STATUS" => ev.cancelled = value.trim().eq_ignore_ascii_case("CANCELLED"),
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
        // Teams writes its own property; it names the meeting outright.
        "X-MICROSOFT-SKYPETEAMSMEETINGURL" if is_web_link(value) => {
            ev.join_url = Some(value.trim().to_string());
        }
        // A `CONFERENCE` may also be a dial-in (`tel:`); only a web link joins.
        "CONFERENCE" | "X-GOOGLE-CONFERENCE" if is_web_link(value) => {
            ev.join_url.get_or_insert_with(|| value.trim().to_string());
        }
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

fn is_web_link(value: &str) -> bool {
    let v = value.trim_start().to_ascii_lowercase();
    v.starts_with("https://") || v.starts_with("http://")
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

/// An `ATTENDEE` line as someone invited, or `None` for a room or other
/// resource booked through the same property.
fn attendee(params: &[(String, String)], value: &str) -> Option<ParsedAttendee> {
    let param = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.to_ascii_uppercase())
    };
    if matches!(param("CUTYPE").as_deref(), Some("ROOM" | "RESOURCE")) {
        return None;
    }
    let response = match param("PARTSTAT").as_deref() {
        Some("ACCEPTED") => Response::Accepted,
        Some("TENTATIVE") => Response::Tentative,
        Some("DECLINED") => Response::Declined,
        _ => Response::Pending,
    };
    Some(ParsedAttendee {
        name: organizer_display(params, value),
        address: address_of(value),
        response,
    })
}

/// A `CAL-ADDRESS` without its `mailto:` scheme.
fn address_of(value: &str) -> String {
    let value = value.trim();
    match value.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("mailto:") => value[7..].to_string(),
        _ => value.to_string(),
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
        let ev = evs[0].clone().into_cal_event("Personal", None).unwrap();
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
        let ev = parse_events(ics).remove(0).into_cal_event("c", None).unwrap();
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
        let ev = parse_events(ics).remove(0).into_cal_event("c", None).unwrap();
        assert_eq!(ev.end.format("%H:%M").to_string(), "10:30");
    }

    #[test]
    fn transp_falls_back_when_no_busystatus() {
        let ics =
            "BEGIN:VEVENT\nUID:d3\nDTSTART:20240115T090000Z\nTRANSP:TRANSPARENT\nEND:VEVENT\n";
        let ev = parse_events(ics).remove(0).into_cal_event("c", None).unwrap();
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
        assert!(parse_events(ics).remove(0).into_cal_event("c", None).is_none());
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
            .into_cal_event("Personal", None)
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
        let ev = parse_events(&ics).remove(0).into_cal_event("c", None).unwrap();
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
        let ev = parse_events(&ics).remove(0).into_cal_event("c", None).unwrap();
        assert_eq!(ev.title, "x".repeat(200));
    }

    fn one_event(extra: &str) -> CalEvent {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:m-1\r\nDTSTART:20240115T090000Z\r\n\
             {extra}END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        parse_events(&ics).remove(0).into_cal_event("Work", None).unwrap()
    }

    const INVITED: &str = "ORGANIZER;CN=Jane Doe:mailto:jane@example.invalid\r\n\
        ATTENDEE;CN=\"Doe, Jane\";PARTSTAT=ACCEPTED:mailto:jane@example.invalid\r\n\
        ATTENDEE;PARTSTAT=TENTATIVE:MAILTO:Me@Example.invalid\r\n\
        ATTENDEE;CN=Bob;PARTSTAT=DECLINED:mailto:bob@example.invalid\r\n\
        ATTENDEE;CN=Carol:mailto:carol@example.invalid\r\n\
        ATTENDEE;CUTYPE=ROOM;CN=Room 1;PARTSTAT=ACCEPTED:mailto:room@example.invalid\r\n";

    fn invited(me: Option<&str>) -> CalEvent {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:m-1\r\nDTSTART:20240115T090000Z\r\n\
             {INVITED}END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        parse_events(&ics).remove(0).into_cal_event("Work", me).unwrap()
    }

    #[test]
    fn attendees_come_with_their_answer_and_rooms_are_left_out() {
        let ev = invited(None);
        let attendees: Vec<_> = ev
            .attendees
            .iter()
            .map(|a| (a.name.as_str(), a.response))
            .collect();
        assert_eq!(
            attendees,
            [
                ("Doe, Jane", Response::Accepted),
                ("Me@Example.invalid", Response::Tentative),
                ("Bob", Response::Declined),
                ("Carol", Response::Pending),
            ]
        );
        assert_eq!(ev.my_response, None, "nobody to compare with");
        assert!(!ev.cancelled);
    }

    #[test]
    fn the_own_answer_is_found_by_address_and_the_organizer_answers_nothing() {
        assert_eq!(
            invited(Some("me@example.invalid")).my_response,
            Some(Response::Tentative)
        );
        assert_eq!(
            invited(Some("jane@example.invalid")).my_response,
            Some(Response::Organizer)
        );
        assert_eq!(invited(Some("nobody@example.invalid")).my_response, None);
    }

    #[test]
    fn a_cancelled_status_calls_the_event_off() {
        assert!(one_event("STATUS:CANCELLED\r\n").cancelled);
        assert!(!one_event("STATUS:CONFIRMED\r\n").cancelled);
    }

    #[test]
    fn the_teams_property_makes_a_teams_meeting() {
        let ev = one_event(
            "CONFERENCE;VALUE=URI:https://example.invalid/other\r\n\
             X-MICROSOFT-SKYPETEAMSMEETINGURL:https://teams.microsoft.com/l/meetup-join/x\r\n",
        );
        let meeting = ev.online_meeting.unwrap();
        assert_eq!(meeting.provider.as_str(), "teams");
        assert_eq!(
            meeting.join_url.as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/x")
        );
    }

    #[test]
    fn a_conference_web_link_joins_but_a_dial_in_does_not() {
        let ev = one_event(
            "CONFERENCE;VALUE=URI;FEATURE=PHONE:tel:+1-555-0100\r\n\
             CONFERENCE;VALUE=URI;FEATURE=VIDEO:https://meet.google.com/abc-defg-hij\r\n",
        );
        assert_eq!(ev.online_meeting.unwrap().provider.as_str(), "meet");
        assert_eq!(one_event("").online_meeting, None);
    }

    const STORED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:1@x\r\nDTSTAMP:20300101T000000Z\r\nSEQUENCE:4\r\nDTSTART;TZID=Europe/Berlin:20300115T090000\r\nDURATION:PT1H\r\nSUMMARY:Dentist\r\nLOCATION:Main street\r\nATTENDEE;CN=Jane:mailto:jane@example.org\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn a_patch_replaces_only_what_it_changes() {
        let patch = EventPatch {
            title: Some("Dentist, again".into()),
            location: Some(None),
            ..EventPatch::default()
        };
        let out = apply_patch(STORED, &patch, utc("2030-01-10T08:00:00Z")).unwrap();
        let ev = &parse_events(&out)[0];
        assert_eq!(ev.summary.as_deref(), Some("Dentist, again"));
        assert_eq!(ev.location, None, "a cleared location is gone");
        // Untouched: the times as written (zone and duration), the attendee,
        // the alarm with its own DESCRIPTION, the time zone definition.
        assert!(out.contains("DTSTART;TZID=Europe/Berlin:20300115T090000\r\n"));
        assert!(out.contains("DURATION:PT1H\r\n"));
        assert!(out.contains("ATTENDEE;CN=Jane:mailto:jane@example.org\r\n"));
        assert!(out.contains("DESCRIPTION:Reminder\r\n"));
        assert!(out.contains("TZID:Europe/Berlin\r\n"));
        assert!(out.contains("SEQUENCE:5\r\n") && !out.contains("SEQUENCE:4"));
        assert!(out.contains("DTSTAMP:20300110T080000Z\r\n"));
        assert!(!out.contains("DTSTAMP:20300101T"));
    }

    #[test]
    fn new_times_replace_start_end_and_duration() {
        let patch = EventPatch {
            times: Some(EventTimes {
                start: utc("2030-01-16T13:00:00Z"),
                end: utc("2030-01-16T14:30:00Z"),
                all_day: false,
            }),
            show_as: Some(ShowAs::Free),
            body: Some(Some("Bring the card".into())),
            ..EventPatch::default()
        };
        let out = apply_patch(STORED, &patch, utc("2030-01-10T08:00:00Z")).unwrap();
        assert!(!out.contains("TZID=Europe/Berlin:") && !out.contains("DURATION"));
        let ev = parse_events(&out).remove(0).into_cal_event("cal", None).unwrap();
        assert_eq!(ev.start, utc("2030-01-16T13:00:00Z"));
        assert_eq!(ev.end, utc("2030-01-16T14:30:00Z"));
        assert_eq!(ev.show_as, ShowAs::Free);
        assert_eq!(ev.body.as_deref(), Some("Bring the card"));
        assert!(out.contains("DESCRIPTION:Reminder\r\n"), "the alarm keeps its text");
        // The event's properties come before its alarm.
        assert!(out.find("DTSTART").unwrap() < out.find("BEGIN:VALARM").unwrap());
    }

    #[test]
    fn a_series_is_not_patched() {
        let series = STORED.replace("DURATION:PT1H\r\n", "DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\n");
        let patch = EventPatch {
            title: Some("x".into()),
            ..EventPatch::default()
        };
        assert_eq!(
            apply_patch(&series, &patch, utc("2030-01-10T08:00:00Z")),
            Err(PatchRefusal::Recurring)
        );
        assert_eq!(
            apply_patch("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n", &patch, utc("2030-01-10T08:00:00Z")),
            Err(PatchRefusal::NoEvent)
        );
    }

}
