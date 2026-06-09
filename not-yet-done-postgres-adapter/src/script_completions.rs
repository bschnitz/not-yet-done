//! Table-name completions for the DB-script editor.
//!
//! The Postgres DB-script editor appends a single SQL-comment line at
//! the end of the buffer listing every `(schema, table)` pair as a
//! token of the form `tt_<schema>__<table>` (double-underscore between
//! schema and table, so single underscores inside identifiers stay
//! unambiguous). The user can copy a token into their SQL; at execute
//! time the adapter substitutes each known token with the fully-
//! qualified, double-quoted identifier `"<schema>"."<table>"`.
//!
//! The completion line is purely an editor affordance — it is appended
//! when the buffer is loaded and stripped again before the file is
//! written back to disk, so the on-disk script never contains the
//! generated block. The substitution step is independent of insertion:
//! any `tt_<schema>__<table>` token that matches a real table is
//! replaced at execute time, whether or not the user copied it from
//! our generated line.

use regex::Regex;

/// Marker prefix for the trailing completions line. The line format is
/// `"-- table completions: <token>, <token>, …"` with no other
/// surrounding markers — a script that happens to start an SQL comment
/// with this exact phrase would collide, which we accept as
/// vanishingly unlikely (and any breakage is local to the editor
/// buffer, never the persisted file).
pub const COMPLETIONS_PREFIX: &str = "-- table completions: ";

/// Build the single-line completion comment from a sorted list of
/// `(schema, table)` pairs. Returns `None` when the input is empty so
/// the caller can skip the append step (and the editor doesn't show an
/// orphan header).
pub fn build_completions_line(tables: &[(String, String)]) -> Option<String> {
    if tables.is_empty() {
        return None;
    }
    let tokens: Vec<String> = tables
        .iter()
        .map(|(schema, table)| format!("tt_{schema}__{table}"))
        .collect();
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
    let Some(idx) = lines
        .iter()
        .position(|l| l.starts_with(COMPLETIONS_PREFIX))
    else {
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

/// Replace every `tt_<schema>__<table>` token in `sql` that corresponds
/// to a known `(schema, table)` pair with the double-quoted form
/// `"<schema>"."<table>"`. Tokens that don't match any known table are
/// left untouched — Postgres will then surface them as a normal syntax
/// error on execute, which gives the user a recognisable failure (the
/// literal `tt_*` text appears in the error).
///
/// The match is exact and case-sensitive against the literal identifier
/// strings from `pg_class`. Word-boundary anchors prevent partial
/// substitution: a token `tt_public__user` won't match inside
/// `tt_public__user_orders`.
pub fn substitute_table_tokens(sql: &str, tables: &[(String, String)]) -> String {
    if !sql.contains("tt_") || tables.is_empty() {
        return sql.to_string();
    }
    let mut out = sql.to_string();
    for (schema, table) in tables {
        let pattern = format!(
            r"\btt_{}__{}\b",
            regex::escape(schema),
            regex::escape(table)
        );
        let Ok(re) = Regex::new(&pattern) else {
            continue;
        };
        if !re.is_match(&out) {
            continue;
        }
        let replacement = format!("\"{}\".\"{}\"", schema, table);
        out = re.replace_all(&out, replacement.as_str()).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(s, t)| (s.to_string(), t.to_string())).collect()
    }

    #[test]
    fn build_returns_none_for_empty_input() {
        assert!(build_completions_line(&[]).is_none());
    }

    #[test]
    fn build_joins_tokens_with_comma_and_space() {
        let line = build_completions_line(&pairs(&[("public", "users"), ("public", "orders")]))
            .unwrap();
        assert_eq!(line, "-- table completions: tt_public__users, tt_public__orders");
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
        let line = build_completions_line(&pairs(&[("public", "users")])).unwrap();
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
        let out = substitute_table_tokens(sql, &pairs(&[("public", "users")]));
        assert_eq!(out, "SELECT * FROM \"public\".\"users\";");
    }

    #[test]
    fn substitute_skips_unknown_token() {
        let sql = "SELECT * FROM tt_other__missing;";
        let out = substitute_table_tokens(sql, &pairs(&[("public", "users")]));
        assert_eq!(out, sql);
    }

    #[test]
    fn substitute_handles_single_underscore_in_table_name() {
        let sql = "SELECT * FROM tt_public__user_orders;";
        let out =
            substitute_table_tokens(sql, &pairs(&[("public", "user_orders")]));
        assert_eq!(out, "SELECT * FROM \"public\".\"user_orders\";");
    }

    #[test]
    fn substitute_respects_word_boundary() {
        // `tt_public__user` exists but the SQL has `tt_public__user_orders`
        // with `user_orders` not in the known list. The match should not
        // partially consume the longer identifier.
        let sql = "SELECT tt_public__user, tt_public__user_orders FROM t;";
        let out = substitute_table_tokens(sql, &pairs(&[("public", "user")]));
        assert_eq!(
            out,
            "SELECT \"public\".\"user\", tt_public__user_orders FROM t;"
        );
    }

    #[test]
    fn substitute_handles_case_sensitive_identifiers() {
        let sql = "SELECT * FROM tt_public__MyTable;";
        let out = substitute_table_tokens(sql, &pairs(&[("public", "MyTable")]));
        assert_eq!(out, "SELECT * FROM \"public\".\"MyTable\";");
        // Different-case input does NOT match a different-case table.
        let out2 = substitute_table_tokens(sql, &pairs(&[("public", "mytable")]));
        assert_eq!(out2, sql);
    }

    #[test]
    fn substitute_fast_path_returns_unchanged_when_no_marker() {
        let sql = "SELECT 1;";
        let out = substitute_table_tokens(sql, &pairs(&[("public", "users")]));
        assert_eq!(out, sql);
    }

    #[test]
    fn substitute_skips_when_table_list_empty() {
        let sql = "SELECT * FROM tt_public__users;";
        let out = substitute_table_tokens(sql, &[]);
        assert_eq!(out, sql);
    }

    #[test]
    fn substitute_handles_multiple_distinct_tokens() {
        let sql = "INSERT INTO tt_public__users SELECT * FROM tt_public__staging;";
        let out = substitute_table_tokens(
            sql,
            &pairs(&[("public", "users"), ("public", "staging")]),
        );
        assert_eq!(
            out,
            "INSERT INTO \"public\".\"users\" SELECT * FROM \"public\".\"staging\";"
        );
    }
}
