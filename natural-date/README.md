# natural-date

Resolve natural-language date/time and offset phrases to concrete instants,
relative to a caller-supplied reference time. App-agnostic: it depends only on
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
- **`now` / `asap`**.

## API

- `resolve_datetime(s, now) -> Option<DateTime<Utc>>` — an absolute instant.
- `resolve_date(s, now) -> Option<NaiveDate>` — a calendar date for all-day
  fields.
- `resolve_offset(s) -> Option<Duration>` — a signed offset grammar (`+1h`,
  `-30min`, `+2days`); a leading sign is required.

## License

Licensed under either of Apache-2.0 or MIT at your option.
