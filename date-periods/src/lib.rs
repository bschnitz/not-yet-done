//! Resolve natural-language *period-boundary* phrases to concrete dates.
//!
//! This crate fills the gap left by relative-date parsers such as
//! `chrono-english`, which understand `today` / `2 weeks` / `next monday` but
//! not calendar *period boundaries* like `end of next week` or `start of month`.
//!
//! # Grammar
//!
//! ```text
//! <boundary> of [the] [<rel>] <unit>
//!
//! boundary := start | beginning | end
//! rel      := this | next | last | previous     (optional; default: this)
//! unit     := day | week | month | quarter | year
//! ```
//!
//! Examples: `end of next week`, `start of month`, `beginning of last quarter`,
//! `end of the year`.
//!
//! # Usage
//!
//! ```
//! use chrono::{Local, TimeZone};
//! use date_periods::{resolve, WeekStart};
//!
//! let now = Local.with_ymd_and_hms(2026, 7, 9, 12, 0, 0).unwrap(); // a Thursday
//! let eonw = resolve("end of next week", now, WeekStart::Monday).unwrap();
//! assert_eq!(eonw.date_naive().to_string(), "2026-07-19"); // the coming-after Sunday
//! ```
//!
//! # Extending
//!
//! Adding a phrase is a matter of extending the small [`Unit`] / [`Rel`] /
//! [`Boundary`] enums and their token tables — the [`PeriodSpec::range`]
//! computation is driven entirely by those enums, so a new unit only needs its
//! date range implemented in one place.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone};

/// Which weekday a week starts on — governs `start`/`end of week`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeekStart {
    Monday,
    Sunday,
}

/// Which end of a period a phrase refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    /// The first day of the period, at `00:00:00`.
    Start,
    /// The last day of the period, at `23:59:59`.
    End,
}

/// Which occurrence of the period relative to "now".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rel {
    This,
    Next,
    Last,
}

/// The calendar unit a period spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

/// A fully parsed period-boundary phrase, independent of any concrete instant.
///
/// Parse a phrase into this with [`parse`], then turn it into a date with
/// [`PeriodSpec::range`] / [`PeriodSpec::boundary_date`], or straight into a
/// zoned instant with [`resolve`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeriodSpec {
    pub boundary: Boundary,
    pub rel: Rel,
    pub unit: Unit,
}

/// Parse a period-boundary phrase (case-insensitive, whitespace-tolerant).
///
/// Returns `None` if the phrase is not a recognised period boundary — callers
/// typically fall through to another parser (e.g. `chrono-english`) on `None`.
pub fn parse(phrase: &str) -> Option<PeriodSpec> {
    let lower = phrase.trim().to_lowercase();
    let mut tokens = lower.split_whitespace();

    let boundary = match tokens.next()? {
        "start" | "beginning" => Boundary::Start,
        "end" => Boundary::End,
        _ => return None,
    };

    // Require the connective `of`.
    if tokens.next()? != "of" {
        return None;
    }

    // Optional filler `the`.
    let mut next = tokens.next()?;
    if next == "the" {
        next = tokens.next()?;
    }

    // Optional relative qualifier; if absent, `next` already holds the unit.
    let (rel, unit_token) = match next {
        "this" | "current" => (Rel::This, tokens.next()?),
        "next" => (Rel::Next, tokens.next()?),
        "last" | "previous" => (Rel::Last, tokens.next()?),
        other => (Rel::This, other),
    };

    let unit = match unit_token {
        "day" => Unit::Day,
        "week" => Unit::Week,
        "month" => Unit::Month,
        "quarter" => Unit::Quarter,
        "year" => Unit::Year,
        _ => return None,
    };

    // Reject trailing junk so "end of week foo" doesn't silently succeed.
    if tokens.next().is_some() {
        return None;
    }

    Some(PeriodSpec {
        boundary,
        rel,
        unit,
    })
}

impl PeriodSpec {
    /// The `[first, last]` day span of the referenced period, relative to
    /// `today`. `week_start` only affects [`Unit::Week`].
    pub fn range(&self, today: NaiveDate, week_start: WeekStart) -> (NaiveDate, NaiveDate) {
        match self.unit {
            Unit::Day => {
                let d = shift_days(today, self.rel, 1);
                (d, d)
            }
            Unit::Week => {
                let start = week_start_date(today, week_start);
                let start = shift_days(start, self.rel, 7);
                (start, start + Duration::days(6))
            }
            Unit::Month => {
                let first = shift_months(first_of_month(today), self.rel, 1);
                (first, last_of_month(first))
            }
            Unit::Quarter => {
                let first = shift_months(first_of_quarter(today), self.rel, 3);
                // Last day of the 3-month span: day before the next quarter.
                (first, add_months(first, 3) - Duration::days(1))
            }
            Unit::Year => {
                let first = shift_years(first_of_year(today), self.rel);
                let last = NaiveDate::from_ymd_opt(first.year(), 12, 31).unwrap();
                (first, last)
            }
        }
    }

    /// The single boundary date: the first day for [`Boundary::Start`], the last
    /// day for [`Boundary::End`].
    pub fn boundary_date(&self, today: NaiveDate, week_start: WeekStart) -> NaiveDate {
        let (first, last) = self.range(today, week_start);
        match self.boundary {
            Boundary::Start => first,
            Boundary::End => last,
        }
    }

    /// The time-of-day this boundary implies: `00:00:00` for a start,
    /// `23:59:59` for an end.
    pub fn boundary_time(&self) -> NaiveTime {
        match self.boundary {
            Boundary::Start => NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            Boundary::End => NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
        }
    }
}

/// Resolve a period-boundary phrase to a concrete zoned instant, relative to
/// `now`. The result carries `now`'s time zone. Returns `None` if the phrase is
/// not a recognised period boundary.
///
/// A `start` resolves to `00:00:00` of the boundary day, an `end` to
/// `23:59:59`. On a DST gap/overlap the earliest valid instant is chosen.
pub fn resolve<Tz: TimeZone>(
    phrase: &str,
    now: DateTime<Tz>,
    week_start: WeekStart,
) -> Option<DateTime<Tz>> {
    let spec = parse(phrase)?;
    let today = now.date_naive();
    let date = spec.boundary_date(today, week_start);
    let naive = date.and_time(spec.boundary_time());
    now.timezone()
        .from_local_datetime(&naive)
        .earliest()
        .or_else(|| now.timezone().from_local_datetime(&naive).latest())
}

// --- date arithmetic helpers ------------------------------------------------

fn shift_days(base: NaiveDate, rel: Rel, step: i64) -> NaiveDate {
    match rel {
        Rel::This => base,
        Rel::Next => base + Duration::days(step),
        Rel::Last => base - Duration::days(step),
    }
}

fn week_start_date(today: NaiveDate, week_start: WeekStart) -> NaiveDate {
    let offset = match week_start {
        WeekStart::Monday => today.weekday().num_days_from_monday(),
        WeekStart::Sunday => today.weekday().num_days_from_sunday(),
    };
    today - Duration::days(offset as i64)
}

fn first_of_month(d: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap()
}

fn last_of_month(first: NaiveDate) -> NaiveDate {
    add_months(first, 1) - Duration::days(1)
}

fn first_of_quarter(d: NaiveDate) -> NaiveDate {
    let q_first_month = ((d.month() - 1) / 3) * 3 + 1;
    NaiveDate::from_ymd_opt(d.year(), q_first_month, 1).unwrap()
}

fn first_of_year(d: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year(), 1, 1).unwrap()
}

fn shift_months(first_of_period: NaiveDate, rel: Rel, step: u32) -> NaiveDate {
    match rel {
        Rel::This => first_of_period,
        Rel::Next => add_months(first_of_period, step),
        Rel::Last => sub_months(first_of_period, step),
    }
}

fn shift_years(first_of_year: NaiveDate, rel: Rel) -> NaiveDate {
    let y = match rel {
        Rel::This => first_of_year.year(),
        Rel::Next => first_of_year.year() + 1,
        Rel::Last => first_of_year.year() - 1,
    };
    NaiveDate::from_ymd_opt(y, 1, 1).unwrap()
}

/// Month arithmetic on a first-of-month date. Both operate purely on a
/// (year, month) index so month-length differences never matter — the input is
/// always day 1, and the result is day 1 of the shifted month.
fn add_months(first: NaiveDate, n: u32) -> NaiveDate {
    shift_month_index(first, n as i32)
}

fn sub_months(first: NaiveDate, n: u32) -> NaiveDate {
    shift_month_index(first, -(n as i32))
}

fn shift_month_index(first: NaiveDate, delta: i32) -> NaiveDate {
    let total = first.year() * 12 + (first.month() as i32 - 1) + delta;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) as u32 + 1;
    NaiveDate::from_ymd_opt(year, month, 1).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    // 2026-07-09 is a Thursday.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 9).unwrap()
    }

    fn spec(p: &str) -> PeriodSpec {
        parse(p).unwrap()
    }

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn parse_variants() {
        assert_eq!(
            spec("end of next week"),
            PeriodSpec {
                boundary: Boundary::End,
                rel: Rel::Next,
                unit: Unit::Week
            }
        );
        assert_eq!(spec("Start Of Month").rel, Rel::This);
        assert_eq!(
            spec("beginning of the last quarter").boundary,
            Boundary::Start
        );
        assert_eq!(spec("beginning of the last quarter").rel, Rel::Last);
        assert!(parse("sometime soon").is_none());
        assert!(parse("end of week foo").is_none());
        assert!(parse("middle of week").is_none());
    }

    #[test]
    fn week_boundaries_monday_start() {
        // This week: Mon 07-06 .. Sun 07-12.
        assert_eq!(
            spec("start of week").boundary_date(today(), WeekStart::Monday),
            d("2026-07-06")
        );
        assert_eq!(
            spec("end of week").boundary_date(today(), WeekStart::Monday),
            d("2026-07-12")
        );
        // Next week: Mon 07-13 .. Sun 07-19.
        assert_eq!(
            spec("start of next week").boundary_date(today(), WeekStart::Monday),
            d("2026-07-13")
        );
        assert_eq!(
            spec("end of next week").boundary_date(today(), WeekStart::Monday),
            d("2026-07-19")
        );
        // Last week: Mon 06-29 .. Sun 07-05.
        assert_eq!(
            spec("end of last week").boundary_date(today(), WeekStart::Monday),
            d("2026-07-05")
        );
    }

    #[test]
    fn week_boundaries_sunday_start() {
        // Sunday-start week containing Thu 07-09: Sun 07-05 .. Sat 07-11.
        assert_eq!(
            spec("start of week").boundary_date(today(), WeekStart::Sunday),
            d("2026-07-05")
        );
        assert_eq!(
            spec("end of week").boundary_date(today(), WeekStart::Sunday),
            d("2026-07-11")
        );
    }

    #[test]
    fn month_boundaries() {
        assert_eq!(
            spec("start of month").boundary_date(today(), WeekStart::Monday),
            d("2026-07-01")
        );
        assert_eq!(
            spec("end of month").boundary_date(today(), WeekStart::Monday),
            d("2026-07-31")
        );
        assert_eq!(
            spec("end of next month").boundary_date(today(), WeekStart::Monday),
            d("2026-08-31")
        );
        assert_eq!(
            spec("start of last month").boundary_date(today(), WeekStart::Monday),
            d("2026-06-01")
        );
        // December → next month rolls the year.
        let dec = NaiveDate::from_ymd_opt(2026, 12, 15).unwrap();
        assert_eq!(
            spec("start of next month").boundary_date(dec, WeekStart::Monday),
            d("2027-01-01")
        );
    }

    #[test]
    fn quarter_and_year_boundaries() {
        // Q3 2026 = Jul..Sep.
        assert_eq!(
            spec("start of quarter").boundary_date(today(), WeekStart::Monday),
            d("2026-07-01")
        );
        assert_eq!(
            spec("end of quarter").boundary_date(today(), WeekStart::Monday),
            d("2026-09-30")
        );
        assert_eq!(
            spec("end of last quarter").boundary_date(today(), WeekStart::Monday),
            d("2026-06-30")
        );
        assert_eq!(
            spec("end of next quarter").boundary_date(today(), WeekStart::Monday),
            d("2026-12-31")
        );
        assert_eq!(
            spec("start of year").boundary_date(today(), WeekStart::Monday),
            d("2026-01-01")
        );
        assert_eq!(
            spec("end of year").boundary_date(today(), WeekStart::Monday),
            d("2026-12-31")
        );
        assert_eq!(
            spec("end of next year").boundary_date(today(), WeekStart::Monday),
            d("2027-12-31")
        );
    }

    #[test]
    fn day_boundaries() {
        assert_eq!(
            spec("start of day").boundary_date(today(), WeekStart::Monday),
            today()
        );
        assert_eq!(
            spec("end of next day").boundary_date(today(), WeekStart::Monday),
            d("2026-07-10")
        );
        assert_eq!(
            spec("start of last day").boundary_date(today(), WeekStart::Monday),
            d("2026-07-08")
        );
        assert!(parse("end of tomorrow").is_none()); // "tomorrow" isn't a unit
    }

    #[test]
    fn resolve_gives_zoned_instant_with_time_of_day() {
        let now = Local.with_ymd_and_hms(2026, 7, 9, 12, 0, 0).unwrap();
        let start = resolve("start of next week", now, WeekStart::Monday).unwrap();
        assert_eq!(start.date_naive(), d("2026-07-13"));
        assert_eq!(start.time(), NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        let end = resolve("end of next week", now, WeekStart::Monday).unwrap();
        assert_eq!(end.date_naive(), d("2026-07-19"));
        assert_eq!(end.time(), NaiveTime::from_hms_opt(23, 59, 59).unwrap());
        assert!(resolve("2 weeks", now, WeekStart::Monday).is_none());
    }
}
