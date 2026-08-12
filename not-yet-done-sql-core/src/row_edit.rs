//! Editing one data row through a text editor.
//!
//! A row has no text form of its own the way a view definition does, so
//! the editor is handed a *rendering* of it: one YAML mapping,
//! `column: value` per cell. What comes back is compared against the
//! rendering that was handed out, and the columns that actually differ
//! become the `SET` list of a single `UPDATE`. The row is addressed by
//! the key columns the adapter determined (a primary key, or SQLite's
//! `rowid`) using the values it had *before* the edit — so changing a key
//! column is an ordinary edit and not a lost row.
//!
//! Why YAML and not `key = value` lines: values contain newlines, leading
//! spaces and quotes, and `NULL` has to be distinguishable from the text
//! `"null"`. YAML answers all three (block scalars, quoting, `null`) with
//! a parser that already ships with the workspace, and the result is a
//! format the user can be expected to already know.
//!
//! Everything here is pure text and dialect-free: which columns identify
//! a row, how to read one and how to run the statement is the adapter's
//! business. What has to be shared is the buffer protocol and the
//! statement builder, because a second copy of either would drift — the
//! same reason [`crate::view_ddl`] exists.

/// One cell of the row being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowCell {
    /// Column name as the backend spells it.
    pub column: String,
    /// The stored value rendered as text, or `None` for SQL `NULL`.
    pub value: Option<String>,
    /// Whether this cell's text is a faithful, writable rendering of what
    /// is stored. `false` for values a backend can only summarise (a BLOB
    /// becomes `<blob, 12 bytes>`): writing that summary back would
    /// replace the data with a description of it, so the cell is shown as
    /// a comment and refused if it reappears in the mapping.
    pub editable: bool,
}

impl RowCell {
    /// A cell whose text can be written back unchanged.
    pub fn editable(column: impl Into<String>, value: Option<String>) -> Self {
        Self {
            column: column.into(),
            value,
            editable: true,
        }
    }

    /// A cell that can only be displayed — see [`RowCell::editable`].
    pub fn read_only(column: impl Into<String>, value: Option<String>) -> Self {
        Self {
            column: column.into(),
            value,
            editable: false,
        }
    }
}

/// The row as it was when the editor opened: every column in the table's
/// own order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSnapshot {
    pub cells: Vec<RowCell>,
}

impl RowSnapshot {
    pub fn new(cells: Vec<RowCell>) -> Self {
        Self { cells }
    }

    pub fn cell(&self, column: &str) -> Option<&RowCell> {
        self.cells.iter().find(|c| c.column == column)
    }

    /// Opaque token identifying this exact row content, for the same
    /// purpose as the view editor's stored definition: comparing it on
    /// save tells a concurrent change from an unchanged row without a
    /// modification timestamp the backends do not keep.
    ///
    /// Unit separators rather than YAML, because nothing ever parses this
    /// back — it is only ever compared, and a value containing the
    /// separators still cannot forge a different row's token (the column
    /// names are part of the text and separators cannot be typed into a
    /// column name).
    pub fn version_token(&self) -> String {
        let mut out = String::new();
        for cell in &self.cells {
            out.push_str(&cell.column);
            out.push('\u{1f}');
            match &cell.value {
                Some(v) => out.push_str(v),
                None => out.push('\u{0}'),
            }
            out.push('\u{1e}');
        }
        out
    }
}

/// A change the buffer asks for: the column and its new value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellChange {
    pub column: String,
    pub value: Option<String>,
}

/// How one row of a relation is addressed by the row editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowKeySpec {
    /// Key columns in their declared order.
    pub columns: Vec<String>,
    pub source: RowKeySource,
}

/// Where a [`RowKeySpec`] came from — worth telling the user, because a
/// primary key is stable *and* meaningful, while the fallbacks are only
/// one of the two.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKeySource {
    PrimaryKey,
    /// SQLite's implicit `rowid`: stable while the row exists, but not part
    /// of the data.
    RowId,
    /// A unique index, named so the user can see which one was picked when
    /// a table has several.
    UniqueIndex(String),
}

impl RowKeySpec {
    /// One line for the editor's buffer header, naming what the row is
    /// addressed by.
    pub fn note(&self) -> String {
        let columns = self.columns.join(", ");
        match &self.source {
            RowKeySource::PrimaryKey => format!(
                "Addressed by its primary key ({columns}) as it was before the edit, \
                 so changing that column renames the row instead of adding one."
            ),
            RowKeySource::RowId => format!(
                "This table has no primary key, so the row is addressed by its implicit \
                 {columns} — stable while the row exists, but not part of the data."
            ),
            RowKeySource::UniqueIndex(name) => format!(
                "This table has no primary key, so the row is addressed by the unique \
                 index {name} ({columns}) as it was before the edit."
            ),
        }
    }
}

/// One row read for editing: its cells, plus the values of the key columns
/// that address it.
#[derive(Clone, Debug)]
pub struct RowRead {
    pub cells: Vec<RowCell>,
    pub key_values: Vec<(String, Option<String>)>,
}

// ---------------------------------------------------------------------------
// Buffer rendering
// ---------------------------------------------------------------------------

const ERROR_BANNER_START: &str = "# ─── ERRORS ───";
const ERROR_BANNER_END: &str = "# ─────────────────";

/// The buffer the row editor opens with: a comment header explaining what
/// saving does, then the row as a YAML mapping.
///
/// `title` names the row's table for the header, `key_note` says how the
/// row is addressed (the adapter knows whether that is a primary key or a
/// `rowid`), and `dialect_note` is any backend-specific warning worth a
/// line. Both notes may span several lines; each becomes its own comment,
/// so callers write prose and not comment syntax.
pub fn edit_buffer(title: &str, key_note: &str, dialect_note: &str, row: &RowSnapshot) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {title}\n"));
    out.push_str(
        "# Every column is one YAML key. Save to write the columns you changed;\n\
         # untouched ones are left alone, and a column you delete counts as\n\
         # untouched. `null` writes SQL NULL — the text \"null\" has to be quoted.\n",
    );
    for line in comment_lines(key_note) {
        out.push_str(&line);
    }
    for line in comment_lines(dialect_note) {
        out.push_str(&line);
    }

    let read_only: Vec<&RowCell> = row.cells.iter().filter(|c| !c.editable).collect();
    if !read_only.is_empty() {
        out.push_str(
            "#\n# Shown for context only — these values cannot be rendered as text\n\
             # without losing what is stored, so they are not editable here:\n",
        );
        for cell in read_only {
            out.push_str(&format!(
                "#   {}: {}\n",
                cell.column,
                cell.value.as_deref().unwrap_or("null")
            ));
        }
    }

    out.push('\n');
    for cell in row.cells.iter().filter(|c| c.editable) {
        out.push_str(&render_entry(&cell.column, cell.value.as_deref()));
    }
    out
}

/// Prepend an error banner, replacing one already there. Each line of
/// `error` becomes its own YAML comment, so the buffer stays parseable
/// and a multi-line backend message — or the statement that failed —
/// stays readable.
pub fn render_with_error(text: &str, error: &str) -> String {
    let stripped = strip_error_banner(text);
    let mut out = String::new();
    out.push_str(ERROR_BANNER_START);
    out.push('\n');
    for line in error.lines() {
        out.push_str("# • ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(ERROR_BANNER_END);
    out.push('\n');
    out.push_str(stripped);
    out
}

/// Strip a previously rendered banner. Idempotent, and a no-op on a
/// buffer that has none — reopening must not stack banners.
pub fn strip_error_banner(text: &str) -> &str {
    let Some(rest) = text.strip_prefix(ERROR_BANNER_START) else {
        return text;
    };
    let after_start = rest.strip_prefix('\n').unwrap_or(rest);
    let needle = format!("\n{ERROR_BANNER_END}");
    match after_start.find(&needle) {
        Some(pos) => {
            let after_end = &after_start[pos + needle.len()..];
            after_end.strip_prefix('\n').unwrap_or(after_end)
        }
        // Truncated banner: drop what we recognised rather than returning
        // the mangled original.
        None => after_start,
    }
}

fn comment_lines(note: &str) -> Vec<String> {
    note.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| format!("# {}\n", line.trim()))
        .collect()
}

/// One `column: value` line (or a block scalar spanning several).
fn render_entry(column: &str, value: Option<&str>) -> String {
    let key = render_key(column);
    match value {
        None => format!("{key}: null\n"),
        Some(v) if use_block_scalar(v) => {
            let mut out = format!("{key}: |-\n");
            for line in v.lines() {
                if line.is_empty() {
                    out.push('\n');
                } else {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
            out
        }
        Some(v) => format!("{key}: {}\n", quote_scalar(v)),
    }
}

/// Column names are quoted whenever a bare key could be read as
/// something else (a number, `null`, an indicator character) — cheaper
/// than reasoning about which names are safe.
fn render_key(column: &str) -> String {
    let plain = !column.is_empty()
        && column
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !column.starts_with('-')
        && !column.chars().next().is_some_and(|c| c.is_ascii_digit());
    if plain && !is_yaml_keyword(column) {
        column.to_string()
    } else {
        quote_scalar(column)
    }
}

fn is_yaml_keyword(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "null" | "true" | "false" | "yes" | "no" | "on" | "off" | "y" | "n" | "~"
    )
}

/// A multi-line value is far more readable as a block scalar, but only
/// when it round-trips: trailing whitespace and `\r` are invisible in a
/// block scalar and would be silently rewritten, so those fall back to
/// the quoted form.
fn use_block_scalar(v: &str) -> bool {
    v.contains('\n')
        && !v.contains('\r')
        && !v.ends_with('\n')
        && !v.lines().any(|l| l.ends_with(' ') || l.ends_with('\t'))
        && !v.chars().any(is_yaml_control)
}

fn is_yaml_control(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t')
        || c == '\u{85}'
        || ('\u{7f}'..='\u{9f}').contains(&c)
}

/// Single quotes where they suffice (nothing inside needs escaping),
/// double quotes with escapes otherwise. Always quoting keeps the value a
/// string: an unquoted `5` would come back as a number, and while that
/// stringifies to the same text, `007` and `1e3` would not.
fn quote_scalar(v: &str) -> String {
    let needs_escapes = v.contains('\n') || v.contains('\r') || v.chars().any(is_yaml_control);
    if !needs_escapes {
        return format!("'{}'", v.replace('\'', "''"));
    }
    let mut out = String::from("\"");
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if is_yaml_control(c) => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Parsing the buffer back
// ---------------------------------------------------------------------------

/// Parse an edited buffer into `column → value` pairs, in the order they
/// appear.
///
/// The `Err` string is written for the user: it lands in the buffer's
/// error banner, so it says what is wrong with *their* text.
pub fn parse_row_buffer(text: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let body = strip_error_banner(text);
    if body.lines().all(|l| {
        let t = l.trim();
        t.is_empty() || t.starts_with('#')
    }) {
        return Err("nothing to save: the buffer holds no columns, only comments".into());
    }

    let value: serde_yaml::Value = serde_yaml::from_str(body).map_err(|e| {
        format!(
            "this is not valid YAML any more: {e}. Every line is `column: value`; \
             a value with a colon or a leading space has to be quoted."
        )
    })?;
    let mapping = match value {
        serde_yaml::Value::Mapping(m) => m,
        _ => {
            return Err(
                "expected one `column: value` mapping — a list or a bare value cannot \
                 describe a row."
                    .into(),
            );
        }
    };

    let mut out = Vec::with_capacity(mapping.len());
    for (key, value) in mapping {
        let column = match key {
            serde_yaml::Value::String(s) => s,
            other => {
                return Err(format!(
                    "`{}` is not a column name — column names are text",
                    scalar_text(&other).unwrap_or_else(|| "?".into())
                ));
            }
        };
        let value = match value {
            serde_yaml::Value::Null => None,
            other => Some(scalar_text(&other).ok_or_else(|| {
                format!(
                    "column {column}: a list or nested mapping is not a cell value — \
                     write it as text (quote it, or use a `|-` block for several lines)"
                )
            })?),
        };
        out.push((column, value));
    }
    Ok(out)
}

fn scalar_text(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Which columns the buffer actually changes, checked against the row it
/// was rendered from.
///
/// Rejects an unknown column (a typo would otherwise pass as "no change")
/// and any attempt to write a cell that was only shown for context.
pub fn changed_cells(
    row: &RowSnapshot,
    edited: &[(String, Option<String>)],
) -> Result<Vec<CellChange>, String> {
    let mut changes = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for (column, value) in edited {
        let cell = row.cell(column).ok_or_else(|| {
            format!(
                "the row has no column {column} — it cannot be added here. \
                 Columns of this row: {}",
                row.cells
                    .iter()
                    .map(|c| c.column.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        if seen.contains(&column.as_str()) {
            return Err(format!(
                "column {column} appears twice — which of the two values should be written?"
            ));
        }
        seen.push(column.as_str());
        if !cell.editable {
            return Err(format!(
                "column {column} cannot be written from here: its stored value has no \
                 faithful text form, so saving this line would replace the data with a \
                 description of it. Remove the line, or use a DB script."
            ));
        }
        if cell.value != *value {
            changes.push(CellChange {
                column: column.clone(),
                value: value.clone(),
            });
        }
    }
    Ok(changes)
}

// ---------------------------------------------------------------------------
// Statement building
// ---------------------------------------------------------------------------

/// The `UPDATE` for a set of changes, addressing the row by `keys`.
///
/// `table` is the already-quoted (and, where a dialect has one,
/// qualified) table name — how a name is qualified differs between
/// backends, so the caller owns it. `keys` are the key columns with the
/// values the row had *before* the edit, which is what makes changing a
/// key column an edit rather than a new row.
///
/// Values are spliced in as literals rather than bound as parameters:
/// the statement is shown to the user when it fails, and a statement full
/// of `?` placeholders would not tell them anything. Every literal goes
/// through [`quote_literal`], and both dialects convert a text literal
/// according to the target column's type.
///
/// Laid out over several lines — this text is read by a human in the
/// error banner far more often than by a parser.
pub fn build_update(
    table: &str,
    changes: &[CellChange],
    keys: &[(String, Option<String>)],
) -> String {
    let sets = changes
        .iter()
        .map(|c| {
            format!(
                "    {} = {}",
                crate::quote_ident(&c.column),
                quote_literal(c.value.as_deref())
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("UPDATE {table} SET\n{sets}\n  WHERE {}", render_where(keys))
}

/// The `WHERE` clause addressing one row by its key columns. Also used on
/// its own, to re-read the row before writing it.
pub fn render_where(keys: &[(String, Option<String>)]) -> String {
    if keys.is_empty() {
        // Callers must refuse before this point; a WHERE-less UPDATE would
        // rewrite the whole table.
        return "1 = 0".into();
    }
    keys.iter()
        .map(|(column, value)| match value {
            // `= NULL` is never true, so a nullable key column has to be
            // compared with IS NULL or the row would not be found.
            None => format!("{} IS NULL", crate::quote_ident(column)),
            Some(v) => format!(
                "{} = {}",
                crate::quote_ident(column),
                quote_literal(Some(v))
            ),
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// A value as a SQL literal: `NULL`, or a single-quoted string with
/// embedded quotes doubled. Doubling is the SQL-standard escape and the
/// only one both dialects need — Postgres runs with
/// `standard_conforming_strings` on (its default since 9.1), so a
/// backslash in a literal is a backslash.
pub fn quote_literal(value: Option<&str>) -> String {
    match value {
        None => "NULL".into(),
        Some(v) => format!("'{}'", v.replace('\'', "''")),
    }
}

// ---------------------------------------------------------------------------
// The editor session's `version` token
// ---------------------------------------------------------------------------

/// Everything the save needs to know about the row the editor opened on:
/// the key values that address it, which of its columns may not be written,
/// and the row as it was read.
///
/// One opaque string, because that is all
/// `EditorPrep::version` carries — and carrying the keys *with* the values
/// is what makes the save independent of the offset in a row's node id.
///
/// The three segments are separated by `\u{1d}`, cells last: a cell value
/// that happens to contain that separator then still survives, because the
/// cell segment is the whole remainder.
pub fn version_token(key_values: &[(String, Option<String>)], row: &RowSnapshot) -> String {
    let keys = key_values
        .iter()
        .map(|(column, value)| format!("{column}\u{1f}{}", value.as_deref().unwrap_or("\u{0}")))
        .collect::<Vec<_>>()
        .join("\u{1e}");
    // Which columns are read-only is a property of what was *read*, and the
    // save has to know it: the buffer shows such a cell as a comment, and
    // uncommenting it must be refused rather than write the description of
    // the data over the data.
    let read_only = row
        .cells
        .iter()
        .filter(|cell| !cell.editable)
        .map(|cell| cell.column.as_str())
        .collect::<Vec<_>>()
        .join("\u{1e}");
    format!("{keys}\u{1d}{read_only}\u{1d}{}", row.version_token())
}

/// Inverse of [`version_token`]. `None` only when the token was not
/// produced by it, which means the session is not one this node opened.
pub fn parse_version_token(token: &str) -> Option<(Vec<(String, Option<String>)>, RowSnapshot)> {
    let (keys, rest) = token.split_once('\u{1d}')?;
    let (read_only, cells) = rest.split_once('\u{1d}')?;
    let key_values = keys
        .split('\u{1e}')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (column, value) = part.split_once('\u{1f}')?;
            Some((column.to_string(), unmarked(value)))
        })
        .collect::<Option<Vec<_>>>()?;
    let read_only: Vec<&str> = read_only
        .split('\u{1e}')
        .filter(|c| !c.is_empty())
        .collect();
    let cells = cells
        .split('\u{1e}')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (column, value) = part.split_once('\u{1f}')?;
            let value = unmarked(value);
            Some(if read_only.contains(&column) {
                RowCell::read_only(column, value)
            } else {
                RowCell::editable(column, value)
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((key_values, RowSnapshot::new(cells)))
}

/// `\u{0}` is how [`RowSnapshot::version_token`] spells SQL NULL.
fn unmarked(value: &str) -> Option<String> {
    if value == "\u{0}" {
        None
    } else {
        Some(value.to_string())
    }
}

/// "1 column" / "3 columns", for the success message.
pub fn plural_columns(count: usize) -> String {
    if count == 1 {
        "1 column".into()
    } else {
        format!("{count} columns")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> RowSnapshot {
        RowSnapshot::new(vec![
            RowCell::editable("id", Some("1".into())),
            RowCell::editable("title", Some("first".into())),
            RowCell::editable("note", None),
        ])
    }

    fn parse(buffer: &str) -> Vec<(String, Option<String>)> {
        parse_row_buffer(buffer).expect("should parse")
    }

    #[test]
    fn buffer_round_trips_every_cell() {
        let buffer = edit_buffer("Row of t", "keyed by id", "", &row());
        assert_eq!(
            parse(&buffer),
            vec![
                ("id".into(), Some("1".into())),
                ("title".into(), Some("first".into())),
                ("note".into(), None),
            ]
        );
        // An untouched buffer changes nothing.
        assert!(
            changed_cells(&row(), &parse(&buffer))
                .expect("valid")
                .is_empty()
        );
    }

    /// The values that break naive `key = value` formats: quotes, leading
    /// space, newlines, and text that looks like YAML's own keywords.
    #[test]
    fn awkward_values_survive_the_round_trip() {
        let awkward = RowSnapshot::new(vec![
            RowCell::editable("quoted", Some("it's \"here\"".into())),
            RowCell::editable("spaced", Some("  padded  ".into())),
            RowCell::editable("multi", Some("line one\nline two".into())),
            RowCell::editable("keyword", Some("null".into())),
            RowCell::editable("numberish", Some("007".into())),
            RowCell::editable("colon", Some("a: b # c".into())),
            RowCell::editable("empty", Some(String::new())),
            RowCell::editable("tabbed", Some("a\tb".into())),
            RowCell::editable("crlf", Some("a\r\nb".into())),
        ]);
        let buffer = edit_buffer("Row of t", "keyed by id", "", &awkward);
        let parsed = parse(&buffer);
        for cell in &awkward.cells {
            let found = parsed
                .iter()
                .find(|(c, _)| c == &cell.column)
                .unwrap_or_else(|| panic!("{} missing", cell.column));
            assert_eq!(found.1, cell.value, "{}", cell.column);
        }
        assert!(
            changed_cells(&awkward, &parsed).expect("valid").is_empty(),
            "an untouched buffer must report no changes"
        );
    }

    /// A multi-line value is rendered readably, not as one escaped line —
    /// the whole point of using YAML.
    #[test]
    fn multi_line_values_use_a_block_scalar() {
        let buffer = edit_buffer(
            "Row of t",
            "keyed by id",
            "",
            &RowSnapshot::new(vec![RowCell::editable(
                "body",
                Some("line one\nline two".into()),
            )]),
        );
        assert!(
            buffer.contains("body: |-\n  line one\n  line two\n"),
            "{buffer}"
        );
    }

    #[test]
    fn a_column_name_needing_quotes_is_quoted() {
        let buffer = edit_buffer(
            "Row of t",
            "k",
            "",
            &RowSnapshot::new(vec![
                RowCell::editable("odd: name", Some("x".into())),
                RowCell::editable("null", Some("y".into())),
            ]),
        );
        let parsed = parse(&buffer);
        assert_eq!(parsed[0].0, "odd: name");
        assert_eq!(parsed[1].0, "null");
    }

    #[test]
    fn read_only_cells_are_shown_as_comments_only() {
        let with_blob = RowSnapshot::new(vec![
            RowCell::editable("id", Some("1".into())),
            RowCell::read_only("data", Some("<blob, 12 bytes>".into())),
        ]);
        let buffer = edit_buffer("Row of t", "keyed by id", "", &with_blob);
        assert!(buffer.contains("#   data: <blob, 12 bytes>"), "{buffer}");
        assert_eq!(parse(&buffer), vec![("id".into(), Some("1".into()))]);
    }

    /// Writing a read-only cell back would replace the data with its own
    /// description, so it is refused rather than attempted.
    #[test]
    fn writing_a_read_only_cell_is_refused() {
        let with_blob = RowSnapshot::new(vec![
            RowCell::editable("id", Some("1".into())),
            RowCell::read_only("data", Some("<blob, 12 bytes>".into())),
        ]);
        let err =
            changed_cells(&with_blob, &parse("id: '1'\ndata: 'oops'\n")).expect_err("refused");
        assert!(err.contains("data"), "{err}");
        assert!(err.contains("faithful text form"), "{err}");
    }

    #[test]
    fn only_changed_columns_are_reported() {
        let changes = changed_cells(
            &row(),
            &parse("id: '1'\ntitle: 'second'\nnote: 'now set'\n"),
        )
        .expect("valid");
        assert_eq!(
            changes,
            vec![
                CellChange {
                    column: "title".into(),
                    value: Some("second".into())
                },
                CellChange {
                    column: "note".into(),
                    value: Some("now set".into())
                },
            ]
        );
    }

    /// Deleting a line means "leave it alone" — the alternative (treating
    /// it as NULL) would make an accidental `dd` destructive.
    #[test]
    fn an_omitted_column_is_left_alone() {
        let changes = changed_cells(&row(), &parse("title: 'second'\n")).expect("valid");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].column, "title");
    }

    #[test]
    fn setting_a_value_to_null_is_a_change() {
        let changes = changed_cells(&row(), &parse("title: null\n")).expect("valid");
        assert_eq!(
            changes,
            vec![CellChange {
                column: "title".into(),
                value: None
            }]
        );
    }

    #[test]
    fn an_unknown_column_is_refused_with_the_real_ones() {
        let err = changed_cells(&row(), &parse("titel: 'typo'\n")).expect_err("refused");
        assert!(err.contains("no column titel"), "{err}");
        assert!(err.contains("id, title, note"), "{err}");
    }

    #[test]
    fn a_broken_buffer_explains_itself() {
        let err = parse_row_buffer("id: 'unterminated\n").expect_err("refused");
        assert!(err.contains("not valid YAML"), "{err}");

        let err = parse_row_buffer("- id\n- title\n").expect_err("refused");
        assert!(err.contains("mapping"), "{err}");

        let err = parse_row_buffer("# only a comment\n").expect_err("refused");
        assert!(err.contains("no columns"), "{err}");

        let err = parse_row_buffer("id:\n  nested: 1\n").expect_err("refused");
        assert!(err.contains("not a cell value"), "{err}");
    }

    #[test]
    fn banner_round_trips_and_does_not_stack() {
        let buffer = edit_buffer("Row of t", "keyed by id", "", &row());
        let once = render_with_error(&buffer, "no such column: titel");
        assert!(once.starts_with(ERROR_BANNER_START));
        assert_eq!(strip_error_banner(&once), buffer);
        assert_eq!(render_with_error(&once, "no such column: titel"), once);
        // A banner must not stop the buffer from parsing.
        assert_eq!(parse(&once).len(), 3);
    }

    #[test]
    fn the_statement_shown_in_a_banner_stays_readable() {
        let statement = build_update(
            "\"notes\"",
            &[CellChange {
                column: "title".into(),
                value: Some("second".into()),
            }],
            &[("id".into(), Some("1".into()))],
        );
        let banner = render_with_error("id: '1'\n", &format!("failed:\n{statement}"));
        for line in statement.lines() {
            assert!(banner.contains(&format!("# • {line}\n")), "{banner}");
        }
    }

    #[test]
    fn update_sets_only_the_changed_columns() {
        let sql = build_update(
            "\"notes\"",
            &[
                CellChange {
                    column: "title".into(),
                    value: Some("it's new".into()),
                },
                CellChange {
                    column: "note".into(),
                    value: None,
                },
            ],
            &[("id".into(), Some("1".into()))],
        );
        assert_eq!(
            sql,
            "UPDATE \"notes\" SET\n    \"title\" = 'it''s new',\n    \"note\" = NULL\n  WHERE \"id\" = '1'"
        );
    }

    #[test]
    fn a_composite_key_is_matched_in_full() {
        let sql = build_update(
            "\"public\".\"t\"",
            &[CellChange {
                column: "x".into(),
                value: Some("1".into()),
            }],
            &[("a".into(), Some("2".into())), ("b".into(), None)],
        );
        assert!(
            sql.ends_with("WHERE \"a\" = '2' AND \"b\" IS NULL"),
            "{sql}"
        );
    }

    /// A quote in a value must not end the literal — the one escaping
    /// mistake that would turn an edit into arbitrary SQL.
    #[test]
    fn literals_double_embedded_quotes() {
        assert_eq!(
            quote_literal(Some("Bobby'); DROP TABLE t;--")),
            "'Bobby''); DROP TABLE t;--'"
        );
        assert_eq!(quote_literal(None), "NULL");
    }

    /// Changing a key column is an ordinary edit: the WHERE keeps the old
    /// value, so the row is found and renamed rather than duplicated.
    #[test]
    fn a_changed_key_column_is_addressed_by_its_old_value() {
        let changes = changed_cells(&row(), &parse("id: '2'\n")).expect("valid");
        let sql = build_update("\"t\"", &changes, &[("id".into(), Some("1".into()))]);
        assert!(sql.contains("\"id\" = '2'"), "{sql}");
        assert!(sql.ends_with("WHERE \"id\" = '1'"), "{sql}");
    }

    /// The token is what tells a concurrent change from an untouched row,
    /// so two different rows must never share one.
    #[test]
    fn version_token_changes_with_any_cell() {
        let base = row().version_token();
        assert_eq!(base, row().version_token());

        let mut other = row();
        other.cells[1].value = Some("second".into());
        assert_ne!(base, other.version_token());

        // NULL and the text "null" are different rows.
        let mut nulled = row();
        nulled.cells[2].value = Some("null".into());
        assert_ne!(base, nulled.version_token());
    }

    /// The session token is the only thing that survives between `prepare`
    /// and `execute`, so everything the save needs has to come back out of
    /// it unchanged — including a NULL key value.
    #[test]
    fn the_session_token_round_trips_keys_and_cells() {
        let keys = vec![
            ("id".to_string(), Some("1".to_string())),
            ("kind".to_string(), None),
        ];
        let token = version_token(&keys, &row());

        let (parsed_keys, parsed_row) = parse_version_token(&token).expect("own token");
        assert_eq!(parsed_keys, keys);
        assert_eq!(parsed_row, row());
    }

    /// A read-only cell that came back editable would let an uncommented
    /// BLOB line write the description of the data over the data — the save
    /// path's only source for that is this token.
    #[test]
    fn a_read_only_cell_stays_read_only_through_the_token() {
        let snapshot = RowSnapshot::new(vec![
            RowCell::editable("id", Some("1".into())),
            RowCell::read_only("payload", Some("<blob, 3 bytes>".into())),
        ]);
        let token = version_token(&[("id".to_string(), Some("1".to_string()))], &snapshot);

        let (_, parsed) = parse_version_token(&token).expect("own token");
        assert_eq!(parsed, snapshot);
        assert!(!parsed.cells[1].editable);
    }

    /// Cells are the token's last segment precisely so that a value
    /// containing the segment separator cannot cut the token short.
    #[test]
    fn a_separator_inside_a_value_survives_the_token() {
        let snapshot = RowSnapshot::new(vec![RowCell::editable(
            "body",
            Some("before\u{1d}after".into()),
        )]);
        let token = version_token(&[("id".to_string(), Some("1".to_string()))], &snapshot);

        let (_, parsed) = parse_version_token(&token).expect("own token");
        assert_eq!(parsed, snapshot);
    }

    /// A token from anywhere else means the session is not one this node
    /// opened, and the save has to say so rather than guess at the keys.
    #[test]
    fn a_foreign_token_is_not_parsed() {
        assert!(parse_version_token("").is_none());
        assert!(parse_version_token("id\u{1f}1").is_none());
    }

    #[test]
    fn the_column_count_reads_as_prose() {
        assert_eq!(plural_columns(1), "1 column");
        assert_eq!(plural_columns(0), "0 columns");
        assert_eq!(plural_columns(3), "3 columns");
    }
}
