//! The saved-query document: a named query plus its options.
//!
//! ```yaml
//! name: overdue
//! query:
//!   and:
//!     - [deleted, =, false]
//!     - [due, '<', today]
//! options:
//!   include_ancestors: true
//! ```
//!
//! The `query:` value is a [`FilterExpr`] in the shared language; everything
//! around it is this application's. Dates are resolved before the expression is
//! deserialized, and the tree predicates are checked against the ones this
//! application implements, so a misspelt `in_tre` is a load error rather than a
//! branch that quietly matches nothing.

use crate::TREE_PREDICATES;
use rowsieve::FilterExpr;
use rowsieve::dates::resolve_dates;

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

    expr.validate_custom(TREE_PREDICATES)
        .map_err(|e| QueryError::Field {
            message: e.to_string(),
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
        let _ = &parsed;
        assert!(matches!(parsed.expr, FilterExpr::And(ref v) if v.len() == 3));
    }
}
