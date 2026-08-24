//! Comment-preserving, surgical editor for the app's YAML config files.
//!
//! The interactive keybinding editor has to rewrite individual `key:` /
//! binding entries in `tui.yaml` and `views/*.yaml` **without** disturbing
//! anything else — comments, blank lines, key order and formatting all have
//! to survive verbatim. A serde round-trip cannot do that (it drops comments
//! and reorders keys), so this module never reserialises: it uses
//! [`marked_yaml`] purely to *locate* the target node by line/column, then
//! splices the raw source text at the line level.
//!
//! Guarantees:
//! - Lines other than the edited entry are returned byte-for-byte unchanged
//!   (including their comments, indentation and trailing whitespace).
//! - When an entry's value sits on the same physical line as its key, a
//!   trailing inline comment on that line is preserved.
//! - Inserting a new entry only *adds* a line; it never rewrites an existing
//!   one.
//!
//! Known limitation (documented, warned by callers): replacing a value that
//! spans **multiple** lines (a block sequence/mapping) collapses it to a
//! single-line flow value, so comments living *inside* that block are lost.
//! Comments on every other line always survive.

// Wired into the interactive keybinding editor in a later phase; until then
// only the test suite exercises it.
#![allow(dead_code)]

use marked_yaml::types::MarkedMappingNode;
use marked_yaml::{Node, parse_yaml};

/// One step along a path from the document root to the target mapping whose
/// entry we want to edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathStep {
    /// Descend into `mapping[key]`.
    Key(String),
    /// Descend into `sequence[index]`.
    Index(usize),
    /// Within the current sequence, select the mapping whose scalar child
    /// `field` equals `value` (e.g. an action identified by its `name`).
    Find { field: String, value: String },
    /// Within the current sequence, select the action whose identity equals
    /// `value` — its `name:` when it declares one, otherwise its `id:`. This
    /// mirrors [`ActionDef::name`](crate::config::view_config::ActionDef::name),
    /// the identity every other layer addresses an action by, so an entry
    /// that leaves the label to the adapter (`- {{ key: d, id: delete }}`) is
    /// still reachable from the keybinding editor.
    FindAction { value: String },
}

impl PathStep {
    pub fn key(s: impl Into<String>) -> Self {
        PathStep::Key(s.into())
    }
    pub fn find(field: impl Into<String>, value: impl Into<String>) -> Self {
        PathStep::Find {
            field: field.into(),
            value: value.into(),
        }
    }
    pub fn find_action(value: impl Into<String>) -> Self {
        PathStep::FindAction {
            value: value.into(),
        }
    }
}

type EditResult<T> = Result<T, String>;

/// Raw marker positions of a mapping entry, before its multi-line extent is
/// resolved against the source lines.
struct RawLoc {
    key_line: usize,       // 1-based line of the key scalar
    key_col: usize,        // 1-based column where the key text starts
    val_start_line: usize, // 1-based line of the value's first char
    val_start_col: usize,  // 1-based column of the value's first char
    /// The value begins on a line *below* its key (a block sequence/mapping)
    /// rather than inline after the `key:`.
    is_block: bool,
}

/// Located extent of a single mapping entry within the source text.
struct EntryLoc {
    key_line: usize,
    key_col: usize,
    val_start_col: usize,
    val_end_line: usize, // 1-based line of the value's last physical line
    single_line: bool,   // value occupies exactly one physical line
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set the mapping entry `entry` (under the node reached by `path`) to
/// `values`, rendering:
/// - `[]` when `values` is empty (the "disable this binding" form),
/// - a bare/quoted scalar for exactly one value,
/// - a flow sequence `[a, b, …]` for several.
///
/// If the entry already exists its value is replaced in place (an inline
/// comment on a single-line value is preserved). If it is absent, a new line
/// is inserted as a sibling of the mapping's existing entries.
pub fn set_entry(
    source: &str,
    path: &[PathStep],
    entry: &str,
    values: &[String],
) -> EditResult<String> {
    let rendered = render_value(values);
    let root = parse_yaml(0, source).map_err(|e| format!("YAML parse error: {e}"))?;
    let target = navigate(&root, path)?;
    let map = target
        .as_mapping()
        .ok_or_else(|| "target of path is not a mapping".to_string())?;

    let (mut lines, trailing_nl) = split_lines(source);

    match entry_loc(map, entry, &lines) {
        Some(loc) => {
            let key_line = lines
                .get(loc.key_line - 1)
                .ok_or_else(|| "key line out of range".to_string())?;
            let prefix = char_prefix(key_line, loc.key_col - 1);
            let new_line = if loc.single_line {
                let suffix = value_suffix(key_line, loc.val_start_col - 1);
                format!("{prefix}{entry}: {rendered}{suffix}")
            } else {
                format!("{prefix}{entry}: {rendered}")
            };
            let start = loc.key_line - 1;
            let end = loc.val_end_line - 1;
            splice(&mut lines, start, end, new_line);
        }
        None => {
            let (after_line, key_col) = first_entry_anchor(map)
                .ok_or_else(|| "cannot insert into an empty mapping".to_string())?;
            let indent = " ".repeat(key_col - 1);
            let new_line = format!("{indent}{entry}: {rendered}");
            // `after_line` is 1-based; inserting at that 0-based index places
            // the new line directly after it.
            lines.insert(after_line, new_line);
        }
    }

    Ok(join_lines(lines, trailing_nl))
}

/// Append `- {{ <fields> }}` as a new item to the block sequence `seq_key`
/// under the mapping reached by `path`, creating the `seq_key:` line when the
/// mapping doesn't have it yet.
///
/// This is what binding a key to an adapter action the view file never
/// mentions needs: there is no entry to rewrite, so one has to be written.
/// The new item is a single-line flow mapping, which keeps the splice to a
/// pure insertion — no existing line is touched, so every comment survives.
///
/// The insertion point is found textually rather than from the parsed spans:
/// the last line belonging to the sequence is the last one indented at least
/// as deep as its first item (blank lines in between are carried along), so
/// the new item lands after the sequence's real end and before whatever key
/// follows it.
pub fn append_seq_item(
    source: &str,
    path: &[PathStep],
    seq_key: &str,
    fields: &[(String, String)],
) -> EditResult<String> {
    let rendered = format!(
        "{{ {} }}",
        fields
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let root = parse_yaml(0, source).map_err(|e| format!("YAML parse error: {e}"))?;
    let target = navigate(&root, path)?;
    let map = target
        .as_mapping()
        .ok_or_else(|| "target of path is not a mapping".to_string())?;
    let (mut lines, trailing_nl) = split_lines(source);

    let existing = map
        .get_node(seq_key)
        .and_then(|n| n.as_sequence().map(|s| s.iter().next().is_some()))
        .unwrap_or(false);
    if existing {
        let (key_line, _) =
            key_pos(map, seq_key).ok_or_else(|| format!("key '{seq_key}' not found"))?;
        // Indentation of the sequence's first item line (the one carrying `-`).
        let first_item = (key_line..lines.len())
            .find(|i| lines[*i].trim_start().starts_with('-'))
            .ok_or_else(|| format!("no list item found under '{seq_key}'"))?;
        let indent = indent_of(&lines[first_item]);
        // The sequence ends at the last non-blank line still indented at
        // least as deep as its items; anything shallower belongs to the
        // enclosing mapping again.
        let mut last = first_item;
        for (i, line) in lines.iter().enumerate().skip(first_item + 1) {
            if line.trim().is_empty() {
                continue;
            }
            if indent_of(line) < indent {
                break;
            }
            last = i;
        }
        let insert_at = last + 1;
        let pad = " ".repeat(indent);
        lines.insert(insert_at, format!("{pad}- {rendered}"));
    } else {
        let (after_line, key_col) = match key_pos(map, seq_key) {
            // `seq_key:` present but empty — insert the first item under it.
            Some((line, col)) => (line, col),
            None => {
                let (after_line, key_col) = first_entry_anchor(map)
                    .ok_or_else(|| "cannot insert into an empty mapping".to_string())?;
                let indent = " ".repeat(key_col - 1);
                lines.insert(after_line, format!("{indent}{seq_key}:"));
                (after_line + 1, key_col)
            }
        };
        let pad = " ".repeat(key_col - 1 + 2);
        lines.insert(after_line, format!("{pad}- {rendered}"));
    }
    Ok(join_lines(lines, trailing_nl))
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Remove the mapping entry `entry` (under the node reached by `path`)
/// entirely, deleting its full line range. Errors if the entry is absent or
/// if it is the leading entry of a block sequence item (its line carries the
/// `- ` marker, so removing the line would corrupt the list).
pub fn remove_entry(source: &str, path: &[PathStep], entry: &str) -> EditResult<String> {
    let root = parse_yaml(0, source).map_err(|e| format!("YAML parse error: {e}"))?;
    let target = navigate(&root, path)?;
    let map = target
        .as_mapping()
        .ok_or_else(|| "target of path is not a mapping".to_string())?;
    let (mut lines, trailing_nl) = split_lines(source);
    let loc = entry_loc(map, entry, &lines)
        .ok_or_else(|| format!("entry '{entry}' not found to remove"))?;
    let key_line = lines
        .get(loc.key_line - 1)
        .ok_or_else(|| "key line out of range".to_string())?;
    if char_prefix(key_line, loc.key_col - 1).contains('-') {
        return Err(format!(
            "refusing to remove '{entry}': it is the first entry of a list item"
        ));
    }
    let start = loc.key_line - 1;
    let end = loc.val_end_line - 1;
    lines.drain(start..=end);
    Ok(join_lines(lines, trailing_nl))
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

fn navigate<'a>(root: &'a Node, path: &[PathStep]) -> EditResult<&'a Node> {
    let mut cur = root;
    for (i, step) in path.iter().enumerate() {
        match step {
            PathStep::Key(k) => {
                let m = cur
                    .as_mapping()
                    .ok_or_else(|| format!("step {i}: expected a mapping to descend key '{k}'"))?;
                cur = m
                    .get_node(k)
                    .ok_or_else(|| format!("step {i}: key '{k}' not found"))?;
            }
            PathStep::Index(idx) => {
                let s = cur
                    .as_sequence()
                    .ok_or_else(|| format!("step {i}: expected a sequence to index [{idx}]"))?;
                cur = s
                    .get_node(*idx)
                    .ok_or_else(|| format!("step {i}: index {idx} out of range"))?;
            }
            PathStep::Find { field, value } => {
                let s = cur.as_sequence().ok_or_else(|| {
                    format!("step {i}: expected a sequence to find {field}={value}")
                })?;
                cur = s
                    .iter()
                    .find(|item| {
                        item.as_mapping()
                            .and_then(|m| m.get_scalar(field))
                            .map(|sc| sc.as_str() == value)
                            .unwrap_or(false)
                    })
                    .ok_or_else(|| format!("step {i}: no item with {field}={value}"))?;
            }
            PathStep::FindAction { value } => {
                let s = cur.as_sequence().ok_or_else(|| {
                    format!("step {i}: expected a sequence to find the action '{value}'")
                })?;
                cur = s
                    .iter()
                    .find(|item| action_identity(item).as_deref() == Some(value.as_str()))
                    .ok_or_else(|| format!("step {i}: no action named '{value}'"))?;
            }
        }
    }
    Ok(cur)
}

/// The `(line, column)` (both 1-based) of `entry`'s key scalar within `map`,
/// or `None` if absent. Unlike [`raw_loc`] this needs only the *key* span, so
/// it locates keys whose value is null/empty (a bare `actions:` block).
fn key_pos(map: &MarkedMappingNode, entry: &str) -> Option<(usize, usize)> {
    for (k, _v) in map.iter() {
        if k.as_str() == entry {
            let ks = k.span().start()?;
            return Some((ks.line(), ks.column()));
        }
    }
    None
}

fn raw_loc(map: &MarkedMappingNode, entry: &str) -> Option<RawLoc> {
    for (k, v) in map.iter() {
        if k.as_str() != entry {
            continue;
        }
        let ks = k.span().start()?;
        let vs = v.span().start()?;
        return Some(RawLoc {
            key_line: ks.line(),
            key_col: ks.column(),
            val_start_line: vs.line(),
            val_start_col: vs.column(),
            is_block: vs.line() > ks.line(),
        });
    }
    None
}

/// Resolve the entry's full extent. Inline values (scalar or flow list on the
/// key's own line) occupy exactly one line. Block values (sequence/mapping
/// starting below the key) are measured by scanning downward while the
/// indentation stays deeper than the key — `marked-yaml`'s end marker points
/// at the *following* token, so it can't be trusted here.
fn entry_loc(map: &MarkedMappingNode, entry: &str, lines: &[String]) -> Option<EntryLoc> {
    let raw = raw_loc(map, entry)?;
    let (val_end_line, single_line) = if raw.is_block {
        (
            block_end_line(lines, raw.val_start_line, raw.key_col),
            false,
        )
    } else {
        (raw.key_line, true)
    };
    Some(EntryLoc {
        key_line: raw.key_line,
        key_col: raw.key_col,
        val_start_col: raw.val_start_col,
        val_end_line,
        single_line,
    })
}

/// Last 1-based line belonging to a block value that begins at
/// `val_start_line`: walk down while lines are blank or indented deeper than
/// the key (`key_col`), stopping at the first line dedented to the key's
/// level or shallower (a sibling key or the parent's next entry).
fn block_end_line(lines: &[String], val_start_line: usize, key_col: usize) -> usize {
    let key_indent = key_col - 1; // 0-based column of the key
    let mut end = val_start_line;
    let mut j = val_start_line + 1;
    while let Some(line) = lines.get(j - 1) {
        if line.trim().is_empty() {
            j += 1;
            continue; // blank lines don't extend the value's last line
        }
        let indent = line.chars().take_while(|c| c.is_whitespace()).count();
        if indent > key_indent {
            end = j;
            j += 1;
        } else {
            break;
        }
    }
    end
}

/// Returns `(end_line, key_col)` of the mapping's first entry: the 1-based
/// line after which a new sibling entry should be inserted, and the column at
/// which its key sits (the indentation to match).
/// The identity of one `actions:` sequence item: its `name:` when present,
/// otherwise its `id:`. Mirrors `ActionDef::name`.
fn action_identity(item: &Node) -> Option<String> {
    let m = item.as_mapping()?;
    m.get_scalar("name")
        .or_else(|| m.get_scalar("id"))
        .map(|sc| sc.as_str().to_string())
}

fn first_entry_anchor(map: &MarkedMappingNode) -> Option<(usize, usize)> {
    let (k, v) = map.iter().next()?;
    let ks = k.span().start()?;
    let end_line = v
        .span()
        .end()
        .map(|m| m.line())
        .or_else(|| v.span().start().map(|m| m.line()))
        .unwrap_or_else(|| ks.line());
    Some((end_line, ks.column()))
}

// ---------------------------------------------------------------------------
// Value rendering
// ---------------------------------------------------------------------------

pub(crate) fn render_value(values: &[String]) -> String {
    match values.len() {
        0 => "[]".to_string(),
        1 => render_scalar(&values[0]),
        _ => {
            let inner = values
                .iter()
                .map(|v| render_scalar(v))
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
    }
}

/// Render one binding string as a YAML scalar. Plain identifiers made of
/// letters, digits and `+` (e.g. `a`, `ctrl+shift+a`, `f12`, the legacy chord
/// `zr`) are emitted bare; anything else — space-separated sequences
/// (`ctrl+k l`), punctuation keys (`/`, `<`, `:`), the empty string, or a
/// token the YAML resolver would read as a non-string (a tab digit like `1`,
/// or a bool/null word like `n`/`no`) — is single-quoted, doubling any
/// embedded quote.
fn render_scalar(s: &str) -> String {
    if needs_quote(s) {
        format!("'{}'", s.replace('\'', "''"))
    } else {
        s.to_string()
    }
}

fn needs_quote(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    // Anything outside the plain-identifier set (punctuation, spaces) must be
    // quoted.
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '+') {
        return true;
    }
    // A token made only of those chars can still be resolved as a non-string
    // scalar and fail to deserialize into a `String`/`Vec<String>` binding:
    //  - an all-digit token (`1`, the positional tab-switch key) parses as an
    //    integer — exactly the `tab.key: invalid type: integer 1` failure;
    //  - a YAML 1.1 bool/null word (`y`/`n`/`yes`/`no`/`on`/`off`/`true`/
    //    `false`/`null`) parses as a bool/null — and `n`/`y` are real keys.
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    matches!(
        s.to_ascii_lowercase().as_str(),
        "y" | "n" | "yes" | "no" | "on" | "off" | "true" | "false" | "null"
    )
}

// ---------------------------------------------------------------------------
// Line-level text helpers
// ---------------------------------------------------------------------------

/// Split into lines, remembering whether the source ended with a newline so
/// we can restore it exactly.
fn split_lines(s: &str) -> (Vec<String>, bool) {
    let trailing = s.ends_with('\n');
    let body = if trailing { &s[..s.len() - 1] } else { s };
    (body.split('\n').map(|l| l.to_string()).collect(), trailing)
}

fn join_lines(lines: Vec<String>, trailing: bool) -> String {
    let mut s = lines.join("\n");
    if trailing {
        s.push('\n');
    }
    s
}

/// Replace the inclusive line range `start..=end` (0-based) with a single
/// `new_line`.
fn splice(lines: &mut Vec<String>, start: usize, end: usize, new_line: String) {
    lines.splice(start..=end, std::iter::once(new_line));
}

/// First `n` characters of `line` (char-based, not bytes).
fn char_prefix(line: &str, n: usize) -> String {
    line.chars().take(n).collect()
}

/// Whether the value starting at char index `from` sits inside a flow
/// collection opened earlier on the same line — the `- {{ key: d, id: delete }}`
/// form the view files use for node actions.
fn in_flow_context(line: &str, from: usize) -> bool {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in line.chars().enumerate() {
        if i >= from {
            break;
        }
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                '#' => break,
                _ => {}
            },
        }
    }
    depth > 0
}

/// Everything that must survive after the value starting at char index
/// `from`. In a flow mapping that is the rest of the item (`, id: delete }`);
/// otherwise it is the trailing comment, if any. Without this a rewrite of
/// one field inside `- {{ key: d, id: delete }}` would truncate the item at the
/// replaced value.
fn value_suffix(line: &str, from: usize) -> String {
    if !in_flow_context(line, from) {
        return trailing_comment(line, from);
    }
    let chars: Vec<char> = line.chars().collect();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for i in from.min(chars.len())..chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '{' | '[' => depth += 1,
                '}' | ']' if depth > 0 => depth -= 1,
                ',' | '}' | ']' => return chars[i..].iter().collect(),
                _ => {}
            },
        }
    }
    String::new()
}

/// The trailing comment on `line` (a `#` preceded by whitespace) at or after
/// char index `from`, including the whitespace that separates it from the
/// value. Empty string when there is no comment.
fn trailing_comment(line: &str, from: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut i = from.min(chars.len());
    while i < chars.len() {
        if chars[i] == '#' && i > 0 && chars[i - 1].is_whitespace() {
            // Back up over the run of whitespace before the `#`.
            let mut ws = i;
            while ws > 0 && chars[ws - 1].is_whitespace() {
                ws -= 1;
            }
            return chars[ws..].iter().collect();
        }
        i += 1;
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(source: &str, path: &[PathStep], entry: &str, values: &[&str]) -> String {
        let vals: Vec<String> = values.iter().map(|s| s.to_string()).collect();
        set_entry(source, path, entry, &vals).expect("set_entry")
    }

    #[test]
    fn replace_scalar_same_line_preserves_inline_comment() {
        let src = "keybindings:\n  global:\n    quit: ctrl+c  # exit the app\n";
        let out = set(
            src,
            &[PathStep::key("keybindings"), PathStep::key("global")],
            "quit",
            &["ctrl+q"],
        );
        assert_eq!(
            out,
            "keybindings:\n  global:\n    quit: ctrl+q  # exit the app\n"
        );
    }

    #[test]
    fn replace_leaves_every_other_line_byte_identical() {
        let src = "\
# leading comment
tabs:
  order:
    - Tasks
keybindings:
  global:
    # a comment above quit
    quit: ctrl+c
    tab_next: tab   # cycle forward
  common:
    list_next: j
theme:
  name: X
";
        let out = set(
            src,
            &[PathStep::key("keybindings"), PathStep::key("global")],
            "quit",
            &["ctrl+q"],
        );
        let expected = "\
# leading comment
tabs:
  order:
    - Tasks
keybindings:
  global:
    # a comment above quit
    quit: ctrl+q
    tab_next: tab   # cycle forward
  common:
    list_next: j
theme:
  name: X
";
        assert_eq!(out, expected);
    }

    #[test]
    fn replace_inline_flow_list_in_place() {
        let src = "common:\n  list_next: [j, down]  # move down\n";
        let out = set(src, &[PathStep::key("common")], "list_next", &["j", "k l"]);
        assert_eq!(out, "common:\n  list_next: [j, 'k l']  # move down\n");
    }

    #[test]
    fn replace_block_list_collapses_to_flow() {
        let src = "\
content:
  open:
    - enter
    - l
  back: h
";
        let out = set(src, &[PathStep::key("content")], "open", &["enter", "l"]);
        let expected = "\
content:
  open: [enter, l]
  back: h
";
        assert_eq!(out, expected);
    }

    #[test]
    fn insert_missing_key_after_first_entry() {
        let src = "\
actions:
  - name: Edit
    type: adapter
    id: edit
";
        let out = set(
            src,
            &[PathStep::key("actions"), PathStep::find("name", "Edit")],
            "key",
            &["e"],
        );
        let expected = "\
actions:
  - name: Edit
    key: e
    type: adapter
    id: edit
";
        assert_eq!(out, expected);
    }

    #[test]
    fn set_empty_list_disables() {
        let src = "global:\n  quit: ctrl+c\n";
        let out = set(src, &[PathStep::key("global")], "quit", &[]);
        assert_eq!(out, "global:\n  quit: []\n");
    }

    #[test]
    fn quoting_covers_sequences_and_punctuation() {
        assert_eq!(render_scalar("a"), "a");
        assert_eq!(render_scalar("ctrl+shift+a"), "ctrl+shift+a");
        assert_eq!(render_scalar("zr"), "zr");
        assert_eq!(render_scalar("ctrl+k l"), "'ctrl+k l'");
        assert_eq!(render_scalar("/"), "'/'");
        assert_eq!(render_scalar(":"), "':'");
        assert_eq!(render_scalar("<"), "'<'");
    }

    #[test]
    fn quoting_covers_number_and_bool_lookalikes() {
        // Digit tab-switch keys must not round-trip as integers.
        assert_eq!(render_scalar("1"), "'1'");
        assert_eq!(render_scalar("0"), "'0'");
        assert_eq!(render_scalar("12"), "'12'");
        // Bool/null-word keys must not round-trip as bool/null.
        assert_eq!(render_scalar("n"), "'n'");
        assert_eq!(render_scalar("y"), "'y'");
        assert_eq!(render_scalar("no"), "'no'");
        assert_eq!(render_scalar("off"), "'off'");
        // Letters-with-digits and normal keys stay bare.
        assert_eq!(render_scalar("f12"), "f12");
        assert_eq!(render_scalar("ctrl+1"), "ctrl+1");
        assert_eq!(render_scalar("no1"), "no1");
    }

    #[test]
    fn nested_find_edits_action_key_in_view() {
        let src = "\
tab:
  name: Jira
actions:
  - name: Edit
    key: e
    type: adapter
  - name: Delete
    key: d
    type: adapter
";
        let out = set(
            src,
            &[PathStep::key("actions"), PathStep::find("name", "Delete")],
            "key",
            &["x", "ctrl+shift+d"],
        );
        let expected = "\
tab:
  name: Jira
actions:
  - name: Edit
    key: e
    type: adapter
  - name: Delete
    key: [x, ctrl+shift+d]
    type: adapter
";
        assert_eq!(out, expected);
    }

    #[test]
    fn insert_key_on_first_entry_line_item() {
        // Action whose leading entry IS `key` (carrying the `- ` marker):
        // replacing it must keep the marker.
        let src = "actions:\n  - key: e\n    name: Edit\n";
        let out = set(
            src,
            &[PathStep::key("actions"), PathStep::find("name", "Edit")],
            "key",
            &["e", "ctrl+e"],
        );
        assert_eq!(out, "actions:\n  - key: [e, ctrl+e]\n    name: Edit\n");
    }

    #[test]
    fn remove_entry_deletes_only_its_line() {
        let src = "global:\n  quit: ctrl+c\n  tab_next: tab   # keep me\n";
        let out = remove_entry(src, &[PathStep::key("global")], "quit").expect("remove");
        assert_eq!(out, "global:\n  tab_next: tab   # keep me\n");
    }

    #[test]
    fn remove_refuses_list_marker_line() {
        let src = "actions:\n  - key: e\n    name: Edit\n";
        let err = remove_entry(
            src,
            &[PathStep::key("actions"), PathStep::find("name", "Edit")],
            "key",
        )
        .unwrap_err();
        assert!(err.contains("first entry"), "got: {err}");
    }

    #[test]
    fn missing_path_is_an_error() {
        let src = "global:\n  quit: ctrl+c\n";
        let err = set_entry(src, &[PathStep::key("nope")], "x", &[]).unwrap_err();
        assert!(err.contains("not found"), "got: {err}");
    }

    // ── append_seq_item ──────────────────────────────────────────────
    //
    // Binding a key to an adapter action the view file never mentions has
    // to *create* the `actions:` entry. The three cases below are the three
    // states a level can be in when that happens.

    fn append(source: &str, path: &[PathStep], seq_key: &str, fields: &[(&str, &str)]) -> String {
        let owned: Vec<(String, String)> = fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        append_seq_item(source, path, seq_key, &owned).expect("append_seq_item")
    }

    #[test]
    fn append_seq_item_adds_to_a_populated_sequence() {
        let src = "\
views:
  - name: tickets
    actions:
      - { name: edit, key: e, type: edit }
      - name: search
        key: /
        type: search
    node_type: t
";
        let out = append(
            src,
            &[PathStep::key("views"), PathStep::find("name", "tickets")],
            "actions",
            &[("key", "d"), ("id", "delete")],
        );
        // Appended after the *whole* last item, not after its first line.
        assert!(
            out.contains("        type: search\n      - { key: d, id: delete }\nnode_type")
                || out.contains("        type: search\n      - { key: d, id: delete }\n    node_type"),
            "got:\n{out}"
        );
        // Everything else is untouched, comments and block style included.
        assert!(out.contains("      - { name: edit, key: e, type: edit }"));
    }

    #[test]
    fn append_seq_item_fills_a_present_but_empty_key() {
        let src = "views:\n  - name: tickets\n    actions:\n    node_type: t\n";
        let out = append(
            src,
            &[PathStep::key("views"), PathStep::find("name", "tickets")],
            "actions",
            &[("key", "d"), ("id", "delete")],
        );
        assert_eq!(
            out,
            "views:\n  - name: tickets\n    actions:\n      - { key: d, id: delete }\n    node_type: t\n"
        );
    }

    #[test]
    fn append_seq_item_creates_an_absent_key() {
        let src = "views:\n  - name: tickets\n    node_type: t\n";
        let out = append(
            src,
            &[PathStep::key("views"), PathStep::find("name", "tickets")],
            "actions",
            &[("key", "d"), ("id", "delete")],
        );
        assert_eq!(
            out,
            "views:\n  - name: tickets\n    actions:\n      - { key: d, id: delete }\n    node_type: t\n"
        );
    }

    #[test]
    fn append_seq_item_keeps_a_trailing_comment_with_its_item() {
        // A comment indented under the last item belongs to that item; the
        // new entry goes after it, not between the item and its comment.
        let src = "\
views:
  - name: tickets
    actions:
      - name: edit
        key: e
        # why `e`: the editor opens in place
    node_type: t
";
        let out = append(
            src,
            &[PathStep::key("views"), PathStep::find("name", "tickets")],
            "actions",
            &[("key", "d"), ("id", "delete")],
        );
        assert!(
            out.contains("        # why `e`: the editor opens in place\n      - { key: d, id: delete }"),
            "got:\n{out}"
        );
    }

    #[test]
    fn find_action_locates_an_entry_by_id_when_it_has_no_name() {
        // A `type: node` action may omit `name:` — its `id` is then the
        // identity the keybinding editor addresses it by.
        let src = "\
views:
  - name: tickets
    actions:
      - { key: d, id: delete }
";
        let out = set(
            src,
            &[
                PathStep::key("views"),
                PathStep::find("name", "tickets"),
                PathStep::key("actions"),
                PathStep::find_action("delete"),
            ],
            "key",
            &["D"],
        );
        assert!(out.contains("key: D"), "got:\n{out}");
    }
}
