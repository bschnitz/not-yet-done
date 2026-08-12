# date-periods

Resolve natural-language **period-boundary** phrases — `end of next week`,
`start of month`, `beginning of last quarter` — to concrete dates, relative to a
reference instant.

This fills the gap left by relative-date parsers such as
[`chrono-english`](https://crates.io/crates/chrono-english), which understand
`today` / `2 weeks` / `next monday` but not calendar _period boundaries_. It is
app-agnostic: it depends only on [`chrono`](https://crates.io/crates/chrono) and
takes `now` as a parameter, so resolution is deterministic and testable.

## Grammar

```text
<boundary> of [the] [<rel>] <unit>

boundary := start | beginning | end
rel      := this | next | last | previous     (optional; default: this)
unit     := day | week | month | quarter | year
```

A `start` resolves to `00:00:00` of the boundary day, an `end` to `23:59:59`.

## Usage

```rust
use chrono::{Local, TimeZone};
use date_periods::{resolve, WeekStart};

let now = Local.with_ymd_and_hms(2026, 7, 9, 12, 0, 0).unwrap(); // a Thursday
let end = resolve("end of next week", now, WeekStart::Monday).unwrap();
assert_eq!(end.date_naive().to_string(), "2026-07-19");
```

`parse` returns a `PeriodSpec` if you want the parsed boundary/rel/unit without
zoning it to an instant.

## License

Licensed under either of Apache-2.0 or MIT at your option.
