//! Evaluating a [`FilterExpr`] against rows that are already in memory.
//!
//! A host with a database can walk the AST and let SQL do the work. That is not
//! available whenever the rows arrived from somewhere else — a merged event
//! list, an API result that a query then narrows locally, a queue of
//! notifications held in memory. This module is the other half: the same
//! language, evaluated row by row, so the two cannot drift apart in what a
//! query means.
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
//! - **A dotted field name is passed through whole.** `tags.sender` reaches
//!   [`RowFields::field`] as `"tags.sender"`: rows in memory have no joins, so
//!   a qualifier is part of the path rather than a table to resolve.
//! - **Host predicates evaluate to false.** A `[name, argument]` leaf means
//!   something only the host can resolve; see [`crate::FilterExpr::Custom`].
//! - **Field-vs-field comparison is not supported** here and evaluates to
//!   false; rows expose values, not a resolvable schema.
//! - **`matches` is the exception to case-insensitivity.** A regex is a
//!   classification rather than a search, so it matches the raw value exactly
//!   as written; `(?i)` asks for the other behaviour. It is also the one
//!   operator with no portable SQL half — see [`crate::Operator::Matches`].

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
    /// The row has no value for this field.
    Null,
}

impl Field<'_> {
    /// A [`Field::DateTime`] from a Unix timestamp.
    ///
    /// Chrono is this crate's business, not its caller's: a host that keeps
    /// time in `time`, `jiff` or a bare `i64` can answer with an instant
    /// without naming a date library of its own. Out-of-range values become
    /// [`Field::Null`] — a row far outside the representable range has no
    /// usable instant to compare, and a filter is no place to panic.
    pub fn timestamp(seconds: i64, nanoseconds: u32) -> Self {
        match DateTime::from_timestamp(seconds, nanoseconds) {
            Some(dt) => Field::DateTime(dt),
            None => Field::Null,
        }
    }
}

/// A row an expression can be evaluated against.
///
/// Returning [`Field::Null`] for a field this row does not have is the
/// expected answer, not an error: a filter is written against a whole
/// vocabulary and a given row may not carry all of it. Validate the *names* up
/// front with [`validate_fields`] if a typo should be a configuration error;
/// by evaluation time, "matches nothing" beats a panic in a render path.
pub trait RowFields {
    fn field(&self, name: &str) -> Field<'_>;
}

/// Whether `row` satisfies `expr`.
pub fn matches<R: RowFields + ?Sized>(expr: &FilterExpr, row: &R) -> bool {
    match expr {
        FilterExpr::And(children) => children.iter().all(|c| matches(c, row)),
        FilterExpr::Or(children) => children.iter().any(|c| matches(c, row)),
        FilterExpr::Not(inner) => !matches(inner, row),
        FilterExpr::Leaf(leaf) => matches_leaf(leaf, row),
        // A host predicate is the host's to resolve. Nothing here knows what
        // it means, and guessing would be worse than the documented `false`.
        FilterExpr::Custom { .. } => false,
    }
}

fn matches_leaf<R: RowFields + ?Sized>(leaf: &FilterLeaf, row: &R) -> bool {
    // The whole written path, not just the column: a row in memory has no
    // joins, so `tags.sender_pids` names one field of it. See [`ColRef::path`].
    let field = row.field(&leaf.lhs.path());
    match leaf.op {
        Operator::IsNull => field == Field::Null,
        Operator::IsNotNull => field != Field::Null,
        Operator::Matches => match &leaf.rhs {
            Rhs::Lit(Literal::String(pattern)) => matches_regex(&field, pattern),
            _ => false,
        },
        _ => match &leaf.rhs {
            Rhs::Lit(lit) => eval_op(&field, &leaf.op, lit),
            Rhs::Col(_) | Rhs::None => false,
        },
    }
}

/// Match a compiled regex against the field's **raw** value.
///
/// Raw, not rendered: matching the rendering would test `1h 30m` where the
/// data says `5400`, and a narrow column would be matched including its
/// ellipsis. A null matches nothing — the same three-valued logic every other
/// operator here follows.
fn matches_regex(field: &Field, pattern: &str) -> bool {
    let subject: Cow<str> = match field {
        Field::Null => return false,
        Field::Text(s) => Cow::Borrowed(s.as_ref()),
        Field::Number(n) => Cow::Owned(format_number(*n)),
        Field::Bool(b) => Cow::Borrowed(if *b { "true" } else { "false" }),
        Field::DateTime(dt) => Cow::Owned(dt.to_rfc3339()),
    };
    // A pattern that failed to compile matches nothing; the loader has already
    // reported it by name, and a render path is no place to raise it again.
    crate::regex_cache::compiled(pattern).is_ok_and(|re| re.is_match(&subject))
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
/// written as an RFC 3339 string — by hand, or by the `natural-dates` pre-pass
/// that turns "end of next week" into one — so it is parsed back and compared.
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
pub fn validate_fields(expr: &FilterExpr, known: &[&str], noun: &str) -> Result<(), String> {
    for_each_leaf(expr, &mut |leaf| {
        let col = leaf.lhs.path();
        if known.contains(&col.as_ref()) {
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
        let col = leaf.lhs.path();
        let is_comparison = matches!(
            leaf.op,
            Operator::Eq
                | Operator::Ne
                | Operator::Gt
                | Operator::Gte
                | Operator::Lt
                | Operator::Lte
        );
        if !datetime_columns.contains(&col.as_ref()) || !is_comparison {
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
        FilterExpr::Custom { .. } => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// A dotted name is one field of one row, not a join.
    #[test]
    fn dotted_field_reaches_the_row() {
        let row = TestRow(vec![
            ("tags.sender", Field::Text(Cow::Borrowed("kitty"))),
            ("sender", Field::Text(Cow::Borrowed("wrong one"))),
        ]);
        assert!(matches(&expr("[tags.sender, '=', kitty]"), &row));
        assert!(!matches(&expr("[tags.sender, '=', 'wrong one']"), &row));
    }

    /// And the vocabulary is checked against the same written name.
    #[test]
    fn dotted_field_validates_against_the_written_name() {
        let e = expr("[tags.sender, is_not_null]");
        assert!(validate_fields(&e, &["tags.sender"], "field").is_ok());
        assert!(validate_fields(&e, &["sender"], "field").is_err());
    }

    fn expr(yaml: &str) -> FilterExpr {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        // The same pre-pass a host runs, so the datetime tests below compare
        // what they would compare in production. Without the feature the
        // right-hand side has to be written as a timestamp already.
        #[cfg(feature = "natural-dates")]
        let value = crate::dates::resolve_dates(value);
        serde_yaml::from_value(value).unwrap()
    }

    fn hits(yaml: &str) -> bool {
        matches(&expr(yaml), &row())
    }

    #[test]
    fn matches_is_case_sensitive_unlike_every_other_text_operator() {
        // A regex here is a classification, not a search — silent case-folding
        // in a classification is a surprise.
        assert!(hits("[title, matches, '^Sprint']"));
        assert!(!hits("[title, matches, '^sprint']"));
        // ...and `(?i)` is how you ask for the other behaviour.
        assert!(hits("[title, matches, '(?i)^sprint']"));
    }

    #[test]
    fn matches_is_unanchored() {
        assert!(hits("[title, matches, 'Plan']"));
        assert!(hits("[title, matches, '^Sprint Planning$']"));
    }

    #[test]
    fn matches_sees_the_raw_value_of_a_non_text_column() {
        // Not the rendering: a duration column says 5400, not "1h 30m".
        assert!(hits("[prio, matches, '^5$']"));
        assert!(!hits("[prio, matches, '^5.0$']"));
        assert!(hits("[done, matches, '^false$']"));
        assert!(hits("[updated, matches, '^2030-01-15']"));
    }

    #[test]
    fn matches_against_a_null_is_false() {
        // The same three-valued logic every other operator here follows —
        // including against a pattern that would match the empty string.
        assert!(!hits("[note, matches, '.*']"));
        assert!(!hits("[nonexistent, matches, '.*']"));
    }

    #[test]
    fn a_pattern_that_does_not_compile_matches_nothing() {
        // Reported by the config loader; a render path is no place to raise it.
        assert!(!hits("[title, matches, '(']"));
    }

    #[test]
    fn a_pattern_is_never_read_as_a_column_reference() {
        // `is_col_ref` would take `a.b` for the column `b` of table `a`.
        let e = expr("[title, matches, 'Sprint.Planning']");
        assert!(matches(&e, &row()));
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
    #[cfg(feature = "natural-dates")]
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
    fn host_predicates_and_field_comparisons_are_false_not_a_panic() {
        // Neither is answerable from a row on its own: one needs whatever the
        // host means by `in_tree`, the other needs a schema. False is the
        // documented answer for both — an evaluator in a render path has
        // nowhere to raise anything.
        assert!(!hits("[in_tree, AcmeCorp]"));
        assert!(!hits("[prio, '>', .prio]"));
    }

    #[test]
    fn a_host_predicate_survives_parsing_with_its_argument_intact() {
        // The point of carrying it through untouched: the host is the only one
        // that can resolve it, and it can only do that if it still has it.
        assert_eq!(
            expr("[in_tree, AcmeCorp]"),
            FilterExpr::Custom {
                name: "in_tree".into(),
                arg: Literal::String("AcmeCorp".into()),
            }
        );
        assert_eq!(
            expr("and:\n  - [assigned_to_me, true]\n  - [prio, '>', 3]\n")
                .validate_custom(&["in_tree"])
                .unwrap_err()
                .to_string(),
            "unknown predicate 'assigned_to_me'"
        );
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
            validate_fields(&expr("[titel, has, x]"), &["title", "prio"], "column").unwrap_err();
        assert!(
            err.contains("'titel'") && err.contains("title, prio"),
            "{err}"
        );
        assert!(validate_fields(&expr("[title, has, x]"), &["title"], "column").is_ok());
    }

    #[test]
    #[cfg(feature = "natural-dates")]
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
