//! Evaluating a [`FilterExpr`] against rows that are already in memory.
//!
//! The database half of the filter system turns an expression into a SeaORM
//! `Condition` and lets SQL do the work. That is not available whenever the
//! rows arrived from somewhere else — a calendar's merged event list, an
//! adapter's `list()` result that an extended query then narrows locally. This
//! module is the other half: the same DSL, evaluated row by row.
//!
//! A caller supplies rows through [`RowFields`], which answers "what is the
//! value of column X for this row, and what type is it". Everything else —
//! operator semantics, null handling, `LIKE` wildcards — lives here, so the
//! two in-memory consumers cannot drift apart in what a query means.
//!
//! # Semantics
//!
//! - **Text compares case-insensitively.** These filters are a search feature,
//!   not an exact-match store.
//! - **Any comparison against a null is false**, including `!=`. A missing
//!   value is not "different from X", it is unknown — the same three-valued
//!   logic SQL applies. `is_null` / `is_not_null` are the way to ask.
//! - **Tree operators evaluate to false.** `has_ancestor` / `in_tree` need a
//!   task hierarchy that a flat row list does not have.
//! - **Column-vs-column comparison is not supported** here and evaluates to
//!   false; rows expose values, not a resolvable schema.

use std::borrow::Cow;

use chrono::{DateTime, Utc};

use crate::{FilterExpr, FilterLeaf, Literal, Operator, Rhs};

/// The typed value of one column for one row.
#[derive(Debug, Clone, PartialEq)]
pub enum Field<'a> {
    Text(Cow<'a, str>),
    Number(f64),
    DateTime(DateTime<Utc>),
    Bool(bool),
    /// The column exists but this row has no value for it.
    Null,
}

/// A row an expression can be evaluated against.
///
/// Returning [`Field::Null`] for an unknown column is deliberate: the column
/// set is validated up front by [`validate_columns`], so by evaluation time an
/// unknown name is a bug, and a wrong answer is better delivered as "matches
/// nothing" than as a panic in a render path.
pub trait RowFields {
    fn field(&self, column: &str) -> Field<'_>;
}

/// Whether `row` satisfies `expr`.
pub fn matches<R: RowFields + ?Sized>(expr: &FilterExpr, row: &R) -> bool {
    match expr {
        FilterExpr::And(children) => children.iter().all(|c| matches(c, row)),
        FilterExpr::Or(children) => children.iter().any(|c| matches(c, row)),
        FilterExpr::Not(inner) => !matches(inner, row),
        FilterExpr::Leaf(leaf) => matches_leaf(leaf, row),
    }
}

fn matches_leaf<R: RowFields + ?Sized>(leaf: &FilterLeaf, row: &R) -> bool {
    let field = row.field(&leaf.lhs.column);
    match leaf.op {
        Operator::IsNull => field == Field::Null,
        Operator::IsNotNull => field != Field::Null,
        Operator::HasAncestor | Operator::InTree => false,
        _ => match &leaf.rhs {
            Rhs::Lit(lit) => eval_op(&field, &leaf.op, lit),
            Rhs::Col(_) | Rhs::None => false,
        },
    }
}

fn eval_op(field: &Field, op: &Operator, lit: &Literal) -> bool {
    match field {
        Field::Null => false,
        Field::DateTime(dt) => eval_datetime(*dt, op, lit),
        Field::Number(n) => eval_number(*n, op, lit),
        Field::Bool(b) => eval_bool(*b, op, lit),
        Field::Text(s) => eval_text(s, op, lit),
    }
}

/// Datetime columns compare as instants. The right-hand side has already been
/// turned into an RFC 3339 string by [`crate::query_filter::resolve_dates`]
/// (natural language → timestamp), so it is parsed back and compared.
fn eval_datetime(value: DateTime<Utc>, op: &Operator, lit: &Literal) -> bool {
    let Literal::String(s) = lit else {
        return false;
    };
    let Ok(rhs) = DateTime::parse_from_rfc3339(s) else {
        return false;
    };
    ordering_holds(value.cmp(&rhs.with_timezone(&Utc)), op)
}

fn eval_number(value: f64, op: &Operator, lit: &Literal) -> bool {
    let one = |lit: &Literal| -> Option<f64> {
        match lit {
            Literal::Int(i) => Some(*i as f64),
            Literal::Float(f) => Some(*f),
            // A number written in quotes is still a number; refusing it would
            // punish YAML's habit of stringifying anything ambiguous.
            Literal::String(s) => s.trim().parse().ok(),
            Literal::Bool(_) | Literal::List(_) => None,
        }
    };
    match op {
        Operator::In => number_list_contains(lit, value),
        Operator::NotIn => !number_list_contains(lit, value),
        // Substring and wildcard matching on a number falls back to its
        // rendered form, which is what a user filtering `[id, has, "42"]`
        // means.
        Operator::Has | Operator::Like | Operator::NotLike => {
            eval_text(&format_number(value), op, lit)
        }
        _ => one(lit).is_some_and(|rhs| {
            value
                .partial_cmp(&rhs)
                .is_some_and(|ord| ordering_holds(ord, op))
        }),
    }
}

fn number_list_contains(lit: &Literal, value: f64) -> bool {
    match lit {
        Literal::List(items) => items.iter().any(|i| eval_number(value, &Operator::Eq, i)),
        other => eval_number(value, &Operator::Eq, other),
    }
}

/// Render a float the way a user wrote it: `5`, not `5.0`.
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

fn eval_bool(value: bool, op: &Operator, lit: &Literal) -> bool {
    let rhs = match lit {
        Literal::Bool(b) => *b,
        Literal::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => true,
            "false" | "no" | "0" => false,
            _ => return false,
        },
        _ => return false,
    };
    match op {
        Operator::Eq => value == rhs,
        Operator::Ne => value != rhs,
        _ => false,
    }
}

fn eval_text(value: &str, op: &Operator, lit: &Literal) -> bool {
    let lower = value.to_lowercase();
    match op {
        Operator::Has => lit_as_str(lit).is_some_and(|r| lower.contains(&r.to_lowercase())),
        Operator::Like => lit_as_str(lit).is_some_and(|r| like_match(&lower, &r.to_lowercase())),
        Operator::NotLike => lit_as_str(lit).is_none_or(|r| !like_match(&lower, &r.to_lowercase())),
        Operator::In => lit_list_contains(lit, &lower),
        Operator::NotIn => !lit_list_contains(lit, &lower),
        Operator::Ne => lit_as_str(lit).is_none_or(|r| lower != r.to_lowercase()),
        _ => lit_as_str(lit)
            .is_some_and(|r| ordering_holds(lower.as_str().cmp(r.to_lowercase().as_str()), op)),
    }
}

/// Whether an [`std::cmp::Ordering`] satisfies a comparison operator. Shared by
/// every typed comparison so `>=` cannot mean one thing for dates and another
/// for numbers.
fn ordering_holds(ord: std::cmp::Ordering, op: &Operator) -> bool {
    use std::cmp::Ordering::*;
    match op {
        Operator::Eq => ord == Equal,
        Operator::Ne => ord != Equal,
        Operator::Gt => ord == Greater,
        Operator::Gte => ord != Less,
        Operator::Lt => ord == Less,
        Operator::Lte => ord != Greater,
        _ => false,
    }
}

/// Render a scalar literal as a string for text comparison. Numbers and bools
/// are stringified so a mixed `[account, in, [1, 2]]` still compares sanely.
fn lit_as_str(lit: &Literal) -> Option<String> {
    match lit {
        Literal::String(s) => Some(s.clone()),
        Literal::Int(i) => Some(i.to_string()),
        Literal::Float(f) => Some(f.to_string()),
        Literal::Bool(b) => Some(b.to_string()),
        Literal::List(_) => None,
    }
}

fn lit_list_contains(lit: &Literal, lower_value: &str) -> bool {
    match lit {
        Literal::List(items) => items
            .iter()
            .filter_map(lit_as_str)
            .any(|r| r.to_lowercase() == lower_value),
        // A bare scalar with `in` behaves like equality.
        other => lit_as_str(other).is_some_and(|r| r.to_lowercase() == lower_value),
    }
}

/// SQL-`LIKE` match with `%` (any run) and `_` (any single char). Both sides
/// are expected pre-lowercased by the caller. Iterative two-pointer with
/// backtracking on `%` — no allocation, linear in the common case.
pub fn like_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut ti, mut pi) = (0usize, 0usize);
    let (mut star_p, mut star_t): (Option<usize>, usize) = (None, 0);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '_' || p[pi] == t[ti]) {
            ti += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == '%' {
            star_p = Some(pi);
            star_t = ti;
            pi += 1;
        } else if let Some(sp) = star_p {
            pi = sp + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '%' {
        pi += 1;
    }
    pi == p.len()
}

// ---------------------------------------------------------------------------
// Up-front validation
// ---------------------------------------------------------------------------

/// Reject column names the row source does not know.
///
/// Worth doing before evaluating anything: an unknown column silently matches
/// nothing, and "empty view" is the least diagnosable failure a query can
/// have. `noun` names the row kind in the message ("calendar column",
/// "column").
pub fn validate_columns(expr: &FilterExpr, known: &[&str], noun: &str) -> Result<(), String> {
    for_each_leaf(expr, &mut |leaf| {
        let col = leaf.lhs.column.as_str();
        if known.contains(&col) {
            return Ok(());
        }
        Err(format!(
            "unknown {noun} '{col}' — valid columns: {}",
            known.join(", ")
        ))
    })
}

/// Reject date comparisons whose right-hand side never became a date.
///
/// Guards the trap where a phrase the resolver cannot parse survives as a
/// literal string, fails to parse as a timestamp during evaluation, and makes
/// the whole clause — often the whole query — quietly false.
pub fn validate_datetime_literals(
    expr: &FilterExpr,
    datetime_columns: &[&str],
) -> Result<(), String> {
    for_each_leaf(expr, &mut |leaf| {
        let col = leaf.lhs.column.as_str();
        let is_comparison = matches!(
            leaf.op,
            Operator::Eq
                | Operator::Ne
                | Operator::Gt
                | Operator::Gte
                | Operator::Lt
                | Operator::Lte
        );
        if !datetime_columns.contains(&col) || !is_comparison {
            return Ok(());
        }
        let Rhs::Lit(Literal::String(s)) = &leaf.rhs else {
            return Ok(());
        };
        if DateTime::parse_from_rfc3339(s).is_ok() {
            return Ok(());
        }
        Err(format!(
            "could not interpret '{s}' as a date for column '{col}'. Use e.g. 'today', \
             'tomorrow', 'next monday', 'in 2 weeks', 'end of month', or an ISO date like \
             2026-07-20."
        ))
    })
}

fn for_each_leaf(
    expr: &FilterExpr,
    f: &mut impl FnMut(&FilterLeaf) -> Result<(), String>,
) -> Result<(), String> {
    match expr {
        FilterExpr::And(children) | FilterExpr::Or(children) => {
            children.iter().try_for_each(|c| for_each_leaf(c, f))
        }
        FilterExpr::Not(inner) => for_each_leaf(inner, f),
        FilterExpr::Leaf(leaf) => f(leaf),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_filter;
    use chrono::TimeZone;

    /// A row backed by a literal column table, so the tests exercise the
    /// evaluator rather than any particular row type.
    struct TestRow(Vec<(&'static str, Field<'static>)>);

    impl RowFields for TestRow {
        fn field(&self, column: &str) -> Field<'_> {
            self.0
                .iter()
                .find(|(k, _)| *k == column)
                .map(|(_, v)| v.clone())
                .unwrap_or(Field::Null)
        }
    }

    fn row() -> TestRow {
        TestRow(vec![
            ("title", Field::Text(Cow::Borrowed("Sprint Planning"))),
            ("prio", Field::Number(5.0)),
            (
                "updated",
                Field::DateTime(Utc.with_ymd_and_hms(2030, 1, 15, 9, 0, 0).unwrap()),
            ),
            ("done", Field::Bool(false)),
            ("note", Field::Null),
        ])
    }

    fn expr(yaml: &str) -> FilterExpr {
        let resolved = query_filter::resolve_dates(serde_yaml::from_str(yaml).unwrap());
        serde_yaml::from_value(resolved).unwrap()
    }

    fn hits(yaml: &str) -> bool {
        matches(&expr(yaml), &row())
    }

    #[test]
    fn text_compares_case_insensitively() {
        assert!(hits("[title, has, planning]"));
        assert!(hits("[title, has, PLAN]"));
        assert!(!hits("[title, has, retro]"));
        assert!(hits("[title, '=', 'sprint planning']"));
        assert!(hits("[title, like, '%planning']"));
        assert!(!hits("[title, like, 'planning%']"));
        assert!(hits("[title, in, ['sprint planning', retro]]"));
    }

    #[test]
    fn numbers_compare_numerically_not_lexically() {
        // The lexical trap: "5" > "10" as text, 5 < 10 as numbers.
        assert!(hits("[prio, '<', 10]"));
        assert!(hits("[prio, '>', 4]"));
        assert!(hits("[prio, '=', 5]"));
        assert!(hits("[prio, '>=', '5']"), "a quoted number is a number");
        assert!(hits("[prio, in, [3, 5, 7]]"));
        assert!(!hits("[prio, '>', 5]"));
    }

    #[test]
    fn substring_on_a_number_uses_its_rendered_form() {
        // `5`, not `5.0` — the user filters on what the table shows.
        assert!(hits("[prio, has, '5']"));
        assert!(!hits("[prio, has, '.']"));
    }

    #[test]
    fn dates_resolve_and_compare_as_instants() {
        assert!(hits("[updated, '>=', 2030-01-01]"));
        assert!(hits("[updated, '<', 2031-01-01]"));
        assert!(!hits("[updated, '>=', 2030-06-01]"));
    }

    #[test]
    fn every_comparison_against_null_is_false_including_inequality() {
        // Three-valued logic: a missing value is unknown, not "different".
        assert!(hits("[note, is_null]"));
        assert!(!hits("[note, is_not_null]"));
        assert!(!hits("[note, '=', anything]"));
        assert!(!hits("[note, '!=', anything]"));
        assert!(!hits("[note, has, anything]"));
        // A column the row does not expose behaves the same way.
        assert!(hits("[nonexistent, is_null]"));
    }

    #[test]
    fn bools_accept_the_spellings_yaml_produces() {
        assert!(hits("[done, '=', false]"));
        assert!(hits("[done, '=', 'no']"));
        assert!(hits("[done, '!=', true]"));
        assert!(!hits("[done, '>', false]"), "ordering bools is meaningless");
    }

    #[test]
    fn boolean_connectives_and_negation_nest() {
        assert!(hits(
            "and:\n  - [title, has, sprint]\n  - or:\n      - [prio, '>', 100]\n      - not:\n          [done, '=', true]"
        ));
    }

    #[test]
    fn tree_operators_and_column_comparisons_are_false_not_a_panic() {
        assert!(!hits("[title, has_ancestor, x]"));
        assert!(!hits("[prio, '>', .prio]"));
    }

    #[test]
    fn like_wildcards_cover_both_kinds() {
        assert!(like_match("standup", "s_andup"));
        assert!(like_match("weekly standup", "%stand%"));
        assert!(like_match("abc", "%"));
        assert!(!like_match("abc", "a_"));
    }

    #[test]
    fn validation_names_the_offending_column_and_the_alternatives() {
        let err =
            validate_columns(&expr("[titel, has, x]"), &["title", "prio"], "column").unwrap_err();
        assert!(
            err.contains("'titel'") && err.contains("title, prio"),
            "{err}"
        );
        assert!(validate_columns(&expr("[title, has, x]"), &["title"], "column").is_ok());
    }

    #[test]
    fn an_unresolvable_date_is_rejected_rather_than_matching_nothing() {
        let err =
            validate_datetime_literals(&expr("[updated, '<', 'next blorpday']"), &["updated"])
                .unwrap_err();
        assert!(err.contains("next blorpday"), "{err}");
        // A resolved one passes, and a non-date column is none of its business.
        assert!(
            validate_datetime_literals(&expr("[updated, '<', 'tomorrow']"), &["updated"]).is_ok()
        );
        assert!(
            validate_datetime_literals(&expr("[title, '<', 'whatever']"), &["updated"]).is_ok()
        );
    }
}
