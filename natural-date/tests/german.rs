//! German phrases, and the multi-language API around them. `now` is pinned to
//! Tuesday 2026-10-06 10:00:00 UTC.

use chrono::{DateTime, TimeZone, Utc};
use natural_date::{Context, Language, Meaning, Reading, Resolver, WeekStart};

fn n() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 6, 10, 0, 0).unwrap()
}

/// Wall clock plus whether the phrase named a time of day; `"NONE"` when
/// unresolved.
fn de(s: &str) -> String {
    natural_date::resolve_detailed(s, n())
        .map(|r| {
            let t = if r.has_time { "time" } else { "date" };
            format!("{} {t}", r.instant.format("%Y-%m-%d %H:%M:%S"))
        })
        .unwrap_or_else(|| "NONE".into())
}

#[test]
fn german_phrases() {
    let table = [
        // day words
        ("heute", "2026-10-06 00:00:00 date"),
        ("morgen", "2026-10-07 00:00:00 date"),
        ("Morgen", "2026-10-07 00:00:00 date"),
        ("übermorgen", "2026-10-08 00:00:00 date"),
        ("uebermorgen", "2026-10-08 00:00:00 date"),
        ("gestern", "2026-10-05 00:00:00 date"),
        ("vorgestern", "2026-10-04 00:00:00 date"),
        // the instant
        ("jetzt", "2026-10-06 10:00:00 time"),
        ("sofort", "2026-10-06 10:00:00 time"),
        // day + clock time
        ("morgen 18:00", "2026-10-07 18:00:00 time"),
        ("morgen 18 Uhr", "2026-10-07 18:00:00 time"),
        ("morgen um 18:30 Uhr", "2026-10-07 18:30:00 time"),
        ("morgen um 18.30 Uhr", "2026-10-07 18:30:00 time"),
        ("um 18 Uhr", "2026-10-06 18:00:00 time"),
        ("14:30", "2026-10-06 14:30:00 time"),
        // part of day; `morgen` is tomorrow unless the phrase says morning
        ("heute abend", "2026-10-06 18:00:00 time"),
        ("heute morgen", "2026-10-06 09:00:00 time"),
        ("heute früh", "2026-10-06 09:00:00 time"),
        ("heute Nacht", "2026-10-06 20:00:00 time"),
        ("morgen früh", "2026-10-07 09:00:00 time"),
        ("morgen frueh", "2026-10-07 09:00:00 time"),
        ("morgen Vormittag", "2026-10-07 09:00:00 time"),
        ("morgen nachmittag", "2026-10-07 14:00:00 time"),
        ("morgen Abend", "2026-10-07 18:00:00 time"),
        ("mittags", "2026-10-06 12:00:00 time"),
        ("Mitternacht", "2026-10-06 00:00:00 time"),
        ("Feierabend", "2026-10-06 17:00:00 time"),
        // weekdays (today is a Tuesday)
        ("Fr", "2026-10-09 00:00:00 date"),
        ("Fr.", "2026-10-09 00:00:00 date"),
        ("Freitag", "2026-10-09 00:00:00 date"),
        ("am Freitag", "2026-10-09 00:00:00 date"),
        ("Di", "2026-10-06 00:00:00 date"),
        ("Dienstag", "2026-10-06 00:00:00 date"),
        ("Freitag 17 Uhr", "2026-10-09 17:00:00 time"),
        ("am Freitag um 9 Uhr", "2026-10-09 09:00:00 time"),
        ("Freitag abend", "2026-10-09 18:00:00 time"),
        ("diesen Freitag", "2026-10-09 00:00:00 date"),
        ("nächsten Montag", "2026-10-12 00:00:00 date"),
        ("naechsten Montag", "2026-10-12 00:00:00 date"),
        ("letzten Freitag", "2026-10-02 00:00:00 date"),
        ("letzten Dienstag", "2026-09-29 00:00:00 date"),
        // periods
        ("nächste Woche", "2026-10-12 00:00:00 date"),
        ("diese Woche", "2026-10-05 00:00:00 date"),
        ("letzten Monat", "2026-09-01 00:00:00 date"),
        ("Ende nächster Woche", "2026-10-18 23:59:59 date"),
        ("Ende der Woche", "2026-10-11 23:59:59 date"),
        ("Ende des Monats", "2026-10-31 23:59:59 date"),
        ("Monatsende", "2026-10-31 23:59:59 date"),
        ("zum Monatsende", "2026-10-31 23:59:59 date"),
        ("Anfang nächsten Monats", "2026-11-01 00:00:00 date"),
        ("Jahresende", "2026-12-31 23:59:59 date"),
        ("Ende des Quartals", "2026-12-31 23:59:59 date"),
        // spans
        ("in 2 Std", "2026-10-06 12:00:00 time"),
        ("in 2 Stunden", "2026-10-06 12:00:00 time"),
        ("in 30 Minuten", "2026-10-06 10:30:00 time"),
        ("in einer Stunde", "2026-10-06 11:00:00 time"),
        ("in einer halben Stunde", "2026-10-06 10:30:00 time"),
        ("in 3 Tagen", "2026-10-09 10:00:00 date"),
        ("in drei Tagen", "2026-10-09 10:00:00 date"),
        ("in einer Woche", "2026-10-13 10:00:00 date"),
        ("in 2 Wochen", "2026-10-20 10:00:00 date"),
        ("in einem Monat", "2026-11-06 10:00:00 date"),
        ("vor 3 Tagen", "2026-10-03 10:00:00 date"),
        ("vor einer Woche", "2026-09-29 10:00:00 date"),
        // business days
        ("nächster Werktag", "2026-10-07 00:00:00 date"),
        ("in 2 Werktagen", "2026-10-08 10:00:00 date"),
        // dotted dates, day first
        ("8.10.", "2026-10-08 00:00:00 date"),
        ("08.10.2026", "2026-10-08 00:00:00 date"),
        ("8.10.26", "2026-10-08 00:00:00 date"),
        ("08.10.2026 14:30", "2026-10-08 14:30:00 time"),
        ("am 8.10. um 18 Uhr", "2026-10-08 18:00:00 time"),
        ("31.2.", "NONE"),
        // month names
        ("8. Oktober", "2026-10-08 00:00:00 date"),
        ("8. Okt 2026 14:30", "2026-10-08 14:30:00 time"),
        ("1. März 2027", "2027-03-01 00:00:00 date"),
        // spoken time (24-hour, like the English `half past 2`)
        ("halb 3", "2026-10-06 02:30:00 time"),
        ("viertel nach 5", "2026-10-06 05:15:00 time"),
        ("viertel vor 5", "2026-10-06 04:45:00 time"),
        ("dreiviertel 5", "2026-10-06 04:45:00 time"),
        ("10 nach 8", "2026-10-06 08:10:00 time"),
        ("10 vor 8", "2026-10-06 07:50:00 time"),
    ];
    let failures: Vec<String> = table
        .iter()
        .filter_map(|(phrase, want)| {
            let got = de(phrase);
            (got != *want).then(|| format!("{phrase:?}: want {want}, got {got}"))
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn english_is_unchanged_by_german() {
    // Words German claims for itself mean nothing different in English.
    assert_eq!(de("tomorrow morning"), "2026-10-07 09:00:00 time");
    assert_eq!(de("so"), "2026-10-11 00:00:00 date"); // German "So" = Sunday
    assert_eq!(de("next monday"), "2026-10-12 00:00:00 date");
    assert_eq!(de("5pm"), "2026-10-06 17:00:00 time");
    assert_eq!(de("2 weeks"), "2026-10-20 10:00:00 date");
    assert_eq!(de("day after tomorrow"), "2026-10-08 00:00:00 date");
    assert_eq!(de("this friday"), "2026-10-09 00:00:00 date");
}

#[test]
fn resolver_with_english_only_ignores_german() {
    let en = Resolver::new().languages([Language::english()]);
    assert_eq!(en.resolve_datetime("morgen", n()), None);
    assert!(en.resolve_datetime("tomorrow", n()).is_some());
    let de = Resolver::new().languages([Language::german()]);
    assert_eq!(
        de.resolve_date("übermorgen", n()),
        chrono::NaiveDate::from_ymd_opt(2026, 10, 8)
    );
}

#[test]
fn a_language_from_outside_the_crate() {
    static FR: &[natural_date::Entry] = &[("demain", "tomorrow"), ("{n} h", "{n}:00"), ("à", "")];
    let r = Resolver::new().language(Language::new("fr", FR));
    assert_eq!(
        r.resolve_datetime("demain à 18 h", n()),
        Some(Utc.with_ymd_and_hms(2026, 10, 7, 18, 0, 0).unwrap())
    );
}

#[test]
fn a_meaning_serves_every_language() {
    // A new canonical phrase, reachable from German by a lexicon entry alone.
    static DE_EXTRA: &[natural_date::Entry] = &[("zahltag", "payday")];
    fn payday(s: &str, cx: &Context) -> Option<Reading> {
        use chrono::Datelike;
        (s == "payday").then(|| Reading::day(cx.today().with_day(25).unwrap()))
    }
    let r = Resolver::new()
        .meaning(Meaning::new("payday", payday))
        .language(Language::new("de-extra", DE_EXTRA));
    let want = chrono::NaiveDate::from_ymd_opt(2026, 10, 25);
    assert_eq!(r.resolve_date("payday", n()), want);
    assert_eq!(r.resolve_date("Zahltag", n()), want);
}

#[test]
fn week_start_is_configurable() {
    let sunday = Resolver::new().week_start(WeekStart::Sunday);
    assert_eq!(
        sunday.resolve_date("Ende der Woche", n()),
        chrono::NaiveDate::from_ymd_opt(2026, 10, 10)
    );
}

#[test]
fn has_time_on_english_phrases() {
    assert_eq!(de("tomorrow"), "2026-10-07 00:00:00 date");
    assert_eq!(de("tomorrow 0:00"), "2026-10-07 00:00:00 time");
    assert_eq!(de("in 2 hours"), "2026-10-06 12:00:00 time");
    assert_eq!(de("eod"), "2026-10-06 23:59:59 date");
    assert_eq!(de("cob"), "2026-10-06 17:00:00 time");
    assert_eq!(de("2026-10-06T08:00:00Z"), "2026-10-06 08:00:00 time");
}
