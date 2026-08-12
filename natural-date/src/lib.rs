//! Resolve natural-language date/time phrases to concrete instants, relative to
//! a caller-supplied reference time. App-agnostic: it depends only on `chrono`
//! and the standalone parser crates, never on any application type, so the same
//! resolver backs the TUI form widget, the filter query engine, the trackings
//! adapter and the CLI.
//!
//! # Layers
//!
//! Every phrase is run through, in order:
//!
//! 1. **Machine formats** — RFC 3339 on the raw input (case-sensitive `T`/`Z`).
//! 2. **Preprocessor** (this crate's own extensions over the external parsers):
//!    `in <n> <unit>`, part-of-day words (`morning`, `noon`, `tonight`, …),
//!    abbreviations (`eod`, `eow`, `cob`, …), quarter shorthand (`q3`,
//!    `end of q4`), ISO week (`2026-w30`), spoken time (`half past 2`),
//!    business days (`next business day`, `in 2 working days`), and `now`/`asap`.
//! 3. **[`date-periods`]** — period-boundary phrases (`end of next week`).
//! 4. **[`chrono-english`]** — relative expressions (`tomorrow`, `next friday 8pm`).
//! 5. **Naive / [`dateparser`] fallbacks** — broad absolute formats.
//!
//! `now` is always a parameter (never read from the clock here) so resolution is
//! deterministic and testable.

use chrono::{
    DateTime, Datelike, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
    Weekday,
};
use chrono_english::{Dialect, parse_date_string};
use date_periods::WeekStart;

mod offset;
pub use offset::resolve_offset;

/// Resolve a phrase to an absolute UTC instant, or `None` if it is not a date.
pub fn resolve_datetime<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Utc>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    resolve_zoned(s, now).map(|d| d.with_timezone(&Utc))
}

/// Resolve a phrase to a calendar date (in `now`'s timezone), dropping the time
/// component. Intended for all-day fields.
pub fn resolve_date<Tz>(s: &str, now: DateTime<Tz>) -> Option<NaiveDate>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    resolve_zoned(s, now).map(|d| d.date_naive())
}

/// The whole pipeline, producing an instant in `now`'s timezone.
fn resolve_zoned<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let raw = s.trim();
    if raw.is_empty() || raw.contains('%') {
        return None;
    }

    // 1. Machine formats on the raw string (before lowercasing, which would
    //    mangle RFC 3339's `T`/`Z`).
    if let Ok(d) = DateTime::parse_from_rfc3339(raw) {
        return Some(d.with_timezone(&now.timezone()));
    }

    let s = normalize(raw);
    if s.is_empty() {
        return None;
    }

    // 2. Part-of-day may consume a trailing time word ("tomorrow morning"), so
    //    it runs before the core resolvers.
    if let Some(d) = part_of_day(&s, now) {
        return Some(d);
    }
    resolve_core(&s, now)
}

/// Everything except the part-of-day pass (which recurses into this to resolve
/// the date portion of "tomorrow morning").
fn resolve_core<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    if s == "now" || s == "asap" {
        return Some(now);
    }
    if let Some(d) = in_prefix(s, now) {
        return Some(d);
    }
    if let Some(d) = abbreviation(s, now) {
        return Some(d);
    }
    if let Some(d) = quarter(s, now) {
        return Some(d);
    }
    if let Some(d) = iso_week(s, now) {
        return Some(d);
    }
    if let Some(d) = spoken_time(s, now) {
        return Some(d);
    }
    if let Some(d) = business_days(s, now) {
        return Some(d);
    }
    if let Some(d) = date_periods::resolve(s, now, WeekStart::Monday) {
        return Some(d);
    }
    if let Ok(d) = parse_date_string(s, now, Dialect::Us) {
        return Some(d);
    }
    // Naive date-only, then broad dateparser fallback.
    if let Ok(nd) = s.parse::<NaiveDate>() {
        return build(nd, NaiveTime::from_hms_opt(0, 0, 0)?, now);
    }
    if let Ok(dt) = dateparser::parse(s) {
        return Some(dt.with_timezone(&now.timezone()));
    }
    None
}

// --- preprocessor passes ----------------------------------------------------

/// Lowercase, collapse internal whitespace, and strip leading filler words.
fn normalize(s: &str) -> String {
    let mut t = s.trim().to_lowercase();
    t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let stripped = ["the ", "at ", "on "]
            .iter()
            .find_map(|f| t.strip_prefix(f))
            .map(str::to_string);
        match stripped {
            Some(r) => t = r,
            None => break,
        }
    }
    t
}

/// `in <n> <unit>`: `now` + a duration. `a`/`an` count as 1.
fn in_prefix<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let rest = s.strip_prefix("in ")?;
    let mut it = rest.split_whitespace();
    let num_tok = it.next()?;
    let num: i64 = match num_tok {
        "a" | "an" => 1,
        _ => num_tok.parse().ok()?,
    };
    let unit = it.next()?;
    if it.next().is_some() {
        return None; // e.g. "in 2 business days" — handled elsewhere.
    }
    match unit {
        "s" | "sec" | "secs" | "second" | "seconds" => Some(now + Duration::seconds(num)),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(now + Duration::minutes(num)),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(now + Duration::hours(num)),
        "d" | "day" | "days" => Some(now + Duration::days(num)),
        "w" | "wk" | "week" | "weeks" => Some(now + Duration::weeks(num)),
        "month" | "months" => now.checked_add_months(Months::new(num as u32)),
        "year" | "years" => now.checked_add_months(Months::new(num as u32 * 12)),
        _ => None,
    }
}

/// Part-of-day words → a fixed local time, optionally on a leading date
/// ("friday noon", "tomorrow morning"). Standalone means today.
fn part_of_day<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    const PODS: &[(&str, (u32, u32))] = &[
        ("morning", (9, 0)),
        ("noon", (12, 0)),
        ("midday", (12, 0)),
        ("afternoon", (14, 0)),
        ("evening", (18, 0)),
        ("tonight", (20, 0)),
        ("night", (20, 0)),
        ("midnight", (0, 0)),
    ];
    for (word, (h, m)) in PODS {
        let pre = if s == *word {
            Some("")
        } else {
            s.strip_suffix(&format!(" {word}"))
        };
        let Some(pre) = pre else { continue };
        let date = if pre.is_empty() || pre == "this" || pre == "today" {
            now.date_naive()
        } else {
            resolve_core(pre, now)?.date_naive()
        };
        return build(date, NaiveTime::from_hms_opt(*h, *m, 0)?, now);
    }
    None
}

/// Common workplace abbreviations.
fn abbreviation<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    // Period-boundary abbreviations delegate to date-periods for correctness.
    let phrase = match s {
        "eod" => Some("end of day"),
        "sod" | "bod" => Some("start of day"),
        "eow" => Some("end of week"),
        "sow" => Some("start of week"),
        "eom" => Some("end of month"),
        "som" => Some("start of month"),
        "eoq" => Some("end of quarter"),
        "soq" => Some("start of quarter"),
        "eoy" => Some("end of year"),
        "soy" => Some("start of year"),
        _ => None,
    };
    if let Some(p) = phrase {
        return date_periods::resolve(p, now, WeekStart::Monday);
    }
    // Business-hours abbreviations → a fixed time today.
    let time = match s {
        "cob" | "eob" => (17, 0), // close / end of business
        "sob" | "bob" => (9, 0),  // start / beginning of business
        _ => return None,
    };
    build(
        now.date_naive(),
        NaiveTime::from_hms_opt(time.0, time.1, 0)?,
        now,
    )
}

/// `q3`, `end of q4`, `q1 2027`, `start of q1`. Default boundary is the start.
fn quarter<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let (at_end, rest) = if let Some(r) = s.strip_prefix("end of ") {
        (true, r)
    } else if let Some(r) = s
        .strip_prefix("start of ")
        .or_else(|| s.strip_prefix("beginning of "))
    {
        (false, r)
    } else {
        (false, s)
    };

    let mut it = rest.split_whitespace();
    let qtok = it.next()?;
    let qn: u32 = qtok.strip_prefix('q')?.parse().ok()?;
    if !(1..=4).contains(&qn) {
        return None;
    }
    let year: i32 = match it.next() {
        Some(y) => y.parse().ok()?,
        None => now.year(),
    };
    if it.next().is_some() {
        return None;
    }

    let start_month = (qn - 1) * 3 + 1;
    if at_end {
        let end_month = start_month + 2;
        let last = last_day_of_month(year, end_month)?;
        build(last, NaiveTime::from_hms_opt(23, 59, 59)?, now)
    } else {
        let first = NaiveDate::from_ymd_opt(year, start_month, 1)?;
        build(first, NaiveTime::from_hms_opt(0, 0, 0)?, now)
    }
}

/// ISO week designator `2026-w30` → the Monday of that week.
fn iso_week<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let (y, w) = s.split_once("-w")?;
    let year: i32 = y.parse().ok()?;
    let week: u32 = w.parse().ok()?;
    let d = NaiveDate::from_isoywd_opt(year, week, Weekday::Mon)?;
    build(d, NaiveTime::from_hms_opt(0, 0, 0)?, now)
}

/// `half past 2`, `quarter past 9`, `quarter to 5`, `10 past 8`, `20 to 6`.
fn spoken_time<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let toks: Vec<&str> = s.split_whitespace().collect();
    if toks.len() != 3 || (toks[1] != "past" && toks[1] != "to") {
        return None;
    }
    let mins: u32 = match toks[0] {
        "half" => 30,
        "quarter" => 15,
        other => other.parse().ok()?,
    };
    if mins >= 60 {
        return None;
    }
    let h: u32 = toks[2].parse().ok()?;
    if h >= 24 {
        return None;
    }
    let (hour, minute) = if toks[1] == "past" {
        (h, mins)
    } else {
        (h.checked_sub(1)?, 60 - mins)
    };
    build(
        now.date_naive(),
        NaiveTime::from_hms_opt(hour, minute, 0)?,
        now,
    )
}

/// `next business day`, `in 2 business days`, `in 1 working day`.
fn business_days<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    if s == "next business day" || s == "next working day" {
        let d = add_business_days(now.date_naive(), 1);
        return build(d, NaiveTime::from_hms_opt(0, 0, 0)?, now);
    }
    let rest = s.strip_prefix("in ")?;
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let is_bday = matches!(toks.get(1), Some(&"business") | Some(&"working"))
        && matches!(toks.get(2), Some(&"day") | Some(&"days"));
    if toks.len() != 3 || !is_bday {
        return None;
    }
    let n: i64 = match toks[0] {
        "a" | "an" => 1,
        _ => toks[0].parse().ok()?,
    };
    let d = add_business_days(now.date_naive(), n);
    build(d, now.time(), now)
}

// --- helpers ----------------------------------------------------------------

/// Zone a naive local date+time into `now`'s timezone (earliest valid instant
/// on a DST gap/overlap).
fn build<Tz>(date: NaiveDate, time: NaiveTime, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
{
    let naive = NaiveDateTime::new(date, time);
    now.timezone()
        .from_local_datetime(&naive)
        .earliest()
        .or_else(|| now.timezone().from_local_datetime(&naive).latest())
}

fn last_day_of_month(year: i32, month: u32) -> Option<NaiveDate> {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    Some(NaiveDate::from_ymd_opt(ny, nm, 1)? - Duration::days(1))
}

/// Advance `date` by `n` business days (Mon–Fri), skipping weekends.
fn add_business_days(date: NaiveDate, n: i64) -> NaiveDate {
    let mut d = date;
    let mut remaining = n;
    while remaining > 0 {
        d += Duration::days(1);
        if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
            remaining -= 1;
        }
    }
    d
}
