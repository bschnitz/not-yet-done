//! Extract date bounds from a [`FilterExpr`] by walking the tree.
//!
//! Examines comparisons on date columns (`started_at`, `ended_at`, `created_at`)
//! and derives the tightest possible `(min, max)` range.
//!
//! - `And`: intersect children (tightest bounds)
//! - `Or`: union children (widest bounds)
//! - `Not`: invert the inner bounds (lower→upper, upper→lower)
//! - `Leaf`: extract bound from comparison operator + literal

use chrono::{DateTime, Utc};

use super::expr::{FilterExpr, FilterLeaf, Literal, Operator, Rhs};

/// Extracted date range. `None` means unbounded in that direction.
#[derive(Debug, Clone, Default)]
pub struct DateBounds {
    pub min: Option<DateTime<Utc>>,
    pub max: Option<DateTime<Utc>>,
}

impl DateBounds {
    fn unbounded() -> Self {
        Self { min: None, max: None }
    }

    /// Intersect: take the tighter of two bounds.
    fn intersect(self, other: Self) -> Self {
        Self {
            min: match (self.min, other.min) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
            max: match (self.max, other.max) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
        }
    }

    /// Union: take the wider of two bounds.
    fn union(self, other: Self) -> Self {
        Self {
            min: match (self.min, other.min) {
                (Some(a), Some(b)) => Some(a.min(b)),
                _ => None, // If either is unbounded, union is unbounded.
            },
            max: match (self.max, other.max) {
                (Some(a), Some(b)) => Some(a.max(b)),
                _ => None,
            },
        }
    }

    /// Invert: a lower bound becomes an upper bound and vice versa.
    fn invert(self) -> Self {
        Self {
            min: self.max,
            max: self.min,
        }
    }
}

/// Date-relevant column names.
const DATE_COLUMNS: &[&str] = &["started_at", "ended_at", "created_at"];

fn is_date_column(col: &str) -> bool {
    DATE_COLUMNS.contains(&col)
}

/// Try to parse a literal as a UTC datetime.
fn literal_to_datetime(lit: &Literal) -> Option<DateTime<Utc>> {
    match lit {
        Literal::String(s) => {
            // Try RFC 3339 / ISO 8601.
            DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
                .or_else(|| {
                    // Try common formats.
                    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
                        .map(|ndt| ndt.and_utc())
                        .ok()
                })
                .or_else(|| {
                    // Date only → start of day.
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                        .map(|nd| nd.and_hms_opt(0, 0, 0).unwrap().and_utc())
                        .ok()
                })
        }
        _ => None,
    }
}

/// Extract date bounds from a filter expression.
pub fn extract_date_bounds(expr: &FilterExpr) -> DateBounds {
    match expr {
        FilterExpr::And(children) => {
            let mut result = DateBounds::unbounded();
            for child in children {
                result = result.intersect(extract_date_bounds(child));
            }
            result
        }
        FilterExpr::Or(children) => {
            if children.is_empty() {
                return DateBounds::unbounded();
            }
            let mut result = extract_date_bounds(&children[0]);
            for child in &children[1..] {
                result = result.union(extract_date_bounds(child));
            }
            result
        }
        FilterExpr::Not(inner) => {
            extract_date_bounds(inner).invert()
        }
        FilterExpr::Leaf(leaf) => extract_leaf_bounds(leaf),
    }
}

fn extract_leaf_bounds(leaf: &FilterLeaf) -> DateBounds {
    if !is_date_column(&leaf.lhs.column) {
        return DateBounds::unbounded();
    }

    let dt = match &leaf.rhs {
        Rhs::Lit(lit) => literal_to_datetime(lit),
        _ => None,
    };

    let Some(dt) = dt else {
        return DateBounds::unbounded();
    };

    match leaf.op {
        // started_at = X → min=X, max=X
        Operator::Eq => DateBounds { min: Some(dt), max: Some(dt) },
        // started_at > X or >= X → min=X
        Operator::Gt | Operator::Gte => DateBounds { min: Some(dt), max: None },
        // started_at < X or <= X → max=X
        Operator::Lt | Operator::Lte => DateBounds { min: None, max: Some(dt) },
        // Other operators don't constrain dates meaningfully.
        _ => DateBounds::unbounded(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::*;

    #[test]
    fn and_with_range() {
        let expr = FilterExpr::And(vec![
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("started_at"),
                op: Operator::Gte,
                rhs: Rhs::Lit(Literal::String("2026-04-01T00:00:00Z".into())),
            }),
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("started_at"),
                op: Operator::Lte,
                rhs: Rhs::Lit(Literal::String("2026-04-30T23:59:59Z".into())),
            }),
        ]);
        let bounds = extract_date_bounds(&expr);
        assert!(bounds.min.is_some());
        assert!(bounds.max.is_some());
        assert_eq!(bounds.min.unwrap().date_naive().to_string(), "2026-04-01");
        assert_eq!(bounds.max.unwrap().date_naive().to_string(), "2026-04-30");
    }

    #[test]
    fn non_date_column_is_unbounded() {
        let expr = FilterExpr::Leaf(FilterLeaf {
            lhs: ColRef::unqualified("description"),
            op: Operator::Has,
            rhs: Rhs::Lit(Literal::String("test".into())),
        });
        let bounds = extract_date_bounds(&expr);
        assert!(bounds.min.is_none());
        assert!(bounds.max.is_none());
    }

    #[test]
    fn or_widens() {
        let expr = FilterExpr::Or(vec![
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("started_at"),
                op: Operator::Gte,
                rhs: Rhs::Lit(Literal::String("2026-04-10T00:00:00Z".into())),
            }),
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("started_at"),
                op: Operator::Gte,
                rhs: Rhs::Lit(Literal::String("2026-04-01T00:00:00Z".into())),
            }),
        ]);
        let bounds = extract_date_bounds(&expr);
        // Union of >=Apr10 and >=Apr01 → min=Apr01
        assert_eq!(bounds.min.unwrap().date_naive().to_string(), "2026-04-01");
    }

    #[test]
    fn not_inverts() {
        let expr = FilterExpr::Not(Box::new(FilterExpr::Leaf(FilterLeaf {
            lhs: ColRef::unqualified("started_at"),
            op: Operator::Gte,
            rhs: Rhs::Lit(Literal::String("2026-04-15T00:00:00Z".into())),
        })));
        let bounds = extract_date_bounds(&expr);
        // Not(>= Apr15) → < Apr15 → max=Apr15
        assert!(bounds.min.is_none());
        assert_eq!(bounds.max.unwrap().date_naive().to_string(), "2026-04-15");
    }
}
