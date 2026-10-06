//! Resolve natural-language date/time phrases to concrete instants, relative to
//! a caller-supplied reference time, in several languages. App-agnostic: it
//! depends only on `chrono` and the standalone parser crates, never on any
//! application type, so the same resolver backs the TUI form widget, the filter
//! query engine, the trackings adapter and the CLI.
//!
//! # Layers
//!
//! 1. **Machine formats** — RFC 3339 on the raw input (case-sensitive `T`/`Z`).
//! 2. **Languages** ([`Language`]) — each rewrites the phrase into the
//!    canonical (English) vocabulary: `morgen 18 Uhr` → `tomorrow 18:00`,
//!    `Ende nächster Woche` → `end of next week`. A language is a lexicon, not
//!    code.
//! 3. **Meanings** ([`Meaning`]) — passes over the canonical phrase: `in <n>
//!    <unit>`, part-of-day words, day words, abbreviations (`eod`, `cob`),
//!    quarters, ISO weeks, spoken time, business days, dotted dates (`8.10.`)
//!    and `date-periods` boundaries.
//! 4. **Fallbacks** — `chrono-english`, a naive ISO date, [`dateparser`].
//!    They see a language's rewrite only after no meaning answered any
//!    language's, so a broad guess never beats an exact reading.
//!
//! `now` is always a parameter (never read from the clock here) so resolution is
//! deterministic and testable.

use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};

mod lang;
mod meaning;
mod offset;
mod passes;
mod resolver;

pub use date_periods::WeekStart;
pub use lang::{Entry, Language};
pub use meaning::{Context, Meaning, Moment, Pass, Reading};
pub use offset::resolve_offset;
pub use resolver::{Resolution, Resolver};

/// Every built-in language (English, then German), weeks starting on Monday.
static DEFAULT: LazyLock<Resolver> = LazyLock::new(Resolver::new);

/// Resolve a phrase to an absolute UTC instant, or `None` if it is not a date.
/// Understands every built-in language; see [`Resolver`] to choose.
pub fn resolve_datetime<Tz>(s: &str, now: DateTime<Tz>) -> Option<DateTime<Utc>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    DEFAULT.resolve_datetime(s, now)
}

/// Resolve a phrase to a calendar date (in `now`'s timezone), dropping the time
/// component. Intended for all-day fields.
pub fn resolve_date<Tz>(s: &str, now: DateTime<Tz>) -> Option<NaiveDate>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    DEFAULT.resolve_date(s, now)
}

/// Resolve a phrase to an instant plus whether it named a time of day — so a
/// date-only phrase (`morgen`, `8.10.`) can be told from `morgen 0:00`.
pub fn resolve_detailed<Tz>(s: &str, now: DateTime<Tz>) -> Option<Resolution>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    DEFAULT.resolve_detailed(s, now)
}
