//! Postgres side of the DB-script editor's table completions.
//!
//! The mechanism — the trailing `-- table completions: …` comment, its
//! append/strip round trip and the token substitution at execute time —
//! lives in
//! [`not_yet_done_sql_core::script_completions`]. All that is
//! Postgres-specific is *what* a table is called here: schema-qualified,
//! so a token is `tt_<schema>__<table>` and expands to
//! `"<schema>"."<table>"`.

use not_yet_done_sql_core::script_completions::{Completion, qualified_table};

/// Turn the `(schema, table)` pairs the catalogue query returns into
/// editor completions, keeping the catalogue's order (sorted by schema,
/// then table) so the generated line is stable across opens.
pub fn completions_for_tables(tables: &[(String, String)]) -> Vec<Completion> {
    tables
        .iter()
        .map(|(schema, table)| qualified_table(&[schema, table]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_sql_core::script_completions::{build_completions_line, substitute_tokens};

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(s, t)| (s.to_string(), t.to_string()))
            .collect()
    }

    #[test]
    fn tokens_are_schema_qualified() {
        let completions = completions_for_tables(&pairs(&[("public", "users")]));
        assert_eq!(completions[0].token, "tt_public__users");
        assert_eq!(completions[0].replacement, "\"public\".\"users\"");
    }

    #[test]
    fn the_line_lists_every_table_in_catalogue_order() {
        let completions = completions_for_tables(&pairs(&[("audit", "log"), ("public", "users")]));
        assert_eq!(
            build_completions_line(&completions).unwrap(),
            "-- table completions: tt_audit__log, tt_public__users"
        );
    }

    /// End-to-end for the shape this adapter emits: what the editor line
    /// offers is exactly what execute time resolves.
    #[test]
    fn a_copied_token_resolves_on_execute() {
        let completions = completions_for_tables(&pairs(&[("public", "users")]));
        let out = substitute_tokens("SELECT * FROM tt_public__users;", &completions);
        assert_eq!(out, "SELECT * FROM \"public\".\"users\";");
    }
}
