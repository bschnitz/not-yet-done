//! Identifier quoting.

/// Wrap a SQL identifier in `"…"`, doubling any embedded `"`.
///
/// Adapters have to splice schema/table/column names directly into the
/// SQL text (`SELECT col` / `FROM schema.table`) because drivers only
/// parametrise *values*; quoting is the standard mitigation.
///
/// Double quotes are the SQL-standard delimiter and are understood by
/// both Postgres and SQLite, so one implementation serves both. (SQLite
/// additionally accepts `[x]` and `` `x` `` for MSSQL/MySQL
/// compatibility — we never need to emit those, only to tolerate them
/// in user-written scripts, which is the parser's business, not ours.)
pub fn quote_ident(name: &str) -> String {
    let escaped = name.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_plain_identifier() {
        assert_eq!(quote_ident("users"), "\"users\"");
    }

    #[test]
    fn doubles_embedded_quote() {
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn keeps_case_and_spaces_verbatim() {
        assert_eq!(quote_ident("Mixed Case"), "\"Mixed Case\"");
    }
}
