//! YAML-based query filter: parse a YAML document with a named query,
//! resolve natural-language dates, and produce a [`FilterExpr`].

use super::{FilterExpr, Operator};
use chrono::Local;

/// Parsed query file result.
#[derive(Debug)]
pub struct ParsedQuery {
    pub name: String,
    pub expr: FilterExpr,
    pub options: QueryOptions,
}

/// Query options parsed from the `options:` key.
#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
    /// Include all ancestor tasks of matching results.
    pub include_ancestors: bool,
}

/// Parse a query YAML document. Resolves natural-language dates in string
/// literals before deserializing the [`FilterExpr`].
pub fn parse(content: &str) -> Result<ParsedQuery, QueryError> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(content).map_err(|e| QueryError::Yaml(e.to_string()))?;

    let map = doc
        .as_mapping()
        .ok_or_else(|| QueryError::Yaml("Expected a YAML mapping at top level".into()))?;

    let name = map
        .get(&serde_yaml::Value::String("name".into()))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let query_value = map
        .get(&serde_yaml::Value::String("query".into()))
        .ok_or_else(|| QueryError::Field {
            message: "Missing 'query' key".into(),
        })?
        .clone();

    let resolved = resolve_dates(query_value);

    let expr: FilterExpr = serde_yaml::from_value(resolved).map_err(|e| QueryError::Field {
        message: format!("Invalid query: {e}"),
    })?;

    // Parse options.
    let options = if let Some(opts) = map.get(&serde_yaml::Value::String("options".into())) {
        let include_ancestors = opts
            .get("include_ancestors")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        QueryOptions { include_ancestors }
    } else {
        QueryOptions::default()
    };

    Ok(ParsedQuery {
        name,
        expr,
        options,
    })
}

/// Return the resolved YAML value after date resolution (for debugging).
pub fn resolve_and_dump(content: &str) -> Result<String, QueryError> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(content).map_err(|e| QueryError::Yaml(e.to_string()))?;

    let map = doc
        .as_mapping()
        .ok_or_else(|| QueryError::Yaml("Expected a YAML mapping at top level".into()))?;

    let query_value = map
        .get(&serde_yaml::Value::String("query".into()))
        .ok_or_else(|| QueryError::Field {
            message: "Missing 'query' key".into(),
        })?
        .clone();

    let resolved = resolve_dates(query_value);
    serde_yaml::to_string(&resolved).map_err(|e| QueryError::Yaml(e.to_string()))
}

// ---------------------------------------------------------------------------
// Date resolution
// ---------------------------------------------------------------------------

/// Walk the YAML value tree and try to resolve string literals that look
/// like natural-language dates into RFC 3339 strings.
///
/// Only touches strings that appear as the third element of a 3-element
/// sequence (i.e. the RHS of `[col, op, value]`), and only if they
/// successfully parse as a date.
///
/// Public because filter expressions are embedded in documents this crate
/// knows nothing about — an extended query's `local_filter:` needs the same
/// date resolution as a saved query's `query:`, and re-deriving the walk there
/// would let the two drift apart.
pub fn resolve_dates(value: serde_yaml::Value) -> serde_yaml::Value {
    use serde_yaml::Value;

    match value {
        // A leaf `[lhs, op, rhs]` — resolve only the rhs. Distinguished from a
        // 3-element `and`/`or` clause list (whose elements are themselves
        // sequences/maps, not an operator) by the middle element being a
        // recognized operator string; otherwise recurse into every element so
        // dates inside a 3-clause compound are still resolved.
        Value::Sequence(seq) if seq.len() == 3 && is_operator(&seq[1]) => {
            let mut new_seq: Vec<Value> = Vec::with_capacity(3);
            new_seq.push(resolve_dates(seq[0].clone()));
            new_seq.push(seq[1].clone());
            new_seq.push(resolve_rhs(seq[2].clone()));
            Value::Sequence(new_seq)
        }
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
        .is_some_and(|s| Operator::from_str(s).is_some())
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
                    if let Value::String(s) = v {
                        if let Some(resolved) = try_resolve_date(s) {
                            return Value::String(resolved);
                        }
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
/// Delegates the whole grammar to the shared [`natural_date`] resolver (period
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

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum QueryError {
    Yaml(String),
    Field { message: String },
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::Yaml(msg) => write!(f, "{msg}"),
            QueryError::Field { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for QueryError {}

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
    fn parse_simple_query() {
        let content = "name: my filter\nquery:\n  [deleted, =, false]\n";
        let result = parse(content).unwrap();
        assert_eq!(result.name, "my filter");
        assert!(matches!(result.expr, FilterExpr::Leaf(_)));
    }

    #[test]
    fn parse_and_query() {
        let content = "name:\nquery:\n  and:\n    - [deleted, =, false]\n    - [status, =, todo]\n";
        let result = parse(content).unwrap();
        assert!(matches!(result.expr, FilterExpr::And(_)));
    }

    #[test]
    fn parse_missing_query_key() {
        let content = "name: test\n";
        let err = parse(content).unwrap_err();
        assert!(err.to_string().contains("Missing 'query' key"));
    }

    #[test]
    fn resolves_dates_in_three_clause_compound() {
        // Regression: a 3-clause `and:` list must NOT be mistaken for a
        // `[lhs, op, rhs]` leaf — every clause's date must still resolve.
        let content = "query:\n  and:\n    - [deleted, =, false]\n    - [started_at, gte, 2024-06-15]\n    - [description, has, x]\n";
        let dumped = resolve_and_dump(content).unwrap();
        // The middle clause's date resolved to an RFC3339 timestamp ('T').
        assert!(dumped.contains("2024-06-1"));
        assert!(dumped.contains('T'), "date was not resolved: {dumped}");
        // And it still parses into an And of three leaves.
        let parsed = parse(content).unwrap();
        assert!(matches!(parsed.expr, FilterExpr::And(ref v) if v.len() == 3));
    }

    #[test]
    fn resolves_period_boundary_phrases() {
        // These are the phrases chrono-english can't do; the date-periods
        // pre-resolver must handle them (exact value is date-dependent, so we
        // assert only that they resolve to a valid RFC3339 instant).
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
}
