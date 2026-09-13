//! `OnCalendar=` out of what a person types.
//!
//! # Normalised, not translated
//!
//! systemd already accepts nearly everything one would write by hand:
//! `daily`, `hourly`, `Mon 09:00`, `Mon..Fri 09:00`, `sat,sun 10:00`,
//! `*:0/15`, `*-*-01 00:00`, `2026-09-14 09:00`. What it does not accept is
//! the filler around them — `every monday at 9` fails, `monday 09:00` does
//! not — and an hour without minutes.
//!
//! So this module **normalises**: it drops the filler, puts the systemd token
//! in place of the English word that means the same thing, completes a bare
//! hour, and then hands the result to `systemd-analyze calendar` unchanged.
//! What goes into the file is systemd's own normalised form, not ours. A
//! translator would make the form field and the unit file two different
//! languages, of which the file only speaks one.
//!
//! Every rewrite below is there because the un-rewritten phrase was measured
//! to fail on systemd 261:
//!
//! | typed | rewritten to | why |
//! |---|---|---|
//! | `every monday at 9` | `monday 9:00` | filler, bare hour |
//! | `every day at 9` | `*-*-* 9:00` | `day`/`daily` is not a systemd token with a time |
//! | `mondays 9:00` | `monday 9:00` | the plural is not a day name |
//! | `weekdays 9:00` | `Mon..Fri 9:00` | no such word in the grammar |
//! | `every day at 9pm` | `*-*-* 21:00` | systemd has no 12-hour clock |
//! | `noon` | `12:00` | ditto |
//!
//! # Recurrence is not a `natural-date` question
//!
//! `natural-date` resolves a phrase to **one instant** — that is its whole
//! API. `OnCalendar=` is a recurrence, so using it here would mean writing a
//! recurrence grammar next to systemd's. It does have a place in this phase,
//! but the other one: the **one-shot** timer, where `tomorrow at 9` really is
//! a single instant and `OnCalendar=2026-09-14 09:00:00` is what the file
//! wants. See [`crate::create`].
//!
//! # The exit code is the gate
//!
//! Unlike `systemd-analyze verify` (see [`crate::edit`]), where the exit code
//! only separates "systemd would load it anyway" from "systemd refuses it",
//! `systemd-analyze calendar` exits 1 on anything it cannot parse and 0 with
//! the normalised form and the next elapses on anything it can. So the check
//! is the gate, and its own message is what the user gets to read.

use not_yet_done_content::{ContentError, Result};

/// How many upcoming elapses the success message reports.
pub const ELAPSES_SHOWN: usize = 3;

/// What systemd made of an expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// systemd's own normalised form — this is what is written to the file.
    pub expression: String,
    /// The next elapses in local time, as systemd printed them.
    pub elapses: Vec<String>,
}

impl Schedule {
    /// The one-line version for a notification: the expression plus where it
    /// lands next.
    pub fn summary(&self) -> String {
        match self.elapses.first() {
            Some(next) => format!("{} — next {next}", self.expression),
            None => self.expression.clone(),
        }
    }
}

/// Words that carry no information for systemd and are dropped.
const FILLER: &[&str] = &["every", "each", "at", "on", "the"];

/// An English word and the systemd token that means the same thing. Applied
/// per token, case-insensitively, and only to an expression that has more than
/// one token: `daily` on its own is systemd's own word for the same thing and
/// must survive untouched, while `daily 9:00` is not a spec systemd reads.
const WORDS: &[(&str, &str)] = &[
    ("day", "*-*-*"),
    ("days", "*-*-*"),
    ("daily", "*-*-*"),
    ("weekday", "Mon..Fri"),
    ("weekdays", "Mon..Fri"),
    ("workday", "Mon..Fri"),
    ("workdays", "Mon..Fri"),
    ("weekend", "Sat,Sun"),
    ("weekends", "Sat,Sun"),
    ("noon", "12:00"),
    ("midnight", "00:00"),
];

/// The day names whose plural a person writes without thinking.
const DAYS: &[&str] = &[
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// Rewrite what the user typed into something systemd's grammar can hold.
///
/// Pure and total: anything it does not recognise is passed through untouched,
/// because systemd is the authority on what is valid — not this function. An
/// expression that is already systemd's own (`*-*-* 09:00:00`, `Mon..Fri
/// 09:00`) survives unchanged.
pub fn normalize(raw: &str) -> String {
    let stripped: Vec<&str> = raw
        .split_whitespace()
        .filter(|t| !FILLER.contains(&t.to_ascii_lowercase().as_str()))
        .collect();
    let alone = stripped.len() == 1;
    let mut out: Vec<String> = Vec::new();
    for token in stripped {
        let lower = token.to_ascii_lowercase();
        if let Some((_, systemd)) = WORDS.iter().find(|(word, _)| *word == lower)
            && !alone
        {
            out.push((*systemd).to_string());
            continue;
        }
        // `mondays` is the same day as `monday`; systemd only knows the latter.
        if let Some(day) = DAYS.iter().find(|d| lower == format!("{d}s")) {
            out.push((*day).to_string());
            continue;
        }
        if let Some(time) = twelve_hour(&lower) {
            out.push(time);
            continue;
        }
        out.push(token.to_string());
    }
    // A bare hour is an hour: `monday 9` means nine o'clock, and systemd wants
    // the minutes spelled out. Only the last token, and only when no other one
    // already carries a time — `*:0/15` is not an hour missing its minutes.
    if let Some(last) = out.last_mut()
        && let Ok(hour) = last.parse::<u32>()
        && hour < 24
        && !last.is_empty()
    {
        *last = format!("{hour}:00");
    }
    out.join(" ")
}

/// `9pm` / `9:30am` → the 24-hour clock systemd speaks. `None` when the token
/// is not a 12-hour time at all.
fn twelve_hour(token: &str) -> Option<String> {
    let (digits, pm) = match token.strip_suffix("am") {
        Some(rest) => (rest, false),
        None => (token.strip_suffix("pm")?, true),
    };
    let (hour, minute) = match digits.split_once(':') {
        Some((h, m)) => (h, m),
        None => (digits, "00"),
    };
    let hour: u32 = hour.parse().ok()?;
    if hour == 0 || hour > 12 || minute.len() != 2 || minute.parse::<u32>().ok()? > 59 {
        return None;
    }
    let hour = match (hour, pm) {
        (12, false) => 0,
        (12, true) => 12,
        (h, false) => h,
        (h, true) => h + 12,
    };
    Some(format!("{hour:02}:{minute}"))
}

/// Normalise, then let systemd have the last word.
///
/// The returned [`Schedule`] carries systemd's normalised form — write *that*
/// into the unit, so the file and the tool agree on what was meant. A rejected
/// expression comes back as systemd's own message, which names the offending
/// spec and is more useful than anything this module could say about it.
pub async fn check(raw: &str) -> Result<Schedule> {
    let expr = normalize(raw);
    if expr.is_empty() {
        return Err(ContentError::Other("no schedule given".into()));
    }
    let output = tokio::process::Command::new("systemd-analyze")
        .arg("calendar")
        .arg(format!("--iterations={ELAPSES_SHOWN}"))
        .arg(&expr)
        // systemd speaks the caller's language; this output is parsed.
        .env("LC_ALL", "C")
        .output()
        .await
        .map_err(|e| {
            ContentError::Other(format!("systemd-analyze could not be run: {e}").into())
        })?;

    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.lines().next().unwrap_or("").trim();
        let said = if said.is_empty() {
            format!("systemd cannot read the schedule '{expr}'")
        } else {
            said.to_string()
        };
        // The user typed something else than what was checked; say both, or
        // the message points at a string they never wrote.
        return Err(ContentError::Other(if expr == raw.trim() {
            said.into()
        } else {
            format!("{said} (read as '{expr}')").into()
        }));
    }
    Ok(
        parse(&String::from_utf8_lossy(&output.stdout)).unwrap_or(Schedule {
            expression: expr,
            elapses: Vec::new(),
        }),
    )
}

/// Pull the normalised form and the elapses out of `systemd-analyze calendar`.
///
/// ```text
///   Original form: Mon..Fri 9:00
/// Normalized form: Mon..Fri *-*-* 09:00:00
///     Next elapse: Mon 2026-09-14 09:00:00 CEST
///        (in UTC): Mon 2026-09-14 07:00:00 UTC
///        From now: 1 day 2h left
///    Iteration #2: Tue 2026-09-15 09:00:00 CEST
/// ```
///
/// The UTC and "from now" lines are dropped: the user picked a wall-clock
/// time, so the wall clock is what should be read back to them.
fn parse(stdout: &str) -> Option<Schedule> {
    let mut expression = None;
    let mut elapses = Vec::new();
    for line in stdout.lines() {
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        match label.trim() {
            "Normalized form" => expression = Some(value),
            "Next elapse" => elapses.push(value),
            label if label.starts_with("Iteration #") => elapses.push(value),
            _ => {}
        }
    }
    Some(Schedule {
        expression: expression?,
        elapses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rewrite in the module header, in the form the header claims.
    #[test]
    fn the_filler_a_person_types_comes_off() {
        assert_eq!(normalize("every monday at 9"), "monday 9:00");
        assert_eq!(normalize("each monday 09:00"), "monday 09:00");
        assert_eq!(normalize("every day at 9"), "*-*-* 9:00");
        assert_eq!(normalize("on friday at 18:00"), "friday 18:00");
    }

    #[test]
    fn an_english_word_becomes_its_systemd_token() {
        assert_eq!(normalize("weekdays 9:00"), "Mon..Fri 9:00");
        assert_eq!(normalize("every weekend at 10:00"), "Sat,Sun 10:00");
        assert_eq!(normalize("mondays 9:00"), "monday 9:00");
        assert_eq!(normalize("every day at noon"), "*-*-* 12:00");
    }

    #[test]
    fn the_twelve_hour_clock_becomes_the_one_systemd_reads() {
        assert_eq!(normalize("every day at 9pm"), "*-*-* 21:00");
        assert_eq!(normalize("daily 7am"), "*-*-* 07:00");
        assert_eq!(normalize("monday 12am"), "monday 00:00");
        assert_eq!(normalize("monday 12pm"), "monday 12:00");
        assert_eq!(normalize("sunday 9:30pm"), "sunday 21:30");
        // Not a time at all — left for systemd to reject by its own name.
        assert_eq!(normalize("spam"), "spam");
        assert_eq!(normalize("13pm"), "13pm");
    }

    /// What systemd already accepts must arrive unchanged, or the file would
    /// stop saying what the user wrote.
    #[test]
    fn systemds_own_grammar_passes_through() {
        for expr in [
            "daily",
            "hourly",
            "Mon..Fri 09:00",
            "sat,sun 10:00",
            "*:0/15",
            "*-*-01 00:00",
            "2026-09-14 09:00:00",
        ] {
            assert_eq!(normalize(expr), expr, "{expr} must survive normalisation");
        }
    }

    /// A bare hour is completed; something that only looks like one is not.
    #[test]
    fn a_bare_hour_gets_its_minutes() {
        assert_eq!(normalize("monday 9"), "monday 9:00");
        assert_eq!(normalize("9"), "9:00");
        assert_eq!(normalize("*:0/15"), "*:0/15");
        assert_eq!(normalize("monday 99"), "monday 99");
    }

    #[test]
    fn the_output_of_the_real_tool_is_read_back() {
        let stdout = "  Original form: Mon..Fri 9:00\n\
                      Normalized form: Mon..Fri *-*-* 09:00:00\n    \
                          Next elapse: Mon 2026-09-14 09:00:00 CEST\n       \
                             (in UTC): Mon 2026-09-14 07:00:00 UTC\n       \
                             From now: 1 day 2h left\n   \
                         Iteration #2: Tue 2026-09-15 09:00:00 CEST\n       \
                             (in UTC): Tue 2026-09-15 07:00:00 UTC\n";
        let schedule = parse(stdout).expect("a normalised form was printed");
        assert_eq!(schedule.expression, "Mon..Fri *-*-* 09:00:00");
        assert_eq!(
            schedule.elapses,
            vec![
                "Mon 2026-09-14 09:00:00 CEST".to_string(),
                "Tue 2026-09-15 09:00:00 CEST".to_string(),
            ]
        );
        assert!(schedule.summary().contains("next Mon 2026-09-14"));
    }

    /// The gate itself, against the real tool. Skipped where systemd-analyze
    /// is not installed — the adapter is useless there anyway, but the test
    /// suite should not be.
    #[tokio::test]
    async fn systemd_has_the_last_word() {
        if tokio::process::Command::new("systemd-analyze")
            .arg("--version")
            .output()
            .await
            .is_err()
        {
            return;
        }
        let ok = check("every monday at 9")
            .await
            .expect("systemd accepts it");
        assert_eq!(ok.expression, "Mon *-*-* 09:00:00");
        assert_eq!(ok.elapses.len(), ELAPSES_SHOWN);

        let refused = check("whenever I feel like it").await.unwrap_err();
        assert!(
            refused.to_string().contains("Failed to parse"),
            "systemd's own words: {refused}"
        );
        // What was checked is not what was typed, so the message says both —
        // otherwise it quotes a string the user never wrote.
        let rewritten = check("every whenever").await.unwrap_err().to_string();
        assert!(
            rewritten.contains("read as 'whenever'"),
            "the message names what was actually checked: {rewritten}"
        );
    }
}
