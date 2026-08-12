//! Reading and re-writing the statement that *defines* a database view.
//!
//! A view is the one catalogue object whose whole content is SQL text, so
//! it can be edited the same way a stored script is: hand the user the
//! `CREATE VIEW …` statement the backend has, take the edited text back,
//! and replace the object with it. Everything in this module is pure
//! string work — the actual replacement is dialect-specific (SQLite has
//! to `DROP` first, Postgres can `CREATE OR REPLACE`) and stays in the
//! adapters.
//!
//! What has to be shared is the *checking*, because a buffer that reaches
//! the backend unchecked can do far more than edit a view: a second
//! statement after the `CREATE`, or a `CREATE` that names a different
//! object, would silently touch something the user never selected. Both
//! adapters need the same guard, so it lives here.

use not_yet_done_content::script_buffer::QUERY_MARKER;

use crate::sql_shape::{has_multiple_statements, strip_leading_sql_noise};

/// A `CREATE VIEW` statement the user wrote, as far as it has to be
/// understood to be safe to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCreateView {
    /// Schema/database qualifier written before the name (`main.v`,
    /// `public.v`), if any. SQLite adapters can ignore it — the file is
    /// the namespace; Postgres needs it to tell `public.v` from `other.v`.
    pub qualifier: Option<String>,
    /// The view's name, unquoted.
    pub name: String,
    /// The statement itself: trimmed, without a trailing `;`, ready to be
    /// executed as a single statement.
    pub sql: String,
}

/// Parse the single `CREATE VIEW` statement in an edited buffer body.
///
/// Accepts the union of both dialects' spellings — `CREATE [OR REPLACE]
/// [TEMP|TEMPORARY] VIEW [IF NOT EXISTS] [<qualifier>.]<name>` — because
/// rejecting a form the backend would happily accept only teaches the user
/// to distrust the editor.
///
/// The `Err` string is written for the user: it lands in the editor's
/// error banner, so it says what is wrong with *their* text rather than
/// naming a parser state.
pub fn parse_create_view(body: &str) -> Result<ParsedCreateView, String> {
    let sql = body.trim();
    // One trailing `;` is punctuation, not a statement separator — trim it
    // before the multi-statement check so a normal-looking statement is
    // not reported as two.
    let sql = sql.strip_suffix(';').unwrap_or(sql).trim_end();
    if sql.is_empty() {
        return Err("nothing to save: the definition below the marker is empty".into());
    }
    if has_multiple_statements(sql) {
        return Err(
            "only the CREATE VIEW statement itself belongs here — a second statement \
             would change something else. Use a DB script for that."
                .into(),
        );
    }

    let mut rest = strip_leading_sql_noise(sql);
    rest = take_keyword(rest, "create").ok_or_else(|| {
        format!(
            "this is not a view definition: it has to start with CREATE VIEW, not with `{}`",
            first_word(rest)
        )
    })?;
    // `OR REPLACE` (Postgres) and `TEMP`/`TEMPORARY` are optional noise
    // here: both are the backend's business, not ours.
    if let Some(after_or) = take_keyword(rest, "or") {
        rest = take_keyword(after_or, "replace")
            .ok_or_else(|| "CREATE OR … : expected REPLACE after OR".to_string())?;
    }
    for temp in ["temporary", "temp"] {
        if let Some(after) = take_keyword(rest, temp) {
            rest = after;
            break;
        }
    }
    rest = take_keyword(rest, "view").ok_or_else(|| {
        format!(
            "only a view can be edited here — CREATE {} is a different object",
            first_word(rest).to_uppercase()
        )
    })?;
    if let Some(after_if) = take_keyword(rest, "if") {
        let after_not = take_keyword(after_if, "not")
            .ok_or_else(|| "CREATE VIEW IF … : expected NOT EXISTS".to_string())?;
        rest = take_keyword(after_not, "exists")
            .ok_or_else(|| "CREATE VIEW IF NOT … : expected EXISTS".to_string())?;
    }

    let missing_name =
        || "CREATE VIEW without a name — what should the view be called?".to_string();
    let (first, first_quoted, rest) = take_identifier(rest).ok_or_else(missing_name)?;
    let (qualifier, name, name_quoted) = match rest.strip_prefix('.') {
        Some(after_dot) => {
            let (second, second_quoted, _) = take_identifier(strip_leading_sql_noise(after_dot))
                .ok_or_else(|| format!("`{first}.` is missing the name after the qualifier"))?;
            (Some(first), second, second_quoted)
        }
        None => (None, first, first_quoted),
    };
    // `CREATE VIEW AS SELECT …` parses as a view named `as`, which is not
    // what anyone meant. Only the bare spelling is refused — a view
    // genuinely called `as` has to be quoted, and then it is honoured.
    if !name_quoted && name.eq_ignore_ascii_case("as") {
        return Err(missing_name());
    }

    Ok(ParsedCreateView {
        qualifier,
        name,
        sql: sql.to_string(),
    })
}

/// True when the name the user wrote addresses the object they opened.
///
/// Compared ASCII-case-insensitively: SQLite folds every identifier, and
/// Postgres folds every *unquoted* one, so a difference in case alone is
/// far more likely a typo than an attempt to address a second view. A
/// difference in anything else is a rename, which the caller rejects.
pub fn same_object_name(written: &str, expected: &str) -> bool {
    written.eq_ignore_ascii_case(expected)
}

/// Whether two `CREATE VIEW` statements say the same thing, as far as a
/// view editor has to care.
///
/// Only trailing punctuation and whitespace are ignored: the buffer ends in
/// a `;` that neither backend stores, so comparing verbatim would report
/// every save as a change and replace the view for nothing. Everything else
/// — including reformatting — counts as an edit, because it is one.
pub fn same_definition(a: &str, b: &str) -> bool {
    normalize_definition(a) == normalize_definition(b)
}

fn normalize_definition(sql: &str) -> &str {
    sql.trim().trim_end_matches(';').trim_end()
}

/// The buffer a view editor opens with: a header explaining what saving
/// does, the [`QUERY_MARKER`], then the definition.
///
/// Same two-region protocol as every script buffer, so the notes a user
/// leaves above the marker survive and are never executed. `replace_note`
/// is the one dialect-specific part — how the backend swaps the definition
/// is exactly what differs between SQLite and Postgres, and the user
/// deserves to know which one they are looking at. It may span several
/// lines; each becomes its own comment, so a caller writes prose and not
/// comment syntax.
pub fn edit_buffer(name: &str, definition: &str, replace_note: &str) -> String {
    let body = definition.trim_end().trim_end_matches(';');
    let note: String = replace_note
        .lines()
        .map(|line| format!("-- {}\n", line.trim()))
        .collect();
    format!(
        "-- View {name}. The statement below is its whole definition.\n\
         {note}\
         -- The name has to stay {name}: a renamed view would leave the old\n\
         -- one in place, so saving under a different name is rejected.\n\
         -- Lines above the marker are notes — they are never executed.\n\
         \n\
         {QUERY_MARKER}\n\
         \n\
         {body};\n"
    )
}

/// Strip one leading keyword, case-insensitively, returning the remainder
/// with leading whitespace and comments removed. `None` when `s` does not
/// start with the keyword as a whole word.
fn take_keyword<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    if s.len() < keyword.len() || !s[..keyword.len()].eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &s[keyword.len()..];
    // `VIEWS` is not `VIEW`, and `CREATEX` is not `CREATE`.
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(strip_leading_sql_noise(rest))
}

/// Take one identifier off the front: quoted (`"x"`, `` `x` ``, `[x]`) or
/// bare. Returns the unquoted name, whether it was quoted, and the
/// untrimmed remainder — untrimmed so the caller can see a following `.`
/// without whitespace ambiguity.
fn take_identifier(s: &str) -> Option<(String, bool, &str)> {
    let mut chars = s.char_indices();
    let (_, first) = chars.next()?;
    let closing = match first {
        '"' => Some('"'),
        '`' => Some('`'),
        '[' => Some(']'),
        _ => None,
    };
    if let Some(closing) = closing {
        let mut name = String::new();
        let mut rest_at = s.len();
        while let Some((idx, c)) = chars.next() {
            if c == closing {
                // A doubled delimiter is an escaped one (`"we""ird"`);
                // only `"` uses that rule, which is also the only
                // delimiter we ever emit.
                if closing == '"' && s[idx + c.len_utf8()..].starts_with('"') {
                    name.push('"');
                    chars.next();
                    continue;
                }
                rest_at = idx + c.len_utf8();
                break;
            }
            name.push(c);
        }
        if name.is_empty() {
            return None;
        }
        return Some((name, true, &s[rest_at..]));
    }

    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    Some((s[..end].to_string(), false, &s[end..]))
}

/// The first whitespace-delimited word of `s`, for error messages.
fn first_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::script_buffer::parse_query_area;

    fn parse(sql: &str) -> ParsedCreateView {
        parse_create_view(sql).expect("should parse")
    }

    #[test]
    fn parses_a_plain_create_view() {
        let v = parse("CREATE VIEW v_balance AS SELECT 1;");
        assert_eq!(v.name, "v_balance");
        assert_eq!(v.qualifier, None);
        // The trailing `;` is trimmed so the statement can be prepared.
        assert_eq!(v.sql, "CREATE VIEW v_balance AS SELECT 1");
    }

    #[test]
    fn parses_the_quoted_and_qualified_spellings() {
        assert_eq!(
            parse(r#"CREATE VIEW "we""ird" AS SELECT 1"#).name,
            "we\"ird"
        );
        assert_eq!(parse("create view `v` as select 1").name, "v");
        assert_eq!(parse("CREATE VIEW [v] AS SELECT 1").name, "v");

        let qualified = parse("CREATE VIEW main.v AS SELECT 1");
        assert_eq!(qualified.qualifier.as_deref(), Some("main"));
        assert_eq!(qualified.name, "v");

        let quoted_qualified = parse(r#"CREATE VIEW "public"."v" AS SELECT 1"#);
        assert_eq!(quoted_qualified.qualifier.as_deref(), Some("public"));
        assert_eq!(quoted_qualified.name, "v");
    }

    /// Both dialects' optional modifiers, and a leading comment — none of
    /// them change what the statement addresses.
    #[test]
    fn tolerates_optional_modifiers_and_leading_comments() {
        assert_eq!(parse("CREATE OR REPLACE VIEW v AS SELECT 1").name, "v");
        assert_eq!(parse("CREATE TEMP VIEW v AS SELECT 1").name, "v");
        assert_eq!(parse("CREATE TEMPORARY VIEW v AS SELECT 1").name, "v");
        assert_eq!(parse("CREATE VIEW IF NOT EXISTS v AS SELECT 1").name, "v");
        assert_eq!(
            parse("-- why\n/* really */\nCREATE VIEW v AS SELECT 1").name,
            "v"
        );
    }

    /// A multi-line body is what a real view looks like; the whole text
    /// (including its internal `--` comments) has to survive verbatim.
    #[test]
    fn keeps_a_multi_line_body_verbatim() {
        let sql = "CREATE VIEW v AS\nSELECT a, -- the a\n       b\n  FROM t\n WHERE a > 0;\n";
        let parsed = parse(sql);
        assert_eq!(parsed.name, "v");
        assert_eq!(parsed.sql, sql.trim().trim_end_matches(';'));
    }

    #[test]
    fn rejects_an_empty_body() {
        let err = parse_create_view("  \n\n ").expect_err("empty");
        assert!(err.contains("empty"), "{err}");
    }

    /// The important guard: a second statement would touch something the
    /// user never selected.
    #[test]
    fn rejects_a_second_statement() {
        let err = parse_create_view("CREATE VIEW v AS SELECT 1; DROP TABLE t;")
            .expect_err("two statements");
        assert!(err.contains("second statement"), "{err}");
    }

    #[test]
    fn a_semicolon_inside_the_body_is_not_a_second_statement() {
        let v = parse("CREATE VIEW v AS SELECT ';' AS semi");
        assert_eq!(v.name, "v");
    }

    #[test]
    fn rejects_anything_that_is_not_a_create_view() {
        for (body, needle) in [
            ("SELECT 1", "CREATE VIEW"),
            ("DROP VIEW v", "CREATE VIEW"),
            ("CREATE TABLE t (a INT)", "different object"),
            ("CREATE VIRTUAL TABLE t USING fts5(a)", "different object"),
            // `VIEWS` is a different word, not an abbreviation of `VIEW`.
            ("CREATE VIEWS v AS SELECT 1", "different object"),
        ] {
            let err = parse_create_view(body).expect_err(body);
            assert!(err.contains(needle), "{body} → {err}");
        }
    }

    /// `CREATE VIEW AS SELECT …` would otherwise parse as a view called
    /// `as` and be reported as a rename of the wrong object.
    #[test]
    fn rejects_a_view_without_a_name() {
        let err = parse_create_view("CREATE VIEW AS SELECT 1").expect_err("no name");
        assert!(err.contains("without a name"), "{err}");
        // Quoted, `as` is an ordinary name and stays one.
        assert_eq!(parse(r#"CREATE VIEW "as" AS SELECT 1"#).name, "as");
    }

    #[test]
    fn name_comparison_folds_case_only() {
        assert!(same_object_name("V_Balance", "v_balance"));
        assert!(!same_object_name("v_balance_2", "v_balance"));
    }

    /// The stored statement has no trailing `;`, the buffer's does — if
    /// that alone counted as an edit, every save would replace the view.
    #[test]
    fn definition_comparison_ignores_only_trailing_punctuation() {
        assert!(same_definition(
            "CREATE VIEW v AS SELECT 1",
            "CREATE VIEW v AS SELECT 1;\n"
        ));
        assert!(!same_definition(
            "CREATE VIEW v AS SELECT 1",
            "CREATE VIEW v AS SELECT 2"
        ));
    }

    #[test]
    fn edit_buffer_round_trips_through_the_marker() {
        let buffer = edit_buffer(
            "v_balance",
            "CREATE VIEW v_balance AS SELECT 1",
            "replaced on save",
        );
        assert!(buffer.contains("replaced on save"));
        let body = parse_query_area(&buffer);
        let parsed = parse_create_view(body).expect("the buffer's own body must parse back");
        assert_eq!(parsed.name, "v_balance");
        assert_eq!(parsed.sql, "CREATE VIEW v_balance AS SELECT 1");
    }

    /// A definition that already ends in `;` must not gain a second one.
    #[test]
    fn edit_buffer_does_not_double_the_semicolon() {
        let buffer = edit_buffer("v", "CREATE VIEW v AS SELECT 1;\n", "note");
        assert!(buffer.ends_with("CREATE VIEW v AS SELECT 1;\n"), "{buffer}");
    }
}
