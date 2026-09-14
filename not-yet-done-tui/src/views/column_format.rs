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
//! - `duration` → integer **seconds** (`"5400"`); rendered with the shared
//!   [`format_duration`] (`H:MM:SS`) so the adapterized view matches the
//!   legacy Trackings rendering exactly, right-aligned.
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
        ColumnKind::Duration => (format_duration_secs(raw), CellAlignment::Right),
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

/// Parse integer seconds and render them via the shared [`format_duration`].
/// Non-integer input is passed through unchanged.
fn format_duration_secs(raw: &str) -> String {
    match raw.trim().parse::<i64>() {
        Ok(secs) => format_duration(Duration::seconds(secs)),
        Err(_) => raw.to_string(),
    }
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
        assert_eq!(format_duration_secs("5400"), "1:30:00");
        assert_eq!(format_duration_secs("90"), "01:30");
        assert_eq!(format_duration_secs("5"), "05");
    }

    #[test]
    fn duration_is_right_aligned() {
        let (_, align) = format_typed_value("5400", ColumnKind::Duration, None, "/");
        assert_eq!(align, CellAlignment::Right);
    }

    #[test]
    fn duration_non_integer_passes_through() {
        assert_eq!(format_duration_secs("not-a-number"), "not-a-number");
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
