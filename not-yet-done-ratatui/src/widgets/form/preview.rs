//! Natural-language date/time preview, gated behind the `natural-date` feature.
//!
//! Kept separate from rendering so the resolver is unit-testable with an
//! injected `now`. The submitted field value is always the raw phrase; this only
//! feeds the preview line.

/// Resolves a natural-language phrase for the preview line, formatted in `now`'s
/// timezone. Returns `None` when the phrase is empty or unrecognised.
///
/// - `with_time == true`  → `%Y-%m-%d %H:%M` via [`natural_date::resolve_datetime`].
/// - `with_time == false` → `%Y-%m-%d`       via [`natural_date::resolve_date`].
pub fn datetime_preview<Tz>(
    value: &str,
    with_time: bool,
    now: chrono::DateTime<Tz>,
) -> Option<String>
where
    Tz: chrono::TimeZone,
    Tz::Offset: Copy + std::fmt::Display,
{
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if with_time {
        let tz = now.timezone();
        natural_date::resolve_datetime(v, now)
            .map(|d| d.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string())
    } else {
        natural_date::resolve_date(v, now).map(|d| d.format("%Y-%m-%d").to_string())
    }
}
