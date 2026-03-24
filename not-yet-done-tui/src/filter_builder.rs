// not-yet-done-tui/src/filter_builder.rs

//! Converts the TUI [`FilterState`] into a [`FilterExpr`] that can be passed
//! to [`TaskService::list_filtered`].
//!
//! Date fields accept both ISO-8601 strings and natural-language English
//! expressions supported by `chrono-english` (e.g. "last monday",
//! "two weeks ago", "yesterday").

use chrono::{DateTime, Local, TimeZone, Utc};
use chrono_english::{parse_date_string, Dialect};

use not_yet_done_core::filter::{ColRef, FilterExpr, FilterLeaf, Literal, Operator, Rhs};

use crate::tabs::FilterState;

/// Parse error returned when a date field cannot be understood.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub field: &'static str,
    pub message: String,
}

/// Result of building a FilterExpr from FilterState.
pub struct BuildResult {
    pub expr: FilterExpr,
    /// Any non-fatal parse errors (field-level feedback shown in the UI).
    pub errors: Vec<ParseError>,
}

/// Build a [`FilterExpr`] (AND of all active conditions) from user inputs.
///
/// On date parse failure the field is silently skipped and the error is
/// returned alongside the expression so the UI can display inline feedback.
pub fn build(state: &FilterState) -> BuildResult {
    let mut leaves: Vec<FilterExpr> = Vec::new();
    let mut errors: Vec<ParseError> = Vec::new();

    // ── Created after ────────────────────────────────────────────────────
    if !state.created_after_raw.trim().is_empty() {
        match parse_date(&state.created_after_raw) {
            Ok(dt) => leaves.push(leaf(
                "created_at",
                Operator::Gte,
                Literal::String(dt.to_rfc3339()),
            )),
            Err(e) => errors.push(ParseError {
                field: "Created after",
                message: e,
            }),
        }
    }

    // ── Created before ────────────────────────────────────────────────────
    if !state.created_before_raw.trim().is_empty() {
        match parse_date(&state.created_before_raw) {
            Ok(dt) => leaves.push(leaf(
                "created_at",
                Operator::Lte,
                Literal::String(dt.to_rfc3339()),
            )),
            Err(e) => errors.push(ParseError {
                field: "Created before",
                message: e,
            }),
        }
    }

    // ── Description LIKE ──────────────────────────────────────────────────
    if !state.description_like.trim().is_empty() {
        let pattern = format!("%{}%", state.description_like.trim());
        leaves.push(leaf(
            "description",
            Operator::Like,
            Literal::String(pattern),
        ));
    }

    // ── Status (multi-select) ─────────────────────────────────────────────
    if !state.status.is_empty() {
        let mut selected: Vec<Literal> = Vec::new();
        if state.status.todo {
            selected.push(Literal::String("todo".into()));
        }
        if state.status.in_progress {
            selected.push(Literal::String("in_progress".into()));
        }
        if state.status.done {
            selected.push(Literal::String("done".into()));
        }
        if state.status.cancelled {
            selected.push(Literal::String("cancelled".into()));
        }
        leaves.push(leaf("status", Operator::In, Literal::List(selected)));
    }

    // ── Priority min ──────────────────────────────────────────────────────
    if !state.priority_min_raw.trim().is_empty() {
        match state.priority_min_raw.trim().parse::<i64>() {
            Ok(p) => leaves.push(leaf("priority", Operator::Gte, Literal::Int(p))),
            Err(_) => errors.push(ParseError {
                field: "Priority ≥",
                message: "must be an integer".to_string(),
            }),
        }
    }

    // ── Deleted ───────────────────────────────────────────────────────────
    // Default (show_deleted = false): only non-deleted tasks
    // show_deleted = true: include all (no deleted filter at all)
    if !state.show_deleted {
        leaves.push(leaf("deleted", Operator::Eq, Literal::Bool(false)));
    }

    // ── Combine with AND ──────────────────────────────────────────────────
    let expr = if leaves.is_empty() {
        // No active filters: return everything (deleted=false implicit)
        leaf("deleted", Operator::Eq, Literal::Bool(false))
    } else if leaves.len() == 1 {
        leaves.remove(0)
    } else {
        FilterExpr::And(leaves)
    };

    BuildResult { expr, errors }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn leaf(column: &str, op: Operator, lit: Literal) -> FilterExpr {
    FilterExpr::Leaf(FilterLeaf {
        lhs: ColRef::unqualified(column),
        op,
        rhs: Rhs::Lit(lit),
    })
}

/// Parse a date string into a UTC [`DateTime`].
///
/// Tries in order:
/// 1. `chrono-english` natural language ("last monday", "2 weeks ago")
/// 2. ISO-8601 date-time with timezone
/// 3. ISO-8601 date only (midnight local time)
fn parse_date(raw: &str) -> Result<DateTime<Utc>, String> {
    let trimmed = raw.trim();

    // chrono-english — use the current local time as "now"
    let now_local: DateTime<Local> = Local::now();
    if let Ok(dt) = parse_date_string(trimmed, now_local, Dialect::Us) {
        // chrono-english returns local DateTime — convert to UTC
        return Ok(dt.with_timezone(&Utc));
    }

    // Try full ISO datetime
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = DateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%z") {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try date-only "YYYY-MM-DD" → midnight local time
    if let Ok(nd) = trimmed.parse::<chrono::NaiveDate>() {
        let midnight = nd.and_hms_opt(0, 0, 0).expect("midnight is always valid");
        if let Some(local_dt) = Local.from_local_datetime(&midnight).earliest() {
            return Ok(local_dt.with_timezone(&Utc));
        }
    }

    Err(format!(
        "Cannot parse '{trimmed}'. Try: 'last monday', '2 weeks ago', or 'YYYY-MM-DD'."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::{FilterState, StatusFilter};

    #[test]
    fn empty_state_produces_deleted_false_filter() {
        let state = FilterState::default();
        let result = build(&state);
        assert!(result.errors.is_empty());
        // Should be a single leaf: deleted = false
        assert!(matches!(result.expr, FilterExpr::Leaf(_)));
    }

    #[test]
    fn description_filter_wraps_with_wildcards() {
        let mut state = FilterState::default();
        state.description_like = "foo".to_string();
        let result = build(&state);
        assert!(result.errors.is_empty());
        // Should be AND of two leaves: description LIKE %foo% AND deleted = false
        assert!(matches!(result.expr, FilterExpr::And(_)));
    }

    #[test]
    fn invalid_priority_produces_error() {
        let mut state = FilterState::default();
        state.priority_min_raw = "notanumber".to_string();
        let result = build(&state);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].field, "Priority ≥");
    }

    #[test]
    fn iso_date_parses() {
        let result = parse_date("2024-01-15");
        assert!(result.is_ok());
    }

    #[test]
    fn invalid_date_returns_error() {
        let result = parse_date("not a date at all !!");
        assert!(result.is_err());
    }
}
