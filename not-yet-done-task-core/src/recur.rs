//! Recurrence rules: the `recur:` line of a task and the window of the task
//! that follows it.
//!
//! Pure: no clock is read and no zone is assumed — the caller passes the
//! completion instant and the local zone. A successor is only ever computed
//! when a task is completed, so "now" never enters a fixed rhythm: slots
//! missed while a task was overdue are skipped, not made up.
//!
//! ```text
//! daily at 08:00, due +4h
//! weekdays at 07:30
//! mon,wed,fri at 18:00
//! every 2 days at 08:00
//! every 2 weeks on sat at 10:00
//! monthly on 1 at 09:00
//! 3d after done
//! 3 days after done at 07:00, due +12h
//! weekly at 09:00, subtasks open, reopen
//! ```

use std::fmt;
use std::str::FromStr;

use chrono::{
    DateTime, Datelike, Days, Duration, LocalResult, Months, NaiveDate, NaiveDateTime, NaiveTime,
    TimeZone, Utc, Weekday,
};

/// Largest `N` accepted in `every N …` and `N<unit> after done`. Bounds the
/// slot search and catches a typo long before it means anything.
const MAX_AMOUNT: u32 = 999;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub schedule: Schedule,
    /// `due` lies this long after `not_before`; `None` = no deadline.
    pub due: Option<Duration>,
    /// Which children of a completed occurrence its successor gets a copy of.
    pub subtasks: Subtasks,
    /// Every copied child starts as `todo`; without it a done child stays done.
    pub reopen: bool,
}

/// `subtasks all|open|none`. Deleted children are never copied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Subtasks {
    #[default]
    All,
    /// Only children still `todo` or `in_progress`.
    Open,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schedule {
    /// The slots stand still; completion only picks the next one.
    Fixed { pattern: Pattern, at: NaiveTime },
    /// The next window is measured from the completion.
    AfterDone {
        amount: u32,
        unit: Unit,
        /// With `at`: by calendar day, opening at that wall-clock time.
        /// Never set together with [`Unit::Hours`].
        at: Option<NaiveTime>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// Every `N` days, counted from the anchor's date (`daily` = 1).
    EveryDays(u32),
    /// The listed weekdays of every `every`-th week, counted from the
    /// anchor's week (`weekdays` = mon–fri, every week).
    Weekdays { days: Vec<Weekday>, every: u32 },
    /// That day of every month, clamped to the month's last day.
    MonthDay(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Hours,
    Days,
    Weeks,
}

/// A task's time window, both bounds in UTC like every other stored instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub not_before: DateTime<Utc>,
    pub due: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurError(pub String);

impl fmt::Display for RecurError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RecurError {}

fn err<T>(msg: impl Into<String>) -> Result<T, RecurError> {
    Err(RecurError(msg.into()))
}

impl FromStr for Rule {
    type Err = RecurError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let text = s.trim().to_ascii_lowercase();
        if text.is_empty() {
            return err("empty recurrence rule");
        }
        let (head, options) = split_options(&text)?;
        let (head, at) = split_at(head)?;
        let schedule = if let Some(after) = head.strip_suffix("after done") {
            parse_after_done(after.trim(), at)?
        } else {
            let pattern = parse_pattern(head)?;
            let Some(at) = at else {
                return err(format!("'{head}' needs a time: '{head} at HH:MM'"));
            };
            Schedule::Fixed { pattern, at }
        };
        let Options { due, subtasks, reopen } = options;
        Ok(Rule { schedule, due, subtasks: subtasks.unwrap_or_default(), reopen })
    }
}

#[derive(Default)]
struct Options {
    due: Option<Duration>,
    subtasks: Option<Subtasks>,
    reopen: bool,
}

/// Splits off the trailing `, due +<offset>`, `, subtasks …` and `, reopen`
/// clauses, in any order. A comma inside the schedule (`mon,wed at 18:00`)
/// ends the peeling, because what follows it is not an option.
fn split_options(text: &str) -> Result<(&str, Options), RecurError> {
    let mut options = Options::default();
    let mut head = text;
    while let Some(pos) = head.rfind(',') {
        let clause = head[pos + 1..].trim();
        let (word, arg) = clause.split_once(char::is_whitespace).unwrap_or((clause, ""));
        let arg = arg.trim();
        match word {
            "due" => {
                let Some(offset) = arg.strip_prefix('+') else {
                    return err(format!("due offset '{arg}' must start with '+', e.g. 'due +4h'"));
                };
                if options.due.replace(parse_offset(offset)?).is_some() {
                    return err("'due' is given twice");
                }
            }
            "subtasks" => {
                let which = match arg {
                    "all" => Subtasks::All,
                    "open" => Subtasks::Open,
                    "none" => Subtasks::None,
                    _ => return err(format!("'subtasks {arg}' (expected all, open or none)")),
                };
                if options.subtasks.replace(which).is_some() {
                    return err("'subtasks' is given twice");
                }
            }
            "reopen" if arg.is_empty() => {
                if std::mem::replace(&mut options.reopen, true) {
                    return err("'reopen' is given twice");
                }
            }
            _ => break,
        }
        head = head[..pos].trim_end();
    }
    for word in ["due", "subtasks", "reopen"] {
        if head.split(|c: char| c == ',' || c.is_whitespace()).any(|w| w == word) {
            return err(format!("'{word}' must follow the schedule after a comma: '…, {word} …'"));
        }
    }
    Ok((head, options))
}

/// `4h`, `30m`, `1d`, `2d12h`, `1w` — a sum of `<N><unit>` terms.
fn parse_offset(s: &str) -> Result<Duration, RecurError> {
    let bad = || err(format!("bad offset '+{s}' (expected e.g. +4h, +30m, +2d12h)"));
    let mut total = Duration::zero();
    let mut digits = String::new();
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let Ok(n) = digits.parse::<i64>() else {
            return bad();
        };
        if n > i64::from(MAX_AMOUNT) {
            return bad();
        }
        digits.clear();
        total += match c {
            'w' => Duration::weeks(n),
            'd' => Duration::days(n),
            'h' => Duration::hours(n),
            'm' => Duration::minutes(n),
            _ => return bad(),
        };
    }
    if !digits.is_empty() || total.is_zero() {
        return bad();
    }
    Ok(total)
}

/// Splits off a trailing `at HH:MM`.
fn split_at(text: &str) -> Result<(&str, Option<NaiveTime>), RecurError> {
    let Some(pos) = text.rfind(" at ") else {
        return Ok((text.trim(), None));
    };
    let time = text[pos + " at ".len()..].trim();
    match NaiveTime::parse_from_str(time, "%H:%M") {
        Ok(t) => Ok((text[..pos].trim(), Some(t))),
        Err(_) => err(format!("bad time '{time}' (expected HH:MM)")),
    }
}

fn parse_amount(s: &str) -> Result<u32, RecurError> {
    match s.parse::<u32>() {
        Ok(n) if (1..=MAX_AMOUNT).contains(&n) => Ok(n),
        _ => err(format!("'{s}' is not a count between 1 and {MAX_AMOUNT}")),
    }
}

fn parse_unit(s: &str) -> Result<Unit, RecurError> {
    match s {
        "h" | "hour" | "hours" => Ok(Unit::Hours),
        "d" | "day" | "days" => Ok(Unit::Days),
        "w" | "week" | "weeks" => Ok(Unit::Weeks),
        _ => err(format!("unknown unit '{s}' (expected h, d or w)")),
    }
}

/// `3d`, `3 d`, `3 days`, `1 week`.
fn parse_after_done(s: &str, at: Option<NaiveTime>) -> Result<Schedule, RecurError> {
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    if num.is_empty() {
        return err(format!("'{s} after done' needs an amount, e.g. '3d after done'"));
    }
    let amount = parse_amount(num)?;
    let unit = parse_unit(unit.trim())?;
    if unit == Unit::Hours && at.is_some() {
        return err("'at HH:MM' needs a day or week interval, not hours");
    }
    Ok(Schedule::AfterDone { amount, unit, at })
}

fn parse_pattern(s: &str) -> Result<Pattern, RecurError> {
    match s {
        "daily" => return Ok(Pattern::EveryDays(1)),
        "weekly" => return Ok(Pattern::EveryDays(7)),
        "weekdays" => {
            return Ok(Pattern::Weekdays {
                days: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri],
                every: 1,
            });
        }
        _ => {}
    }
    if let Some(day) = s.strip_prefix("monthly on ") {
        return match day.trim().parse::<u32>() {
            Ok(d) if (1..=31).contains(&d) => Ok(Pattern::MonthDay(d)),
            _ => err(format!("'{}' is not a day of the month (1–31)", day.trim())),
        };
    }
    if let Some(rest) = s.strip_prefix("every ") {
        return parse_every(rest.trim());
    }
    parse_weekday_list(s)
        .map(|days| Pattern::Weekdays { days, every: 1 })
        .map_err(|_| {
            RecurError(format!(
                "unknown schedule '{s}' (expected daily, weekdays, mon,wed,…, \
                 every N days|weeks, monthly on D, or N<unit> after done)"
            ))
        })
}

/// `2 days`, `day`, `2 weeks`, `week on mon,thu`.
fn parse_every(s: &str) -> Result<Pattern, RecurError> {
    let (head, on) = match s.split_once(" on ") {
        Some((h, days)) => (h.trim(), Some(parse_weekday_list(days.trim())?)),
        None => (s, None),
    };
    let (n, unit) = match head.split_once(char::is_whitespace) {
        Some((n, unit)) => (parse_amount(n)?, unit.trim()),
        None => (1, head),
    };
    match (parse_unit(unit)?, on) {
        (Unit::Days, None) => Ok(Pattern::EveryDays(n)),
        (Unit::Weeks, None) => Ok(Pattern::EveryDays(n * 7)),
        (Unit::Weeks, Some(days)) => Ok(Pattern::Weekdays { days, every: n }),
        (Unit::Days, Some(_)) => err("'on <weekdays>' only goes with weeks"),
        (Unit::Hours, _) => err("a fixed rhythm counts days or weeks, not hours"),
    }
}

fn parse_weekday_list(s: &str) -> Result<Vec<Weekday>, RecurError> {
    let mut days = Vec::new();
    for name in s.split(',') {
        let name = name.trim();
        let day = match name {
            "mon" | "monday" => Weekday::Mon,
            "tue" | "tuesday" => Weekday::Tue,
            "wed" | "wednesday" => Weekday::Wed,
            "thu" | "thursday" => Weekday::Thu,
            "fri" | "friday" => Weekday::Fri,
            "sat" | "saturday" => Weekday::Sat,
            "sun" | "sunday" => Weekday::Sun,
            _ => return err(format!("unknown weekday '{name}'")),
        };
        if !days.contains(&day) {
            days.push(day);
        }
    }
    Ok(days)
}

impl Rule {
    /// The window of the task that follows one completed at `completed_at`.
    /// `not_before` is the completed task's own lower bound: a fixed rhythm
    /// never hands a task's own slot to its successor (done at 07:50 for the
    /// 08:00 slot), and it anchors `every N …` counting.
    pub fn successor<Tz: TimeZone>(
        &self,
        completed_at: DateTime<Utc>,
        not_before: Option<DateTime<Utc>>,
        tz: &Tz,
    ) -> Window {
        let opens = match &self.schedule {
            Schedule::Fixed { pattern, at } => {
                let reference = not_before.map_or(completed_at, |nb| nb.max(completed_at));
                next_slot(pattern, *at, not_before.unwrap_or(reference), reference, tz)
            }
            Schedule::AfterDone { amount, unit, at } => {
                after_done(*amount, *unit, *at, completed_at, tz)
            }
        };
        self.window(opens)
    }

    /// The window of a new task that has a rule but no window of its own:
    /// as if it had just been completed — the next slot, or open at once.
    pub fn first<Tz: TimeZone>(&self, now: DateTime<Utc>, tz: &Tz) -> Window {
        let opens = match &self.schedule {
            Schedule::Fixed { pattern, at } => next_slot(pattern, *at, now, now, tz),
            Schedule::AfterDone { .. } => now,
        };
        self.window(opens)
    }

    fn window(&self, not_before: DateTime<Utc>) -> Window {
        Window { not_before, due: self.due.map(|d| not_before + d) }
    }
}

/// First slot of `pattern` strictly after `reference`, counting `every N`
/// from `anchor`'s date.
fn next_slot<Tz: TimeZone>(
    pattern: &Pattern,
    at: NaiveTime,
    anchor: DateTime<Utc>,
    reference: DateTime<Utc>,
    tz: &Tz,
) -> DateTime<Utc> {
    let anchor = anchor.with_timezone(tz).date_naive();
    let start = reference.with_timezone(tz).date_naive();
    let horizon = match pattern {
        Pattern::EveryDays(n) => u64::from(*n) + 1,
        Pattern::Weekdays { every, .. } => u64::from(*every) * 7 + 7,
        Pattern::MonthDay(_) => 63,
    };
    (0..=horizon)
        .filter_map(|i| start.checked_add_days(Days::new(i)))
        .filter(|date| matches(pattern, anchor, *date))
        .map(|date| local_instant(tz, date, at))
        .find(|slot| *slot > reference)
        .expect("every pattern has a slot within its horizon")
}

fn matches(pattern: &Pattern, anchor: NaiveDate, date: NaiveDate) -> bool {
    match pattern {
        Pattern::EveryDays(n) => (date - anchor).num_days().rem_euclid(i64::from(*n)) == 0,
        Pattern::Weekdays { days, every } => {
            let monday = |d: NaiveDate| d - Duration::days(i64::from(d.weekday().num_days_from_monday()));
            let weeks = (monday(date) - monday(anchor)).num_days() / 7;
            days.contains(&date.weekday()) && weeks.rem_euclid(i64::from(*every)) == 0
        }
        Pattern::MonthDay(day) => date.day() == (*day).min(last_day_of_month(date)),
    }
}

fn last_day_of_month(date: NaiveDate) -> u32 {
    let first = date.with_day(1).expect("day 1 exists");
    let next = first + Months::new(1);
    (next - Duration::days(1)).day()
}

fn after_done<Tz: TimeZone>(
    amount: u32,
    unit: Unit,
    at: Option<NaiveTime>,
    completed_at: DateTime<Utc>,
    tz: &Tz,
) -> DateTime<Utc> {
    let days = match unit {
        Unit::Hours => return completed_at + Duration::hours(i64::from(amount)),
        Unit::Days => amount,
        Unit::Weeks => amount * 7,
    };
    let local = completed_at.with_timezone(tz).naive_local();
    let date = local.date() + Days::new(u64::from(days));
    // Days are calendar days: 19:20 stays 19:20 across a DST change.
    local_instant(tz, date, at.unwrap_or(local.time()))
}

/// `date` at `time` in `tz`. An ambiguous time (clocks going back) takes the
/// earlier instant; a time that does not exist (clocks going forward) takes
/// the first one that does.
fn local_instant<Tz: TimeZone>(tz: &Tz, date: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    let mut naive = NaiveDateTime::new(date, time);
    loop {
        match tz.from_local_datetime(&naive) {
            LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => return t.with_timezone(&Utc),
            LocalResult::None => naive += Duration::minutes(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use chrono_tz::Europe::Berlin;

    fn rule(s: &str) -> Rule {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    /// Wall-clock time in Berlin, as UTC.
    fn berlin(s: &str) -> DateTime<Utc> {
        let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        Berlin.from_local_datetime(&naive).earliest().unwrap().with_timezone(&Utc)
    }

    fn show(at: DateTime<Utc>) -> String {
        at.with_timezone(&Berlin).format("%Y-%m-%d %a %H:%M").to_string()
    }

    fn next(rule_text: &str, done: &str, not_before: Option<&str>) -> String {
        let w = rule(rule_text).successor(berlin(done), not_before.map(berlin), &Berlin);
        show(w.not_before)
    }

    // ── parsing ─────────────────────────────────────────────────────────

    #[test]
    fn parses_the_documented_examples() {
        assert_eq!(
            rule("daily at 08:00, due +4h"),
            Rule {
                schedule: Schedule::Fixed { pattern: Pattern::EveryDays(1), at: t(8, 0) },
                due: Some(Duration::hours(4)),
                subtasks: Subtasks::All,
                reopen: false,
            }
        );
        assert_eq!(
            rule("mon,wed,fri at 18:00").schedule,
            Schedule::Fixed {
                pattern: Pattern::Weekdays {
                    days: vec![Weekday::Mon, Weekday::Wed, Weekday::Fri],
                    every: 1
                },
                at: t(18, 0),
            }
        );
        assert_eq!(
            rule("every 2 weeks on sat at 10:00").schedule,
            Schedule::Fixed {
                pattern: Pattern::Weekdays { days: vec![Weekday::Sat], every: 2 },
                at: t(10, 0),
            }
        );
        assert_eq!(
            rule("monthly on 1 at 09:00").schedule,
            Schedule::Fixed { pattern: Pattern::MonthDay(1), at: t(9, 0) }
        );
        assert_eq!(
            rule("3d after done"),
            Rule {
                schedule: Schedule::AfterDone { amount: 3, unit: Unit::Days, at: None },
                due: None,
                subtasks: Subtasks::All,
                reopen: false,
            }
        );
        assert_eq!(
            rule("3 days after done at 07:00, due +12h"),
            Rule {
                schedule: Schedule::AfterDone { amount: 3, unit: Unit::Days, at: Some(t(7, 0)) },
                due: Some(Duration::hours(12)),
                subtasks: Subtasks::All,
                reopen: false,
            }
        );
    }

    #[test]
    fn parses_variants() {
        assert_eq!(
            rule("  Weekdays AT 07:30 ").schedule,
            Schedule::Fixed {
                pattern: Pattern::Weekdays {
                    days: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri],
                    every: 1,
                },
                at: t(7, 30),
            }
        );
        assert_eq!(
            rule("every 2 weeks at 10:00").schedule,
            Schedule::Fixed { pattern: Pattern::EveryDays(14), at: t(10, 0) }
        );
        assert_eq!(
            rule("every day at 10:00").schedule,
            Schedule::Fixed { pattern: Pattern::EveryDays(1), at: t(10, 0) }
        );
        assert_eq!(
            rule("monday, thursday at 06:00").schedule,
            Schedule::Fixed {
                pattern: Pattern::Weekdays { days: vec![Weekday::Mon, Weekday::Thu], every: 1 },
                at: t(6, 0),
            }
        );
        assert_eq!(
            rule("12 h after done").schedule,
            Schedule::AfterDone { amount: 12, unit: Unit::Hours, at: None }
        );
        assert_eq!(rule("1w after done, due +2d12h").due, Some(Duration::hours(60)));
        assert_eq!(rule("1w after done, due +30m").due, Some(Duration::minutes(30)));
    }

    #[test]
    fn parses_the_subtask_options_in_any_order() {
        let r = rule("weekly at 09:00, subtasks open, reopen");
        assert_eq!((r.subtasks, r.reopen, r.due), (Subtasks::Open, true, None));
        let r = rule("mon,thu at 09:00, reopen, due +1h, subtasks none");
        assert_eq!((r.subtasks, r.reopen, r.due), (Subtasks::None, true, Some(Duration::hours(1))));
        assert_eq!(
            r.schedule,
            Schedule::Fixed {
                pattern: Pattern::Weekdays { days: vec![Weekday::Mon, Weekday::Thu], every: 1 },
                at: t(9, 0),
            }
        );
        let r = rule("3d after done, subtasks all");
        assert_eq!((r.subtasks, r.reopen), (Subtasks::All, false));
    }

    #[test]
    fn rejects_bad_rules() {
        for bad in [
            "",
            "daily",                         // a fixed rhythm needs a time
            "daily at 8",                    // HH:MM
            "daily at 25:00",
            "fortnightly at 08:00",
            "mon,funday at 08:00",
            "monthly on 32 at 08:00",
            "monthly on 0 at 08:00",
            "every 0 days at 08:00",
            "every 2 hours at 08:00",
            "every 2 days on mon at 08:00",
            "after done",
            "3 fortnights after done",
            "12h after done at 07:00",       // `at` needs days or weeks
            "daily at 08:00, due 4h",        // offset needs '+'
            "daily at 08:00, due +4",
            "daily at 08:00, due +4x",
            "daily at 08:00 due +4h",        // comma before due
            "daily at 08:00, subtasks some",
            "daily at 08:00, subtasks",
            "daily at 08:00, reopen, reopen",
            "daily at 08:00, subtasks open, subtasks all",
            "daily at 08:00, due +1h, due +2h",
            "daily at 08:00 reopen",         // comma before reopen
            "daily at 08:00, reopen now",
            "every 1000 days at 08:00",
        ] {
            assert!(bad.parse::<Rule>().is_err(), "accepted {bad:?}");
        }
    }

    // ── fixed rhythm ────────────────────────────────────────────────────

    #[test]
    fn daily_opens_the_next_day() {
        assert_eq!(
            next("daily at 08:00", "2026-10-05 08:10", Some("2026-10-05 08:00")),
            "2026-10-06 Tue 08:00"
        );
    }

    #[test]
    fn done_early_does_not_hand_over_its_own_slot() {
        // Completed 07:50 for the 08:00 slot: the successor is tomorrow's.
        assert_eq!(
            next("daily at 08:00", "2026-10-05 07:50", Some("2026-10-05 08:00")),
            "2026-10-06 Tue 08:00"
        );
    }

    #[test]
    fn missed_slots_are_skipped_not_made_up() {
        assert_eq!(
            next("daily at 08:00", "2026-10-09 21:00", Some("2026-10-05 08:00")),
            "2026-10-10 Sat 08:00"
        );
    }

    #[test]
    fn weekday_list_picks_the_next_listed_day() {
        // Wednesday evening, after the 18:00 slot → Friday.
        assert_eq!(next("mon,wed,fri at 18:00", "2026-10-07 18:30", None), "2026-10-09 Fri 18:00");
        // Friday → Monday.
        assert_eq!(next("mon,wed,fri at 18:00", "2026-10-09 19:00", None), "2026-10-12 Mon 18:00");
        assert_eq!(next("weekdays at 07:30", "2026-10-09 08:00", None), "2026-10-12 Mon 07:30");
    }

    #[test]
    fn every_n_counts_from_the_tasks_own_window() {
        // Anchored on Sat 3 Oct: the next is Sat 17 Oct, not Sat 10 Oct.
        assert_eq!(
            next("every 2 weeks on sat at 10:00", "2026-10-03 11:00", Some("2026-10-03 10:00")),
            "2026-10-17 Sat 10:00"
        );
        // Done late, during the off week: still the rhythm's next Saturday.
        assert_eq!(
            next("every 2 weeks on sat at 10:00", "2026-10-11 09:00", Some("2026-10-03 10:00")),
            "2026-10-17 Sat 10:00"
        );
        assert_eq!(
            next("every 3 days at 08:00", "2026-10-06 12:00", Some("2026-10-05 08:00")),
            "2026-10-08 Thu 08:00"
        );
    }

    #[test]
    fn monthly_clamps_to_the_last_day() {
        assert_eq!(
            next("monthly on 31 at 09:00", "2026-10-31 10:00", Some("2026-10-31 09:00")),
            "2026-11-30 Mon 09:00"
        );
        assert_eq!(
            next("monthly on 1 at 09:00", "2026-10-01 09:30", Some("2026-10-01 09:00")),
            "2026-11-01 Sun 09:00"
        );
    }

    #[test]
    fn due_follows_not_before() {
        let w = rule("daily at 08:00, due +4h").successor(
            berlin("2026-10-05 09:00"),
            Some(berlin("2026-10-05 08:00")),
            &Berlin,
        );
        assert_eq!(show(w.not_before), "2026-10-06 Tue 08:00");
        assert_eq!(show(w.due.unwrap()), "2026-10-06 Tue 12:00");
    }

    // ── after done ──────────────────────────────────────────────────────

    #[test]
    fn after_done_without_at_is_exact() {
        assert_eq!(next("3d after done", "2026-10-05 19:20", None), "2026-10-08 Thu 19:20");
        assert_eq!(next("12h after done", "2026-10-05 19:20", None), "2026-10-06 Tue 07:20");
    }

    #[test]
    fn after_done_with_at_goes_by_calendar_day() {
        assert_eq!(
            next("3 days after done at 07:00", "2026-10-05 19:20", None),
            "2026-10-08 Thu 07:00"
        );
        assert_eq!(next("1w after done at 07:00", "2026-10-05 19:20", None), "2026-10-12 Mon 07:00");
    }

    #[test]
    fn after_done_ignores_the_tasks_own_window() {
        assert_eq!(
            next("3d after done", "2026-10-05 19:20", Some("2026-10-20 08:00")),
            "2026-10-08 Thu 19:20"
        );
    }

    // ── zones and DST ───────────────────────────────────────────────────

    #[test]
    fn wall_clock_survives_dst() {
        // Berlin falls back on Sun 25 Oct 2026.
        assert_eq!(
            next("daily at 08:00", "2026-10-24 09:00", Some("2026-10-24 08:00")),
            "2026-10-25 Sun 08:00"
        );
        assert_eq!(next("3d after done", "2026-10-23 19:20", None), "2026-10-26 Mon 19:20");
    }

    #[test]
    fn a_missing_hour_opens_when_the_clock_resumes() {
        // Berlin springs forward on Sun 28 Mar 2027: 02:30 does not exist.
        assert_eq!(
            next("daily at 02:30", "2027-03-27 03:00", Some("2027-03-27 02:30")),
            "2027-03-28 Sun 03:00"
        );
    }

    #[test]
    fn ambiguous_hour_takes_the_first() {
        // 02:30 happens twice on 25 Oct 2026; the first is still CEST (+02).
        let w = rule("daily at 02:30").successor(
            berlin("2026-10-24 03:00"),
            Some(berlin("2026-10-24 02:30")),
            &Berlin,
        );
        assert_eq!(w.not_before, "2026-10-25T00:30:00Z".parse::<DateTime<Utc>>().unwrap());
    }

    #[test]
    fn the_day_is_the_local_day() {
        // 23:30 UTC on 5 Oct is already 6 Oct in UTC+2.
        let plus2 = FixedOffset::east_opt(2 * 3600).unwrap();
        let done = "2026-10-05T22:30:00Z".parse::<DateTime<Utc>>().unwrap();
        let w = rule("3 days after done at 07:00").successor(done, None, &plus2);
        assert_eq!(w.not_before, "2026-10-09T05:00:00Z".parse::<DateTime<Utc>>().unwrap());
    }

    // ── first window ────────────────────────────────────────────────────

    #[test]
    fn first_window() {
        let now = berlin("2026-10-05 09:00");
        assert_eq!(show(rule("daily at 08:00").first(now, &Berlin).not_before), "2026-10-06 Tue 08:00");
        assert_eq!(show(rule("daily at 10:00").first(now, &Berlin).not_before), "2026-10-05 Mon 10:00");
        let w = rule("3d after done, due +1d").first(now, &Berlin);
        assert_eq!(w.not_before, now);
        assert_eq!(w.due, Some(now + Duration::days(1)));
    }
}
