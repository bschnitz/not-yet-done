//! The built-in meanings. Each pass reads one canonical (English) phrase; a
//! language never reaches this file, it only rewrites into these phrases.

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime, Weekday};

use crate::meaning::{Context, Meaning, Reading};

/// The built-in meanings, in order. Part-of-day goes first because it may
/// consume a trailing word (`tomorrow morning`); period boundaries go last so
/// the more specific shorthands win.
pub(crate) fn builtin() -> Vec<Meaning> {
    vec![
        Meaning::new("part-of-day", part_of_day),
        Meaning::new("now", now),
        Meaning::new("day-word", day_word),
        Meaning::new("todays-weekday", todays_weekday),
        Meaning::new("this-weekday", this_weekday),
        Meaning::new("last-weekday", last_weekday),
        Meaning::new("in-span", in_prefix),
        Meaning::new("abbreviation", abbreviation),
        Meaning::new("quarter", quarter),
        Meaning::new("iso-week", iso_week),
        Meaning::new("spoken-time", spoken_time),
        Meaning::new("business-days", business_days),
        Meaning::new("dotted-date", dotted_date),
        Meaning::new("period", period),
    ]
}

/// `now` / `asap`: the reference instant itself.
fn now(s: &str, _: &Context) -> Option<Reading> {
    matches!(s, "now" | "asap").then(|| Reading::after(Duration::zero(), true))
}

/// A day named on its own is a calendar day, so it starts at midnight.
/// chrono-english would keep the reference time of day, which turns `> today`
/// in a filter into "later than this very moment". A day word followed by a
/// time (`day after tomorrow 18:00`) takes whatever `today <time>` takes and
/// moves it by the day's offset — chrono-english knows only some of the words.
fn day_word(s: &str, cx: &Context) -> Option<Reading> {
    const WORDS: [(&str, i64); 6] = [
        ("day after tomorrow", 2),
        ("day before yesterday", -2),
        ("overmorrow", 2),
        ("today", 0),
        ("tomorrow", 1),
        ("yesterday", -1),
    ];
    let (rest, days) = WORDS
        .iter()
        .find_map(|&(word, days)| Some((s.strip_prefix(word)?, days)))?;
    if rest.is_empty() {
        return Some(Reading::day(cx.today() + Duration::days(days)));
    }
    // `today <time>` is what the others are resolved through, so it must not
    // come back here.
    if days == 0 {
        return None;
    }
    let time = rest.strip_prefix(' ')?;
    let r = cx.resolve(&format!("today {time}"))?;
    Some(Reading::wall(r.local(cx.now()) + Duration::days(days), r.has_time))
}

/// A phrase that starts with today's weekday (`friday`, `fri 17:00` on a
/// Friday) is read as starting with `today`. chrono-english reads a bare
/// weekday as the *next* one, which on that very day is a week away; someone
/// naming today's weekday means today. `next friday` doesn't start with the
/// weekday and keeps its week jump (`last friday`: see [`last_weekday`]).
fn todays_weekday(s: &str, cx: &Context) -> Option<Reading> {
    let (word, rest) = s.split_once(' ').unwrap_or((s, ""));
    if word.parse::<Weekday>().ok()? != cx.today().weekday() {
        return None;
    }
    cx.resolve(&if rest.is_empty() {
        "today".to_string()
    } else {
        format!("today {rest}")
    })
}

/// `this <weekday> [time]`: the coming such day, today included — the same
/// day the bare weekday names.
fn this_weekday(s: &str, cx: &Context) -> Option<Reading> {
    let rest = s.strip_prefix("this ")?;
    let (word, _) = rest.split_once(' ').unwrap_or((rest, ""));
    word.parse::<Weekday>().ok()?;
    cx.resolve(rest)
}

/// `last <weekday> [time]`: the most recent such day *before* today, so `last
/// friday` on a Friday is a week back. chrono-english counts today in and
/// answers with today. The time part is resolved on today and moved back, so it
/// takes whatever `today <time>` takes.
fn last_weekday(s: &str, cx: &Context) -> Option<Reading> {
    let rest = s.strip_prefix("last ")?;
    let (word, time) = rest.split_once(' ').unwrap_or((rest, ""));
    let day = word.parse::<Weekday>().ok()?;
    let today = cx.today().weekday();
    let back = match (7 + today.num_days_from_monday() - day.num_days_from_monday()) % 7 {
        0 => 7,
        n => n,
    };
    let on_today = if time.is_empty() {
        "today".to_string()
    } else {
        format!("today {time}")
    };
    let r = cx.resolve(&on_today)?;
    Some(Reading::wall(
        r.local(cx.now()) - Duration::days(i64::from(back)),
        r.has_time,
    ))
}

/// `in <n> <unit>`: the reference instant plus a span. `a`/`an` count as 1, a
/// negative count goes back. Spans of a day or more carry no time of day.
fn in_prefix(s: &str, cx: &Context) -> Option<Reading> {
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
    let span = |d: Duration, has_time| Some(Reading::after(d, has_time));
    match unit {
        "s" | "sec" | "secs" | "second" | "seconds" => span(Duration::seconds(num), true),
        "m" | "min" | "mins" | "minute" | "minutes" => span(Duration::minutes(num), true),
        "h" | "hr" | "hrs" | "hour" | "hours" => span(Duration::hours(num), true),
        "d" | "day" | "days" => span(Duration::days(num), false),
        "w" | "wk" | "week" | "weeks" => span(Duration::weeks(num), false),
        "month" | "months" => add_months(cx, num),
        "year" | "years" => add_months(cx, num.checked_mul(12)?),
        _ => None,
    }
}

fn add_months(cx: &Context, n: i64) -> Option<Reading> {
    let months = Months::new(u32::try_from(n.unsigned_abs()).ok()?);
    let at = if n >= 0 {
        cx.now().checked_add_months(months)?
    } else {
        cx.now().checked_sub_months(months)?
    };
    Some(Reading::wall(at, false))
}

/// Part-of-day words → a fixed local time, optionally on a leading date
/// (`friday noon`, `tomorrow morning`). Standalone means today.
fn part_of_day(s: &str, cx: &Context) -> Option<Reading> {
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
            cx.today()
        } else {
            cx.resolve_local(pre)?.date()
        };
        return Some(Reading::wall(date.and_hms_opt(*h, *m, 0)?, true));
    }
    None
}

/// Common workplace abbreviations.
fn abbreviation(s: &str, cx: &Context) -> Option<Reading> {
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
        return period(p, cx);
    }
    // Business-hours abbreviations → a fixed time today.
    let (h, m) = match s {
        "cob" | "eob" => (17, 0), // close / end of business
        "sob" | "bob" => (9, 0),  // start / beginning of business
        _ => return None,
    };
    Some(Reading::wall(cx.today().and_hms_opt(h, m, 0)?, true))
}

/// `q3`, `end of q4`, `q1 2027`, `start of q1`. Default boundary is the start.
fn quarter(s: &str, cx: &Context) -> Option<Reading> {
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
    let qn: u32 = it.next()?.strip_prefix('q')?.parse().ok()?;
    if !(1..=4).contains(&qn) {
        return None;
    }
    let year: i32 = match it.next() {
        Some(y) => y.parse().ok()?,
        None => cx.today().year(),
    };
    if it.next().is_some() {
        return None;
    }

    let start_month = (qn - 1) * 3 + 1;
    if at_end {
        let last = last_day_of_month(year, start_month + 2)?;
        Some(Reading::wall(last.and_hms_opt(23, 59, 59)?, false))
    } else {
        Some(Reading::day(NaiveDate::from_ymd_opt(year, start_month, 1)?))
    }
}

/// ISO week designator `2026-w30` → the Monday of that week.
fn iso_week(s: &str, _: &Context) -> Option<Reading> {
    let (y, w) = s.split_once("-w")?;
    let d = NaiveDate::from_isoywd_opt(y.parse().ok()?, w.parse().ok()?, Weekday::Mon)?;
    Some(Reading::day(d))
}

/// `half past 2`, `quarter past 9`, `quarter to 5`, `10 past 8`, `20 to 6`.
fn spoken_time(s: &str, cx: &Context) -> Option<Reading> {
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
        ((h + 23) % 24, 60 - mins)
    };
    Some(Reading::wall(
        cx.today()
            .and_time(NaiveTime::from_hms_opt(hour, minute, 0)?),
        true,
    ))
}

/// `next business day`, `in 2 business days`, `in 1 working day`.
fn business_days(s: &str, cx: &Context) -> Option<Reading> {
    if s == "next business day" || s == "next working day" {
        return Some(Reading::day(add_business_days(cx.today(), 1)));
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
    let d = add_business_days(cx.today(), n);
    Some(Reading::wall(d.and_time(cx.now().time()), false))
}

/// Day-first dotted dates, `d.m.` / `d.m.yy` / `d.m.yyyy`, optionally followed
/// by a time (`08.10.2026 14:30`). Without a year it is the current one. Two
/// dots or a year are required: a lone `8.10` could as well be a time.
fn dotted_date(s: &str, cx: &Context) -> Option<Reading> {
    let (head, rest) = s.split_once(' ').unwrap_or((s, ""));
    let parts: Vec<&str> = head.split('.').collect();
    let (d, m, y) = match parts.as_slice() {
        [d, m, ""] => (*d, *m, None),
        [d, m, y] if !y.is_empty() => (*d, *m, Some(*y)),
        _ => return None,
    };
    let all_digits =
        |t: &str| !t.is_empty() && t.len() <= 2 && t.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(d) || !all_digits(m) {
        return None;
    }
    let year = match y {
        None => cx.today().year(),
        Some(y) if y.len() == 2 && y.bytes().all(|b| b.is_ascii_digit()) => {
            2000 + y.parse::<i32>().ok()?
        }
        Some(y) if y.len() == 4 && y.bytes().all(|b| b.is_ascii_digit()) => y.parse().ok()?,
        Some(_) => return None,
    };
    let date = NaiveDate::from_ymd_opt(year, m.parse().ok()?, d.parse().ok()?)?;
    if rest.is_empty() {
        return Some(Reading::day(date));
    }
    let time = cx.resolve(&format!("today {rest}"))?;
    Some(Reading::wall(
        date.and_time(time.local(cx.now()).time()),
        time.has_time,
    ))
}

/// Period boundaries and bare periods via [`date_periods`] (`end of next
/// week`, `last month`). They name days, not times of day.
fn period(s: &str, cx: &Context) -> Option<Reading> {
    let spec = date_periods::parse(s)?;
    let date = spec.boundary_date(cx.today(), cx.week_start());
    Some(Reading::wall(date.and_time(spec.boundary_time()), false))
}

// --- helpers ----------------------------------------------------------------

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
