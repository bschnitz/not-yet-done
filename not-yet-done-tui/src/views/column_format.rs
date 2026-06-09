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
//! - `text`     → returned verbatim, left-aligned (the default).
//!
//! A value that fails to parse for its kind is returned verbatim (and left
//! aligned) rather than blanked, so malformed data stays visible instead of
//! silently vanishing. An empty string stays empty for every kind.

use chrono::{DateTime, Duration, Local};

use not_yet_done_table::CellAlignment;

use crate::config::view_config::ColumnKind;
use crate::tabs::trackings_state::format_duration;

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
        // `elapsed` is time-derived: it needs `now` and reads a *different*
        // source field, so it is computed by the cell builder via
        // [`format_elapsed_since`], not here. This arm is only reached if a
        // view mislabels a plain column `elapsed` — render the raw verbatim,
        // right-aligned, rather than panicking.
        ColumnKind::Elapsed => (raw.to_string(), CellAlignment::Right),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_verbatim_left() {
        assert_eq!(
            format_typed_value("hello", ColumnKind::Text, None, "/"),
            ("hello".to_string(), CellAlignment::Left)
        );
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
