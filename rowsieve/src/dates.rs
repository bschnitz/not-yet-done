//! Resolving natural-language dates on the right-hand side of a comparison.
//!
//! `[due, '<', 'end of next week']` is a filter people can write; `[due, '<',
//! '2026-09-04T23:59:59+02:00']` is one an evaluator can answer. This module is
//! the step between: a pass over the YAML *before* it is deserialized, which
//! replaces right-hand strings that resolve as dates with RFC 3339 timestamps
//! and leaves everything else alone.
//!
//! It runs before parsing rather than after because the language has no date
//! type — a resolved date is simply a string literal that [`crate::eval`] then
//! compares as an instant. Doing it here also means a host that embeds a filter
//! inside a larger document (a saved query, a highlight rule) gets the same
//! resolution in every one of them without re-deriving the walk.
//!
//! Enabled by the `natural-dates` feature; the language itself has no opinion
//! on how a date is written.

use crate::Operator;
use chrono::Local;

/// Walk the YAML value tree and try to resolve string literals that look
/// like natural-language dates into RFC 3339 strings.
///
/// Only touches strings that appear as the third element of a 3-element
/// sequence (i.e. the RHS of `[col, op, value]`), and only if they
/// successfully parse as a date.
///
pub fn resolve_dates(value: serde_yaml::Value) -> serde_yaml::Value {
    use serde_yaml::Value;

    match value {
        // A leaf `[lhs, op, rhs]` — resolve only the rhs. Distinguished from a
        // 3-element `and`/`or` clause list (whose elements are themselves
        // sequences/maps, not an operator) by the middle element being a
        // recognized operator string; otherwise recurse into every element so
        // dates inside a 3-clause compound are still resolved.
        Value::Sequence(seq) if seq.len() == 3 && is_operator(&seq[1]) => Value::Sequence(vec![
            resolve_dates(seq[0].clone()),
            seq[1].clone(),
            resolve_rhs(seq[2].clone()),
        ]),
        Value::Sequence(seq) => Value::Sequence(seq.into_iter().map(resolve_dates).collect()),
        Value::Mapping(map) => {
            let new_map = map
                .into_iter()
                .map(|(k, v)| (k, resolve_dates(v)))
                .collect();
            Value::Mapping(new_map)
        }
        other => other,
    }
}

/// Whether a YAML value is a scalar string naming a filter operator — the
/// marker that a 3-element sequence is a leaf and not a compound clause list.
fn is_operator(value: &serde_yaml::Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| s.parse::<Operator>().is_ok())
}

fn resolve_rhs(value: serde_yaml::Value) -> serde_yaml::Value {
    use serde_yaml::Value;

    match &value {
        Value::String(s) => {
            if let Some(resolved) = try_resolve_date(s) {
                Value::String(resolved)
            } else {
                value
            }
        }
        Value::Sequence(seq) => Value::Sequence(
            seq.iter()
                .map(|v| {
                    if let Value::String(s) = v
                        && let Some(resolved) = try_resolve_date(s)
                    {
                        return Value::String(resolved);
                    }
                    v.clone()
                })
                .collect(),
        ),
        _ => value,
    }
}

/// Try to parse a string as a natural-language or ISO date.
/// Returns `Some(rfc3339)` on success, `None` if it's not a date.
///
/// Delegates the whole grammar to the [`natural_date`] resolver (period
/// boundaries, `in X`, part-of-day, abbreviations, chrono-english, ISO, …).
///
/// Two guards keep it from resolving things that are not dates:
///
/// - `%` — an SQL `LIKE` pattern is not a date.
/// - a bare number — the resolver happily reads `5` as a year in antiquity and
///   `5.5` as the time 03:05, so a quoted number on the right-hand side of a
///   comparison used to turn into a nonsense timestamp instead of staying the
///   number the user wrote. Anyone who does mean a date writes enough for it to
///   be recognisable as one.
pub fn try_resolve_date(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.contains('%') || trimmed.is_empty() || trimmed.parse::<f64>().is_ok() {
        return None;
    }
    natural_date::resolve_datetime(trimmed, Local::now()).map(|dt| dt.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    #[test]
    fn a_quoted_number_stays_a_number() {
        // The resolver reads `5` as a year in antiquity and `5.5` as a clock
        // time, so without the numeric guard `[prio, '>', '5']` would compare
        // against a timestamp and match nothing.
        for n in ["5", "0", "42", "5.5", "-3", " 7 "] {
            assert_eq!(try_resolve_date(n), None, "input {n:?}");
        }
        // Something that only starts with digits is still a date.
        assert!(try_resolve_date("2026-07-20").is_some());
    }

    #[test]
    fn resolves_period_boundary_phrases() {
        // These are the phrases chrono-english cannot do; the date-periods
        // pre-resolver must handle them (the exact value is date-dependent, so
        // only that they resolve to a valid RFC 3339 instant is asserted).
        for p in ["end of next week", "start of month", "end of quarter"] {
            let r = try_resolve_date(p).unwrap_or_else(|| panic!("{p} should resolve"));
            assert!(DateTime::parse_from_rfc3339(&r).is_ok(), "{p} -> {r}");
        }
    }

    #[test]
    fn resolve_iso_date() {
        let resolved = try_resolve_date("2024-06-15");
        assert!(resolved.is_some());
        assert!(resolved.unwrap().contains("2024-06-1"));
    }

    #[test]
    fn no_resolve_sql_wildcard() {
        assert!(try_resolve_date("%search%").is_none());
    }

    #[test]
    fn no_resolve_empty() {
        assert!(try_resolve_date("").is_none());
    }

    #[test]
    fn a_three_clause_compound_is_not_mistaken_for_a_leaf() {
        // Regression: `and:` with exactly three clauses has the shape of a
        // leaf, and only the middle element tells them apart. Every clause's
        // date must still resolve.
        let yaml = "and:\n  - [deleted, =, false]\n  - [started_at, gte, 2024-06-15]\n  - [description, has, x]\n";
        let resolved = resolve_dates(serde_yaml::from_str(yaml).unwrap());
        let dumped = serde_yaml::to_string(&resolved).unwrap();
        assert!(dumped.contains('T'), "date was not resolved: {dumped}");
    }

    #[test]
    fn a_left_hand_side_is_never_read_as_a_date() {
        // `[created_at, '>', ...]`: the field name is a word the resolver could
        // plausibly read, and turning it into a timestamp would compare two
        // constants.
        let yaml = "[today, '>', 2024-06-15]";
        let resolved = resolve_dates(serde_yaml::from_str(yaml).unwrap());
        let dumped = serde_yaml::to_string(&resolved).unwrap();
        assert!(dumped.contains("today"), "lhs was resolved away: {dumped}");
    }
}
