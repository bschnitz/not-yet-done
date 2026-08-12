//! Turning a document into an [`ExtendedQuery`].
//!
//! Every error carries the path of the node it happened in (`or[1].and[0]`),
//! because the interesting mistakes in this format are structural: a key
//! misspelled two levels down, a `without` with a single operand, an
//! `order_by` written as a mapping. A bare "invalid query" would leave the
//! user hunting through the tree.
//!
//! Nothing here talks to an adapter. Language checking needs to know which
//! adapter the document will run against, so it lives in the separate
//! [`check_languages`] the executor calls once it does.

use not_yet_done_filter::{FilterExpr, Operator, query_filter};
use serde_yaml::Value;

use crate::ast::{Direction, ExtendedQuery, Fetch, FetchSource, Node, NodeKind, OrderKey};
use crate::markdown::{self, Document, MarkdownError};

/// Keys that select what a node *is*. Exactly one may appear per node.
const OPERATOR_KEYS: &[&str] = &[
    "query",
    "q",
    "query-ref",
    "query_ref",
    "and",
    "or",
    "without",
];
/// Keys that modify the set a node produces.
const ATTRIBUTE_KEYS: &[&str] = &["local_filter", "limit"];

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error(transparent)]
    Markdown(#[from] MarkdownError),

    #[error("the specification fence is not valid YAML: {0}")]
    Yaml(String),

    #[error("{path}: {message}")]
    Spec { path: String, message: String },

    #[error(
        "{what} declares query language `{found}`, but this view's adapter speaks `{expected}`"
    )]
    Language {
        what: String,
        found: String,
        expected: String,
    },
}

impl ParseError {
    fn at(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Spec {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Parse a whole extended-query document.
pub fn parse(source: &str) -> Result<ExtendedQuery, ParseError> {
    parse_document(&markdown::split(source)?)
}

/// Parse an already-split document — useful when the caller needs the fences
/// for something else as well.
pub fn parse_document(doc: &Document) -> Result<ExtendedQuery, ParseError> {
    let value: Value =
        serde_yaml::from_str(&doc.spec.text).map_err(|e| ParseError::Yaml(e.to_string()))?;

    let Some(map) = value.as_mapping() else {
        return Err(ParseError::at(
            "spec",
            "expected a mapping, e.g. `and:` with a list of branches",
        ));
    };

    let order_by = match map.get("order_by") {
        Some(v) => parse_order_by(v)?,
        None => Vec::new(),
    };

    // The root node shares the top-level mapping with `order_by`, so it is
    // parsed from the mapping with that key removed rather than from a nested
    // one — a document whose whole content is one fetch stays a one-liner.
    let mut root_map = map.clone();
    root_map.remove("order_by");
    let root = parse_node(&Value::Mapping(root_map), "spec", doc)?;

    Ok(ExtendedQuery { root, order_by })
}

fn parse_node(value: &Value, path: &str, doc: &Document) -> Result<Node, ParseError> {
    let Some(map) = value.as_mapping() else {
        return Err(ParseError::at(
            path,
            "expected a mapping, e.g. `- query: …` or `- and: [ … ]`",
        ));
    };

    let mut present: Vec<String> = Vec::new();
    for k in map.keys() {
        let Some(name) = k.as_str() else {
            return Err(ParseError::at(path, "keys must be plain strings"));
        };
        if OPERATOR_KEYS.contains(&name) {
            present.push(name.to_string());
        } else if !ATTRIBUTE_KEYS.contains(&name) {
            return Err(ParseError::at(
                path,
                format!(
                    "unknown key `{name}`; allowed here: {}",
                    OPERATOR_KEYS
                        .iter()
                        .chain(ATTRIBUTE_KEYS)
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }

    let kind = match present.as_slice() {
        [] => {
            return Err(ParseError::at(
                path,
                format!(
                    "no query key; exactly one of {} is required",
                    OPERATOR_KEYS.join(", ")
                ),
            ));
        }
        [only] => parse_kind(
            only,
            map.get(only.as_str()).expect("key was just read"),
            path,
            doc,
        )?,
        many => {
            return Err(ParseError::at(
                path,
                format!(
                    "`{}` cannot be combined — a node produces exactly one set, so it carries \
                     exactly one of {}",
                    many.join("` and `"),
                    OPERATOR_KEYS.join(", ")
                ),
            ));
        }
    };

    let local_filter = match map.get("local_filter") {
        Some(v) => Some(parse_local_filter(v, path)?),
        None => None,
    };
    let limit = match map.get("limit") {
        Some(v) => Some(parse_limit(v, path)?),
        None => None,
    };

    Ok(Node {
        kind,
        local_filter,
        limit,
    })
}

fn parse_kind(op: &str, value: &Value, path: &str, doc: &Document) -> Result<NodeKind, ParseError> {
    match op {
        "query" | "q" => {
            let text = require_str(value, path, op)?;
            Ok(NodeKind::Fetch(Fetch {
                text: trimmed(text),
                language: None,
                source: FetchSource::Inline,
            }))
        }
        "query-ref" | "query_ref" => {
            let name = require_str(value, path, op)?;
            let Some(fence) = doc.library_entry(name) else {
                let known = doc.library_names();
                let available = if known.is_empty() {
                    "the document declares no named fences".to_string()
                } else {
                    format!("available: {}", known.join(", "))
                };
                return Err(ParseError::at(
                    path,
                    format!("no fence named `{name}` — {available}"),
                ));
            };
            Ok(NodeKind::Fetch(Fetch {
                text: trimmed(&fence.text),
                language: fence.language.clone(),
                source: FetchSource::Ref(name.to_string()),
            }))
        }
        "and" | "or" | "without" => {
            let Some(items) = value.as_sequence() else {
                return Err(ParseError::at(
                    path,
                    format!("`{op}` takes a list of nodes"),
                ));
            };
            // `without` needs a minuend and something to subtract; `and`/`or`
            // stay legal with one operand so the single-branch pass-through
            // template needs no special case.
            let minimum = if op == "without" { 2 } else { 1 };
            if items.len() < minimum {
                return Err(ParseError::at(
                    path,
                    format!(
                        "`{op}` needs at least {minimum} operand{}, found {}",
                        if minimum == 1 { "" } else { "s" },
                        items.len()
                    ),
                ));
            }
            let mut operands = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                operands.push(parse_node(item, &format!("{path}.{op}[{i}]"), doc)?);
            }
            Ok(match op {
                "and" => NodeKind::And(operands),
                "or" => NodeKind::Or(operands),
                _ => NodeKind::Without(operands),
            })
        }
        other => unreachable!("unhandled operator key `{other}`"),
    }
}

/// A node's `local_filter`, as a list of leaves that are implicitly AND-ed.
///
/// A single leaf may also be written directly (`local_filter: [done, "=", false]`).
/// The two forms are told apart the same way [`query_filter::resolve_dates`]
/// does it — by the middle element of a three-element sequence naming an
/// operator — because a list of leaves always holds sequences or mappings,
/// never bare operator strings.
fn parse_local_filter(value: &Value, path: &str) -> Result<FilterExpr, ParseError> {
    let resolved = query_filter::resolve_dates(value.clone());
    let where_ = format!("{path}.local_filter");

    let single = |v: &Value| -> Result<FilterExpr, ParseError> {
        serde_yaml::from_value::<FilterExpr>(v.clone())
            .map_err(|e| ParseError::at(&where_, format!("invalid filter: {e}")))
    };

    match &resolved {
        Value::Sequence(items) if looks_like_leaf(items) => single(&resolved),
        Value::Sequence(items) if items.is_empty() => Err(ParseError::at(
            &where_,
            "empty; drop the key instead of filtering on nothing",
        )),
        Value::Sequence(items) => {
            let mut exprs = Vec::with_capacity(items.len());
            for item in items {
                exprs.push(single(item)?);
            }
            Ok(if exprs.len() == 1 {
                exprs.remove(0)
            } else {
                FilterExpr::And(exprs)
            })
        }
        Value::Mapping(_) => single(&resolved),
        _ => Err(ParseError::at(
            &where_,
            "expected a list of `[column, op, value]` leaves",
        )),
    }
}

fn looks_like_leaf(items: &[Value]) -> bool {
    items.len() == 3
        && items[1]
            .as_str()
            .is_some_and(|s| Operator::from_str(s).is_some())
}

fn parse_limit(value: &Value, path: &str) -> Result<usize, ParseError> {
    match value.as_u64() {
        Some(0) | None => Err(ParseError::at(
            format!("{path}.limit"),
            "expected a positive whole number of rows",
        )),
        Some(n) => Ok(n as usize),
    }
}

/// The document-level sort keys.
///
/// A list is required, not a mapping: a YAML mapping has no guaranteed key
/// order, so `{updated: desc, summary: asc}` could not express which key is
/// more significant. In a list, position is significance.
fn parse_order_by(value: &Value) -> Result<Vec<OrderKey>, ParseError> {
    let items = match value {
        Value::Sequence(items) => items,
        Value::Mapping(_) => {
            return Err(ParseError::at(
                "order_by",
                "must be a list of single-key entries (`- updated: desc`), not a mapping — \
                 list position decides which key sorts first",
            ));
        }
        _ => {
            return Err(ParseError::at(
                "order_by",
                "must be a list of single-key entries (`- updated: desc`)",
            ));
        }
    };

    let mut keys = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let path = format!("order_by[{i}]");
        match item {
            // A bare column name is shorthand for ascending.
            Value::String(column) => keys.push(OrderKey {
                column: column.clone(),
                direction: Direction::Asc,
            }),
            Value::Mapping(map) if map.len() == 1 => {
                let (k, v) = map.iter().next().expect("length checked");
                let column = k
                    .as_str()
                    .ok_or_else(|| ParseError::at(&path, "column name must be a string"))?;
                keys.push(OrderKey {
                    column: column.to_string(),
                    direction: parse_direction(v, &path)?,
                });
            }
            Value::Mapping(_) => {
                return Err(ParseError::at(
                    &path,
                    "one column per list entry — split it up so the order of the keys is \
                     explicit",
                ));
            }
            _ => {
                return Err(ParseError::at(
                    &path,
                    "expected `- column` or `- column: asc|desc`",
                ));
            }
        }
    }
    Ok(keys)
}

fn parse_direction(value: &Value, path: &str) -> Result<Direction, ParseError> {
    let raw = value
        .as_str()
        .ok_or_else(|| ParseError::at(path, "direction must be `asc` or `desc`"))?;
    match raw.to_ascii_lowercase().as_str() {
        "asc" | "ascending" | "up" => Ok(Direction::Asc),
        "desc" | "descending" | "down" => Ok(Direction::Desc),
        other => Err(ParseError::at(
            path,
            format!("unknown sort direction `{other}`; expected `asc` or `desc`"),
        )),
    }
}

/// Reject fences written in a language the target adapter does not speak.
///
/// Kept out of [`parse`] on purpose: the adapter is known only at execution
/// time, and a document must stay parseable (and thus editable, and thus
/// fixable) without one.
pub fn check_languages(query: &ExtendedQuery, adapter_language: &str) -> Result<(), ParseError> {
    for fetch in query.fetches() {
        let Some(found) = fetch.language.as_deref() else {
            continue;
        };
        if found.eq_ignore_ascii_case(adapter_language) {
            continue;
        }
        let what = match &fetch.source {
            FetchSource::Ref(name) => format!("fence `{name}`"),
            FetchSource::Inline => "an inline query".to_string(),
        };
        return Err(ParseError::Language {
            what,
            found: found.to_string(),
            expected: adapter_language.to_string(),
        });
    }
    Ok(())
}

/// What the editor scaffolds for a new extended query: a single branch, which
/// behaves exactly like the conventional query it wraps — same result set,
/// same ordering — so the user can start by pasting and grow from there.
pub fn default_template(language: &str) -> String {
    format!(
        "```yaml\nand:\n  - query-ref: default\n```\n\n```{language} default\n\
         # the conventional query goes here\n```\n"
    )
}

/// A fetch's query text, without the trailing whitespace the container adds.
///
/// A fence always ends in a newline, an inline `query:` never does. Left as
/// they are, the same query written both ways would be two different texts —
/// and the executor deduplicates round-trips by exactly that text, so the
/// document would pay twice for one query. No backend language gives trailing
/// whitespace a meaning.
fn trimmed(text: &str) -> String {
    text.trim_end().to_string()
}

fn require_str<'a>(value: &'a Value, path: &str, op: &str) -> Result<&'a str, ParseError> {
    value
        .as_str()
        .ok_or_else(|| ParseError::at(path, format!("`{op}` takes a string")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_filter::{ColRef, FilterLeaf, Literal, Rhs};

    /// Wrap a specification in the minimal document around it.
    fn spec(yaml: &str) -> String {
        format!("```yaml\n{yaml}\n```\n")
    }

    fn parse_spec(yaml: &str) -> Result<ExtendedQuery, ParseError> {
        parse(&spec(yaml))
    }

    fn fetch(node: &Node) -> &Fetch {
        match &node.kind {
            NodeKind::Fetch(f) => f,
            other => panic!("expected a fetch, got {other:?}"),
        }
    }

    #[test]
    fn a_single_inline_query_is_the_whole_document() {
        let q = parse_spec("query: assignee = currentUser()").unwrap();
        assert_eq!(fetch(&q.root).text, "assignee = currentUser()");
        assert_eq!(fetch(&q.root).source, FetchSource::Inline);
        assert_eq!(fetch(&q.root).language, None);
        assert!(q.order_by.is_empty(), "default order is merge order");
    }

    #[test]
    fn q_is_an_alias_for_query() {
        let q = parse_spec("q: key = ABC-1").unwrap();
        assert_eq!(fetch(&q.root).text, "key = ABC-1");
    }

    #[test]
    fn a_ref_takes_text_and_language_from_its_fence() {
        let src = "```yaml\nand:\n  - query-ref: mine\n```\n\n```jql mine\nassignee = x\n```\n";
        let q = parse(src).unwrap();
        let NodeKind::And(ops) = &q.root.kind else {
            panic!("expected and");
        };
        assert_eq!(ops.len(), 1);
        assert_eq!(fetch(&ops[0]).text, "assignee = x");
        assert_eq!(fetch(&ops[0]).language.as_deref(), Some("jql"));
        assert_eq!(fetch(&ops[0]).source, FetchSource::Ref("mine".into()));
    }

    #[test]
    fn query_ref_underscore_spelling_also_resolves() {
        let src = "```yaml\nquery_ref: mine\n```\n\n```jql mine\nx\n```\n";
        assert_eq!(fetch(&parse(src).unwrap().root).text, "x");
    }

    #[test]
    fn fetches_are_returned_in_walk_order() {
        let q = parse_spec(
            "or:\n  - query: a\n  - and:\n      - query: b\n      - query: c\n  - query: d",
        )
        .unwrap();
        let texts: Vec<&str> = q.fetches().iter().map(|f| f.text.as_str()).collect();
        assert_eq!(texts, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn attributes_hang_on_the_node_that_produces_the_set() {
        let q = parse_spec(
            "or:\n  - query: a\n    limit: 50\n    local_filter:\n      - [prio, \">\", 5]\n  - query: b",
        )
        .unwrap();
        let NodeKind::Or(ops) = &q.root.kind else {
            panic!("expected or");
        };
        assert_eq!(ops[0].limit, Some(50));
        assert_eq!(
            ops[0].local_filter,
            Some(FilterExpr::Leaf(FilterLeaf {
                lhs: ColRef::unqualified("prio"),
                op: Operator::Gt,
                rhs: Rhs::Lit(Literal::Int(5)),
            }))
        );
        assert_eq!(ops[1].limit, None);
        assert_eq!(ops[1].local_filter, None);
    }

    #[test]
    fn several_leaves_are_and_ed_and_a_bare_leaf_is_taken_as_one() {
        let many =
            parse_spec("query: a\nlocal_filter:\n  - [prio, \">\", 5]\n  - [state, \"=\", open]")
                .unwrap();
        assert!(matches!(
            many.root.local_filter,
            Some(FilterExpr::And(ref v)) if v.len() == 2
        ));

        let one = parse_spec("query: a\nlocal_filter: [prio, \">\", 5]").unwrap();
        assert!(matches!(one.root.local_filter, Some(FilterExpr::Leaf(_))));
    }

    #[test]
    fn a_nested_local_filter_mapping_keeps_its_own_shape() {
        let q =
            parse_spec("query: a\nlocal_filter:\n  or:\n    - [a, \"=\", 1]\n    - [b, \"=\", 2]")
                .unwrap();
        assert!(matches!(
            q.root.local_filter,
            Some(FilterExpr::Or(ref v)) if v.len() == 2
        ));
    }

    #[test]
    fn dates_in_a_local_filter_are_resolved() {
        let q = parse_spec("query: a\nlocal_filter:\n  - [updated, \">\", \"yesterday\"]").unwrap();
        let Some(FilterExpr::Leaf(leaf)) = &q.root.local_filter else {
            panic!("expected a leaf");
        };
        let Rhs::Lit(Literal::String(value)) = &leaf.rhs else {
            panic!("expected a string literal");
        };
        assert!(
            value.contains('T') && value.len() > 10,
            "`yesterday` should have become a timestamp, got {value}"
        );
    }

    #[test]
    fn order_by_position_decides_significance() {
        let q = parse_spec("query: a\norder_by:\n  - updated: desc\n  - summary").unwrap();
        assert_eq!(
            q.order_by,
            vec![
                OrderKey {
                    column: "updated".into(),
                    direction: Direction::Desc
                },
                OrderKey {
                    column: "summary".into(),
                    direction: Direction::Asc
                },
            ]
        );
    }

    #[test]
    fn structural_mistakes_name_the_node_they_happened_in() {
        let cases: Vec<(&str, &str, &str)> = vec![
            (
                "or:\n  - query: a\n  - quer: b",
                "spec.or[1]",
                "unknown key `quer`",
            ),
            (
                "and:\n  - query: a\n    or:\n      - query: b",
                "spec.and[0]",
                "cannot be combined",
            ),
            ("local_filter:\n  - [a, \"=\", 1]", "spec", "no query key"),
            (
                "without:\n  - query: a",
                "spec",
                "needs at least 2 operands",
            ),
            ("and:\n  query: a", "spec", "takes a list of nodes"),
            (
                "or:\n  - query: a\n  - and: []",
                "spec.or[1]",
                "needs at least 1 operand",
            ),
            ("query-ref: nope", "spec", "declares no named fences"),
            ("query: a\nlimit: 0", "spec.limit", "positive whole number"),
            (
                "query: a\nlocal_filter: []",
                "spec.local_filter",
                "empty; drop the key",
            ),
            (
                "query: a\norder_by:\n  updated: desc",
                "order_by",
                "not a mapping",
            ),
            (
                "query: a\norder_by:\n  - updated: desc\n    summary: asc",
                "order_by[0]",
                "one column per list entry",
            ),
            (
                "query: a\norder_by:\n  - updated: sideways",
                "order_by[0]",
                "unknown sort direction",
            ),
            ("- query: a", "spec", "expected a mapping"),
        ];
        for (yaml, want_path, want_message) in cases {
            let err = parse_spec(yaml).unwrap_err();
            let ParseError::Spec { path, message } = &err else {
                panic!("expected a spec error for {yaml:?}, got {err}");
            };
            assert_eq!(path, want_path, "path for {yaml:?}");
            assert!(
                message.contains(want_message),
                "message for {yaml:?}: {message}"
            );
        }
    }

    #[test]
    fn an_unresolved_ref_lists_what_is_available() {
        let src = "```yaml\nquery-ref: typo\n```\n\n```jql mine\nx\n```\n";
        let err = parse(src).unwrap_err().to_string();
        assert!(
            err.contains("`typo`") && err.contains("available: mine"),
            "{err}"
        );
    }

    #[test]
    fn a_broken_spec_fence_is_reported_as_yaml_not_as_structure() {
        let err = parse_spec("or:\n  - query: [unclosed").unwrap_err();
        assert!(matches!(err, ParseError::Yaml(_)), "{err}");
    }

    #[test]
    fn language_is_checked_against_the_adapter_not_at_parse_time() {
        let src = "```yaml\nquery-ref: mine\n```\n\n```sql mine\nselect 1\n```\n";
        let q = parse(src).unwrap();
        assert!(
            check_languages(&q, "SQL").is_ok(),
            "matching is case-insensitive"
        );
        let err = check_languages(&q, "jql").unwrap_err();
        assert_eq!(
            err.to_string(),
            "fence `mine` declares query language `sql`, but this view's adapter speaks `jql`"
        );
    }

    #[test]
    fn an_inline_query_inherits_the_adapter_language() {
        let q = parse_spec("query: whatever").unwrap();
        assert!(check_languages(&q, "jql").is_ok());
    }

    #[test]
    fn the_default_template_is_a_single_branch_pass_through() {
        let q = parse(&default_template("jql")).unwrap();
        let NodeKind::And(ops) = &q.root.kind else {
            panic!("expected and");
        };
        assert_eq!(ops.len(), 1, "a pass-through has exactly one branch");
        assert_eq!(fetch(&ops[0]).language.as_deref(), Some("jql"));
        assert!(q.order_by.is_empty(), "must not impose an order of its own");
    }
}
