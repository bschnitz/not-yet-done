//! Pure-string predicates for inspecting a SQL script body without
//! actually parsing it. Every SQL backend needs the same
//! multi-statement detector and SELECT/WITH sniffer, so this lives
//! here rather than in one adapter.
//!
//! Everything here is byte-level scanning, no allocation beyond
//! `to_ascii_lowercase` for keyword comparison. The walkers
//! understand:
//!
//! - single-quoted strings (with doubled `''` escapes)
//! - double-quoted identifiers
//! - `--` line comments
//! - `/* … */` block comments (not nested)
//!
//! Anything outside those contexts is treated as ordinary SQL token
//! soup.

/// True when `query` (after leading whitespace + comments) starts
/// with `SELECT` or `WITH` as a keyword (followed by a word boundary,
/// not part of a longer identifier like `selectish`).
pub fn looks_like_select_or_with(query: &str) -> bool {
    let stripped = strip_leading_sql_noise(query);
    let lower = stripped.to_ascii_lowercase();
    let is_word_boundary = |s: &str, kw: &str| {
        s.len() == kw.len()
            || s.as_bytes()
                .get(kw.len())
                .map(|b| b.is_ascii_whitespace() || *b == b'(' || *b == b'/' || *b == b'-')
                .unwrap_or(false)
    };
    (lower.starts_with("select") && is_word_boundary(&lower, "select"))
        || (lower.starts_with("with") && is_word_boundary(&lower, "with"))
}

/// True when the body has at least one `;` outside of string/identifier
/// literals or `--` / `/* */` comments. The caller is expected to have
/// already trimmed a single trailing semicolon — a final `;` does not
/// count as a statement separator.
pub fn has_multiple_statements(query: &str) -> bool {
    let bytes = query.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if bytes.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b';' => return true,
            _ => i += 1,
        }
    }
    false
}

/// Strip leading whitespace and any number of `--` / `/* */` comments
/// so a first-keyword check sees the actual keyword.
pub fn strip_leading_sql_noise(query: &str) -> &str {
    let mut rest = query;
    loop {
        let trimmed = rest.trim_start();
        if let Some(after_line) = trimmed.strip_prefix("--") {
            rest = after_line.split_once('\n').map(|x| x.1).unwrap_or("");
            continue;
        }
        if let Some(after_block) = trimmed.strip_prefix("/*") {
            rest = after_block.split_once("*/").map(|x| x.1).unwrap_or("");
            continue;
        }
        return trimmed;
    }
}

/// Split a multi-statement script into `(prelude, last)` at the last
/// top-level `;`. Returns `None` if there is no separator (single
/// statement). Splitter respects string/identifier/comment contexts.
/// A single trailing `;` is trimmed before splitting, so
/// `"A; B;"` → `Some(("A", "B"))`.
///
/// The `last` segment is trimmed; the `prelude` keeps its trailing
/// `;` so it can be concatenated and sent verbatim.
pub fn split_trailing_statement(query: &str) -> Option<(&str, &str)> {
    let trimmed = query.trim_end().trim_end_matches(';');
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    let mut last_semi: Option<usize> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if bytes.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b';' => {
                last_semi = Some(i);
                i += 1;
            }
            _ => i += 1,
        }
    }
    last_semi.map(|idx| {
        let prelude = &trimmed[..=idx];
        let last = trimmed[idx + 1..].trim();
        (prelude, last)
    })
}

/// Wrap a single-statement `SELECT`/`WITH` into a derived table with
/// `LIMIT`/`OFFSET`, so a query the user wrote without pagination can be
/// paged anyway. Returns `None` when the body must be executed verbatim:
/// DML/DDL (wrapping would change what it does) or a multi-statement
/// script (only the trailing statement could be wrapped, and the prelude
/// would have to run separately).
///
/// `limit + 1` rows are requested so the caller can tell "last page" from
/// "more to come" without a second round trip; trimming the extra row is
/// the caller's job. `alias` names the derived table — both dialects
/// require one, and the name only has to avoid colliding with the query's
/// own aliases.
pub fn wrap_for_pagination(query: &str, limit: u32, offset: u32, alias: &str) -> Option<String> {
    let trimmed = query.trim().trim_end_matches(';').trim();
    if !looks_like_select_or_with(trimmed) {
        return None;
    }
    if has_multiple_statements(trimmed) {
        return None;
    }
    Some(format!(
        "SELECT * FROM ({}) AS {} LIMIT {} OFFSET {}",
        trimmed,
        alias,
        limit.saturating_add(1),
        offset,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_for_pagination_wraps_a_single_select() {
        assert_eq!(
            wrap_for_pagination("SELECT * FROM t;", 10, 20, "_nyd").as_deref(),
            Some("SELECT * FROM (SELECT * FROM t) AS _nyd LIMIT 11 OFFSET 20")
        );
    }

    #[test]
    fn wrap_for_pagination_declines_what_it_must_not_touch() {
        // DML: wrapping would turn a write into a read.
        assert!(wrap_for_pagination("UPDATE t SET a = 1", 10, 0, "_nyd").is_none());
        // Multi-statement: the prelude has to run on its own.
        assert!(
            wrap_for_pagination("CREATE TEMP TABLE x; SELECT * FROM x", 10, 0, "_nyd").is_none()
        );
    }

    #[test]
    fn wrap_for_pagination_saturates_at_the_limit_ceiling() {
        // `limit + 1` must not wrap around into a tiny page.
        let sql = wrap_for_pagination("SELECT 1", u32::MAX, 0, "_nyd").expect("wrapped");
        assert!(
            sql.ends_with(&format!("LIMIT {} OFFSET 0", u32::MAX)),
            "{sql}"
        );
    }

    #[test]
    fn recognizes_select_and_with() {
        assert!(looks_like_select_or_with("SELECT 1"));
        assert!(looks_like_select_or_with("select 1"));
        assert!(looks_like_select_or_with(
            "WITH x AS (SELECT 1) SELECT * FROM x"
        ));
        assert!(looks_like_select_or_with(
            "with x as (select 1) select * from x"
        ));
    }

    #[test]
    fn rejects_non_select() {
        assert!(!looks_like_select_or_with("UPDATE t SET x = 1"));
        assert!(!looks_like_select_or_with("INSERT INTO t VALUES (1)"));
        assert!(!looks_like_select_or_with("DELETE FROM t"));
        assert!(!looks_like_select_or_with("CREATE TABLE x ()"));
    }

    #[test]
    fn word_boundary_rejects_selectively_named_identifiers() {
        assert!(!looks_like_select_or_with("selectish 1"));
        assert!(!looks_like_select_or_with("within scope"));
    }

    #[test]
    fn leading_comment_and_whitespace_still_detects_select() {
        assert!(looks_like_select_or_with(
            "-- explain\n  /* why */  SELECT 1"
        ));
    }

    #[test]
    fn multi_statement_detects_top_level_semicolon() {
        assert!(has_multiple_statements("SELECT 1; SELECT 2"));
        assert!(has_multiple_statements("UPDATE t SET x = 1; SELECT 1"));
    }

    #[test]
    fn multi_statement_ignores_semicolon_in_string() {
        assert!(!has_multiple_statements("SELECT ';' FROM t"));
    }

    #[test]
    fn multi_statement_ignores_semicolon_in_line_comment() {
        assert!(!has_multiple_statements(
            "SELECT 1 -- trailing ; comment\nFROM t"
        ));
    }

    #[test]
    fn multi_statement_ignores_semicolon_in_block_comment() {
        assert!(!has_multiple_statements(
            "SELECT 1 /* split ; here */ FROM t"
        ));
    }

    #[test]
    fn multi_statement_ignores_semicolon_in_identifier() {
        assert!(!has_multiple_statements(r#"SELECT * FROM "weird;name""#));
    }

    #[test]
    fn split_trailing_returns_none_for_single_statement() {
        assert!(split_trailing_statement("SELECT 1").is_none());
        assert!(split_trailing_statement("SELECT 1;").is_none());
        assert!(split_trailing_statement("  SELECT 1  ").is_none());
    }

    #[test]
    fn split_trailing_two_statements() {
        let (pre, last) = split_trailing_statement("SET x = 1; SELECT 1").unwrap();
        assert_eq!(pre, "SET x = 1;");
        assert_eq!(last, "SELECT 1");
    }

    #[test]
    fn split_trailing_with_trailing_semicolon() {
        let (pre, last) = split_trailing_statement("SET x = 1; SELECT 1;").unwrap();
        assert_eq!(pre, "SET x = 1;");
        assert_eq!(last, "SELECT 1");
    }

    #[test]
    fn split_trailing_ignores_semicolon_in_string() {
        // Only the real separator counts; the `;` inside the literal
        // is invisible to the splitter.
        let (pre, last) = split_trailing_statement("SET msg = ';'; SELECT 1").unwrap();
        assert_eq!(pre, "SET msg = ';';");
        assert_eq!(last, "SELECT 1");
    }

    #[test]
    fn split_trailing_three_statements_takes_last() {
        let (pre, last) = split_trailing_statement("SET a = 1; SET b = 2; SELECT a + b").unwrap();
        assert_eq!(pre, "SET a = 1; SET b = 2;");
        assert_eq!(last, "SELECT a + b");
    }
}
