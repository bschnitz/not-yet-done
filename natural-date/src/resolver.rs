//! The resolver: languages × meanings × fallbacks.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_english::{Dialect, parse_date_string};
use date_periods::WeekStart;

use crate::lang::Language;
use crate::meaning::{Context, Meaning, Moment, Reading};

/// A resolved phrase: the instant, and whether the phrase named a time of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// The instant, in UTC.
    pub instant: DateTime<Utc>,
    /// `false` when the phrase named only a date (`tomorrow`, `8.10.`, `end of
    /// month`, `in 3 days`): the instant's time is then the crate's convention
    /// — midnight, `23:59:59` for the end of a period, the reference time for
    /// a span of days — and a caller with an all-day notion may widen it.
    pub has_time: bool,
}

/// Resolves phrases in a set of languages. [`Resolver::new`] understands every
/// built-in language (English first, then German) with weeks starting on
/// Monday; the free functions of this crate use exactly that.
#[derive(Clone, Debug)]
pub struct Resolver {
    languages: Vec<Language>,
    meanings: Vec<Meaning>,
    week_start: WeekStart,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    /// Every built-in language and meaning, weeks starting on Monday.
    pub fn new() -> Self {
        Self {
            languages: Language::builtin(),
            meanings: Meaning::builtin(),
            week_start: WeekStart::Monday,
        }
    }

    /// Only these languages, tried in this order.
    pub fn languages(mut self, languages: impl IntoIterator<Item = Language>) -> Self {
        self.languages = languages.into_iter().collect();
        self
    }

    /// Add a language after the ones already there.
    pub fn language(mut self, language: Language) -> Self {
        self.languages.push(language);
        self
    }

    /// Add a meaning. It is tried before the built-in ones, so it may also
    /// override how a canonical phrase is read.
    pub fn meaning(mut self, meaning: Meaning) -> Self {
        self.meanings.insert(0, meaning);
        self
    }

    /// The first day of the week (`end of week`, `next week`).
    pub fn week_start(mut self, week_start: WeekStart) -> Self {
        self.week_start = week_start;
        self
    }

    /// Resolve a phrase to an instant plus whether it named a time of day.
    ///
    /// Every language rewrites the phrase into the canonical vocabulary first.
    /// The meanings then get each language's rewrite in turn, and only when no
    /// meaning answered any of them do the broad fallbacks
    /// (`chrono-english`, [`dateparser`]) get them — which keeps a fallback's
    /// guess at one language's words (`monat` read as Monday) from beating an
    /// exact reading in another.
    pub fn resolve_detailed<Tz>(&self, s: &str, now: DateTime<Tz>) -> Option<Resolution>
    where
        Tz: TimeZone,
        Tz::Offset: Copy,
    {
        let raw = s.trim();
        if raw.is_empty() || raw.contains('%') {
            return None;
        }
        // Machine formats on the raw string (lowercasing would mangle RFC
        // 3339's `T`/`Z`).
        if let Ok(d) = DateTime::parse_from_rfc3339(raw) {
            return Some(Resolution {
                instant: d.with_timezone(&Utc),
                has_time: true,
            });
        }

        let mut phrases: Vec<String> = Vec::new();
        for language in &self.languages {
            let p = canonical(&language.rewrite(raw));
            if !p.is_empty() && !phrases.contains(&p) {
                phrases.push(p);
            }
        }
        let reading = phrases
            .iter()
            .find_map(|p| self.by_meaning(p, now))
            .or_else(|| phrases.iter().find_map(|p| fallback(p, now)))?;
        Some(Resolution {
            instant: instant(reading.at, now)?.with_timezone(&Utc),
            has_time: reading.has_time,
        })
    }

    /// Resolve a phrase to an absolute UTC instant, or `None` if it is not a
    /// date.
    pub fn resolve_datetime<Tz>(&self, s: &str, now: DateTime<Tz>) -> Option<DateTime<Utc>>
    where
        Tz: TimeZone,
        Tz::Offset: Copy,
    {
        self.resolve_detailed(s, now).map(|r| r.instant)
    }

    /// Resolve a phrase to a calendar date in `now`'s time zone, dropping the
    /// time component. Intended for all-day fields.
    pub fn resolve_date<Tz>(&self, s: &str, now: DateTime<Tz>) -> Option<NaiveDate>
    where
        Tz: TimeZone,
        Tz::Offset: Copy,
    {
        self.resolve_detailed(s, now)
            .map(|r| r.instant.with_timezone(&now.timezone()).date_naive())
    }

    /// The meanings only, on one canonical phrase.
    fn by_meaning<Tz>(&self, phrase: &str, now: DateTime<Tz>) -> Option<Reading>
    where
        Tz: TimeZone,
        Tz::Offset: Copy,
    {
        let recurse = |p: &str| self.full(p, now);
        let cx = Context {
            now: now.naive_local(),
            week_start: self.week_start,
            resolve: &recurse,
        };
        self.meanings.iter().find_map(|m| m.read(phrase, &cx))
    }

    /// Meanings, then fallbacks, on one canonical phrase — what a pass's
    /// [`Context::resolve`] reaches.
    fn full<Tz>(&self, phrase: &str, now: DateTime<Tz>) -> Option<Reading>
    where
        Tz: TimeZone,
        Tz::Offset: Copy,
    {
        let p = canonical(phrase);
        self.by_meaning(&p, now).or_else(|| fallback(&p, now))
    }
}

/// Collapse whitespace and strip leading filler words of the canonical
/// vocabulary (`the`, `at`, `on`).
fn canonical(s: &str) -> String {
    let mut t = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    while let Some(r) = ["the ", "at ", "on "]
        .iter()
        .find_map(|f| t.strip_prefix(f))
    {
        t = r.to_string();
    }
    t
}

/// The broad parsers: `chrono-english`, a naive ISO date, [`dateparser`].
fn fallback<Tz>(s: &str, now: DateTime<Tz>) -> Option<Reading>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    if let Ok(d) = parse_date_string(s, now, Dialect::Us) {
        return Some(guessed(s, d, &now));
    }
    if let Ok(nd) = s.parse::<NaiveDate>() {
        return Some(Reading::day(nd));
    }
    if let Ok(d) = dateparser::parse(s) {
        let d = d.with_timezone(&now.timezone());
        return Some(guessed(s, d, &now));
    }
    None
}

/// A fallback's answer. These parsers do not say whether the phrase named a
/// time, so it is inferred: a clock token in the phrase, or a time of day that
/// is neither midnight nor the reference time (`2 hours` moves the clock, `2
/// weeks` and `next monday` do not).
fn guessed<Tz>(s: &str, d: DateTime<Tz>, now: &DateTime<Tz>) -> Reading
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let t = d.naive_local().time();
    let moved = t.num_seconds_from_midnight() != 0 && t != now.naive_local().time();
    Reading::after(d.signed_duration_since(*now), moved || names_a_clock(s))
}

fn names_a_clock(s: &str) -> bool {
    s.split_whitespace().any(|t| {
        let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
        matches!(t, "am" | "pm" | "noon" | "midnight")
            || t.split_once(':').is_some_and(|(h, m)| {
                digits(h) && m.bytes().next().is_some_and(|b| b.is_ascii_digit())
            })
            || t.strip_suffix("am")
                .or_else(|| t.strip_suffix("pm"))
                .is_some_and(digits)
    })
}

/// A reading in `now`'s time zone.
fn instant<Tz>(at: Moment, now: DateTime<Tz>) -> Option<DateTime<Tz>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    match at {
        Moment::After(d) => now.checked_add_signed(d),
        Moment::Wall(w) => zone(w, &now),
    }
}

/// Zone a wall clock into `now`'s time zone (earliest valid instant on a DST
/// gap/overlap).
fn zone<Tz: TimeZone>(naive: NaiveDateTime, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    let tz = now.timezone();
    tz.from_local_datetime(&naive)
        .earliest()
        .or_else(|| tz.from_local_datetime(&naive).latest())
}
