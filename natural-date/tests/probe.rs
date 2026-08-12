//! Baseline probe: what do chrono-english + date-periods handle *today*, before
//! any preprocessing? Run with `cargo test -p natural-date --test probe -- --nocapture`.
//!
//! This is not an assertion suite — it prints a coverage table so we can see
//! exactly which target phrasings already resolve and which need the new
//! preprocessor layer. `now` is pinned to a fixed offset so it is reproducible.

use chrono::{FixedOffset, TimeZone};
use chrono_english::{Dialect, parse_date_string};

fn now() -> chrono::DateTime<FixedOffset> {
    // Saturday 2026-07-18 10:00:00 +02:00
    FixedOffset::east_opt(2 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 7, 18, 10, 0, 0)
        .unwrap()
}

/// Try both resolvers, return the resolved local wall-clock, or None.
fn baseline(s: &str) -> Option<String> {
    let n = now();
    if let Some(dt) = date_periods::resolve(s, n, date_periods::WeekStart::Monday) {
        return Some(format!(
            "{}  [date-periods]",
            dt.format("%a %Y-%m-%d %H:%M")
        ));
    }
    if let Ok(dt) = parse_date_string(s, n, Dialect::Us) {
        return Some(format!(
            "{}  [chrono-english]",
            dt.format("%a %Y-%m-%d %H:%M")
        ));
    }
    None
}

#[test]
fn probe_baseline_coverage() {
    let phrases = [
        // --- native chrono-english (expected OK) ---
        "today",
        "tomorrow",
        "yesterday",
        "next monday",
        "next friday 8pm",
        "2 weeks",
        "3 days",
        "friday",
        // --- date-periods (expected OK) ---
        "end of next week",
        "start of month",
        "end of quarter",
        "end of the week",
        // --- Stage A: in-prefix (expected GAP) ---
        "in 2 hours",
        "in 30 min",
        "in 3 days",
        "in 1 week",
        // --- Stage A: part-of-day (expected GAP) ---
        "morning",
        "noon",
        "afternoon",
        "evening",
        "tonight",
        "midnight",
        "tomorrow morning",
        "monday evening",
        "friday noon",
        // --- Stage A: abbreviations (expected GAP) ---
        "eod",
        "sod",
        "bod",
        "eow",
        "eom",
        "eoy",
        "cob",
        // --- Stage B: fillers / articles (mixed) ---
        "at 5pm",
        "on friday",
        "the end of the week",
        // --- Stage B: quarter shorthand (expected GAP) ---
        "q3",
        "end of q4",
        "q1 2027",
        // --- Stage B: now/asap (mixed) ---
        "now",
        "asap",
        // --- absolute (expected OK via chrono-english / iso) ---
        "2026-07-20",
        "2026-07-20 09:15",
        "5pm",
    ];

    println!("\n=== baseline coverage (chrono-english + date-periods) ===");
    let mut gaps = Vec::new();
    for p in phrases {
        match baseline(p) {
            Some(r) => println!("  OK    {:<22} -> {r}", format!("'{p}'")),
            None => {
                println!("  GAP   '{p}'");
                gaps.push(p);
            }
        }
    }
    println!("\n{} gap(s): {:?}\n", gaps.len(), gaps);
}
