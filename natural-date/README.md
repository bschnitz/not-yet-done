# natural-date

Resolve natural-language date/time and offset phrases to concrete instants,
relative to a caller-supplied reference time — in English and German, with
more languages a lexicon away. App-agnostic: it depends only on
[`chrono`](https://crates.io/crates/chrono) and a few standalone parser crates,
never on any application type, and `now` is always a parameter so resolution is
deterministic and testable.

```rust
use chrono::{TimeZone, Utc};

let now = Utc.with_ymd_and_hms(2026, 7, 18, 10, 0, 0).unwrap(); // a Saturday

assert_eq!(
    natural_date::resolve_datetime("tomorrow morning", now).unwrap(),
    Utc.with_ymd_and_hms(2026, 7, 19, 9, 0, 0).unwrap(),
);
assert_eq!(
    natural_date::resolve_datetime("in 2 hours", now).unwrap(),
    Utc.with_ymd_and_hms(2026, 7, 18, 12, 0, 0).unwrap(),
);
```

`morgen 18 Uhr`, `Ende nächster Woche` and `halb 3` resolve through the same
call: the free functions understand every built-in language.

## What it understands

On top of the phrasings inherited from
[`chrono-english`](https://crates.io/crates/chrono-english),
[`date-periods`](https://crates.io/crates/date-periods) and
[`dateparser`](https://crates.io/crates/dateparser), it adds a preprocessor for:

- **Relative** — `in 2 hours`, `in 30 min`, `in a week`.
- **Part of day** — `morning`, `noon`, `tonight`, combinable as
  `tomorrow morning`, `friday noon`.
- **Abbreviations** — `eod`, `sod`, `eow`, `eom`, `eoy`, `cob`, `sob`.
- **Quarters** — `q3`, `end of q4`, `q1 2027`.
- **ISO week** — `2026-w30` (the Monday of that week).
- **Spoken time** — `half past 2`, `quarter to 5`.
- **Business days** — `next business day`, `in 2 working days`.
- **Dotted dates, day first** — `8.10.`, `8.10.26`, `08.10.2026 14:30`
  (without a year: the current one).
- **`now` / `asap`**, **`day after tomorrow`**, **`this friday`**.

### German

Every phrase above has a German spelling, rewritten word by word into the
English one. Umlauts may be written out (`naechste`, `uebermorgen`).

| kind | examples |
|---|---|
| day words | `heute`, `morgen`, `übermorgen`, `gestern`, `vorgestern` |
| the instant | `jetzt`, `sofort` |
| clock time | `14:30`, `18 Uhr`, `um 18:30 Uhr`, `morgen 18 Uhr`, `Freitag 17 Uhr` |
| part of day | `heute abend`, `morgen früh`, `heute morgen`, `mittags`, `morgen nachmittag`, `heute Nacht`, `Feierabend` (17:00) |
| weekdays | `Freitag`, `Fr`, `am Freitag`, `diesen Freitag`, `nächsten Montag`, `letzten Freitag` |
| periods | `nächste Woche`, `diese Woche`, `Ende nächster Woche`, `Ende des Monats`, `Monatsende`, `Anfang nächsten Monats`, `Jahresende` |
| spans | `in 2 Std`, `in 3 Tagen`, `in einer Woche`, `in einer halben Stunde`, `vor 3 Tagen` |
| business days | `nächster Werktag`, `in 2 Werktagen` |
| dates | `8.10.`, `08.10.2026 14:30`, `8. Oktober`, `1. März 2027` |
| spoken time | `halb 3` (2:30), `viertel nach 5`, `viertel vor 5`, `dreiviertel 5`, `10 vor 8` |

`morgen` is the one real ambiguity: lowercased, "tomorrow" and "morning" are
the same word. On its own and before a time it is tomorrow; it is the morning
only where the phrase says so (`heute morgen`, `am Morgen`, `morgen früh`).
Spoken times are 24-hour, like the English `half past 2`.

Three readings differ from a plain `chrono-english` pass, on purpose:

- **Day words are calendar days.** `today`, `yesterday` and `tomorrow` resolve
  to midnight of that day, not to the reference time of day — so that
  `> today` in a filter means "since this morning", not "later than this very
  moment". `now` is the instant; `resolve_date` drops the time either way.
- **A bare period is its start.** `last month`, `this week`, `next year` go to
  `date-periods` and resolve to the first instant of that period. Left to
  `chrono-english`, `last month` reads as `last mon…` and answers with a
  Monday.
- **Today's weekday is today.** `friday` (or `fri 17:00`, `friday noon`) on a
  Friday resolves to that same day, not to the Friday a week later. Both
  `next friday` and `last friday` jump a week from there.

## API

- `resolve_datetime(s, now) -> Option<DateTime<Utc>>` — an absolute instant.
- `resolve_date(s, now) -> Option<NaiveDate>` — a calendar date for all-day
  fields.
- `resolve_detailed(s, now) -> Option<Resolution>` — the instant plus
  `has_time`: `false` when the phrase named only a date (`morgen`, `8.10.`,
  `end of month`, `in 3 days`), whose time is then the convention (midnight,
  `23:59:59` for the end of a period, the reference time for a span of days).
- `resolve_offset(s) -> Option<Duration>` — a signed offset grammar (`+1h`,
  `-30min`, `+2days`); a leading sign is required.

The free functions use every built-in language, English first, weeks starting
on Monday. `Resolver` chooses:

```rust
use natural_date::{Language, Resolver, WeekStart};

let r = Resolver::new()
    .languages([Language::german()])
    .week_start(WeekStart::Sunday);
let when = r.resolve_detailed("morgen 18 Uhr", now);
```

## How it works

A phrase is first tried as RFC 3339. Then every **language** rewrites it into
the canonical vocabulary — English — and the **meanings** (resolver passes)
read the rewrites. Only when no meaning answered any language's rewrite do the
broad fallbacks (`chrono-english`, `dateparser`) get them, so a fallback's
guess at one language's words never beats an exact reading in another.

### Adding a language

A language is a lexicon: `(pattern, rewrite)` pairs in its own file under
`src/lang/` (see `de.rs`), registered with one line in `Language::builtin`.
Patterns are tokens; the longest match wins, unmatched words pass through, an
empty rewrite drops a filler, and the placeholders `{n}`/`{m}` (a number),
`{t}` (a clock time, `18.30` → `18:30`) and `{d}` (an ordinal day, `8.`) carry
values across:

```rust
static FR: &[natural_date::Entry] = &[
    ("demain", "tomorrow"),
    ("{n} h", "{n}:00"),
    ("à", ""),
];
let r = Resolver::new().language(Language::new("fr", FR));
```

`Language::with_fold` sets how words are compared (German folds `ä` → `ae`).
If a language needs a phrase English has no words for, add a meaning for a
canonical phrase and let the lexicon rewrite to it.

### Adding a meaning

A meaning is a pass `fn(&str, &Context) -> Option<Reading>` over a canonical
phrase. Built-in ones live in `src/passes.rs` and are listed in `builtin()`
there; a caller adds its own with `Resolver::meaning(Meaning::new(name, pass))`,
which is tried before the built-ins. `Context` gives the reference wall clock,
the week start and `resolve` for a sub-phrase (how `tomorrow morning` resolves
its `tomorrow`). Because languages only rewrite, one new meaning serves every
language.

## License

Licensed under either of Apache-2.0 or MIT at your option.
