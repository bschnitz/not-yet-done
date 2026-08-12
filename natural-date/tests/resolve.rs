//! Behavioural spec for `natural-date`. `now` is pinned to
//! Saturday 2026-07-18 10:00:00 UTC so every expectation is reproducible.

use chrono::{DateTime, Datelike, TimeZone, Utc};

fn n() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 18, 10, 0, 0).unwrap()
}

/// Resolve to a datetime and format its wall-clock (now is UTC, so the result
/// wall-clock equals the local one). `"NONE"` when unresolved.
fn dt(s: &str) -> String {
    natural_date::resolve_datetime(s, n())
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "NONE".into())
}

fn date(s: &str) -> String {
    natural_date::resolve_date(s, n())
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "NONE".into())
}

// --- native passthrough (must keep working) --------------------------------

#[test]
fn native_chrono_english_still_resolves() {
    assert_eq!(dt("next monday"), "2026-07-20 00:00:00");
    assert_eq!(dt("next friday 8pm"), "2026-07-24 20:00:00");
    assert_eq!(dt("2026-07-20 09:15"), "2026-07-20 09:15:00");
    assert_eq!(dt("5pm"), "2026-07-18 17:00:00");
}

#[test]
fn native_date_periods_still_resolves() {
    assert_eq!(dt("end of next week"), "2026-07-26 23:59:59");
    assert_eq!(dt("start of month"), "2026-07-01 00:00:00");
    assert_eq!(dt("end of quarter"), "2026-09-30 23:59:59");
}

#[test]
fn unresolvable_is_none() {
    assert_eq!(dt("not a date at all"), "NONE");
    assert_eq!(dt(""), "NONE");
    assert_eq!(dt("%Y-%m-%d"), "NONE"); // strftime pattern, never a date
}

// --- Stage A: `in X <unit>` ------------------------------------------------

#[test]
fn in_prefix_relative_future() {
    assert_eq!(dt("in 2 hours"), "2026-07-18 12:00:00");
    assert_eq!(dt("in 30 min"), "2026-07-18 10:30:00");
    assert_eq!(dt("in 90 minutes"), "2026-07-18 11:30:00");
    assert_eq!(dt("in 3 days"), "2026-07-21 10:00:00");
    assert_eq!(dt("in 1 week"), "2026-07-25 10:00:00");
    assert_eq!(dt("in an hour"), "2026-07-18 11:00:00");
    assert_eq!(dt("in a week"), "2026-07-25 10:00:00");
    assert_eq!(dt("in 2 months"), "2026-09-18 10:00:00");
}

// --- Stage A: part-of-day --------------------------------------------------

#[test]
fn part_of_day_standalone_is_today() {
    assert_eq!(dt("morning"), "2026-07-18 09:00:00");
    assert_eq!(dt("noon"), "2026-07-18 12:00:00");
    assert_eq!(dt("midday"), "2026-07-18 12:00:00");
    assert_eq!(dt("afternoon"), "2026-07-18 14:00:00");
    assert_eq!(dt("evening"), "2026-07-18 18:00:00");
    assert_eq!(dt("tonight"), "2026-07-18 20:00:00");
    assert_eq!(dt("midnight"), "2026-07-18 00:00:00");
}

#[test]
fn part_of_day_combined_with_date() {
    assert_eq!(dt("tomorrow morning"), "2026-07-19 09:00:00");
    assert_eq!(dt("monday evening"), "2026-07-20 18:00:00");
    assert_eq!(dt("friday noon"), "2026-07-24 12:00:00");
    assert_eq!(dt("next monday morning"), "2026-07-20 09:00:00");
}

// --- Stage A: abbreviations ------------------------------------------------

#[test]
fn abbreviations_expand() {
    assert_eq!(dt("eod"), "2026-07-18 23:59:59");
    assert_eq!(dt("sod"), "2026-07-18 00:00:00");
    assert_eq!(dt("bod"), "2026-07-18 00:00:00");
    assert_eq!(dt("eow"), "2026-07-19 23:59:59"); // Sunday, ISO week end
    assert_eq!(dt("eom"), "2026-07-31 23:59:59");
    assert_eq!(dt("eoy"), "2026-12-31 23:59:59");
    assert_eq!(dt("cob"), "2026-07-18 17:00:00"); // close of business
    assert_eq!(dt("eob"), "2026-07-18 17:00:00");
    assert_eq!(dt("sob"), "2026-07-18 09:00:00"); // start of business
}

// --- Stage B: fillers / articles -------------------------------------------

#[test]
fn filler_words_are_tolerated() {
    assert_eq!(dt("at 5pm"), "2026-07-18 17:00:00");
    assert_eq!(dt("on friday"), "2026-07-24 00:00:00");
    assert_eq!(dt("the end of the week"), "2026-07-19 23:59:59");
    assert_eq!(dt("  tomorrow   morning "), "2026-07-19 09:00:00");
}

// --- Stage B: quarter shorthand --------------------------------------------

#[test]
fn quarter_shorthand() {
    assert_eq!(dt("q3"), "2026-07-01 00:00:00"); // start of Q3 this year
    assert_eq!(dt("end of q4"), "2026-12-31 23:59:59");
    assert_eq!(dt("q1 2027"), "2027-01-01 00:00:00");
    assert_eq!(dt("start of q1"), "2026-01-01 00:00:00");
}

// --- Stage B: now / asap ---------------------------------------------------

#[test]
fn now_and_asap() {
    assert_eq!(dt("now"), "2026-07-18 10:00:00");
    assert_eq!(dt("asap"), "2026-07-18 10:00:00");
}

// --- Stage C: ISO week / spoken time / business days -----------------------

#[test]
fn iso_week() {
    // Monday of ISO week 30, 2026 — computed with chrono so the test can't drift.
    let expected = chrono::NaiveDate::from_isoywd_opt(2026, 30, chrono::Weekday::Mon).unwrap();
    assert_eq!(
        dt("2026-w30"),
        format!("{} 00:00:00", expected.format("%Y-%m-%d"))
    );
}

#[test]
fn spoken_time() {
    assert_eq!(dt("half past 2"), "2026-07-18 02:30:00");
    assert_eq!(dt("quarter past 9"), "2026-07-18 09:15:00");
    assert_eq!(dt("quarter to 5"), "2026-07-18 04:45:00");
}

#[test]
fn business_days() {
    // Saturday -> next business day is Monday.
    assert_eq!(dt("next business day"), "2026-07-20 00:00:00");
    // From Saturday, +2 business days = Mon(1), Tue(2).
    assert_eq!(dt("in 2 business days"), "2026-07-21 10:00:00");
    assert_eq!(dt("in 1 working day"), "2026-07-20 10:00:00");
}

// --- resolve_date (all-day fields) -----------------------------------------

#[test]
fn resolve_date_strips_time() {
    assert_eq!(date("tomorrow"), "2026-07-19");
    assert_eq!(date("next monday"), "2026-07-20");
    assert_eq!(date("2026-07-20"), "2026-07-20");
    assert_eq!(date("eom"), "2026-07-31");
    assert_eq!(date("q1 2027"), "2027-01-01");
}

// --- resolve_offset (the +1h / -30min grammar) -----------------------------

#[test]
fn offset_grammar() {
    use chrono::Duration;
    assert_eq!(
        natural_date::resolve_offset("+1h"),
        Some(Duration::hours(1))
    );
    assert_eq!(
        natural_date::resolve_offset("-30min"),
        Some(Duration::minutes(-30))
    );
    assert_eq!(
        natural_date::resolve_offset("+2days"),
        Some(Duration::days(2))
    );
    assert_eq!(
        natural_date::resolve_offset("+90m"),
        Some(Duration::minutes(90))
    );
    // Sign required (mirrors the original trackings grammar).
    assert_eq!(natural_date::resolve_offset("1h"), None);
    assert_eq!(natural_date::resolve_offset("+5"), None);
    assert_eq!(natural_date::resolve_offset("+5fortnights"), None);
}

// --- timezone handling ------------------------------------------------------

#[test]
fn respects_reference_timezone() {
    // now in +02:00; "noon" is 12:00 *local* -> 10:00 UTC.
    let now = chrono::FixedOffset::east_opt(2 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 7, 18, 10, 0, 0)
        .unwrap();
    let got = natural_date::resolve_datetime("noon", now).unwrap();
    assert_eq!(got.year(), 2026);
    assert_eq!(got, Utc.with_ymd_and_hms(2026, 7, 18, 10, 0, 0).unwrap());
}
