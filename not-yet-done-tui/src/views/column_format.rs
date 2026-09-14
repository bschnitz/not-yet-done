//! Typed column-value formatting (plan mechanism M2).
//!
//! A [`ColumnDef`](crate::config::view_config::ColumnDef) carries a
//! [`ColumnKind`] declaring the *semantic* type of its value. Adapters stay
//! untyped: they emit a **canonical string** per kind, and this module turns
//! that string into the display form plus its alignment. Keeping the type in
//! the view YAML (not on every adapter's `MetadataField`) means remote
//! adapters — which are all `kind: text` — need no change at all.
//!
//! Canonical encodings the formatter expects:
//!
//! - `number`   → a decimal number (`"42"`, `"3.5"`); rendered verbatim,
//!   right-aligned.
//! - `duration` → **seconds**, integer or fractional (`"5400"`,
//!   `"0.103424"`); rendered right-aligned in the column's
//!   [`DurationFormat`], which its `format:` selects. The default `clock`
//!   uses the shared [`format_duration`] (`H:MM:SS`) so the adapterized view
//!   matches the legacy Trackings rendering exactly; `precise` uses
//!   [`format_span_precise`] (`5.24s`, `103ms`), which is what a span living
//!   below the second needs.
//! - `datetime` → RFC 3339 (`"2026-06-09T08:15:00Z"`); rendered in the local
//!   timezone, `%Y-%m-%d %H:%M` by default or a custom strftime `format`.
//! - `path`     → `/`-separated segments (`"/a/b/c"`); rendered with the
//!   column's display `separator` (default `/`), always leading with one so a
//!   root renders as just the separator. The per-segment *styling* (separator
//!   color) is applied later, in the render layer — see
//!   `path_cell_segments` in `content_view`.
//! - `elapsed`  → carries no value of its own; the cell builder reads the
//!   column's `elapsed_from` field (an RFC 3339 instant) and calls
//!   [`format_elapsed_since`] with the current time, rendering `now − that`
//!   as a duration (right-aligned). Recomputed each repaint tick → a live
//!   timer. Not handled by [`format_typed_value`] (it needs `now` and a
//!   foreign source field).
//! - `countdown` → the mirror of `elapsed`: carries no value of its own, the
//!   cell builder reads the column's `countdown_to` field (an RFC 3339
//!   instant) and calls [`format_countdown_to`], rendering `that − now` on
//!   the coarse scale of [`format_span_coarse`]. Right-aligned, recomputed
//!   each repaint tick. Not handled by [`format_typed_value`].
//! - `bytes`    → a byte count (`"99999744"`); rendered on the binary scale
//!   (`95.4 MiB`), right-aligned. The value stays the number, so filtering
//!   and sorting keep working on bytes.
//! - `text`     → returned verbatim, left-aligned (the default).
//!
//! A value that fails to parse for its kind is returned verbatim (and left
//! aligned) rather than blanked, so malformed data stays visible instead of
//! silently vanishing. An empty string stays empty for every kind.

use chrono::{DateTime, Duration, Local};

use not_yet_done_table::CellAlignment;

use crate::config::view_config::ColumnKind;

/// Render a [`Duration`] as `H:MM:SS` (hours dropped when zero, then
/// `MM:SS`, then `SS`). Negative spans clamp to zero. Shared by the
/// `Duration` column kind and the live-elapsed (M5) path so durations
/// read identically everywhere.
pub fn format_duration(d: Duration) -> String {
    let total_secs = d.num_seconds().max(0);
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else if m > 0 {
        format!("{m:02}:{s:02}")
    } else {
        format!("{s:02}")
    }
}

/// Map a backend-described [`ColumnSchema`](not_yet_done_content::ColumnSchema)
/// `value_type` onto the view's [`ColumnKind`]. `Some` for the types that
/// change rendering (number/duration/datetime); `None` for `text` and any
/// unknown type, leaving the YAML-configured `kind` in place. This is the
/// front-end side of the backend→front-end type channel (`describe_columns`).
pub fn column_kind_from_value_type(value_type: &str) -> Option<ColumnKind> {
    match value_type {
        "number" => Some(ColumnKind::Number),
        "duration" => Some(ColumnKind::Duration),
        "datetime" => Some(ColumnKind::Datetime),
        _ => None,
    }
}

/// The inverse of [`column_kind_from_value_type`]: map a view [`ColumnKind`]
/// onto the canonical `value_type` string the custom-column store understands.
/// Drives the front-end→backend type channel for `edit-cells`, where the TUI
/// sends each cell's type so the store can bootstrap the column on first write.
/// `Path`/`Elapsed`/`Countdown` have no dedicated store type, so they degrade
/// to the nearest fit (`text`/`duration`); `Bytes` is a number and says so.
pub fn value_type_from_column_kind(kind: ColumnKind) -> &'static str {
    match kind {
        ColumnKind::Number | ColumnKind::Bytes => "number",
        ColumnKind::Duration | ColumnKind::Elapsed | ColumnKind::Countdown => "duration",
        ColumnKind::Datetime => "datetime",
        ColumnKind::Text | ColumnKind::Path => "text",
    }
}

/// Default display pattern for `datetime` columns without an explicit
/// `format`.
const DEFAULT_DATETIME_FORMAT: &str = "%Y-%m-%d %H:%M";

/// Format a column's canonical value `raw` for display.
///
/// Returns the display text and the alignment the cell should use. See the
/// [module docs](self) for the per-kind canonical encodings.
pub fn format_typed_value(
    raw: &str,
    kind: ColumnKind,
    format: Option<&str>,
    separator: &str,
) -> (String, CellAlignment) {
    if raw.is_empty() {
        return (String::new(), CellAlignment::Left);
    }
    match kind {
        ColumnKind::Text => (raw.to_string(), CellAlignment::Left),
        ColumnKind::Number => (raw.to_string(), CellAlignment::Right),
        ColumnKind::Duration => (
            format_duration_secs(raw, DurationFormat::parse(format)),
            CellAlignment::Right,
        ),
        ColumnKind::Datetime => (format_datetime(raw, format), CellAlignment::Left),
        ColumnKind::Path => (format_path(raw, separator), CellAlignment::Left),
        // `elapsed` and `countdown` are time-derived: they need `now` and read
        // a *different* source field, so the cell builder computes them via
        // [`format_elapsed_since`] / [`format_countdown_to`], not here. This
        // arm is only reached if a view mislabels a plain column one of the
        // two — render the raw verbatim, right-aligned, rather than panicking.
        ColumnKind::Elapsed | ColumnKind::Countdown => (raw.to_string(), CellAlignment::Right),
        ColumnKind::Bytes => (format_bytes(raw), CellAlignment::Right),
    }
}

/// Render the time span between `now` and the RFC 3339 instant `raw`
/// (M5 live-elapsed). The result uses the shared [`format_duration`]
/// (`H:MM:SS`), right-aligned, so a running timer matches the duration
/// columns next to it. `now` is a parameter so the computation is
/// deterministic under test.
///
/// - Empty input → empty cell (left-aligned, matching the other kinds).
/// - Unparseable input → passed through verbatim, left-aligned.
/// - An instant in the future (clock skew) → `format_duration` clamps the
///   negative span to zero, so it renders as `00` rather than going negative.
pub fn format_elapsed_since(raw: &str, now: DateTime<Local>) -> (String, CellAlignment) {
    if raw.is_empty() {
        return (String::new(), CellAlignment::Left);
    }
    match DateTime::parse_from_rfc3339(raw.trim()) {
        Ok(dt) => {
            let span = now.signed_duration_since(dt.with_timezone(&Local));
            (format_duration(span), CellAlignment::Right)
        }
        Err(_) => (raw.to_string(), CellAlignment::Left),
    }
}

/// The name a `duration` column's `format:` may carry, and what it selects.
///
/// The default is [`Clock`](DurationFormat::Clock) — every duration column
/// that existed before this enum did renders exactly as it always has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DurationFormat {
    /// `H:MM:SS`, hours dropped when zero, then `MM:SS`, then `SS`. What a
    /// tracked time wants: a clock face, with every cell the same shape so a
    /// column of them reads as a column.
    #[default]
    Clock,
    /// The largest unit the value actually carries plus at most one more,
    /// all the way down through milliseconds and microseconds: `1min 3s`,
    /// `5.24s`, `103ms`, `31us`. What a *latency* wants — a span that lives
    /// below the second, where [`Clock`](DurationFormat::Clock) would render
    /// every row as `00`.
    Precise,
}

/// The format names a `duration` column accepts, for the validator's message.
pub const DURATION_FORMATS: [&str; 2] = ["clock", "precise"];

impl DurationFormat {
    /// Parse a column's `format:` string. Unknown names fall back to the
    /// default here; the view validator is what rejects them, so a typo is
    /// reported once at load rather than silently every repaint.
    pub fn parse(name: Option<&str>) -> Self {
        match name.map(str::trim) {
            Some("precise") => Self::Precise,
            _ => Self::Clock,
        }
    }
}

/// Parse seconds and render them in the column's [`DurationFormat`].
///
/// The canonical input is **seconds**, integer or fractional: `"5400"` is
/// ninety minutes and `"0.103424"` is 103 milliseconds. Parsing as `f64` and
/// not `i64` is what lets a sub-second span exist at all, and it costs the
/// whole-second columns nothing — `format_duration` takes whole seconds
/// either way, so `"5400"` renders today's `1:30:00` down to the byte.
///
/// Input that is not a number at all is passed through unchanged, like every
/// other kind: malformed data stays visible instead of silently vanishing.
fn format_duration_secs(raw: &str, format: DurationFormat) -> String {
    let Ok(secs) = raw.trim().parse::<f64>() else {
        return raw.to_string();
    };
    match format {
        DurationFormat::Clock => format_duration(Duration::seconds(secs as i64)),
        DurationFormat::Precise => format_span_precise(secs),
    }
}

/// Render a span in seconds on the scale its own magnitude asks for.
///
/// ```text
///   1min 3s      5.24s      103ms      31us      0
/// ```
///
/// The ladder is the one `systemd-analyze blame` and `ping` and every other
/// latency reporter uses, and for the same reason: the interesting digits of
/// a span sit just below its largest unit, and which unit that is changes by
/// orders of magnitude from row to row. A fixed unit would spend most of a
/// column on leading zeros or trailing noise.
///
/// Above a minute it reads as two whole units (`1min 3s`) — at that distance
/// the milliseconds are noise. Below a minute it reads as one unit with three
/// significant digits (`5.24s`, `103ms`), which is where a latency is
/// actually compared. A zero remainder is dropped (`2min`, not `2min 0s`),
/// and an exact zero is a bare `0`: a span that did not happen and a span
/// that took no measurable time are different things, and the empty cell
/// already means the first one.
///
/// `us` and not `µs` because the table measures cell widths in columns and a
/// terminal's idea of how wide `µ` is depends on its font.
pub fn format_span_precise(secs: f64) -> String {
    let sign = if secs < 0.0 { "-" } else { "" };
    let s = secs.abs();
    if s == 0.0 {
        return "0".to_string();
    }
    // The threshold is 59.95 and not 60, and the ladder rounds rather than
    // truncates, so that a span which would *render* as "60.0s" is promoted
    // to "1min" instead. Every rung below does the same (see the unit choice
    // further down): the ladder must never print a number that belongs on
    // the rung above it.
    if s >= 59.95 {
        let total = s.round() as i64;
        let (big, big_unit, small, small_unit) = if total >= 7 * 86400 {
            (total / (7 * 86400), "w", (total % (7 * 86400)) / 86400, "d")
        } else if total >= 86400 {
            (total / 86400, "d", (total % 86400) / 3600, "h")
        } else if total >= 3600 {
            (total / 3600, "h", (total % 3600) / 60, "min")
        } else {
            (total / 60, "min", total % 60, "s")
        };
        return if small > 0 {
            format!("{sign}{big}{big_unit} {small}{small_unit}")
        } else {
            format!("{sign}{big}{big_unit}")
        };
    }
    // Below a minute: one unit, three significant digits. Each threshold is
    // half a display step below the round number, so a value that would
    // round up into the next unit (`1000ms`) or the next digit band
    // (`10.00s`) moves there instead of printing a fourth digit.
    let (value, unit) = if s >= 0.9995 {
        (s, "s")
    } else if s >= 0.0009995 {
        (s * 1_000.0, "ms")
    } else {
        (s * 1_000_000.0, "us")
    };
    let decimals = if value >= 99.95 {
        0
    } else if value >= 9.995 {
        1
    } else {
        2
    };
    format!("{sign}{value:.decimals$}{unit}")
}

/// Parse an RFC 3339 instant and render it in the local timezone. Unparseable
/// input is passed through unchanged.
fn format_datetime(raw: &str, format: Option<&str>) -> String {
    match DateTime::parse_from_rfc3339(raw.trim()) {
        Ok(dt) => dt
            .with_timezone(&Local)
            .format(format.unwrap_or(DEFAULT_DATETIME_FORMAT))
            .to_string(),
        Err(_) => raw.to_string(),
    }
}

/// Render a canonical `/`-separated path with the display `separator`,
/// always leading with one (so a root path is just the separator).
fn format_path(raw: &str, separator: &str) -> String {
    let mut out = String::new();
    for seg in raw.split('/').filter(|s| !s.is_empty()) {
        out.push_str(separator);
        out.push_str(seg);
    }
    if out.is_empty() {
        // Canonical "/" (or a string of only separators) → a bare root.
        out.push_str(separator);
    }
    out
}

/// Render a span on the coarse two-unit scale a countdown reads best in:
/// the largest non-zero unit plus the next one down, and nothing below it.
///
/// ```text
///   1w 2d      2d 13h      5h 24min      24min 13s      13s
/// ```
///
/// The ladder stops at weeks on purpose. Every unit up to and including a
/// week is a fixed span, so the number never depends on *which* week it is;
/// a month is not, and "in 1mo" would mean something different in February
/// than in July. At that distance the instant column beside a countdown
/// (`next`, `start`) is the honest answer anyway.
///
/// A zero remainder is dropped rather than shown — `5h`, not `5h 0min` —
/// and the span is truncated, not rounded, so a countdown never claims more
/// time than is left. Negative spans are rendered by the caller with a
/// leading `-`; this function takes the magnitude.
pub fn format_span_coarse(d: Duration) -> String {
    let secs = d.num_seconds().abs();
    const WEEK: i64 = 7 * 86400;
    let (big, big_unit, small, small_unit) = if secs >= WEEK {
        (secs / WEEK, "w", (secs % WEEK) / 86400, "d")
    } else if secs >= 86400 {
        (secs / 86400, "d", (secs % 86400) / 3600, "h")
    } else if secs >= 3600 {
        (secs / 3600, "h", (secs % 3600) / 60, "min")
    } else if secs >= 60 {
        (secs / 60, "min", secs % 60, "s")
    } else {
        (secs, "s", 0, "")
    };
    if small > 0 {
        format!("{big}{big_unit} {small}{small_unit}")
    } else {
        format!("{big}{big_unit}")
    }
}

/// Render the span between the RFC 3339 instant `raw` and `now` as a
/// countdown — the mirror of [`format_elapsed_since`]. `now` is a parameter
/// so the computation is deterministic under test.
///
/// - Empty input → empty cell (left-aligned, matching the other kinds).
/// - Unparseable input → passed through verbatim, left-aligned.
/// - An instant already past → the same scale with a leading `-`, so an
///   event that started ten minutes ago reads `-10min`. Clamping to zero
///   was the other option and is wrong here: a calendar's countdown column
///   spends half its life on events that have already begun, and "0s" hides
///   exactly the thing one looks at it for.
pub fn format_countdown_to(raw: &str, now: DateTime<Local>) -> (String, CellAlignment) {
    if raw.is_empty() {
        return (String::new(), CellAlignment::Left);
    }
    match DateTime::parse_from_rfc3339(raw.trim()) {
        Ok(dt) => {
            let span = dt.with_timezone(&Local).signed_duration_since(now);
            let sign = if span.num_seconds() < 0 { "-" } else { "" };
            (
                format!("{sign}{}", format_span_coarse(span)),
                CellAlignment::Right,
            )
        }
        Err(_) => (raw.to_string(), CellAlignment::Left),
    }
}

/// Render a byte count on the binary scale: `512 B`, `95.4 MiB`, `1.2 GiB`.
///
/// One decimal below 100 and none above it, so a cell is at most five
/// digits wide and the column reads as a scale rather than as a wall of
/// numbers. Binary and not decimal because that is what every tool next to
/// which these numbers appear uses — `systemctl status`, `du -h`,
/// `systemd-cgtop`.
///
/// Non-integer input is passed through unchanged, like every other kind:
/// malformed data stays visible instead of silently vanishing.
fn format_bytes(raw: &str) -> String {
    let Ok(bytes) = raw.trim().parse::<f64>() else {
        return raw.to_string();
    };
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let sign = if bytes < 0.0 { "-" } else { "" };
    let mut value = bytes.abs();
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{sign}{} {}", value as i64, UNITS[unit])
    } else if value < 100.0 {
        format!("{sign}{value:.1} {}", UNITS[unit])
    } else {
        format!("{sign}{} {}", value.round() as i64, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A countdown names the largest unit it has and the next one down, and
    /// nothing below that: at a week's distance the seconds are noise, at a
    /// minute's distance they are the whole point.
    #[test]
    fn a_countdown_names_two_units_and_drops_a_zero_remainder() {
        let s = |secs: i64| format_span_coarse(Duration::seconds(secs));
        assert_eq!(s(13), "13s");
        assert_eq!(s(24 * 60 + 13), "24min 13s");
        assert_eq!(s(5 * 3600 + 24 * 60), "5h 24min");
        assert_eq!(s(2 * 86400 + 13 * 3600), "2d 13h");
        assert_eq!(s(9 * 86400), "1w 2d");
        // Nothing below the second unit, and no `5h 0min`.
        assert_eq!(s(5 * 3600 + 59), "5h");
        assert_eq!(s(2 * 86400), "2d");
        // Truncated, not rounded: a countdown never claims more time than
        // is left.
        assert_eq!(s(2 * 3600 - 1), "1h 59min");
    }

    #[test]
    fn a_countdown_reads_from_an_instant_and_goes_negative_when_it_is_past() {
        let now = DateTime::parse_from_rfc3339("2026-09-14T12:00:00Z")
            .unwrap()
            .with_timezone(&Local);
        let at = |s: &str| format_countdown_to(s, now);

        assert_eq!(
            at("2026-09-14T17:24:00Z"),
            ("5h 24min".to_string(), CellAlignment::Right)
        );
        // Already begun — the case a calendar spends half its life in.
        assert_eq!(
            at("2026-09-14T11:50:00Z"),
            ("-10min".to_string(), CellAlignment::Right)
        );
        // Empty stays empty, unparseable stays visible.
        assert_eq!(at(""), (String::new(), CellAlignment::Left));
        assert_eq!(at("soon"), ("soon".to_string(), CellAlignment::Left));
    }

    #[test]
    fn bytes_render_on_the_binary_scale_and_keep_their_value() {
        let b = |raw: &str| format_typed_value(raw, ColumnKind::Bytes, None, "/").0;
        assert_eq!(b("512"), "512 B");
        assert_eq!(b("1024"), "1.0 KiB");
        assert_eq!(b("99999744"), "95.4 MiB");
        assert_eq!(b("1288490189"), "1.2 GiB");
        // One decimal below 100, none above it.
        assert_eq!(b("536870912"), "512 MiB");
        // Not a number → visible, not blanked.
        assert_eq!(b("n/a"), "n/a");
        // Right-aligned like every other number.
        assert_eq!(
            format_typed_value("1024", ColumnKind::Bytes, None, "/").1,
            CellAlignment::Right
        );
    }

    #[test]
    fn text_is_verbatim_left() {
        assert_eq!(
            format_typed_value("hello", ColumnKind::Text, None, "/"),
            ("hello".to_string(), CellAlignment::Left)
        );
    }

    #[test]
    fn value_type_maps_to_typed_kinds_only() {
        assert!(matches!(
            column_kind_from_value_type("number"),
            Some(ColumnKind::Number)
        ));
        assert!(matches!(
            column_kind_from_value_type("duration"),
            Some(ColumnKind::Duration)
        ));
        assert!(matches!(
            column_kind_from_value_type("datetime"),
            Some(ColumnKind::Datetime)
        ));
        // `text` and unknown types leave the YAML-configured kind in place.
        assert!(column_kind_from_value_type("text").is_none());
        assert!(column_kind_from_value_type("bogus").is_none());
    }

    #[test]
    fn empty_stays_empty_for_every_kind() {
        for kind in [
            ColumnKind::Text,
            ColumnKind::Number,
            ColumnKind::Duration,
            ColumnKind::Datetime,
            ColumnKind::Path,
        ] {
            assert_eq!(
                format_typed_value("", kind, None, "/"),
                (String::new(), CellAlignment::Left)
            );
        }
    }

    #[test]
    fn number_is_right_aligned() {
        let (text, align) = format_typed_value("42", ColumnKind::Number, None, "/");
        assert_eq!(text, "42");
        assert_eq!(align, CellAlignment::Right);
    }

    #[test]
    fn duration_seconds_render_like_legacy_trackings() {
        // 1h 30m 0s → "1:30:00"; 90s → "01:30"; 5s → "05".
        let clock = |raw| format_duration_secs(raw, DurationFormat::Clock);
        assert_eq!(clock("5400"), "1:30:00");
        assert_eq!(clock("90"), "01:30");
        assert_eq!(clock("5"), "05");
    }

    /// The whole point of parsing `f64` instead of `i64` is that it must cost
    /// the existing columns nothing. A column that has always sent whole
    /// seconds renders identically whether the parse is integer or float —
    /// including through the public entry point, with no `format:` set.
    #[test]
    fn widening_the_parse_does_not_move_a_whole_second_column() {
        for raw in ["0", "5", "90", "5400", "86399"] {
            let (text, _) = format_typed_value(raw, ColumnKind::Duration, None, "/");
            assert_eq!(
                text,
                format_duration(Duration::seconds(raw.parse().unwrap())),
                "whole seconds {raw} must render as the clock form"
            );
        }
    }

    #[test]
    fn duration_is_right_aligned() {
        let (_, align) = format_typed_value("5400", ColumnKind::Duration, None, "/");
        assert_eq!(align, CellAlignment::Right);
    }

    #[test]
    fn duration_non_numeric_passes_through() {
        assert_eq!(
            format_duration_secs("not-a-number", DurationFormat::Clock),
            "not-a-number"
        );
        assert_eq!(
            format_duration_secs("not-a-number", DurationFormat::Precise),
            "not-a-number"
        );
    }

    /// `format: precise` is selected by name, and any other name — including
    /// a typo the validator will have rejected at load — falls back to the
    /// clock form rather than to something invented.
    #[test]
    fn the_format_name_selects_the_duration_rendering() {
        assert_eq!(DurationFormat::parse(None), DurationFormat::Clock);
        assert_eq!(DurationFormat::parse(Some("clock")), DurationFormat::Clock);
        assert_eq!(
            DurationFormat::parse(Some("precise")),
            DurationFormat::Precise
        );
        assert_eq!(DurationFormat::parse(Some("presise")), DurationFormat::Clock);

        let (text, align) =
            format_typed_value("0.103424", ColumnKind::Duration, Some("precise"), "/");
        assert_eq!(text, "103ms");
        assert_eq!(align, CellAlignment::Right);
    }

    /// The ladder, rung by rung. The values are the ones `systemd-analyze
    /// blame` printed on a live user manager, and the expectations are what
    /// it printed beside them — three significant digits below a minute, two
    /// whole units above it.
    #[test]
    fn a_precise_span_names_the_unit_its_magnitude_asks_for() {
        let p = format_span_precise;
        assert_eq!(p(0.000_031), "31.0us");
        assert_eq!(p(0.000_677), "677us");
        assert_eq!(p(0.015_335), "15.3ms");
        assert_eq!(p(0.103_424), "103ms");
        assert_eq!(p(5.242_346), "5.24s");
        assert_eq!(p(63.0), "1min 3s");
        assert_eq!(p(120.0), "2min");
        assert_eq!(p(5.0 * 3600.0 + 24.0 * 60.0), "5h 24min");
        assert_eq!(p(9.0 * 86400.0), "1w 2d");
    }

    /// A span that did not happen leaves the cell empty (every kind does);
    /// a span that took no measurable time is a real measurement of zero and
    /// must look different from it.
    #[test]
    fn a_precise_zero_is_a_zero_and_an_absent_span_is_empty() {
        assert_eq!(format_span_precise(0.0), "0");
        let (text, _) = format_typed_value("", ColumnKind::Duration, Some("precise"), "/");
        assert_eq!(text, "");
    }

    /// Rounding must not produce a number that belongs on the next rung up:
    /// 999.7 ms is "1.00s", never "1000ms", and the same at the microsecond
    /// boundary. Otherwise a column would show two spellings of one value.
    #[test]
    fn a_precise_span_never_rounds_onto_the_rung_above_it() {
        assert_eq!(format_span_precise(0.999_7), "1.00s");
        assert_eq!(format_span_precise(0.000_999_7), "1.00ms");
        assert_eq!(format_span_precise(0.999_4), "999ms");
        assert_eq!(format_span_precise(0.000_999_4), "999us");
        // The same boundary one rung up: 59.94 still has a tenth worth
        // showing, 59.99 would render as "60.0s" and so becomes a minute.
        assert_eq!(format_span_precise(59.94), "59.9s");
        assert_eq!(format_span_precise(59.99), "1min");
        assert_eq!(format_span_precise(60.0), "1min");
        // And the digit-band boundary inside a unit: never a fourth digit.
        assert_eq!(format_span_precise(9.997), "10.0s");
        assert_eq!(format_span_precise(0.099_97), "100ms");
    }

    /// A negative span is not something a duration column should ever be
    /// sent, but clamping it to zero would hide a clock that ran backwards.
    #[test]
    fn a_precise_span_keeps_its_sign() {
        assert_eq!(format_span_precise(-0.103_424), "-103ms");
        assert_eq!(format_span_precise(-63.0), "-1min 3s");
    }

    #[test]
    fn datetime_rfc3339_renders_with_default_pattern() {
        // Pin to a fixed UTC offset so the assertion is timezone-stable:
        // an instant given in local time renders back to that wall clock.
        let local_now = Local::now();
        let offset = local_now.offset().to_string();
        let raw = format!("2026-06-09T08:15:00{offset}");
        let (text, align) = format_typed_value(&raw, ColumnKind::Datetime, None, "/");
        assert_eq!(text, "2026-06-09 08:15");
        assert_eq!(align, CellAlignment::Left);
    }

    #[test]
    fn datetime_honors_custom_format() {
        let local_now = Local::now();
        let offset = local_now.offset().to_string();
        let raw = format!("2026-06-09T08:15:00{offset}");
        let (text, _) = format_typed_value(&raw, ColumnKind::Datetime, Some("%H:%M"), "/");
        assert_eq!(text, "08:15");
    }

    #[test]
    fn datetime_unparseable_passes_through() {
        assert_eq!(format_datetime("yesterday", None), "yesterday");
    }

    #[test]
    fn path_leads_with_separator_and_joins_segments() {
        assert_eq!(format_path("/a/b/c", "/"), "/a/b/c");
        assert_eq!(format_path("a/b/c", "/"), "/a/b/c");
    }

    #[test]
    fn path_uses_display_separator() {
        assert_eq!(format_path("/a/b/c", " › "), " › a › b › c");
    }

    #[test]
    fn path_root_is_bare_separator() {
        assert_eq!(format_path("/", "/"), "/");
    }

    #[test]
    fn path_is_left_aligned() {
        let (_, align) = format_typed_value("/a/b", ColumnKind::Path, None, "/");
        assert_eq!(align, CellAlignment::Left);
    }

    /// Build an RFC 3339 instant `secs` seconds before `now`, in `now`'s
    /// own offset, so the round-trip is timezone-stable.
    fn instant_before(now: DateTime<Local>, secs: i64) -> String {
        (now - Duration::seconds(secs)).to_rfc3339()
    }

    #[test]
    fn elapsed_renders_now_minus_field_as_duration() {
        let now = Local::now();
        // 1h 30m ago → "1:30:00", right-aligned.
        let (text, align) = format_elapsed_since(&instant_before(now, 5400), now);
        assert_eq!(text, "1:30:00");
        assert_eq!(align, CellAlignment::Right);
    }

    #[test]
    fn elapsed_empty_stays_empty() {
        let now = Local::now();
        assert_eq!(
            format_elapsed_since("", now),
            (String::new(), CellAlignment::Left)
        );
    }

    #[test]
    fn elapsed_unparseable_passes_through() {
        let now = Local::now();
        let (text, align) = format_elapsed_since("not-a-date", now);
        assert_eq!(text, "not-a-date");
        assert_eq!(align, CellAlignment::Left);
    }

    #[test]
    fn elapsed_future_instant_clamps_to_zero() {
        let now = Local::now();
        // 10s in the future → negative span → clamped to "00".
        let future = (now + Duration::seconds(10)).to_rfc3339();
        let (text, _) = format_elapsed_since(&future, now);
        assert_eq!(text, "00");
    }

    #[test]
    fn elapsed_is_deterministic_under_injected_now() {
        // A fixed instant and a fixed `now` 90s later → exactly "01:30",
        // independent of wall-clock time.
        let base = DateTime::parse_from_rfc3339("2026-06-09T08:00:00Z")
            .unwrap()
            .with_timezone(&Local);
        let now = base + Duration::seconds(90);
        let (text, _) = format_elapsed_since(&base.to_rfc3339(), now);
        assert_eq!(text, "01:30");
    }
}
