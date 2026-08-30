//! Compiled regexes for the [`Matches`](crate::Operator::Matches) operator.
//!
//! An expression is evaluated once per row, but its patterns are fixed the
//! moment the config is loaded. Compiling per row would put a regex compiler
//! in a render path; this module compiles each distinct pattern once and hands
//! out shared handles afterwards.
//!
//! [`precompile`] is the other half: the config loader walks a freshly parsed
//! expression through it, so an invalid pattern is a load-time error naming the
//! rule instead of a rule that silently never matches.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use regex::Regex;

use crate::{FilterExpr, Literal, Operator, Rhs};

/// Keyed by pattern text: two rules writing the same regex share one automaton,
/// and a pattern that failed to compile is remembered as a failure so a bad
/// config does not re-enter the compiler on every row.
type Cache = HashMap<String, Result<Arc<Regex>, String>>;

static CACHE: LazyLock<RwLock<Cache>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// The compiled form of `pattern`, compiling it on first sight.
///
/// `Err` carries the compiler's message, ready to be shown to the user.
pub fn compiled(pattern: &str) -> Result<Arc<Regex>, String> {
    if let Ok(cache) = CACHE.read()
        && let Some(hit) = cache.get(pattern)
    {
        return hit.clone();
    }
    let result = Regex::new(pattern).map(Arc::new).map_err(|e| e.to_string());
    if let Ok(mut cache) = CACHE.write() {
        cache.insert(pattern.to_string(), result.clone());
    }
    result
}

/// Compile every `matches` pattern in `expr`, reporting the first bad one.
///
/// Call this once where the expression is read, not where it is evaluated: the
/// point is that a typo in a regex surfaces while the config is being loaded,
/// with a place to name it, rather than as a rule that quietly matches nothing.
pub fn precompile(expr: &FilterExpr) -> Result<(), String> {
    match expr {
        FilterExpr::And(children) | FilterExpr::Or(children) => {
            children.iter().try_for_each(precompile)
        }
        FilterExpr::Not(inner) => precompile(inner),
        FilterExpr::Leaf(leaf) => {
            if leaf.op != Operator::Matches {
                return Ok(());
            }
            match &leaf.rhs {
                Rhs::Lit(Literal::String(pattern)) => compiled(pattern)
                    .map(|_| ())
                    .map_err(|e| format!("invalid regex {pattern:?}: {e}")),
                _ => Err("`matches` needs a regex string on the right-hand side".to_string()),
            }
        }
        FilterExpr::Custom { .. } => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_compiles_once_and_is_handed_out_again() {
        let a = compiled(r"^ab+c$").unwrap();
        let b = compiled(r"^ab+c$").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(a.is_match("abbc"));
    }

    #[test]
    fn a_broken_pattern_reports_instead_of_panicking() {
        let err = compiled("(unclosed").unwrap_err();
        assert!(!err.is_empty());
        // Remembered as a failure, so a bad config does not re-enter the
        // compiler on every row.
        assert!(compiled("(unclosed").is_err());
    }

    #[test]
    fn precompile_finds_a_bad_pattern_anywhere_in_the_tree() {
        let good: FilterExpr = serde_yaml::from_str(
            "or:\n  - [status, matches, '^Done$']\n  - [key, matches, '^ABC-\\d+$']",
        )
        .unwrap();
        assert!(precompile(&good).is_ok());

        let bad: FilterExpr =
            serde_yaml::from_str("and:\n  - [status, '=', Done]\n  - [key, matches, '(']").unwrap();
        let err = precompile(&bad).unwrap_err();
        assert!(err.contains("invalid regex"), "{err}");
    }
}
