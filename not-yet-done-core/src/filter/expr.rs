//! Core data structures for the filter DSL.
//!
//! # Column reference syntax
//!
//! Inside a YAML leaf `[lhs, op, rhs]`, strings are interpreted as follows:
//!
//! | String form         | Meaning                        | Example              |
//! |---------------------|--------------------------------|----------------------|
//! | `.column`           | Unqualified column reference   | `.description`       |
//! | `alias.column`      | Qualified column reference     | `task.description`   |
//! | `anything else`     | String literal                 | `%ustav%`            |
//!
//! The left-hand side of a leaf is **always** parsed as a column reference
//! (the leading `.` is optional there — a bare `description` on the lhs is
//! unambiguous).  The right-hand side is parsed as a column reference only
//! when it contains a dot.
//!
//! # Examples
//!
//! ```yaml
//! # Simple value comparison (lhs = bare column name, rhs = literal)
//! [description, like, '%ustav%']
//!
//! # Unqualified column-vs-column
//! [.updated_at, '>', .created_at]
//!
//! # Future: qualified with alias (for joins)
//! [task.updated_at, '>', task.created_at]
//!
//! # Compound
//! and:
//!   - [priority, '>=', 3]
//!   - or:
//!     - [.description, like, '%foo%']
//!     - [status, =, done]
//! ```

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A recursive filter expression that can be deserialized from YAML/JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpr {
    /// All child expressions must be true.
    And(Vec<FilterExpr>),
    /// At least one child expression must be true.
    Or(Vec<FilterExpr>),
    /// The child expression must be false.
    Not(Box<FilterExpr>),
    /// A leaf condition: `lhs op rhs`.
    Leaf(FilterLeaf),
}

/// A single comparison: `lhs op rhs`.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterLeaf {
    pub lhs: ColRef,
    pub op: Operator,
    pub rhs: Rhs,
}

/// The right-hand side of a leaf condition.
#[derive(Debug, Clone, PartialEq)]
pub enum Rhs {
    /// A column reference (`alias.column` or `.column`).
    Col(ColRef),
    /// A literal value.
    Lit(Literal),
    /// No rhs — for `is_null` / `is_not_null`.
    None,
}

/// A column reference, optionally qualified with a table alias.
///
/// | YAML          | `table`         | `column`       |
/// |---------------|-----------------|----------------|
/// | `.name`       | `None`          | `"name"`       |
/// | `task.name`   | `Some("task")`  | `"name"`       |
/// | `name` (lhs)  | `None`          | `"name"`       |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColRef {
    /// Table alias, if any.  `None` means "current / only table".
    pub table: Option<String>,
    /// The column name, matching the entity field name exactly.
    pub column: String,
}

impl ColRef {
    pub fn unqualified(column: impl Into<String>) -> Self {
        Self { table: None, column: column.into() }
    }

    pub fn qualified(table: impl Into<String>, column: impl Into<String>) -> Self {
        Self { table: Some(table.into()), column: column.into() }
    }
}

/// A literal scalar or list value.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// Used for `in` / `not_in`.
    List(Vec<Literal>),
}

/// Comparison operators supported in filter leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    NotLike,
    IsNull,
    IsNotNull,
    In,
    NotIn,
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

impl Operator {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "=" | "==" => Some(Self::Eq),
            "!=" | "<>" => Some(Self::Ne),
            ">" => Some(Self::Gt),
            ">=" => Some(Self::Gte),
            "<" => Some(Self::Lt),
            "<=" => Some(Self::Lte),
            "like" | "LIKE" => Some(Self::Like),
            "not_like" | "NOT LIKE" => Some(Self::NotLike),
            "is_null" | "IS NULL" => Some(Self::IsNull),
            "is_not_null" | "IS NOT NULL" => Some(Self::IsNotNull),
            "in" | "IN" => Some(Self::In),
            "not_in" | "NOT IN" => Some(Self::NotIn),
            _ => None,
        }
    }

    /// Whether this operator requires a right-hand side.
    pub fn needs_rhs(&self) -> bool {
        !matches!(self, Self::IsNull | Self::IsNotNull)
    }
}

/// Parse a string as a column reference.
///
/// - `.column`       → unqualified ColRef
/// - `alias.column`  → qualified ColRef  
/// - bare string     → unqualified ColRef (only valid in lhs position)
fn parse_col_ref(s: &str) -> ColRef {
    if let Some(col) = s.strip_prefix('.') {
        // .column → unqualified
        return ColRef::unqualified(col);
    }
    if let Some(dot) = s.find('.') {
        // alias.column → qualified
        let (table, rest) = s.split_at(dot);
        return ColRef::qualified(table, &rest[1..]);
    }
    // bare string — caller decides if this is valid as lhs
    ColRef::unqualified(s)
}

/// Determine whether a rhs string is a column reference.
///
/// A rhs string is a column reference iff it starts with `.` or contains `.`.
fn is_col_ref(s: &str) -> bool {
    s.starts_with('.') || s.contains('.')
}

/// Parse a YAML scalar value as a [`Literal`].
fn parse_literal_str(s: &str) -> Literal {
    if let Ok(i) = s.parse::<i64>() { return Literal::Int(i); }
    if let Ok(f) = s.parse::<f64>() { return Literal::Float(f); }
    if s == "true"  { return Literal::Bool(true); }
    if s == "false" { return Literal::Bool(false); }
    Literal::String(s.to_string())
}

/// Convert a `serde_yaml::Value` into a [`Literal`] or [`Rhs::Col`].
fn yaml_value_to_rhs(v: &serde_yaml::Value) -> Result<Rhs, String> {
    match v {
        serde_yaml::Value::String(s) => {
            if is_col_ref(s) {
                Ok(Rhs::Col(parse_col_ref(s)))
            } else {
                Ok(Rhs::Lit(parse_literal_str(s)))
            }
        }
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() { return Ok(Rhs::Lit(Literal::Int(i))); }
            if let Some(f) = n.as_f64() { return Ok(Rhs::Lit(Literal::Float(f))); }
            Err(format!("unrepresentable number: {n}"))
        }
        serde_yaml::Value::Bool(b) => Ok(Rhs::Lit(Literal::Bool(*b))),
        serde_yaml::Value::Sequence(seq) => {
            let items: Result<Vec<_>, _> = seq
                .iter()
                .map(|item| match yaml_value_to_rhs(item)? {
                    Rhs::Lit(l) => Ok(l),
                    Rhs::Col(_) => Err("column references inside lists are not supported".to_string()),
                    Rhs::None => unreachable!(),
                })
                .collect();
            Ok(Rhs::Lit(Literal::List(items?)))
        }
        other => Err(format!("unsupported rhs value: {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// Deserialization
// ---------------------------------------------------------------------------

impl<'de> Deserialize<'de> for FilterExpr {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        de.deserialize_any(FilterExprVisitor)
    }
}

struct FilterExprVisitor;

impl<'de> Visitor<'de> for FilterExprVisitor {
    type Value = FilterExpr;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "a filter expression: map with 'and'/'or'/'not', or a leaf array [lhs, op, rhs]"
        )
    }

    /// Map form: `{ and: [...] }` | `{ or: [...] }` | `{ not: ... }`
    fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let key: String = map
            .next_key()?
            .ok_or_else(|| de::Error::custom("empty map in filter expression"))?;

        let expr = match key.as_str() {
            "and" => FilterExpr::And(map.next_value::<Vec<FilterExpr>>()?),
            "or"  => FilterExpr::Or(map.next_value::<Vec<FilterExpr>>()?),
            "not" => FilterExpr::Not(Box::new(map.next_value::<FilterExpr>()?)),
            other => return Err(de::Error::unknown_field(other, &["and", "or", "not"])),
        };

        Ok(expr)
    }

    /// Array form: `[lhs, op, rhs]` or `[lhs, op]` for null-checks.
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        // lhs — always a column reference (bare name is unambiguous here)
        let lhs_raw: String = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &"2 or 3 elements"))?;
        let lhs = parse_col_ref(&lhs_raw);

        // op
        let op_raw: String = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(1, &"2 or 3 elements"))?;
        let op = Operator::from_str(&op_raw)
            .ok_or_else(|| de::Error::custom(format!("unknown operator '{op_raw}'")))?;

        // rhs — absent for null-check operators
        let rhs = if op.needs_rhs() {
            let val: serde_yaml::Value = seq
                .next_element()?
                .ok_or_else(|| de::Error::invalid_length(2, &"3 elements"))?;
            yaml_value_to_rhs(&val).map_err(de::Error::custom)?
        } else {
            Rhs::None
        };

        Ok(FilterExpr::Leaf(FilterLeaf { lhs, op, rhs }))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> FilterExpr {
        serde_yaml::from_str(yaml).expect("parse failed")
    }

    #[test]
    fn test_bare_lhs_string_rhs() {
        // Most common form: bare column name on lhs, string literal on rhs
        let expr = parse("[description, like, '%ustav%']");
        assert_eq!(
            expr,
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("description"),
                op: Operator::Like,
                rhs: Rhs::Lit(Literal::String("%ustav%".into())),
            })
        );
    }

    #[test]
    fn test_dotted_col_vs_col() {
        // Unqualified column-vs-column using .col syntax
        let expr = parse("[.updated_at, '>', .created_at]");
        assert_eq!(
            expr,
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("updated_at"),
                op: Operator::Gt,
                rhs: Rhs::Col(ColRef::unqualified("created_at")),
            })
        );
    }

    #[test]
    fn test_qualified_col_ref() {
        // Future join syntax: alias.column
        let expr = parse("[task.updated_at, '>', task.created_at]");
        assert_eq!(
            expr,
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::qualified("task", "updated_at"),
                op: Operator::Gt,
                rhs: Rhs::Col(ColRef::qualified("task", "created_at")),
            })
        );
    }

    #[test]
    fn test_integer_rhs() {
        let expr = parse("[priority, '>=', 3]");
        assert_eq!(
            expr,
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("priority"),
                op: Operator::Gte,
                rhs: Rhs::Lit(Literal::Int(3)),
            })
        );
    }

    #[test]
    fn test_bool_rhs() {
        let expr = parse("[deleted, =, false]");
        assert_eq!(
            expr,
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("deleted"),
                op: Operator::Eq,
                rhs: Rhs::Lit(Literal::Bool(false)),
            })
        );
    }

    #[test]
    fn test_is_null() {
        let expr = parse("[parent_id, is_null]");
        assert_eq!(
            expr,
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("parent_id"),
                op: Operator::IsNull,
                rhs: Rhs::None,
            })
        );
    }

    #[test]
    fn test_in_list() {
        let expr = parse("[status, in, [todo, in_progress]]");
        assert!(matches!(
            expr,
            FilterExpr::Leaf(FilterLeaf {
                op: Operator::In,
                rhs: Rhs::Lit(Literal::List(_)),
                ..
            })
        ));
    }

    #[test]
    fn test_and_compound() {
        let yaml = "
and:
  - [priority, '>=', 3]
  - [deleted, =, false]
";
        let expr = parse(yaml);
        assert!(matches!(expr, FilterExpr::And(ref v) if v.len() == 2));
    }

    #[test]
    fn test_nested_and_or() {
        let yaml = "
and:
  - [.updated_at, '>', .created_at]
  - or:
    - [description, like, '%ustav%']
    - [priority, =, 5]
";
        let expr = parse(yaml);
        let FilterExpr::And(children) = expr else { panic!("expected And") };
        assert_eq!(children.len(), 2);
        assert!(matches!(children[1], FilterExpr::Or(_)));
    }

    #[test]
    fn test_not() {
        let yaml = "
not:
  [deleted, =, true]
";
        let expr = parse(yaml);
        assert!(matches!(expr, FilterExpr::Not(_)));
    }
}
