//! The filter AST and how it is written in YAML.
//!
//! # Leaf forms
//!
//! | YAML                     | Means                                          |
//! |--------------------------|------------------------------------------------|
//! | `[field, op, value]`     | The ordinary comparison.                       |
//! | `[field, op]`            | An operator that takes no value (`is_null`).   |
//! | `[name, argument]`       | A host predicate — see [`FilterExpr::Custom`]. |
//!
//! The three-element form is a leaf when its middle element names an
//! [`Operator`]; a two-element form is a leaf when its *second* element does,
//! and a host predicate otherwise. Nothing else can be a leaf, which is what
//! keeps a three-clause `and:` list from being mistaken for one.
//!
//! # Field reference syntax
//!
//! Inside a leaf, strings are read as follows:
//!
//! | String form     | Meaning                       | Example            |
//! |-----------------|-------------------------------|--------------------|
//! | `.field`        | Unqualified field reference   | `.description`     |
//! | `alias.field`   | Qualified field reference     | `task.description` |
//! | `anything else` | String literal                | `%ustav%`          |
//!
//! The left-hand side is **always** a field reference (the leading `.` is
//! optional there — a bare `description` on the left is unambiguous). The
//! right-hand side is one only when it looks like `alias.field` or `.field`,
//! so that a host with a schema can compare two of its own fields. A host
//! without one — anything evaluated through [`crate::eval`] — never writes
//! that form, and there the dotted spelling is simply unavailable to string
//! literals.
//!
//! A qualifier is a join alias only to a host that has joins. To one that
//! evaluates rows in memory, `tags.sender` is a single dotted path into a
//! single row, and that is the whole name the row is asked for — see
//! [`ColRef::path`], which both halves read the left-hand side through.
//!
//! # Examples
//!
//! ```yaml
//! # Simple value comparison (lhs = bare field name, rhs = literal)
//! [description, like, '%ustav%']
//!
//! # Field against field
//! [.updated_at, '>', .created_at]
//!
//! # Compound
//! and:
//!   - [priority, '>=', 3]
//!   - or:
//!     - [.description, like, '%foo%']
//!     - [status, =, done]
//! ```

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
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
    /// A predicate this crate cannot evaluate, kept verbatim for a host that
    /// can: `[in_tree, AcmeCorp]`, `[assigned_to_me, true]`.
    ///
    /// It is a variant of its own rather than an [`Operator`] because it is not
    /// a comparison — there is no field on the left of it. The host decides
    /// what a name means, which fields it reads and how it resolves; the
    /// language only carries it through the tree and, in [`crate::eval`],
    /// answers `false`. Check the names you accept with
    /// [`validate_custom`](FilterExpr::validate_custom) when the filter is
    /// loaded, or a misspelt predicate becomes a branch that quietly matches
    /// nothing.
    Custom { name: String, arg: Literal },
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
        Self {
            table: None,
            column: column.into(),
        }
    }

    pub fn qualified(table: impl Into<String>, column: impl Into<String>) -> Self {
        Self {
            table: Some(table.into()),
            column: column.into(),
        }
    }

    /// The reference as it was written on the left-hand side: `table.column`
    /// when qualified, the bare column otherwise.
    ///
    /// This is the name a row is asked for, and the name to match against a
    /// host's vocabulary. A qualifier means a join alias to a host that builds
    /// SQL, but rows in memory have no joins: to them `tags.sender_pids` is one
    /// dotted path into one row. Reading only [`ColRef::column`] would look up
    /// `sender_pids` there and quietly find nothing.
    pub fn path(&self) -> Cow<'_, str> {
        match &self.table {
            Some(table) => Cow::Owned(format!("{table}.{}", self.column)),
            None => Cow::Borrowed(&self.column),
        }
    }
}

impl fmt::Display for ColRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path())
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

impl Serialize for Literal {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Literal::String(v) => s.serialize_str(v),
            Literal::Int(v) => s.serialize_i64(*v),
            Literal::Float(v) => s.serialize_f64(*v),
            Literal::Bool(v) => s.serialize_bool(*v),
            Literal::List(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
        }
    }
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
    /// Substring match: `[col, has, value]` → `col LIKE '%value%'`.
    Has,
    IsNull,
    IsNotNull,
    In,
    NotIn,
    /// Regular-expression match against the raw field value:
    /// `[status, matches, '^(Blocked|On Hold)$']`.
    ///
    /// Unanchored and case-**sensitive**, unlike the text operators around it:
    /// those are a search feature, while a regex here is a classification, and
    /// silent case-folding in a classification is a surprise. Write `(?i)` for
    /// the other behaviour. A host that translates the AST into SQL has no
    /// portable form for it and should say so rather than translate it into
    /// something that means almost this.
    Matches,
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// A string that names no operator this language has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownOperator(pub String);

impl fmt::Display for UnknownOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown operator '{}'", self.0)
    }
}

impl std::error::Error for UnknownOperator {}

impl std::str::FromStr for Operator {
    type Err = UnknownOperator;

    /// Every operator has several accepted spellings, so a filter can read like
    /// SQL (`>=`, `IS NULL`) or like a word (`gte`, `is_null`), whichever suits
    /// the document it sits in.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let op = match s {
            "=" | "==" | "eq" => Self::Eq,
            "!=" | "<>" | "ne" => Self::Ne,
            ">" | "gt" => Self::Gt,
            ">=" | "ge" | "gte" => Self::Gte,
            "<" | "lt" => Self::Lt,
            "<=" | "le" | "lte" => Self::Lte,
            "like" | "LIKE" => Self::Like,
            "not_like" | "NOT LIKE" => Self::NotLike,
            "has" | "HAS" => Self::Has,
            "is_null" | "IS NULL" => Self::IsNull,
            "is_not_null" | "IS NOT NULL" => Self::IsNotNull,
            "in" | "IN" => Self::In,
            "not_in" | "NOT IN" => Self::NotIn,
            "matches" | "~" => Self::Matches,
            other => return Err(UnknownOperator(other.to_string())),
        };
        Ok(op)
    }
}

impl Operator {
    /// Whether this operator requires a right-hand side.
    pub fn needs_rhs(&self) -> bool {
        !matches!(self, Self::IsNull | Self::IsNotNull)
    }

    /// The canonical spelling, i.e. the one [`Serialize`] writes.
    ///
    /// Every operator has several accepted spellings (`>=`, `ge`, `gte`); this
    /// is the one that is written back, so a filter that is loaded and saved
    /// again does not change shape a second time.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Gt => ">",
            Self::Gte => ">=",
            Self::Lt => "<",
            Self::Lte => "<=",
            Self::Like => "like",
            Self::NotLike => "not_like",
            Self::Has => "has",
            Self::IsNull => "is_null",
            Self::IsNotNull => "is_not_null",
            Self::In => "in",
            Self::NotIn => "not_in",
            Self::Matches => "matches",
        }
    }
}

impl fmt::Display for Operator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A [`FilterExpr::Custom`] whose name the host does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCustom(pub String);

impl fmt::Display for UnknownCustom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown predicate '{}'", self.0)
    }
}

impl std::error::Error for UnknownCustom {}

impl FilterExpr {
    /// Reject host predicates this host does not implement.
    ///
    /// Parsing cannot do this: `[foo, bar]` is a well-formed predicate for a
    /// host that has one called `foo`, and this crate has no way to know which
    /// those are. Call it once where the filter is loaded — it is the
    /// difference between a configuration error and a branch that matches
    /// nothing for a reason nobody can see.
    pub fn validate_custom(&self, known: &[&str]) -> Result<(), UnknownCustom> {
        match self {
            Self::And(children) | Self::Or(children) => {
                children.iter().try_for_each(|c| c.validate_custom(known))
            }
            Self::Not(inner) => inner.validate_custom(known),
            Self::Leaf(_) => Ok(()),
            Self::Custom { name, .. } => {
                if known.contains(&name.as_str()) {
                    Ok(())
                } else {
                    Err(UnknownCustom(name.clone()))
                }
            }
        }
    }
}

/// Parse a **left-hand** string as a column reference.
///
/// - `.column`       → unqualified ColRef
/// - `alias.column`  → qualified ColRef
/// - bare string     → unqualified ColRef
///
/// Everything in this position is a column — there is nothing else it could
/// be — so a dot here is a qualifier and needs no marker. The right-hand side
/// is the opposite case and has its own parser, [`parse_rhs_col_ref`].
fn parse_col_ref(s: &str) -> ColRef {
    if let Some(field) = s.strip_prefix('.') {
        // .field → unqualified
        return ColRef::unqualified(field);
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
/// A **leading dot** marks one, and nothing else does: `.updated_at` for a
/// column of this row, `.task.updated_at` for a qualified one. Any other
/// string is text, dots and all.
///
/// It used to read `alias.column` as a reference too, on the theory that a
/// dotted word is a rarer thing to compare against than a join. That is true
/// of task titles and false of nearly everything else a host filters:
/// `dbus.socket`, `man-db.timer`, `example.com`, `v1.2` all became references
/// to columns that do not exist, and an expression whose right-hand side does
/// not resolve matches nothing — an empty view with no error anywhere. The
/// marker is now explicit, so a value is only a column when it was written as
/// one. Costs a dot in the rare case, removes a trap from the common one.
fn is_col_ref(s: &str) -> bool {
    let Some(path) = s.strip_prefix('.') else {
        return false;
    };
    let is_ident = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_');
    let mut parts = path.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(column), None, _) => is_ident(column),
        (Some(table), Some(column), None) => is_ident(table) && is_ident(column),
        _ => false,
    }
}

/// Parse a **right-hand** string that [`is_col_ref`] has accepted.
///
/// The leading dot is the marker, not part of the path: `.created_at` is the
/// unqualified column, `.task.created_at` the qualified one.
fn parse_rhs_col_ref(s: &str) -> ColRef {
    let path = s.strip_prefix('.').unwrap_or(s);
    match path.split_once('.') {
        Some((table, column)) => ColRef::qualified(table, column),
        None => ColRef::unqualified(path),
    }
}

/// Parse a YAML scalar value as a [`Literal`].
fn parse_literal_str(s: &str) -> Literal {
    if let Ok(i) = s.parse::<i64>() {
        return Literal::Int(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return Literal::Float(f);
    }
    if s == "true" {
        return Literal::Bool(true);
    }
    if s == "false" {
        return Literal::Bool(false);
    }
    Literal::String(s.to_string())
}

/// Convert a `serde_yaml::Value` into a [`Literal`] or [`Rhs::Col`].
fn yaml_value_to_rhs(v: &serde_yaml::Value) -> Result<Rhs, String> {
    match v {
        serde_yaml::Value::String(s) => {
            if is_col_ref(s) {
                Ok(Rhs::Col(parse_rhs_col_ref(s)))
            } else {
                Ok(Rhs::Lit(parse_literal_str(s)))
            }
        }
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Ok(Rhs::Lit(Literal::Int(i)));
            }
            if let Some(f) = n.as_f64() {
                return Ok(Rhs::Lit(Literal::Float(f)));
            }
            Err(format!("unrepresentable number: {n}"))
        }
        serde_yaml::Value::Bool(b) => Ok(Rhs::Lit(Literal::Bool(*b))),
        serde_yaml::Value::Sequence(seq) => {
            let items: Result<Vec<_>, _> = seq
                .iter()
                .map(|item| match yaml_value_to_rhs(item)? {
                    Rhs::Lit(l) => Ok(l),
                    Rhs::Col(_) => {
                        Err("column references inside lists are not supported".to_string())
                    }
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
            "or" => FilterExpr::Or(map.next_value::<Vec<FilterExpr>>()?),
            "not" => FilterExpr::Not(Box::new(map.next_value::<FilterExpr>()?)),
            other => return Err(de::Error::unknown_field(other, &["and", "or", "not"])),
        };

        Ok(expr)
    }

    /// Array form: `[lhs, op, rhs]`, `[lhs, op]` for the operators that take
    /// no value, or `[name, argument]` for a host predicate.
    ///
    /// The three forms are told apart by where an operator name appears, and
    /// nowhere else: second element → a leaf, otherwise → a host predicate.
    /// Reading the second element as a raw YAML value rather than as a string
    /// is what makes `[in_tree, 42]` a predicate with a number in it instead of
    /// a parse error.
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let first: String = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &"2 or 3 elements"))?;
        let second: serde_yaml::Value = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(1, &"2 or 3 elements"))?;

        let Some(op) = second.as_str().and_then(|s| s.parse::<Operator>().ok()) else {
            // A host predicate: `[name, argument]`.
            let Rhs::Lit(arg) = yaml_value_to_rhs(&second).map_err(de::Error::custom)? else {
                return Err(de::Error::custom(format!(
                    "the argument of predicate '{first}' must be a value, not a field reference"
                )));
            };
            if seq.next_element::<serde_yaml::Value>()?.is_some() {
                return Err(de::Error::custom(format!(
                    "'{}' is not an operator, so '{first}' reads as a host predicate — \
                     which takes exactly one argument",
                    second.as_str().unwrap_or_default()
                )));
            }
            return Ok(FilterExpr::Custom { name: first, arg });
        };

        let lhs = parse_col_ref(&first);

        // rhs — absent for the operators that take no value
        let rhs = if op.needs_rhs() {
            let val: serde_yaml::Value = seq
                .next_element()?
                .ok_or_else(|| de::Error::invalid_length(2, &"3 elements"))?;
            if op == Operator::Matches {
                // A pattern is always a string, never a field reference and
                // never a number: `[key, matches, 'a.b']` is a regex with a
                // wildcard in it, not the field `b` of table `a`, and
                // `[n, matches, '2.5']` must not be coerced to a float.
                match val {
                    serde_yaml::Value::String(s) => Rhs::Lit(Literal::String(s)),
                    other => {
                        return Err(de::Error::custom(format!(
                            "the right-hand side of `matches` must be a regex string, got {other:?}"
                        )));
                    }
                }
            } else {
                yaml_value_to_rhs(&val).map_err(de::Error::custom)?
            }
        } else {
            Rhs::None
        };

        Ok(FilterExpr::Leaf(FilterLeaf { lhs, op, rhs }))
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Written back in the canonical spelling of each form, so a filter that is
/// loaded and saved keeps its meaning and settles on one shape.
///
/// It is not byte-for-byte round-tripping, and cannot be: the terse array form
/// resolves `"5"` to a number when it reads it, so a *string literal* that
/// happens to look like one comes back as the other thing. Write such values
/// through a host that quotes them, or keep the source document if the exact
/// bytes matter. A column reference does survive, in both directions: it is
/// written back with its leading dot, which is what marks it.
impl Serialize for FilterExpr {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            FilterExpr::And(children) => single_key(s, "and", children),
            FilterExpr::Or(children) => single_key(s, "or", children),
            FilterExpr::Not(inner) => single_key(s, "not", inner),
            FilterExpr::Leaf(leaf) => {
                let mut seq = s.serialize_seq(Some(if leaf.op.needs_rhs() { 3 } else { 2 }))?;
                seq.serialize_element(&leaf.lhs.path())?;
                seq.serialize_element(leaf.op.as_str())?;
                match &leaf.rhs {
                    Rhs::Lit(lit) => seq.serialize_element(lit)?,
                    Rhs::Col(col) => seq.serialize_element(&dotted(col))?,
                    Rhs::None => {}
                }
                seq.end()
            }
            FilterExpr::Custom { name, arg } => {
                let mut seq = s.serialize_seq(Some(2))?;
                seq.serialize_element(name)?;
                seq.serialize_element(arg)?;
                seq.end()
            }
        }
    }
}

fn single_key<S: Serializer, T: Serialize + ?Sized>(
    s: S,
    key: &str,
    value: &T,
) -> Result<S::Ok, S::Error> {
    let mut map = s.serialize_map(Some(1))?;
    map.serialize_entry(key, value)?;
    map.end()
}

/// The right-hand side, where a leading dot is what marks a field reference.
fn dotted(col: &ColRef) -> String {
    format!(".{}", col.path())
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
        // Join syntax: a qualifier needs no marker on the left, and the dot
        // that marks a reference on the right comes before the whole path.
        let expr = parse("[task.updated_at, '>', .task.created_at]");
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
    fn a_dotted_value_is_text() {
        // The values a host actually filters on are full of dots. None of
        // them is a column reference, and reading them as one used to empty
        // the view without a word.
        for value in ["dbus.socket", "man-db.timer", "example.com", "v1.2"] {
            let expr = parse(&format!("[unit, '=', {value}]"));
            assert_eq!(
                expr,
                FilterExpr::Leaf(FilterLeaf {
                    lhs: ColRef::unqualified("unit"),
                    op: Operator::Eq,
                    rhs: Rhs::Lit(Literal::String(value.into())),
                }),
                "{value} should have stayed text"
            );
        }
    }

    #[test]
    fn a_marked_column_survives_a_round_trip() {
        for written in [".created_at", ".task.created_at"] {
            let expr = parse(&format!("[updated_at, '>', '{written}']"));
            let back = serde_yaml::to_string(&expr).unwrap();
            assert!(
                back.contains(written),
                "{written} came back as {back}, losing its marker"
            );
            assert_eq!(expr, serde_yaml::from_str::<FilterExpr>(&back).unwrap());
        }
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
        let FilterExpr::And(children) = expr else {
            panic!("expected And")
        };
        assert_eq!(children.len(), 2);
        assert!(matches!(children[1], FilterExpr::Or(_)));
    }

    #[test]
    fn test_has_operator() {
        let expr = parse("[description, has, meeting]");
        assert_eq!(
            expr,
            FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("description"),
                op: Operator::Has,
                rhs: Rhs::Lit(Literal::String("meeting".into())),
            })
        );
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
