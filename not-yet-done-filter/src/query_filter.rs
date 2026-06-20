//! YAML-based query filter: parse a YAML document with a named query,
//! resolve natural-language dates, and produce a [`FilterExpr`].

use chrono::{DateTime, Local, TimeZone, Utc};
use chrono_english::{parse_date_string, Dialect};
use super::FilterExpr;

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

    let expr: FilterExpr = serde_yaml::from_value(resolved)
        .map_err(|e| QueryError::Field {
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

    Ok(ParsedQuery { name, expr, options })
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
fn resolve_dates(value: serde_yaml::Value) -> serde_yaml::Value {
    use serde_yaml::Value;

    match value {
        Value::Sequence(seq) => {
            if seq.len() == 3 {
                let mut new_seq: Vec<Value> = Vec::with_capacity(3);
                new_seq.push(resolve_dates(seq[0].clone()));
                new_seq.push(seq[1].clone());
                new_seq.push(resolve_rhs(seq[2].clone()));
                Value::Sequence(new_seq)
            } else {
                Value::Sequence(seq.into_iter().map(resolve_dates).collect())
            }
        }
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
pub fn try_resolve_date(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.contains('%') || trimmed.is_empty() {
        return None;
    }

    // Try chrono-english first.
    let now_local: DateTime<Local> = Local::now();
    if let Ok(dt) = parse_date_string(trimmed, now_local, Dialect::Us) {
        return Some(dt.with_timezone(&Utc).to_rfc3339());
    }

    // Try ISO datetime.
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.with_timezone(&Utc).to_rfc3339());
    }

    // Try date-only.
    if let Ok(nd) = trimmed.parse::<chrono::NaiveDate>() {
        let midnight = nd.and_hms_opt(0, 0, 0)?;
        let local_dt = Local.from_local_datetime(&midnight).earliest()?;
        return Some(local_dt.with_timezone(&Utc).to_rfc3339());
    }

    None
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
