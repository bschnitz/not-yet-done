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
                let comment = trailing_comment(key_line, loc.val_start_col - 1);
                format!("{prefix}{entry}: {rendered}{comment}")
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

/// Set `entry: values` inside the mapping named by the **last** [`PathStep`]
/// of `path` (which must be a [`PathStep::Key`]) — but, unlike [`set_entry`],
/// that mapping need not already exist or be populated. This is what a
/// per-node `shortcuts:` binding needs: the map is often present-but-empty
/// (`shortcuts:` with nothing under it) or absent entirely.
///
/// * If the mapping already has entries, this delegates to [`set_entry`]
///   (identical in-place / insert behaviour, comments preserved).
/// * If the key exists with an empty/null value, `entry: values` is inserted
///   as its first child, indented two spaces past the key.
/// * If the key is absent from its parent, both the `key:` line and its child
///   are inserted into the parent (after the parent's first entry).
pub fn set_entry_in_optional_map(
    source: &str,
    path: &[PathStep],
    entry: &str,
    values: &[String],
) -> EditResult<String> {
    let (map_key, parent_path) = match path.split_last() {
        Some((PathStep::Key(k), rest)) => (k.clone(), rest),
        _ => return Err("set_entry_in_optional_map: last path step must be a key".to_string()),
    };
    let rendered = render_value(values);
    let root = parse_yaml(0, source).map_err(|e| format!("YAML parse error: {e}"))?;
    let parent = navigate(&root, parent_path)?;
    let parent_map = parent
        .as_mapping()
        .ok_or_else(|| "parent of target map is not a mapping".to_string())?;

    // A populated mapping is exactly what the normal in-place editor handles.
    if parent_map
        .get_node(&map_key)
        .and_then(|n| n.as_mapping().map(|m| m.iter().next().is_some()))
        .unwrap_or(false)
    {
        return set_entry(source, path, entry, values);
    }

    let (mut lines, trailing_nl) = split_lines(source);
    match key_pos(parent_map, &map_key) {
        // Key present but empty/null: insert the child two spaces deeper.
        Some((key_line, key_col)) => {
            let child_indent = " ".repeat(key_col - 1 + 2);
            lines.insert(key_line, format!("{child_indent}{entry}: {rendered}"));
        }
        // Key absent: insert the `map_key:` line and its first child.
        None => {
            let (after_line, key_col) = first_entry_anchor(parent_map)
                .ok_or_else(|| "cannot insert into an empty mapping".to_string())?;
            let indent = " ".repeat(key_col - 1);
            let child_indent = " ".repeat(key_col - 1 + 2);
            lines.insert(after_line, format!("{child_indent}{entry}: {rendered}"));
            lines.insert(after_line, format!("{indent}{map_key}:"));
        }
    }
    Ok(join_lines(lines, trailing_nl))
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
        }
    }
    Ok(cur)
}

/// The `(line, column)` (both 1-based) of `entry`'s key scalar within `map`,
/// or `None` if absent. Unlike [`raw_loc`] this needs only the *key* span, so
/// it locates keys whose value is null/empty (a bare `shortcuts:` block).
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

fn render_value(values: &[String]) -> String {
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
    fn optional_map_delegates_when_map_is_populated() {
        // `shortcuts:` already has an entry → in-place insert, comments kept.
        let src = "\
views:
  - name: trackings
    shortcuts:
      d: delete  # soft-delete
";
        let vals = vec!["toggle-tracking".to_string()];
        let out = set_entry_in_optional_map(
            src,
            &[
                PathStep::key("views"),
                PathStep::find("name", "trackings"),
                PathStep::key("shortcuts"),
            ],
            "s",
            &vals,
        )
        .unwrap();
        let expected = "\
views:
  - name: trackings
    shortcuts:
      d: delete  # soft-delete
      s: 'toggle-tracking'
";
        assert_eq!(out, expected);
    }

    #[test]
    fn optional_map_inserts_into_empty_block() {
        // `shortcuts:` present but null → first child inserted under it.
        let src = "\
views:
  - name: condensed
    shortcuts:
  - name: tree
    key: t
";
        let vals = vec!["toggle-tracking".to_string()];
        let out = set_entry_in_optional_map(
            src,
            &[
                PathStep::key("views"),
                PathStep::find("name", "condensed"),
                PathStep::key("shortcuts"),
            ],
            "s",
            &vals,
        )
        .unwrap();
        let expected = "\
views:
  - name: condensed
    shortcuts:
      s: 'toggle-tracking'
  - name: tree
    key: t
";
        assert_eq!(out, expected);
    }

    #[test]
    fn optional_map_creates_absent_key() {
        // No `shortcuts:` at all → key + child inserted after the first entry.
        let src = "\
views:
  - name: trackings
    node_type: x
";
        let vals = vec!["toggle-tracking".to_string()];
        let out = set_entry_in_optional_map(
            src,
            &[
                PathStep::key("views"),
                PathStep::find("name", "trackings"),
                PathStep::key("shortcuts"),
            ],
            "s",
            &vals,
        )
        .unwrap();
        let expected = "\
views:
  - name: trackings
    shortcuts:
      s: 'toggle-tracking'
    node_type: x
";
        assert_eq!(out, expected);
    }

    #[test]
    fn missing_path_is_an_error() {
        let src = "global:\n  quit: ctrl+c\n";
        let err = set_entry(src, &[PathStep::key("nope")], "x", &[]).unwrap_err();
        assert!(err.contains("not found"), "got: {err}");
    }
}
