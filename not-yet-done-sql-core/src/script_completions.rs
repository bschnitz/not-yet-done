//! Table-name completions for the DB-script editor.
//!
//! A SQL script editor is worth little without knowing the names in the
//! database, and the external editor we hand the buffer to knows nothing
//! about our connection. The cheapest bridge that needs no LSP: append a
//! single SQL-comment line listing every table as a short token, let the
//! user copy one into their SQL, and expand the tokens again at execute
//! time.
//!
//! Everything about that mechanism is backend-neutral except *which*
//! names exist and how many levels qualify one — Postgres has
//! `(schema, table)`, a single-file database only has `table`. So an
//! adapter contributes nothing but a list of [`Completion`]s (build them
//! with [`qualified_table`]) and this module does the rest.
//!
//! The completion line is purely an editor affordance: it is appended
//! when the buffer is loaded and stripped again before the file is
//! written back, so the on-disk script never contains the generated
//! block. Substitution is independent of insertion — any token that
//! matches a real table is replaced at execute time, whether or not the
//! user copied it from our generated line.

use std::collections::HashMap;

use crate::ident::quote_ident;

/// Marker prefix for the trailing completions line. The line format is
/// `"-- table completions: <token>, <token>, …"` with no other
/// surrounding markers — a script that happens to start an SQL comment
/// with this exact phrase would collide, which we accept as
/// vanishingly unlikely (and any breakage is local to the editor
/// buffer, never the persisted file).
pub const COMPLETIONS_PREFIX: &str = "-- table completions: ";

/// Prefix every completion token starts with (`tt_` = "table token").
///
/// Shared across adapters on purpose: it is what makes the cheap
/// pre-check in [`may_contain_tokens`] possible, and it keeps the tokens
/// recognisable as ours in an error message when a token *doesn't*
/// resolve. Tokens built by hand must use it too.
pub const TOKEN_PREFIX: &str = "tt_";

/// One editor completion: the token as it appears in the buffer and the
/// SQL text it expands to at execute time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// Must start with [`TOKEN_PREFIX`] and contain only identifier
    /// characters — otherwise it can never be recognised in a query.
    pub token: String,
    pub replacement: String,
}

impl Completion {
    pub fn new(token: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            replacement: replacement.into(),
        }
    }
}

/// Completion for a table addressed by `parts` — `["schema", "table"]`
/// where the backend has a schema level, `["table"]` where it hasn't.
///
/// The token joins the parts with a double underscore
/// (`tt_public__users`) so single underscores inside identifiers stay
/// unambiguous; the replacement is the dotted, individually-quoted form
/// (`"public"."users"`). Quoting goes through [`quote_ident`], so an
/// identifier containing a double quote expands to valid SQL instead of
/// a broken string.
pub fn qualified_table(parts: &[&str]) -> Completion {
    let token = format!("{TOKEN_PREFIX}{}", parts.join("__"));
    let replacement = parts
        .iter()
        .map(|part| quote_ident(part))
        .collect::<Vec<_>>()
        .join(".");
    Completion::new(token, replacement)
}

/// Whether `sql` is worth resolving completions for at all. Adapters use
/// it to skip the catalogue round trip on the overwhelmingly common case
/// of a query with no tokens in it.
pub fn may_contain_tokens(sql: &str) -> bool {
    sql.contains(TOKEN_PREFIX)
}

/// Build the single-line completion comment. Returns `None` when there
/// is nothing to list so the caller can skip the append step (and the
/// editor doesn't show an orphan header).
pub fn build_completions_line(completions: &[Completion]) -> Option<String> {
    if completions.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = completions.iter().map(|c| c.token.as_str()).collect();
    Some(format!("{COMPLETIONS_PREFIX}{}", tokens.join(", ")))
}

/// Append the completions line to a buffer, separated by a single blank
/// line. If `body` already ends in a newline we add one more for the
/// blank-line separator; otherwise we add two. The line ends with a
/// trailing newline so the editor's last line stays cleanly terminated.
pub fn append_completions_line(body: &str, line: &str) -> String {
    let mut out = String::with_capacity(body.len() + line.len() + 4);
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(line);
    out.push('\n');
    out
}

/// Strip the first line that starts with [`COMPLETIONS_PREFIX`] from
/// `text` and return the remainder. If no such line exists, the input
/// is returned unchanged (allocating a copy — callers don't depend on
/// the result being borrowed). Adjacent blank lines that were inserted
/// by [`append_completions_line`] are removed as well so the round-trip
/// is a no-op on persisted disk content.
pub fn strip_completions_line(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let Some(idx) = lines.iter().position(|l| l.starts_with(COMPLETIONS_PREFIX)) else {
        return text.to_string();
    };
    lines.remove(idx);
    // Pull the trailing blank that the appender inserted just before
    // the line we just removed. We only collapse one blank — anything
    // beyond that is user content we have no business touching.
    if idx > 0 && lines.get(idx - 1).is_some_and(|l| l.trim().is_empty()) {
        lines.remove(idx - 1);
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Replace every known token in `sql` with its replacement. Tokens that
/// match no completion are left untouched — the backend then surfaces
/// them as a normal syntax error, which gives the user a recognisable
/// failure (the literal `tt_*` text appears in the error).
///
/// Matching walks identifier runs rather than searching per token, so it
/// is one pass over the query regardless of how many tables the database
/// has, and partial substitution is impossible by construction: a token
/// `tt_public__user` cannot match inside `tt_public__user_orders`. The
/// comparison is exact and case-sensitive — that is what the catalogue
/// stores, and both backends treat quoted identifiers case-sensitively.
pub fn substitute_tokens(sql: &str, completions: &[Completion]) -> String {
    if completions.is_empty() || !may_contain_tokens(sql) {
        return sql.to_string();
    }
    let by_token: HashMap<&str, &str> = completions
        .iter()
        .map(|c| (c.token.as_str(), c.replacement.as_str()))
        .collect();

    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while !rest.is_empty() {
        let ident_len = rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
        if ident_len > 0 {
            let (run, tail) = rest.split_at(ident_len);
            out.push_str(by_token.get(run).copied().unwrap_or(run));
            rest = tail;
        } else {
            // Not an identifier char — copy it verbatim. Splitting by
            // char length keeps multi-byte content intact.
            let ch = rest.chars().next().expect("rest is non-empty");
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

/// Characters an unquoted SQL identifier can consist of. Deliberately
/// includes non-ASCII letters (both backends accept them) and excludes
/// `$`, so a `tt_*` token followed by `$` still resolves.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(items: &[&[&str]]) -> Vec<Completion> {
        items.iter().map(|parts| qualified_table(parts)).collect()
    }

    #[test]
    fn qualified_table_builds_token_and_quoted_replacement() {
        let two = qualified_table(&["public", "users"]);
        assert_eq!(two.token, "tt_public__users");
        assert_eq!(two.replacement, "\"public\".\"users\"");

        let one = qualified_table(&["notes"]);
        assert_eq!(one.token, "tt_notes");
        assert_eq!(one.replacement, "\"notes\"");
    }

    #[test]
    fn qualified_table_escapes_embedded_quotes() {
        let c = qualified_table(&["public", "we\"ird"]);
        assert_eq!(c.replacement, "\"public\".\"we\"\"ird\"");
    }

    #[test]
    fn build_returns_none_for_empty_input() {
        assert!(build_completions_line(&[]).is_none());
    }

    #[test]
    fn build_joins_tokens_with_comma_and_space() {
        let line = build_completions_line(&tables(&[&["public", "users"], &["public", "orders"]]))
            .unwrap();
        assert_eq!(
            line,
            "-- table completions: tt_public__users, tt_public__orders"
        );
    }

    #[test]
    fn append_adds_blank_line_separator() {
        let body = "SELECT 1;\n";
        let appended = append_completions_line(body, "-- table completions: tt_a__b");
        assert_eq!(appended, "SELECT 1;\n\n-- table completions: tt_a__b\n");
    }

    #[test]
    fn append_handles_body_without_trailing_newline() {
        let body = "SELECT 1;";
        let appended = append_completions_line(body, "-- table completions: tt_a__b");
        assert_eq!(appended, "SELECT 1;\n\n-- table completions: tt_a__b\n");
    }

    #[test]
    fn strip_round_trips_appended_block() {
        let body = "SELECT 1;\n";
        let line = build_completions_line(&tables(&[&["public", "users"]])).unwrap();
        let appended = append_completions_line(body, &line);
        assert_eq!(strip_completions_line(&appended), body);
    }

    #[test]
    fn strip_is_idempotent_when_no_marker() {
        let text = "SELECT *\nFROM users;\n";
        assert_eq!(strip_completions_line(text), text);
    }

    #[test]
    fn strip_only_removes_first_matching_line() {
        // Defensive: a stray user-authored line with the same prefix
        // should not corrupt later content. We only strip one line.
        let text = "-- table completions: tt_a__b\nSELECT 1;\n-- table completions: tt_c__d\n";
        let stripped = strip_completions_line(text);
        assert_eq!(stripped, "SELECT 1;\n-- table completions: tt_c__d\n");
    }

    #[test]
    fn substitute_replaces_known_token() {
        let sql = "SELECT * FROM tt_public__users;";
        let out = substitute_tokens(sql, &tables(&[&["public", "users"]]));
        assert_eq!(out, "SELECT * FROM \"public\".\"users\";");
    }

    #[test]
    fn substitute_replaces_a_single_level_token() {
        let sql = "SELECT * FROM tt_notes ORDER BY id;";
        let out = substitute_tokens(sql, &tables(&[&["notes"]]));
        assert_eq!(out, "SELECT * FROM \"notes\" ORDER BY id;");
    }

    #[test]
    fn substitute_skips_unknown_token() {
        let sql = "SELECT * FROM tt_other__missing;";
        let out = substitute_tokens(sql, &tables(&[&["public", "users"]]));
        assert_eq!(out, sql);
    }

    #[test]
    fn substitute_handles_single_underscore_in_table_name() {
        let sql = "SELECT * FROM tt_public__user_orders;";
        let out = substitute_tokens(sql, &tables(&[&["public", "user_orders"]]));
        assert_eq!(out, "SELECT * FROM \"public\".\"user_orders\";");
    }

    #[test]
    fn substitute_respects_identifier_boundaries() {
        // `tt_public__user` exists but the SQL has `tt_public__user_orders`
        // with `user_orders` not in the known list. The match must not
        // partially consume the longer identifier.
        let sql = "SELECT tt_public__user, tt_public__user_orders FROM t;";
        let out = substitute_tokens(sql, &tables(&[&["public", "user"]]));
        assert_eq!(
            out,
            "SELECT \"public\".\"user\", tt_public__user_orders FROM t;"
        );
    }

    #[test]
    fn substitute_ignores_a_token_glued_to_a_longer_identifier() {
        let sql = "SELECT xtt_notes FROM t;";
        let out = substitute_tokens(sql, &tables(&[&["notes"]]));
        assert_eq!(out, sql, "the run does not start with the token prefix");
    }

    #[test]
    fn substitute_handles_case_sensitive_identifiers() {
        let sql = "SELECT * FROM tt_public__MyTable;";
        let out = substitute_tokens(sql, &tables(&[&["public", "MyTable"]]));
        assert_eq!(out, "SELECT * FROM \"public\".\"MyTable\";");
        // Different-case input does NOT match a different-case table.
        let out2 = substitute_tokens(sql, &tables(&[&["public", "mytable"]]));
        assert_eq!(out2, sql);
    }

    #[test]
    fn substitute_keeps_non_ascii_content_intact() {
        let sql = "SELECT 'kaffee ☕' FROM tt_notizen;";
        let out = substitute_tokens(sql, &tables(&[&["notizen"]]));
        assert_eq!(out, "SELECT 'kaffee ☕' FROM \"notizen\";");
    }

    #[test]
    fn substitute_fast_path_returns_unchanged_when_no_marker() {
        let sql = "SELECT 1;";
        assert!(!may_contain_tokens(sql));
        let out = substitute_tokens(sql, &tables(&[&["public", "users"]]));
        assert_eq!(out, sql);
    }

    #[test]
    fn substitute_skips_when_completion_list_empty() {
        let sql = "SELECT * FROM tt_public__users;";
        let out = substitute_tokens(sql, &[]);
        assert_eq!(out, sql);
    }

    #[test]
    fn substitute_handles_multiple_distinct_tokens() {
        let sql = "INSERT INTO tt_public__users SELECT * FROM tt_public__staging;";
        let out = substitute_tokens(
            sql,
            &tables(&[&["public", "users"], &["public", "staging"]]),
        );
        assert_eq!(
            out,
            "INSERT INTO \"public\".\"users\" SELECT * FROM \"public\".\"staging\";"
        );
    }
}
