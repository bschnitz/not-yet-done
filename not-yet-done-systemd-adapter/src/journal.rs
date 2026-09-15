//! A unit's journal, as a level.
//!
//! # Why a level and not a pager
//!
//! `journalctl -u foo` is one keystroke away in any terminal, and the reason to
//! build this anyway is everything a pager has no notion of: sorting, the tab's
//! query language, `highlights:`, the details pane, multi-line rows, copying a
//! cell. None of it is written here — the level inherits all of it by being a
//! level. The pager is still worth one key, which is what [`FOLLOW`] is: it
//! opens `journalctl -f` in a terminal and covers colour and follow, the two
//! things a table genuinely does not do.
//!
//! # Where the rows come from
//!
//! `journalctl --output=json`, one JSON object per line, over a pipe. There is
//! no D-Bus interface for reading the journal — `sd-journal` is a C library and
//! `systemd-journal-gatewayd` is an HTTP service that is not running anywhere by
//! default — so the supported interface really is the command, and its `json`
//! output is the stable machine-readable form of it.
//!
//! # `MESSAGE` is not always a string
//!
//! Measured, not assumed: `--output=json` emits `MESSAGE` as a **JSON array of
//! bytes** whenever the message is not plainly printable UTF-8 — and that is
//! not a corner. Every service that logs through `tracing`'s colours produces
//! it, which in one sample window was 148 of 148 rows for a browser and every
//! row of a Rust daemon. An adapter that reads `MESSAGE` as a string shows an
//! empty column for exactly those units. So the bytes are decoded lossily and
//! the ANSI escapes are dropped on the way in: the colours say what the `level`
//! column already says, and the cell is painted by `highlights:` anyway.
//!
//! # Paging
//!
//! A level is asked for a half-open window `[offset, offset + limit)` — offsets,
//! not cursors, because the cursor protocol in this application belongs to the
//! custom-query road, not to `list()`. The window is cut with `--reverse
//! --lines=<offset + limit>` and the first `offset` rows dropped: newest first,
//! stateless, and exact. (`--after-cursor` is the other direction and is what
//! phase 5's follow will want; `-r --after-cursor <c>` walks backwards from an
//! entry, measured.)
//!
//! # What is pushed down, and what deliberately is not
//!
//! `prio` and `time` become `-p` / `PRIORITY=` and `--since` / `--until`, so a
//! query over a window of time reads a window of journal rather than a tail.
//! The **message is not pushed down to `--grep`**, though it looks like the
//! obvious third: `--grep` matches the raw bytes, and the raw bytes are full of
//! ANSI escapes that the cell no longer has. A pattern that spans one of those
//! escapes matches the cell the user sees and not the row on disk, and a
//! pushdown that drops rows the in-memory filter would have kept is worse than
//! no pushdown at all. Pushdown here only ever *narrows what is read*; the
//! query is always evaluated again in memory, and that pass is the authority.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use not_yet_done_content::{ActionOutcome, ContentError, InputSpec, NodeAction, Result};
use not_yet_done_filter::{FilterExpr, FilterLeaf, Literal, Operator, Rhs};
use serde_json::Value;

use crate::config::Manager;
use crate::model::LogRow;

/// The action that opens `journalctl -f` on this unit in a terminal.
pub const FOLLOW: &str = "follow";

/// The journal fields a row is built from.
///
/// The address fields (`__CURSOR`, both timestamps, `_BOOT_ID`, `__SEQNUM*`)
/// are emitted whether asked for or not, which is why `__CURSOR` is not listed
/// here and is still available. Asking for three fields instead of all
/// thirty-one takes a row from about 1.9 KB to about 0.8 KB — measured.
const OUTPUT_FIELDS: &str = "MESSAGE,PRIORITY,_PID";

/// How many rows a level fetches when the front-end asks for no particular
/// window. Large enough that the usual "what did it say when it failed" is
/// answered by the first page.
pub const DEFAULT_LIMIT: u32 = 200;

/// The journal action every unit level offers.
///
/// Unlike the editing and creating actions this one is offered against the
/// system manager too: reading the journal is not writing to
/// `~/.config/systemd/user`, and a user who may read the system journal may
/// read it from here.
pub fn actions_for(type_id: &str) -> Vec<NodeAction> {
    if !matches!(
        type_id,
        "systemd:service" | "systemd:timer" | "systemd:unitfile"
    ) {
        return Vec::new();
    }
    vec![NodeAction::new(
        FOLLOW,
        "Follow the journal in a terminal",
        InputSpec::None,
    )]
}

/// One window of the journal, newest first.
pub struct Page {
    pub rows: Vec<LogRow>,
    /// Whether the window journalctl filled was full — so there is more
    /// journal behind it. It is a statement about the *journal*, not about the
    /// matches: a query that filters in memory can return a short page and
    /// still have more behind it.
    pub has_more: bool,
}

/// Read `[offset, offset + limit)` of a unit's journal, newest first.
pub async fn page(
    manager: Manager,
    unit: &str,
    filter: Option<&FilterExpr>,
    offset: u32,
    limit: u32,
) -> Result<Page> {
    // A window of zero would mean `--lines=0`, which journalctl reads as "no
    // limit" — the one value that must not reach it.
    let window = offset.saturating_add(limit.max(1));
    let mut args = base_args(manager, unit);
    args.push("--reverse".to_string());
    args.push(format!("--lines={window}"));
    args.extend(Pushdown::from(filter).args());

    let out = run(&args).await?;
    let rows: Vec<LogRow> = out
        .lines()
        .filter_map(|line| LogRow::parse(unit, line))
        .collect();
    Ok(cut(rows, offset, window))
}

/// Turn the whole window journalctl filled into the page that was asked for.
///
/// The window always starts at the newest entry — journalctl has no "skip the
/// first n", so the only way to reach an offset is to read past it and throw
/// the front away. Which also settles `has_more`: a window journalctl filled
/// to the brim is one that stopped because of `--lines`, not because the
/// journal ran out.
fn cut(mut rows: Vec<LogRow>, offset: u32, window: u32) -> Page {
    let has_more = rows.len() as u32 >= window;
    rows.drain(..(offset as usize).min(rows.len()));
    Page { rows, has_more }
}

/// One entry by its cursor — how a row is found again once the table has
/// handed out its id.
pub async fn entry(manager: Manager, unit: &str, cursor: &str) -> Result<LogRow> {
    let mut args = base_args(manager, unit);
    args.push(format!("--cursor={cursor}"));
    args.push("--lines=1".to_string());
    let out = run(&args).await?;
    out.lines()
        .find_map(|line| LogRow::parse(unit, line))
        .ok_or_else(|| ContentError::NotFound(format!("{unit} has no journal entry at {cursor}")))
}

/// Open `journalctl -f` on this unit in a terminal of the user's choosing.
///
/// The adapter spawns it and lets go: the window belongs to the desktop from
/// the moment it exists, and waiting for it would block the level that started
/// it. `terminal` is a command line the journalctl argv is appended to, because
/// terminals disagree about how a command is handed to them (`-e`, `--`, or
/// nothing at all) and guessing is what a config line is for.
pub fn follow(terminal: &str, manager: Manager, unit: &str) -> Result<ActionOutcome> {
    let mut parts = terminal.split_whitespace();
    let Some(program) = parts.next() else {
        return Err(ContentError::NotSupported(
            "no terminal configured — set `terminal:` in the adapter's config".into(),
        ));
    };
    let argv: Vec<String> = parts
        .map(str::to_string)
        .chain(["journalctl".to_string()])
        .chain(base_args(manager, unit).into_iter().take(2))
        .chain(["--follow".to_string(), "--lines=200".to_string()])
        .collect();
    std::process::Command::new(program)
        .args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| ContentError::Other(format!("could not start {program}: {e}").into()))?;
    Ok(ActionOutcome::Done {
        message: Some(format!("following {unit} in {program}")),
    })
}

/// The arguments every call shares: which manager, which unit, and the output
/// shape the rows are parsed from.
///
/// The manager flag and `-u` come first, and [`follow`] relies on that: it
/// takes exactly those two and asks for human output instead.
fn base_args(manager: Manager, unit: &str) -> Vec<String> {
    vec![
        match manager {
            Manager::User => "--user".to_string(),
            Manager::System => "--system".to_string(),
        },
        format!("--unit={unit}"),
        "--no-pager".to_string(),
        "--output=json".to_string(),
        format!("--output-fields={OUTPUT_FIELDS}"),
    ]
}

/// Run journalctl and hand back its stdout.
///
/// A non-zero exit is reported with what journalctl said on stderr rather than
/// with a code: "Failed to add match: Invalid argument" is a sentence the user
/// can act on, `exit status 1` is not.
async fn run(args: &[String]) -> Result<String> {
    let out = tokio::process::Command::new("journalctl")
        .args(args)
        .output()
        .await
        .map_err(|e| ContentError::Other(format!("could not run journalctl: {e}").into()))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        let said = said.trim();
        return Err(ContentError::Other(
            if said.is_empty() {
                format!("journalctl failed ({})", out.status)
            } else {
                said.to_string()
            }
            .into(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// Pushdown
// ---------------------------------------------------------------------------

/// The part of a query journalctl can answer itself.
#[derive(Debug, Default, PartialEq)]
struct Pushdown {
    /// `-p N` — everything at or below this severity.
    max_priority: Option<u8>,
    /// `PRIORITY=N` — exactly this severity.
    priority: Option<u8>,
    /// `--since`/`--until`, as the `@<epoch seconds>` form, which is the one
    /// spelling of an instant systemd reads without a timezone to argue about.
    since: Option<i64>,
    until: Option<i64>,
}

impl Pushdown {
    /// What of a query can be handed to journalctl.
    ///
    /// Only the **top-level conjuncts** are considered: a clause under an `or`
    /// or a `not` constrains nothing on its own, and pushing it down would
    /// narrow the read past what the query asks for. Everything else — and
    /// everything pushed down, again — is evaluated in memory.
    fn from(expr: Option<&FilterExpr>) -> Self {
        let mut leaves = Vec::new();
        if let Some(expr) = expr {
            collect(expr, &mut leaves);
        }
        let mut out = Self::default();
        for leaf in leaves {
            out.take(leaf);
        }
        out
    }

    fn take(&mut self, leaf: &FilterLeaf) {
        let Rhs::Lit(lit) = &leaf.rhs else { return };
        match leaf.lhs.path().as_ref() {
            "prio" => {
                let Some(n) = as_priority(lit) else { return };
                match leaf.op {
                    Operator::Eq => self.priority = Some(n),
                    Operator::Lte => self.max_priority = Some(n),
                    // `< 3` is `<= 2`; `< 0` would be nothing at all, and
                    // journalctl has no way to say that, so it stays in memory.
                    Operator::Lt => self.max_priority = n.checked_sub(1).or(self.max_priority),
                    _ => {}
                }
            }
            "time" => {
                let Some(t) = as_instant(lit) else { return };
                match leaf.op {
                    // `--since`/`--until` are inclusive and `gt`/`lt` are not.
                    // The wider bound is the safe one: the in-memory pass
                    // removes the extra row, and a tighter one would remove a
                    // row the query wanted.
                    Operator::Gte | Operator::Gt => self.since = Some(t),
                    Operator::Lte | Operator::Lt => self.until = Some(t),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(n) = self.max_priority {
            args.push(format!("--priority={n}"));
        }
        if let Some(t) = self.since {
            args.push(format!("--since=@{t}"));
        }
        if let Some(t) = self.until {
            args.push(format!("--until=@{t}"));
        }
        // A field match, not an option — it goes last, after `--`-style flags,
        // the way journalctl's own examples spell it.
        if let Some(n) = self.priority {
            args.push(format!("PRIORITY={n}"));
        }
        args
    }
}

/// Every leaf that must hold for the whole expression to hold.
fn collect<'a>(expr: &'a FilterExpr, out: &mut Vec<&'a FilterLeaf>) {
    match expr {
        FilterExpr::And(parts) => parts.iter().for_each(|p| collect(p, out)),
        FilterExpr::Leaf(leaf) => out.push(leaf),
        _ => {}
    }
}

/// A syslog priority, `0`..=`7`. Anything else is not a priority and stays in
/// memory, where it matches nothing and says so by finding nothing.
fn as_priority(lit: &Literal) -> Option<u8> {
    let n = match lit {
        Literal::Int(n) => *n,
        Literal::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    (0..=7).contains(&n).then_some(n as u8)
}

/// A date literal as epoch seconds. By the time a query reaches an adapter its
/// date literals are already RFC 3339 — `query_filter::parse` resolves
/// "yesterday" on the way in — so this is a parse, not a second resolver.
fn as_instant(lit: &Literal) -> Option<i64> {
    let Literal::String(s) = lit else { return None };
    Some(DateTime::parse_from_rfc3339(s.trim()).ok()?.timestamp())
}

// ---------------------------------------------------------------------------
// Decoding one entry
// ---------------------------------------------------------------------------

/// One `--output=json` line as a field map, with `MESSAGE` already readable.
pub(crate) fn decode(line: &str) -> Option<HashMap<String, String>> {
    let Value::Object(fields) = serde_json::from_str::<Value>(line).ok()? else {
        return None;
    };
    Some(
        fields
            .into_iter()
            .map(|(k, v)| (k, scalar(&v)))
            .collect::<HashMap<_, _>>(),
    )
}

/// A journal field as text.
///
/// Three shapes, all of them real: a string, the byte array journald falls back
/// to for anything not plainly printable, and — for a field logged more than
/// once in one entry — an array of those. The last is joined with newlines,
/// which is what the multi-line row is for.
fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => clean(s),
        Value::Number(n) => n.to_string(),
        Value::Array(items) if items.iter().all(Value::is_number) => {
            let bytes: Vec<u8> = items
                .iter()
                .filter_map(|n| n.as_u64())
                .map(|n| n as u8)
                .collect();
            clean(&String::from_utf8_lossy(&bytes))
        }
        Value::Array(items) => items.iter().map(scalar).collect::<Vec<_>>().join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Text as a table cell: no escape sequences, no stray control bytes.
///
/// Tabs and newlines survive — a stack trace is why the level has multi-line
/// rows at all — and everything else below `0x20` does not, because a raw
/// `\r` or `\x07` in a cell moves the cursor or rings the terminal.
fn clean(raw: &str) -> String {
    strip_ansi(raw)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// Drop ANSI escape sequences.
///
/// The three shapes that actually occur in logs: CSI (`ESC [ … final`, which is
/// every colour), OSC (`ESC ] … BEL` or `ESC \`, which is how a terminal title
/// or a hyperlink is set), and the two-character escapes. Anything else that
/// starts with ESC loses the ESC and keeps its text, which is the safe way to
/// be wrong: a visible stray letter, never a swallowed message.
fn strip_ansi(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // Parameter and intermediate bytes, then one final byte.
                while let Some(c) = chars.next() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                // Up to BEL, or to the ESC of a string terminator.
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// A journal timestamp (microseconds since the epoch, as a decimal string) as
/// an instant.
pub(crate) fn instant(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_micros(raw.trim().parse::<i64>().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_filter::query_filter;

    fn expr(raw: &str) -> FilterExpr {
        query_filter::parse(raw).unwrap().expr
    }

    /// The measured case: a service that logs through `tracing` puts its whole
    /// line into `MESSAGE` as bytes, colours and all. Reading it as a string
    /// would leave the column empty for every such unit.
    #[test]
    fn a_message_of_bytes_reads_as_the_line_it_is() {
        let raw = "\u{1b}[2m2026-09-12T18:04:25Z\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m feed stalled";
        let bytes: Vec<String> = raw.bytes().map(|b| b.to_string()).collect();
        let line = format!(r#"{{"MESSAGE":[{}],"PRIORITY":"4"}}"#, bytes.join(","));
        let fields = decode(&line).unwrap();
        assert_eq!(fields["MESSAGE"], "2026-09-12T18:04:25Z  WARN feed stalled");
        assert_eq!(fields["PRIORITY"], "4");
    }

    #[test]
    fn a_plain_string_message_survives_untouched() {
        let fields = decode(r#"{"MESSAGE":"Started backup-photos.service."}"#).unwrap();
        assert_eq!(fields["MESSAGE"], "Started backup-photos.service.");
    }

    /// A cell may hold the newlines of a stack trace and nothing else that
    /// moves a terminal cursor.
    #[test]
    fn newlines_stay_and_the_rest_of_the_control_bytes_go() {
        assert_eq!(clean("one\ntwo\tthree\r\u{7}four"), "one\ntwo\tthreefour");
        assert_eq!(clean("\u{1b}]0;a title\u{7}body"), "body");
    }

    /// Pushdown is an optimisation, so what it must never do is narrow more
    /// than the query does. A bound under an `or` constrains nothing on its
    /// own and therefore travels nowhere.
    #[test]
    fn only_the_top_level_conjuncts_are_pushed_down() {
        let both = Pushdown::from(Some(&expr(
            "query:\n  and:\n    - [prio, lte, 3]\n    - [message, has, stall]",
        )));
        assert_eq!(both.max_priority, Some(3));
        assert_eq!(both.args(), vec!["--priority=3".to_string()]);

        let either = Pushdown::from(Some(&expr(
            "query:\n  or:\n    - [prio, lte, 3]\n    - [message, has, stall]",
        )));
        assert_eq!(either, Pushdown::default());
        assert!(either.args().is_empty());
    }

    /// `<` is not `<=`, and a date bound arrives already resolved.
    #[test]
    fn the_bounds_journalctl_gets_are_never_tighter_than_the_query() {
        let p = Pushdown::from(Some(&expr("query:\n  [prio, lt, 3]")));
        assert_eq!(p.max_priority, Some(2));

        let when = "2026-09-12T06:00:00+00:00";
        let epoch = DateTime::parse_from_rfc3339(when).unwrap().timestamp();
        let t = Pushdown::from(Some(&expr(&format!("query:\n  [time, gt, '{when}']"))));
        assert_eq!(t.since, Some(epoch));
        assert_eq!(t.args(), vec![format!("--since=@{epoch}")]);
    }

    /// An exact severity is a field match, not a `-p` range — `-p 3` means
    /// "3 and worse" and would widen the query into rows it did not ask for.
    #[test]
    fn an_exact_priority_is_a_field_match() {
        let p = Pushdown::from(Some(&expr("query:\n  [prio, '=', 3]")));
        assert_eq!(p.args(), vec!["PRIORITY=3".to_string()]);
    }

    /// The message is deliberately not pushed down — see the module docs.
    #[test]
    fn the_message_stays_in_memory() {
        let p = Pushdown::from(Some(&expr("query:\n  [message, has, 'connection reset']")));
        assert!(p.args().is_empty());
    }

    /// journalctl can only count back from the newest entry, so a second page
    /// is the first one read again and thrown away down to its offset. What
    /// must not slip is which rows survive and whether there is another page
    /// behind them.
    #[test]
    fn a_later_page_drops_exactly_the_entries_before_it() {
        let rows = |n: usize| -> Vec<LogRow> {
            (0..n)
                .map(|i| LogRow {
                    cursor: format!("s=cursor-{i}"),
                    ..LogRow::default()
                })
                .collect()
        };

        // Second page of fifty: read a hundred, keep the back half.
        let page = cut(rows(100), 50, 100);
        assert_eq!(page.rows.len(), 50);
        assert_eq!(page.rows[0].cursor, "s=cursor-50");
        assert!(
            page.has_more,
            "a window journalctl filled has more behind it"
        );

        // The journal ran out inside the window — the page is short and it is
        // the last one.
        let page = cut(rows(70), 50, 100);
        assert_eq!(page.rows.len(), 20);
        assert_eq!(page.rows[0].cursor, "s=cursor-50");
        assert!(!page.has_more);

        // Past the end of the journal entirely: empty, not a panic.
        let page = cut(rows(20), 50, 100);
        assert!(page.rows.is_empty());
        assert!(!page.has_more);
    }

    #[test]
    fn the_manager_decides_which_journal_is_read() {
        let user = base_args(Manager::User, "backup-photos.service");
        assert_eq!(user[0], "--user");
        assert_eq!(user[1], "--unit=backup-photos.service");
        assert_eq!(base_args(Manager::System, "sshd.service")[0], "--system");
    }

    #[test]
    fn the_journal_level_hangs_off_every_unit_level_and_nothing_else() {
        for level in ["systemd:service", "systemd:timer", "systemd:unitfile"] {
            assert_eq!(actions_for(level).len(), 1, "{level}");
        }
        assert!(actions_for("systemd:property").is_empty());
        assert!(actions_for("systemd:manager").is_empty());
    }
}
