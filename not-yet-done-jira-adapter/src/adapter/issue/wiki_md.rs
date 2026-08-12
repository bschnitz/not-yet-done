//! Conversion between Jira wiki markup and Markdown for the `edit_markdown`
//! action.
//!
//! Markdown cannot represent everything Jira wiki markup can, so the safety
//! guarantee is enforced *per ticket at runtime* rather than by completeness:
//! [`roundtrip_diff`] converts a description to Markdown and back and reports
//! any divergence (modulo whitespace). `prepare("edit_markdown")` only opens
//! the Markdown editor when that check passes; otherwise it aborts with a
//! warning naming the offending fragment. Constructs the converter doesn't
//! handle therefore never corrupt a ticket — they simply fall back to the
//! plain `edit_full` (Jira-markup) flow.
//!
//! ## Handled subset
//!
//! | Jira wiki                     | Markdown                          |
//! |-------------------------------|-----------------------------------|
//! | `h1.` … `h6.`                 | `#` … `######`                    |
//! | `bq. x`                       | `> x`                             |
//! | `{quote}…{quote}`             | `::: quote … :::` (fenced div)    |
//! | `{panel:title=T\|k=v}…{panel}`| `## T <!-- panel k=v --> … <!-- /panel -->` |
//! | `{panel}…{panel}` (no title)  | `<!-- panel --> … <!-- /panel -->` |
//! | `{code:lang}…{code}`          | ```` ```lang … ``` ````           |
//! | `{noformat}…{noformat}`       | `::: noformat … :::`              |
//! | `----`                        | `---`                             |
//! | `\|\|h\|\|` + `\|c\|` table   | GFM table (with `---` separator)  |
//! | `\|c\|` data table (no header)| GFM table + `<!-- jira data-table -->` marker |
//! | `* / ** / # / #*` lists       | `- / 1. 2. 3.` with 2-space indent |
//! | `*bold*`                      | `**bold**`                        |
//! | `_italic_`                    | `_italic_` (already valid MD)     |
//! | `-strike-`                    | `~~strike~~`                      |
//! | `{{mono}}`                    | `` `mono` ``                      |
//! | `[text\|url]`                 | `[text](url)`                     |
//! | `{color:c}…{color}`           | `<span style="color:c">…</span>`  |
//! | single newline (in paragraph) | line + trailing `\` (hard break)  |
//!
//! Jira renders every newline as a visible line break, but a Markdown renderer
//! collapses a single newline to a space. To keep the meaning, `wiki_to_md`
//! appends a CommonMark hard-break `\` to each paragraph line that is followed
//! by another paragraph line; `md_to_wiki` strips it again (in Jira the newline
//! already is the break). Block constructs and blank lines are left alone.
//!
//! This convention also makes `md_to_wiki` tolerant of an `$EDITOR` that hard-
//! wraps long lines at a text width: because the converter only ever omits the
//! `\` on the *last* line of a paragraph, a paragraph line without a `\` that is
//! still followed by more paragraph text can only be an editor soft-wrap (e.g. a
//! long `![alt](url "title")` image link broken at its single space). Such
//! fragments are re-joined before inline conversion, so a re-wrapped but
//! otherwise unchanged buffer converts back to the identical wiki — the comment
//! diff never mistakes a reflow for an edit (which for a foreign comment would
//! otherwise fail the save with a bogus "not authored by you"). Joining before
//! conversion also fixes emphasis/links split across a wrap. A line that *does*
//! end with `\` stays a hard break. (Wrapped list-item first lines are not yet
//! rejoined — a rare case, since prose wraps less predictably than image links.)
//!
//! Attachment/image embeds are converted: `!name.png!` ⇄
//! `![name.png](attachments/name.png)` and the parametrised form
//! `!name.png|thumbnail!` ⇄ `![name.png](attachments/name.png "thumbnail")`
//! (params ride in the Markdown link title so they survive the round-trip).
//! The local path is always `attachments/<name>` — the ticket-workspace layout
//! writes each attachment there under its plain filename — and a filename with
//! spaces is wrapped in `<>` so the Markdown URL stays valid. Only embeds whose
//! filename carries an extension are recognised, so a bare `Wow!` exclamation
//! is left alone.
//!
//! A Jira table with a `||` header row becomes a GFM table. A cell may span
//! several physical lines in Jira; since GFM has no literal newline inside a
//! cell, the wrapped text is joined with `<br>` (and split back on the return
//! trip). Cosmetic cell spacing — casual padding (`Owner |`) and blank cells
//! filled with a bare space (`| |`) — is normalized by the round-trip guard, so
//! prettifying the table into GFM never registers as a divergence. The same
//! applies to a `{panel:...}` opener: padding around its `|`/`=` attribute
//! separators (a stray `title=T |k=v` space) is cosmetic — the title becomes a
//! heading whose trailing whitespace can't be restored — so `normalize_ws`
//! re-renders panel openers canonically and the padding is insignificant.
//!
//! A Jira table with no `||` header row (a headerless data table, often with
//! `*bold*` pseudo-header cells) has no direct GFM equivalent — GFM requires a
//! header row. A *regular* one (every row the same column count, ≥2 columns) is
//! still rendered as a GFM table: its first row becomes the header and a
//! `<!-- jira data-table -->` marker rides above it, which `md_to_wiki` uses to
//! restore the headerless `|…|` form losslessly. An *irregular* data table
//! (uneven column counts) is passed through verbatim in both directions — the
//! cells keep their Jira markup — so it round-trips by construction and stays
//! editable rather than being rejected by the guard.
//!
//! Anything else (`^sup^`/`~sub~`, `[~user]` mentions, `+ins+`, macros) is
//! passed through verbatim: it round-trips as long as it doesn't collide with
//! Markdown syntax, and the guard rejects the ticket if it does.
//!
//! ## Header tables
//!
//! On top of the body conversion, the 3b metadata header is shown as two GFM
//! tables in the Markdown editor: the editable fields (above the `---`
//! `EDITABLE_MARKER`) as the first table, the read-only fields (below it, up to
//! `===`) as the second — see [`header_to_md`] / [`header_from_md`]. This is a
//! *display* transform independent of the wiki⇄Markdown conversion: the
//! read-only table is ignored by the parser, and only the editable table must
//! round-trip. If the header carries anything but plain `key: value` lines (an
//! error/conflict banner), it stays in the plain form so the message is
//! readable, and both forms save correctly.

use std::sync::LazyLock;

use regex::{Captures, Regex};

/// Fenced-div fence used for quotes and noformat blocks — the pandoc-style
/// `:::` container. Chosen because it is visually distinct from every native
/// Markdown block, so the reverse pass can recover the exact Jira construct.
/// (Panels use the level-2-heading + `<!-- panel … -->` marker form instead,
/// which reads more naturally as a section title.)
const DIV_FENCE: &str = ":::";

/// Closing marker for a panel block on the Markdown side. The opener is either
/// a level-2 heading carrying a trailing `<!-- panel … -->` marker (when the
/// panel has a title) or a bare `<!-- panel … -->` marker line (when it does
/// not); both are closed by this line.
const PANEL_CLOSE: &str = "<!-- /panel -->";

/// Marker line placed above a GFM table that originated from a *header-less*
/// Jira data table (rows led by a single `|`, no `||` header). It lets the
/// author's `*bold*` pseudo-header render as the real GFM header while telling
/// `md_to_wiki` to write the table back in the header-less `|…|` form, so the
/// round-trip is lossless. Invisible in rendered Markdown, like the panel and
/// comment markers.
const DATA_TABLE_MARK: &str = "<!-- jira data-table -->";

/// Apply `f` to only the *body* region of a 3b template buffer — the text
/// between the `===` marker and the trailing CACHE section (or EOF) — leaving
/// the editable/read-only header and the CACHE section untouched. Used to
/// convert the body between Markdown and Jira markup without disturbing the
/// metadata scaffolding. If there is no `===` marker the text is returned
/// unchanged.
pub(super) fn map_3b_body(text: &str, f: impl Fn(&str) -> String) -> String {
    use super::markers::{BODY_MARKER, CACHE_MARKER};
    let lines: Vec<&str> = text.split('\n').collect();
    let Some(body_start) = lines.iter().position(|l| l.trim_end() == BODY_MARKER) else {
        return text.to_string();
    };
    let body_end = lines
        .iter()
        .position(|l| l.trim_end() == CACHE_MARKER)
        .unwrap_or(lines.len());
    let head = lines[..=body_start].join("\n");
    let body = lines[body_start + 1..body_end].join("\n");
    let converted = f(&body);
    if body_end < lines.len() {
        let tail = lines[body_end..].join("\n");
        format!("{head}\n{converted}\n{tail}")
    } else {
        format!("{head}\n{converted}")
    }
}

// ─────────────────────────── 3b header ⇄ tables ─────────────────────────

/// Render the metadata header of a 3b buffer (everything before the `===`
/// `BODY_MARKER`) as two GFM tables for the Markdown editor: the editable
/// fields above the `---` `EDITABLE_MARKER` become the first table, the
/// read-only fields below it the second. The body and any trailing CACHE
/// section are left untouched.
///
/// If the header carries anything other than `key: value` lines — an
/// error/conflict banner, git-style conflict markers — the buffer is returned
/// unchanged, so those stay visible in the familiar plain form. Reversed by
/// [`header_from_md`].
pub(super) fn header_to_md(buf: &str) -> String {
    use super::markers::{BODY_MARKER, EDITABLE_MARKER};
    let lines: Vec<&str> = buf.split('\n').collect();
    let Some(body_idx) = lines.iter().position(|l| l.trim_end() == BODY_MARKER) else {
        return buf.to_string();
    };
    let Some(edit_idx) = lines[..body_idx]
        .iter()
        .position(|l| l.trim_end() == EDITABLE_MARKER)
    else {
        return buf.to_string();
    };
    let (Some(editable_kv), Some(readonly_kv)) = (
        parse_header_kv(&lines[..edit_idx]),
        parse_header_kv(&lines[edit_idx + 1..body_idx]),
    ) else {
        return buf.to_string();
    };

    let mut out: Vec<String> = Vec::new();
    out.extend(render_header_table(&editable_kv));
    out.push(String::new());
    out.extend(render_header_table(&readonly_kv));
    out.push(String::new());
    out.extend(lines[body_idx..].iter().map(|s| s.to_string()));
    out.join("\n")
}

/// Reverse of [`header_to_md`]: turn the two GFM header tables back into the
/// plain `key: value` / `---` / `===` header the parser expects. When the
/// header region carries no table (the plain form — e.g. after an error
/// reopen), the buffer is returned unchanged, so both forms save correctly.
pub(super) fn header_from_md(buf: &str) -> String {
    use super::markers::{BODY_MARKER, EDITABLE_MARKER};
    let lines: Vec<&str> = buf.split('\n').collect();
    let Some(body_idx) = lines.iter().position(|l| l.trim_end() == BODY_MARKER) else {
        return buf.to_string();
    };
    let head = &lines[..body_idx];
    if !head.iter().any(|l| l.trim_start().starts_with('|')) {
        return buf.to_string();
    }
    let blocks = header_table_blocks(head);
    let editable = blocks.first().cloned().unwrap_or_default();
    let readonly = blocks.get(1).cloned().unwrap_or_default();

    let mut out: Vec<String> = Vec::new();
    for (k, v) in &editable {
        out.push(format!("{k}: {v}"));
    }
    out.push(EDITABLE_MARKER.to_string());
    for (k, v) in &readonly {
        out.push(format!("{k}: {v}"));
    }
    out.extend(lines[body_idx..].iter().map(|s| s.to_string()));
    out.join("\n")
}

/// Parse a header region into `(key, value)` pairs, skipping blank lines.
/// Returns `None` if any non-blank line is not a plain `key: value` (a banner
/// or conflict marker) — the caller then leaves the header in its plain form.
fn parse_header_kv(lines: &[&str]) -> Option<Vec<(String, String)>> {
    let mut kv = Vec::new();
    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with(['#', '<', '>', '=']) {
            return None;
        }
        let (k, v) = t.split_once(':')?;
        kv.push((k.trim().to_string(), v.trim().to_string()));
    }
    Some(kv)
}

/// Render `(key, value)` pairs as a two-column GFM table. Pipes in cells are
/// escaped so they don't split the column.
fn render_header_table(kv: &[(String, String)]) -> Vec<String> {
    let mut out = vec!["| Field | Value |".to_string(), "| --- | --- |".to_string()];
    for (k, v) in kv {
        out.push(format!("| {} | {} |", escape_cell(k), escape_cell(v)));
    }
    out
}

fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// Split a header region into table blocks (separated by blank lines), each as
/// `(key, value)` data rows. The `| Field | Value |` header row and the
/// `| --- | --- |` separator row are dropped.
fn header_table_blocks(head: &[&str]) -> Vec<Vec<(String, String)>> {
    let mut blocks = Vec::new();
    let mut current: Vec<(String, String)> = Vec::new();
    let mut in_table = false;
    for line in head {
        let t = line.trim();
        if t.is_empty() {
            if in_table {
                blocks.push(std::mem::take(&mut current));
                in_table = false;
            }
            continue;
        }
        if !t.starts_with('|') {
            continue;
        }
        in_table = true;
        if is_md_table_separator(t) {
            continue;
        }
        let (k, v) = split_header_row(t);
        if k == "Field" && v == "Value" {
            continue;
        }
        current.push((k, v));
    }
    if in_table {
        blocks.push(current);
    }
    blocks
}

/// Split a `| key | value |` data row into `(key, value)`, honouring `\|`
/// escapes inside cells.
fn split_header_row(row: &str) -> (String, String) {
    let protected = row.replace("\\|", "\u{0}");
    let inner = protected.trim().trim_matches('|');
    let mut parts = inner.splitn(2, '|');
    let restore = |s: &str| s.replace('\u{0}', "|").trim().to_string();
    (
        restore(parts.next().unwrap_or("")),
        restore(parts.next().unwrap_or("")),
    )
}

// ─────────────────────────── wiki → markdown ───────────────────────────

/// Convert a Jira wiki-markup body to Markdown.
pub(super) fn wiki_to_md(wiki: &str) -> String {
    // Decouple any block terminator glued to the end of a content line first,
    // so the line-based block scanner below sees it as a standalone close.
    let wiki = split_block_macros(wiki);
    let lines: Vec<&str> = wiki.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    // Running item counts of the ordered list currently being emitted, one entry
    // per nesting level. Jira writes every ordered item as a bare `#`, so the
    // numbers have to be generated here (see `ordered_marker`).
    let mut ordinals: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // Block-construct detection is whitespace-insensitive: Jira ignores
        // leading spaces before block macros (e.g. a stray ` {panel}` still
        // closes a panel), and the round-trip guard compares modulo whitespace.
        let trimmed = line.trim();

        // Anything that is not a list item ends the current list, so the next
        // ordered list starts counting at 1 again.
        if parse_list_item(trimmed).is_none() {
            ordinals.clear();
        }

        // {code} / {noformat} — verbatim body, no inline conversion inside.
        if let Some(open) = parse_code_open(trimmed) {
            let (close_tag, block, next) = collect_block(&lines, i + 1, open.close);
            match open.kind {
                VerbatimKind::Code => {
                    out.push(format!("```{}", open.lang));
                    out.extend(block.iter().map(|s| s.to_string()));
                    out.push("```".to_string());
                }
                VerbatimKind::NoFormat => {
                    out.push(format!("{DIV_FENCE} noformat"));
                    out.extend(block.iter().map(|s| s.to_string()));
                    out.push(DIV_FENCE.to_string());
                }
            }
            let _ = close_tag;
            i = next;
            continue;
        }

        // {panel:...} — rendered as a level-2 heading (when the first attribute
        // is a title) or a bare `<!-- panel … -->` marker, closed by
        // `<!-- /panel -->`; body converted. Blank lines around the body keep
        // it readable in the editor and are ignored by the round-trip guard.
        if let Some(attrs) = parse_panel_open(trimmed) {
            let (_close, block, next) = collect_block(&lines, i + 1, "{panel}");
            out.push(String::new());
            out.push(render_panel_open_md(&attrs));
            out.push(String::new());
            let inner = wiki_to_md(&block.join("\n"));
            out.extend(inner.split('\n').map(|s| s.to_string()));
            out.push(String::new());
            out.push(PANEL_CLOSE.to_string());
            i = next;
            continue;
        }

        // {quote} — fenced div, body converted.
        if trimmed == "{quote}" {
            let (_close, block, next) = collect_block(&lines, i + 1, "{quote}");
            out.push(format!("{DIV_FENCE} quote"));
            let inner = wiki_to_md(&block.join("\n"));
            out.extend(inner.split('\n').map(|s| s.to_string()));
            out.push(DIV_FENCE.to_string());
            i = next;
            continue;
        }

        // Table: consume the whole block (multi-line cells included) and let
        // `table_to_md` decide GFM vs. verbatim.
        if is_table_row(trimmed) {
            let (block, next) = collect_table_block(&lines, i);
            out.extend(table_to_md(&block));
            i = next;
            continue;
        }

        // hN. heading
        if let Some((level, rest)) = parse_heading(trimmed) {
            out.push(format!(
                "{} {}",
                "#".repeat(level),
                convert_inline_w2m(rest)
            ));
            i += 1;
            continue;
        }

        // bq. blockquote (single line)
        if let Some(rest) = trimmed.strip_prefix("bq. ") {
            out.push(format!("> {}", convert_inline_w2m(rest)));
            i += 1;
            continue;
        }

        // Horizontal rule: a line of 4+ dashes only.
        if is_hrule_wiki(trimmed) {
            out.push("---".to_string());
            i += 1;
            continue;
        }

        // List item (bullets `*`, numbers `#`, arbitrary nesting).
        if let Some((depth, ordered, rest)) = parse_list_item(trimmed) {
            let indent = "  ".repeat(depth - 1);
            let marker = if ordered {
                format!("{}.", ordered_marker(&mut ordinals, depth))
            } else {
                // A bullet at this level interrupts an ordered list of the same
                // depth, so its counter restarts at the next `#`.
                ordinals.truncate(depth - 1);
                "-".to_string()
            };
            let mut rendered = format!("{indent}{marker} {}", convert_inline_w2m(rest));
            // Mark a trailing hard break exactly as the plain-line branch does
            // when a loose paragraph line follows. This lets `md_to_wiki` tell a
            // genuine following line from an editor soft-wrap of this item: a
            // wrapped item's tail fragment carries no `\`, a real next line does.
            if lines
                .get(i + 1)
                .is_some_and(|n| is_paragraph_continuation_w(n))
            {
                rendered.push_str(" \\");
            }
            out.push(rendered);
            i += 1;
            continue;
        }

        // Plain line: inline conversion. A single newline is a visible line
        // break in Jira but is swallowed by a Markdown renderer, so when the
        // next line continues the same paragraph, mark this one with a trailing
        // `\` (CommonMark hard break). `md_to_wiki` strips it again — in Jira
        // the newline itself carries the break, so the marker is redundant on
        // the way back.
        let mut rendered = convert_inline_w2m(line);
        // Escape a leading marker that Markdown would parse as a block construct
        // (bullet / ordered item / blockquote) but Jira treats as plain text, so
        // the reverse pass does not turn this prose into a list or quote.
        if md_block_marker_prefix(&rendered) {
            rendered.insert(0, '\\');
        }
        if !line.trim().is_empty()
            && lines
                .get(i + 1)
                .is_some_and(|n| is_paragraph_continuation_w(n))
        {
            rendered.push_str(" \\");
        }
        out.push(rendered);
        i += 1;
    }
    out.join("\n")
}

/// True if a *wiki* line is plain paragraph text that continues a paragraph —
/// i.e. it is non-blank and opens none of the block constructs handled before
/// the plain-line fallback in [`wiki_to_md`]. Used to decide whether the
/// preceding line's newline needs an explicit Markdown hard-break marker.
fn is_paragraph_continuation_w(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty()
        && parse_code_open(t).is_none()
        && parse_panel_open(t).is_none()
        && t != "{quote}"
        && !is_table_row(t)
        && parse_heading(t).is_none()
        && !t.starts_with("bq. ")
        && !is_hrule_wiki(t)
        && parse_list_item(t).is_none()
}

// ─────────────────────────── markdown → wiki ───────────────────────────

/// Convert a Markdown body back to Jira wiki markup.
pub(super) fn md_to_wiki(md: &str) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    // Jira marker chain of the list currently being emitted, one char per
    // nesting level (see `wiki_list_markers`). Markdown expresses nesting by
    // indentation only, so the ancestors' bullet/ordered kind has to be
    // remembered to write a mixed run like `*#` back out.
    let mut markers: Vec<char> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Anything that is not a list item ends the current list. (A soft-wrap
        // fragment belonging to an item never reaches here — the list branch
        // below consumes it.)
        if parse_md_list_item(line).is_none() {
            markers.clear();
        }

        // ``` fenced code → {code[:lang]}
        if let Some(lang) = parse_md_code_fence(trimmed) {
            let (block, next) = collect_until(&lines, i + 1, |l| l.trim() == "```");
            if lang.is_empty() {
                out.push("{code}".to_string());
            } else {
                out.push(format!("{{code:{lang}}}"));
            }
            out.extend(block.iter().map(|s| s.to_string()));
            out.push("{code}".to_string());
            i = next;
            continue;
        }

        // ::: fenced div → quote / noformat
        if let Some(div) = parse_md_div_open(trimmed) {
            let (block, next) = collect_until(&lines, i + 1, |l| l.trim() == DIV_FENCE);
            match div.kind {
                DivKind::NoFormat => {
                    out.push("{noformat}".to_string());
                    out.extend(block.iter().map(|s| s.to_string()));
                    out.push("{noformat}".to_string());
                }
                DivKind::Quote => {
                    out.push("{quote}".to_string());
                    let inner = md_to_wiki(&block.join("\n"));
                    out.extend(inner.split('\n').map(|s| s.to_string()));
                    out.push("{quote}".to_string());
                }
            }
            i = next;
            continue;
        }

        // Panel: `## title <!-- panel attrs -->` (heading form) or a bare
        // `<!-- panel attrs -->` marker, closed by `<!-- /panel -->`. Checked
        // before the ATX-heading branch so a panel heading is never mistaken
        // for a plain `h2.`.
        let panel_open = if let Some((title, attrs)) = parse_md_panel_open(trimmed) {
            Some((title, attrs, i + 1))
        } else {
            // The opener may have been hard-wrapped across several lines by an
            // editor; try to reassemble it before giving up.
            rejoin_wrapped_panel_open(&lines, i).and_then(|(joined, body_start)| {
                parse_md_panel_open(joined.trim()).map(|(t, a)| (t, a, body_start))
            })
        };
        if let Some((title, attrs, body_start)) = panel_open {
            let (block, next) = collect_until(&lines, body_start, |l| l.trim() == PANEL_CLOSE);
            let mut wiki_attrs: Vec<(String, String)> = Vec::new();
            if let Some(t) = title {
                wiki_attrs.push(("title".to_string(), t));
            }
            wiki_attrs.extend(attrs);
            if wiki_attrs.is_empty() {
                out.push("{panel}".to_string());
            } else {
                out.push(format!("{{panel:{}}}", render_attrs_wiki(&wiki_attrs)));
            }
            let body = strip_blank_edges(&block);
            let inner = md_to_wiki(&body.join("\n"));
            out.extend(inner.split('\n').map(|s| s.to_string()));
            out.push("{panel}".to_string());
            i = next;
            continue;
        }

        // A `<!-- jira data-table -->` marker introduces a GFM table that must
        // be written back as a header-less Jira data table (`|…|`, no `||`):
        // skip the marker and consume the following GFM table (header +
        // separator + body rows) via `data_table_to_wiki`. If no valid table
        // follows (the user mangled it), just drop the marker and let the next
        // iterations handle the lines.
        if trimmed == DATA_TABLE_MARK {
            i += 1;
            if i + 1 < lines.len()
                && is_table_row(lines[i].trim_end())
                && is_md_table_separator(lines[i + 1].trim_end())
            {
                let start = i;
                i += 2; // header + separator
                while i < lines.len() && is_table_row(lines[i].trim_end()) {
                    i += 1;
                }
                out.extend(data_table_to_wiki(&lines[start..i]));
            }
            continue;
        }

        // GFM table: header row followed by a `---` separator row.
        if is_table_row(trimmed)
            && i + 1 < lines.len()
            && is_md_table_separator(lines[i + 1].trim_end())
        {
            let start = i;
            i += 1; // header
            i += 1; // separator
            while i < lines.len() && is_table_row(lines[i].trim_end()) {
                i += 1;
            }
            out.extend(table_to_wiki(&lines[start..i]));
            continue;
        }

        // A raw `|`-leading run with no marker and no GFM separator: an
        // *irregular* data-only Jira table (uneven column counts) that
        // `wiki_to_md` left verbatim because it can't be a GFM table. Emit it
        // back verbatim — splitting and rejoining the cells would rewrite Jira
        // `*bold*` cells as `_italic_` and collapse whitespace-only cells
        // (`| | |`). A *regular* data-only table arrives as a marked GFM table
        // and is handled by the `DATA_TABLE_MARK` branch above; checked after
        // the GFM branch so a real header table is never caught here.
        if is_table_row(trimmed) {
            let (block, next) = collect_table_block(&lines, i);
            out.extend(block.iter().map(|s| s.to_string()));
            i = next;
            continue;
        }

        // ATX heading
        if let Some((level, rest)) = parse_md_heading(trimmed) {
            out.push(format!("h{level}. {}", convert_inline_m2w(rest)));
            i += 1;
            continue;
        }

        // Blockquote (single `> ` line → bq.)
        if let Some(rest) = trimmed.strip_prefix("> ") {
            out.push(format!("bq. {}", convert_inline_m2w(rest)));
            i += 1;
            continue;
        }

        // Horizontal rule
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            out.push("----".to_string());
            i += 1;
            continue;
        }

        // List item. Re-join editor soft-wrapped fragments onto the item, the
        // same tolerance the plain-paragraph branch applies below. `wiki_to_md`
        // marks a list line with a trailing `\` exactly when a loose paragraph
        // line follows it, and never otherwise, so a fragment with no `\`
        // followed by plain paragraph text is the tail of an editor-wrapped item
        // (not a sibling line) and belongs to this item. Joining before inline
        // conversion also repairs emphasis/links split across the wrap.
        if let Some((depth, ordered, rest)) = parse_md_list_item(line) {
            let marker = wiki_list_markers(&mut markers, depth, ordered);
            let mut frag = rest.to_string();
            while !frag.ends_with('\\')
                && i + 1 < lines.len()
                && is_paragraph_continuation_m(lines[i + 1])
            {
                i += 1;
                let nxt = lines[i].trim();
                if !frag.is_empty() && !nxt.is_empty() {
                    frag.push(' ');
                }
                frag.push_str(nxt);
            }
            let content = frag
                .strip_suffix('\\')
                .map(|s| s.trim_end())
                .unwrap_or(frag.as_str());
            out.push(format!("{marker} {}", convert_inline_m2w(content)));
            i += 1;
            continue;
        }

        // Blank line: preserved verbatim as a paragraph separator.
        if trimmed.is_empty() {
            out.push(String::new());
            i += 1;
            continue;
        }

        // Termination guard: any line that fell through every branch above yet
        // is *not* itself a paragraph continuation would make `run_end == i`
        // below and spin forever. This happens on malformed/edited input — most
        // notably an orphaned `<!-- /panel -->` left behind when an editor
        // hard-wrapped a long `## title <!-- panel … -->` opener, so the panel
        // collector never matched it. Emit such a line verbatim and advance, so
        // `md_to_wiki` always terminates (the round-trip guard then rejects the
        // ticket, rather than the save hanging).
        if !is_paragraph_continuation_m(line) {
            out.push(strip_block_escape(&convert_inline_m2w(trimmed)));
            i += 1;
            continue;
        }

        // Plain paragraph run. `wiki_to_md` marks every *intra*-paragraph break
        // with a trailing `\` (CommonMark hard break) and leaves the last line
        // of a paragraph unmarked. So within a run of continuation lines, a line
        // that does NOT end with `\` but is still followed by more paragraph text
        // cannot have been produced by the converter — it is an editor soft-wrap
        // (the user's `$EDITOR` broke a long line, e.g. an `![alt](url "title")`
        // image link, at a space). Re-join those fragments before converting, so
        // a re-wrapped but otherwise untouched buffer round-trips to the exact
        // same wiki and is never mis-detected as an edit (which for a foreign
        // comment surfaces as a bogus "not authored by you" on save). Lines that
        // DO end with `\` are true hard breaks: each becomes its own Jira line.
        let run_end = {
            let mut j = i;
            while j < lines.len() && is_paragraph_continuation_m(lines[j]) {
                j += 1;
            }
            j
        };
        let mut k = i;
        while k < run_end {
            let mut frag = trim_ascii_end(lines[k]).to_string();
            // Join soft-wrapped fragments (no trailing `\`) onto this logical
            // line, separated by a single space, until one ends with a hard
            // break or the run ends.
            while !frag.ends_with('\\') && k + 1 < run_end {
                k += 1;
                let nxt = trim_ascii_edges(lines[k]);
                if !frag.is_empty() && !nxt.is_empty() {
                    frag.push(' ');
                }
                frag.push_str(nxt);
            }
            // A trailing `\` is a hard break only when another logical line
            // follows; on the run's last line it is a literal backslash.
            let content = if frag.ends_with('\\') && k + 1 < run_end {
                frag[..frag.len() - 1].trim_end().to_string()
            } else {
                frag.clone()
            };
            out.push(strip_block_escape(&convert_inline_m2w(&content)));
            k += 1;
        }
        i = run_end;
    }
    out.join("\n")
}

/// Trim only ASCII spaces/tabs from both ends. Rust's `str::trim` also strips a
/// no-break space (U+00A0), which Jira uses as meaningful indentation/filler; an
/// editor that hard-wraps a line right after an NBSP would otherwise see the
/// NBSP silently dropped on rejoin, making a purely re-wrapped comment diverge
/// from its snapshot (bogus "edited"/"not authored by you" on save). NBSP is
/// preserved here, matching [`normalize_ws`], which also keeps NBSP intact.
fn trim_ascii_edges(s: &str) -> &str {
    s.trim_matches(|c| c == ' ' || c == '\t')
}

/// [`trim_ascii_edges`] for the trailing end only.
fn trim_ascii_end(s: &str) -> &str {
    s.trim_end_matches([' ', '\t'])
}

/// True if a *Markdown* line is plain paragraph text that continues a paragraph
/// — non-blank and opening none of the block constructs handled before the
/// plain-line fallback in [`md_to_wiki`]. Mirror of [`is_paragraph_continuation_w`]
/// for the reverse direction's hard-break stripping.
fn is_paragraph_continuation_m(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty()
        && parse_md_code_fence(t).is_none()
        && parse_md_div_open(t).is_none()
        && parse_md_panel_open(t).is_none()
        && t != PANEL_CLOSE
        && t != DATA_TABLE_MARK
        && !is_table_row(t)
        && parse_md_heading(t).is_none()
        && !t.starts_with("> ")
        && t != "---"
        && t != "***"
        && t != "___"
        && parse_md_list_item(line).is_none()
}

// ───────────────────────────── round-trip guard ─────────────────────────

/// If `wiki` does not survive a `wiki → md → wiki` round-trip (modulo
/// whitespace), return a short human-readable diff of the first offending
/// region; otherwise `None`. Used by `prepare("edit_markdown")` to refuse to
/// open the Markdown editor for a ticket the converter would mangle.
pub(super) fn roundtrip_diff(wiki: &str) -> Option<String> {
    let back = md_to_wiki(&wiki_to_md(wiki));
    if normalize_ws(wiki) == normalize_ws(&back) {
        return None;
    }
    Some(first_divergence(&normalize_ws(wiki), &normalize_ws(&back)))
}

/// Editors that hard-wrap long lines (a fixed `textwidth`) can split one of our
/// structural HTML-comment markers across several physical lines — the comment
/// heading `### @author ts <!-- jira comment id=N -->`, the section divider
/// `## Comments <!-- jira comments section -->`, a `## title <!-- panel … -->`
/// panel opener, or `<!-- jira data-table -->`. Once the `<!-- … -->` no longer
/// sits on one line the structural parsers (`split_md_comment_blocks`, the
/// section finder, `parse_md_panel_open`) stop recognizing it, so a purely
/// re-wrapped comment loses its block boundary and reads as edited/removed on
/// save — surfacing bogus "not authored by you" / "modified upstream" errors.
///
/// Rejoin any marker that opens (`<!--`) on one line and only closes (`-->`) on
/// a later one back onto a single logical line, one space per dropped break
/// (mirroring the editor). Bails on a blank line before the close, so a stray
/// unbalanced `<!--` in body content is left verbatim. The `<!--`/`-->` tokens
/// carry no interior space, so a space-breaking editor never splits them —
/// substring detection is reliable. Idempotent on unwrapped input.
pub(super) fn rejoin_wrapped_html_markers(md: &str) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    // Join lines[start..] forward (one space per break) until a line closes the
    // marker with `-->`. Bails at a blank line (a marker never spans one). On
    // success returns the single logical line and the index just past it.
    let join_until_close = |start: usize| -> Option<(String, usize)> {
        let mut joined = trim_ascii_end(lines[start]).to_string();
        let mut j = start + 1;
        while j < lines.len() {
            let t = lines[j].trim();
            if t.is_empty() {
                return None;
            }
            joined.push(' ');
            joined.push_str(t);
            j += 1;
            if t.contains("-->") {
                return Some((joined, j));
            }
        }
        None
    };
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        // Anchor at the marker's natural start so it never detaches from its
        // heading text: an ATX heading (`##` panel / section, `###` comment)
        // whose `<!-- … -->` marker may have wrapped — possibly beginning on a
        // *later* physical line — or a bare marker line that opens but does not
        // close here. Complete single-line markers already contain `-->` and are
        // left untouched.
        let heading_open = t.starts_with("##") && !line.contains("-->");
        let bare_marker_open = t.starts_with("<!--") && !line.contains("-->");
        if heading_open || bare_marker_open {
            // Only collapse when the run actually closes a marker; a plain
            // heading followed by prose bails (no `-->` before a blank/eof) and
            // is emitted verbatim.
            if let Some((joined, next)) = join_until_close(i) {
                if joined.contains("<!--") {
                    out.push(joined);
                    i = next;
                    continue;
                }
            }
        }
        out.push(line.to_string());
        i += 1;
    }
    out.join("\n")
}

/// Whitespace normalization for the idempotence comparison: normalize EOLs,
/// trim each line, and drop *all* blank lines. Blank lines are therefore
/// insignificant to the round-trip guard, which lets the converter insert
/// readability padding (e.g. around panels) without ever risking a false
/// divergence. The comment-conflict detector in `edit_with_comments` reuses
/// this so a whitespace-only round-trip artifact (e.g. an editor re-wrapping
/// an image line) never registers as a foreign or local edit.
pub(super) fn normalize_ws(s: &str) -> String {
    // Normalize EOLs, then decouple glued block terminators (so a `{panel}`
    // glued to the end of a content line compares equal to the standalone form
    // the converter emits). Then walk the lines: inside a table block, collapse
    // the ASCII padding around every `|` so cosmetic cell spacing (`| |` vs
    // `||`, `Owner |` vs `Owner|`) is insignificant to the guard — the GFM
    // prettifier normalizes that spacing, so a real table must be free to
    // differ from its verbatim source by it. Outside tables, trim each line and
    // drop all blank lines.
    let s = s.replace("\r\n", "\n").replace('\r', "\n");
    let s = split_block_macros(&s);
    let lines: Vec<&str> = s.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if is_table_row(trimmed) {
            let (block, next) = collect_table_block(&lines, i);
            out.extend(block.iter().map(|l| normalize_table_line(l)));
            i = next;
            continue;
        }
        // A `{panel:...}` opener: collapse the cosmetic padding around its `|`
        // and `=` (a stray `title=X |k=v` space) by re-rendering the parsed
        // attributes canonically. The panel title becomes a level-2 heading
        // whose trailing whitespace is trimmed and can't be restored, so this
        // padding must be insignificant to the guard — like table-cell padding.
        if let Some(attrs) = parse_panel_open(trimmed) {
            out.push(render_panel_wiki(&attrs));
            i += 1;
            continue;
        }
        if !trimmed.is_empty() {
            out.push(collapse_ascii_spaces(trimmed));
        }
        i += 1;
    }
    out.join("\n")
}

/// Collapse every run of ASCII spaces/tabs inside a prose line to a single
/// space, leaving NBSP (`\u{00A0}`, used as intentional content filler) intact.
/// Real tickets carry stray *internal* double spaces; when an editor hard-wraps
/// a long line whose break lands at such a run, the trailing space is dropped
/// and re-joining re-inserts only one — so a purely re-wrapped, untouched line
/// would otherwise diverge from its unwrapped snapshot. Since the guard already
/// treats whitespace as insignificant (it trims and drops blank lines), interior
/// space runs must be insignificant too; this keeps a re-wrapped comment from
/// reading as edited (the bogus "not authored by you" on save).
fn collapse_ascii_spaces(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut prev_space = false;
    for c in line.chars() {
        if c == ' ' || c == '\t' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

/// Collapse the ASCII space/tab padding around each `|` in a single physical
/// table line, leaving NBSP (content filler) intact. Applied only inside a
/// detected table block, so prose containing a literal `|` is never touched.
fn normalize_table_line(line: &str) -> String {
    line.split('|').map(trim_cell).collect::<Vec<_>>().join("|")
}

/// First diverging line pair, as `- <original>` / `+ <roundtripped>`. Both
/// lines are shown debug-quoted (`{:?}`) so otherwise-invisible differences —
/// trailing spaces, zero-width or control characters, an empty vs. shifted line
/// — are legible in the guard's error message; a couple of preceding lines are
/// echoed as context so the offending region is easy to locate in the ticket.
fn first_divergence(a: &str, b: &str) -> String {
    let al: Vec<&str> = a.split('\n').collect();
    let bl: Vec<&str> = b.split('\n').collect();
    for idx in 0..al.len().max(bl.len()) {
        let x = al.get(idx).copied().unwrap_or("");
        let y = bl.get(idx).copied().unwrap_or("");
        if x != y {
            let mut ctx = String::new();
            for c in idx.saturating_sub(2)..idx {
                if let Some(l) = al.get(c) {
                    ctx.push_str(&format!("    {}: {l:?}\n", c + 1));
                }
            }
            return format!("line {}:\n{ctx}  - {x:?}\n  + {y:?}", idx + 1);
        }
    }
    "(whole-document divergence)".to_string()
}

// ───────────────────────────── block helpers ────────────────────────────

enum VerbatimKind {
    Code,
    NoFormat,
}

struct CodeOpen {
    kind: VerbatimKind,
    lang: String,
    close: &'static str,
}

/// Parse `{code}`, `{code:lang}` or `{noformat}` opening a verbatim block.
/// Only a bare language token is accepted; parameterized forms
/// (`{code:title=…}`) fall through so the guard catches them.
fn parse_code_open(line: &str) -> Option<CodeOpen> {
    if line == "{noformat}" {
        return Some(CodeOpen {
            kind: VerbatimKind::NoFormat,
            lang: String::new(),
            close: "{noformat}",
        });
    }
    if line == "{code}" {
        return Some(CodeOpen {
            kind: VerbatimKind::Code,
            lang: String::new(),
            close: "{code}",
        });
    }
    if let Some(inner) = line
        .strip_prefix("{code:")
        .and_then(|s| s.strip_suffix('}'))
    {
        // Only a bare language identifier (no `=`, no `|`).
        if !inner.is_empty() && !inner.contains('=') && !inner.contains('|') {
            return Some(CodeOpen {
                kind: VerbatimKind::Code,
                lang: inner.to_string(),
                close: "{code}",
            });
        }
    }
    None
}

/// Collect lines until (and consuming) the `close` tag. Returns the close tag
/// actually seen (or the expected one at EOF), the block body, and the index
/// just past the close.
fn collect_block<'a>(
    lines: &[&'a str],
    start: usize,
    close: &'a str,
) -> (&'a str, Vec<&'a str>, usize) {
    let mut body = Vec::new();
    let mut i = start;
    while i < lines.len() {
        if lines[i].trim() == close {
            return (close, body, i + 1);
        }
        body.push(lines[i]);
        i += 1;
    }
    (close, body, i)
}

fn collect_until<'a>(
    lines: &[&'a str],
    start: usize,
    is_close: impl Fn(&str) -> bool,
) -> (Vec<&'a str>, usize) {
    let mut body = Vec::new();
    let mut i = start;
    while i < lines.len() {
        if is_close(lines[i]) {
            return (body, i + 1);
        }
        body.push(lines[i]);
        i += 1;
    }
    (body, i)
}

/// Block terminators Jira accepts glued to the end of a content line — e.g.
/// `* last point {panel}` or `!mock.png!{panel}`. Jira closes the block right
/// there, but the line-based scanner in [`wiki_to_md`] only recognises a
/// terminator that sits alone on its line ([`collect_block`] matches on
/// `line.trim() == close`), so a glued terminator would be swallowed as body
/// text and the block would over-consume everything up to the next standalone
/// close — most visibly making the *following* titled panel lose its heading.
const GLUED_TERMINATORS: [&str; 4] = ["{panel}", "{code}", "{noformat}", "{quote}"];

/// Move any block terminator glued to the end of a content line onto its own
/// line, so the block scanner sees it as a standalone close. A terminator
/// already alone on its line has no content before it and is left untouched,
/// which makes this idempotent — safe to run at the top of [`wiki_to_md`]
/// (including its recursive calls for panel/quote bodies) and inside
/// [`normalize_ws`], so the decoupled form the converter produces compares
/// equal to the glued original under the round-trip guard.
///
/// A terminator that is genuinely part of verbatim `{code}`/`{noformat}` body
/// text (a code line literally ending in `{code}`) would be split too, but that
/// only makes the round-trip diverge and the guard fall back to `edit_full` —
/// it never corrupts the ticket.
fn split_block_macros(wiki: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in wiki.split('\n') {
        let trimmed = line.trim_end();
        let split = GLUED_TERMINATORS.iter().find_map(|term| {
            trimmed
                .strip_suffix(*term)
                .filter(|head| !head.trim().is_empty())
                .map(|head| (head.trim_end().to_string(), *term))
        });
        match split {
            Some((head, term)) => {
                out.push(head);
                out.push(term.to_string());
            }
            None => out.push(line.to_string()),
        }
    }
    out.join("\n")
}

/// Parse `{panel}` / `{panel:k=v|k2=v2}`; returns the ordered attribute list
/// (empty for a bare panel). `None` if the line is not a panel opener.
fn parse_panel_open(line: &str) -> Option<Vec<(String, String)>> {
    if line == "{panel}" {
        return Some(Vec::new());
    }
    let inner = line
        .strip_prefix("{panel:")
        .and_then(|s| s.strip_suffix('}'))?;
    let mut attrs = Vec::new();
    for part in inner.split('|') {
        let (k, v) = part.split_once('=')?;
        attrs.push((k.trim().to_string(), v.trim().to_string()));
    }
    Some(attrs)
}

/// Render attributes for a Markdown fenced-div header: `key="value"`
/// (quoted, because titles carry spaces), space-separated.
fn render_attrs_md(attrs: &[(String, String)]) -> String {
    attrs
        .iter()
        .map(|(k, v)| format!("{k}=\"{v}\""))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render attributes back into a Jira `{panel:...}` header: `key=value`,
/// pipe-separated.
fn render_attrs_wiki(attrs: &[(String, String)]) -> String {
    attrs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("|")
}

/// Canonical Jira panel opener from a parsed attribute list: `{panel}` when
/// empty, else `{panel:k=v|k2=v2}`. Used by `normalize_ws` so cosmetic padding
/// around the `|`/`=` separators is insignificant to the round-trip guard.
fn render_panel_wiki(attrs: &[(String, String)]) -> String {
    if attrs.is_empty() {
        "{panel}".to_string()
    } else {
        format!("{{panel:{}}}", render_attrs_wiki(attrs))
    }
}

/// Render a panel opener in Markdown. When the first attribute is `title`, the
/// title becomes a level-2 heading and any remaining attributes go into the
/// trailing `<!-- panel … -->` marker; otherwise all attributes go into a bare
/// marker with no heading (Jira panels without a title, or whose first
/// attribute is not the title).
fn render_panel_open_md(attrs: &[(String, String)]) -> String {
    if let Some(((k, title), rest)) = attrs.split_first() {
        if k == "title" {
            let marker_attrs = render_attrs_md(rest);
            return if marker_attrs.is_empty() {
                format!("## {title} <!-- panel -->")
            } else {
                format!("## {title} <!-- panel {marker_attrs} -->")
            };
        }
    }
    let marker_attrs = render_attrs_md(attrs);
    if marker_attrs.is_empty() {
        "<!-- panel -->".to_string()
    } else {
        format!("<!-- panel {marker_attrs} -->")
    }
}

/// Drop leading and trailing blank lines — the readability padding inserted
/// around a Markdown panel body — before converting it back to wiki markup.
fn strip_blank_edges<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let mut start = 0;
    let mut end = lines.len();
    while start < end && lines[start].trim().is_empty() {
        start += 1;
    }
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    lines[start..end].to_vec()
}

enum DivKind {
    Quote,
    NoFormat,
}

struct MdDiv {
    kind: DivKind,
}

static DIV_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(\w+)="([^"]*)"|(\w+)=(\S+)"#).unwrap());

/// Parse a `::: quote` / `::: noformat` opener. Panels use the heading/marker
/// form handled by [`parse_md_panel_open`], not the fenced div.
fn parse_md_div_open(line: &str) -> Option<MdDiv> {
    match line.strip_prefix(DIV_FENCE)?.trim() {
        "quote" => Some(MdDiv {
            kind: DivKind::Quote,
        }),
        "noformat" => Some(MdDiv {
            kind: DivKind::NoFormat,
        }),
        _ => None,
    }
}

static MD_PANEL_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+(.*?)\s*<!--\s*panel\b\s*(.*?)\s*-->$").unwrap());
static MD_PANEL_MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<!--\s*panel\b\s*(.*?)\s*-->$").unwrap());

/// Parse a Markdown panel opener: the heading form `## title <!-- panel attrs
/// -->` yields `(Some(title), attrs)`, the bare marker `<!-- panel attrs -->`
/// yields `(None, attrs)`. `None` if the line is not a panel opener.
fn parse_md_panel_open(line: &str) -> Option<(Option<String>, Vec<(String, String)>)> {
    if let Some(cap) = MD_PANEL_HEADING_RE.captures(line) {
        return Some((
            Some(cap[1].trim().to_string()),
            parse_div_attrs(cap[2].trim()),
        ));
    }
    if let Some(cap) = MD_PANEL_MARKER_RE.captures(line) {
        return Some((None, parse_div_attrs(cap[1].trim())));
    }
    None
}

/// A hard-wrapping editor can split a long panel opener — `## title <!-- panel
/// attrs -->` or a bare `<!-- panel attrs -->` — across several physical lines,
/// spilling its `<!-- … -->` marker onto follow-on lines. If `lines[start]`
/// begins such an opener whose marker is *not* closed on that line, rejoin the
/// run (one space per dropped break, mirroring the editor dropping the break
/// space) up to and including the line bearing `-->`, and return the reassembled
/// opener plus the index just past it. Returns `None` when the line is not a
/// wrapped-open candidate or the marker never closes (blank line / panel close /
/// end of input) — the caller then processes the line normally, preserving the
/// verbatim-safe fallback. Without this, a panel whose title made the opener
/// exceed the editor's textwidth round-trips to a plain `h2.` + a stray marker,
/// so a purely re-wrapped (untouched) comment reads as edited on save.
fn rejoin_wrapped_panel_open(lines: &[&str], start: usize) -> Option<(String, usize)> {
    let first = lines[start].trim();
    // Candidate first line: an ATX-`## ` heading (its `<!-- panel … -->` marker
    // may have wrapped entirely onto follow-on lines) or a line already opening
    // the bare marker. Either way the marker must *not* be closed here yet.
    let is_candidate =
        (first.starts_with("## ") || first.starts_with("<!--")) && !first.contains("-->");
    if !is_candidate {
        return None;
    }
    let mut joined = first.to_string();
    let mut j = start + 1;
    while j < lines.len() {
        let l = lines[j].trim();
        // A blank line or the panel close ends the run without a marker → this
        // was a plain heading, not a wrapped panel opener. (Our renderer always
        // emits a blank line before a panel opener, so a real heading can never
        // be glued to the *next* panel here.)
        if l.is_empty() || l == PANEL_CLOSE {
            return None;
        }
        joined.push(' ');
        joined.push_str(l);
        if l.contains("-->") {
            // Only claim it if the reassembled run actually carries a panel
            // marker — otherwise leave the caller's normal path to handle it.
            return joined.contains("<!--").then_some((joined, j + 1));
        }
        j += 1;
    }
    None
}

fn parse_div_attrs(inner: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    for cap in DIV_ATTR_RE.captures_iter(inner) {
        if let (Some(k), Some(v)) = (cap.get(1), cap.get(2)) {
            attrs.push((k.as_str().to_string(), v.as_str().to_string()));
        } else if let (Some(k), Some(v)) = (cap.get(3), cap.get(4)) {
            attrs.push((k.as_str().to_string(), v.as_str().to_string()));
        }
    }
    attrs
}

/// `hN. rest` → `(level, rest)`.
fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let bytes = line.as_bytes();
    if bytes.len() >= 4 && bytes[0] == b'h' && bytes[1].is_ascii_digit() && &line[2..4] == ". " {
        let level = (bytes[1] - b'0') as usize;
        if (1..=6).contains(&level) {
            return Some((level, &line[4..]));
        }
    }
    None
}

/// `#{1,6} rest` (ATX) → `(level, rest)`.
fn parse_md_heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        return Some((hashes, line[hashes..].trim_start()));
    }
    None
}

/// `----` (4+ dashes, nothing else). Jira's rule; `---` (3) is not a rule in
/// Jira, so we require ≥4 to avoid clashing with the `===`/`---` 3b markers.
fn is_hrule_wiki(line: &str) -> bool {
    line.len() >= 4 && line.chars().all(|c| c == '-')
}

/// Parse a Jira list item: leading run of `*`/`#` markers (mixed allowed),
/// then a space. Depth = number of markers; `ordered` = last marker is `#`.
fn parse_list_item(line: &str) -> Option<(usize, bool, &str)> {
    let markers: String = line.chars().take_while(|&c| c == '*' || c == '#').collect();
    if markers.is_empty() {
        return None;
    }
    let rest = &line[markers.len()..];
    let rest = rest.strip_prefix(' ')?;
    let ordered = markers.ends_with('#');
    Some((markers.len(), ordered, rest))
}

/// Next ordinal for an ordered list item at `depth`, advancing the per-level
/// counters in `ordinals` (index = depth − 1).
///
/// Jira spells every ordered item `#`, regardless of position, so `wiki_to_md`
/// has to number them itself — emitting a literal `1.` for each item survives
/// the round-trip but shows up as a flat `1. 1. 1.` list once the buffer is
/// reopened. Going a level deeper starts a fresh counter; coming back out drops
/// the deeper ones, so a sibling continues where it left off.
fn ordered_marker(ordinals: &mut Vec<usize>, depth: usize) -> usize {
    ordinals.truncate(depth);
    while ordinals.len() < depth {
        ordinals.push(0);
    }
    ordinals[depth - 1] += 1;
    ordinals[depth - 1]
}

/// Jira marker chain for a Markdown list item at `depth`, updating the running
/// per-level kinds in `markers` (index = depth − 1).
///
/// Jira encodes the whole ancestry in the marker run (`*#` is an ordered item
/// inside a bullet), whereas Markdown only indents. Repeating the item's own
/// marker `depth` times therefore rewrote such a mixed run to `##` and the
/// round-trip guard rejected the ticket; remembering each level's kind restores
/// it. A level with no recorded ancestor (a buffer that starts indented) falls
/// back to the item's own kind — the previous behaviour.
fn wiki_list_markers(markers: &mut Vec<char>, depth: usize, ordered: bool) -> String {
    let own = if ordered { '#' } else { '*' };
    markers.truncate(depth);
    while markers.len() < depth {
        markers.push(own);
    }
    markers[depth - 1] = own;
    markers.iter().collect()
}

/// True if `line` (ignoring leading spaces) opens with a marker that Markdown
/// parses as a block construct but Jira leaves as plain text: a `- `/`+ `
/// bullet, an `N. ` ordered item, or a `> ` blockquote. (Jira's own list
/// markers are `*`/`#` and its blockquote is `bq. `, all handled as real
/// blocks earlier, so a plain line reaching the fallback that *starts* like one
/// of these is prose the reverse pass would otherwise mis-parse.)
fn md_block_marker_prefix(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("- ") || t.starts_with("+ ") || t.starts_with("> ") {
        return true;
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && t[digits..].starts_with(". ")
}

/// Remove a single leading backslash that [`wiki_to_md`] added to escape a
/// Markdown block marker (see [`md_block_marker_prefix`]), restoring the plain
/// Jira text. A backslash before anything else is left intact.
fn strip_block_escape(line: &str) -> String {
    if let Some(rest) = line.strip_prefix('\\') {
        if md_block_marker_prefix(rest) {
            return rest.to_string();
        }
    }
    line.to_string()
}

/// Parse a Markdown list item: 2-space indentation levels, `- ` (bullet) or
/// `1. ` (ordered).
fn parse_md_list_item(line: &str) -> Option<(usize, bool, &str)> {
    let indent = line.chars().take_while(|&c| c == ' ').count();
    let body = &line[indent..];
    let depth = indent / 2 + 1;
    if let Some(rest) = body.strip_prefix("- ") {
        return Some((depth, false, rest));
    }
    // ordered: digits, dot, space
    let digits = body.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && body[digits..].starts_with(". ") {
        return Some((depth, true, &body[digits + 2..]));
    }
    None
}

// ─────────────────────────────── tables ─────────────────────────────────

/// A table row starts with `|` (data `|a|b|` or header `||a||b||`).
fn is_table_row(line: &str) -> bool {
    line.starts_with('|') && line.len() > 1
}

/// Collect a Jira table block starting at `start` (a line beginning with `|`).
/// A row that ends with `|` is complete, so the block continues only if the
/// next line also begins with `|`. A row that does *not* end with `|` has an
/// open multi-line cell (Jira allows a cell to span physical lines), so the
/// following line — even without a leading `|` — is a continuation and is
/// pulled in; a blank line still ends the block. Returns the block lines and
/// the index just past it. Used by both directions so a table with multi-line
/// cells is collected identically and can be passed through verbatim.
fn collect_table_block<'a>(lines: &[&'a str], start: usize) -> (Vec<&'a str>, usize) {
    let mut block = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        block.push(line);
        i += 1;
        if line.trim_end().ends_with('|') {
            // Cell closed: the table continues only into another `|`-led row.
            if i >= lines.len() || !lines[i].trim_start().starts_with('|') {
                break;
            }
        } else if i >= lines.len() || lines[i].trim().is_empty() {
            // Open multi-line cell, but nothing (or a blank line) follows.
            break;
        }
    }
    (block, i)
}

/// A GFM separator row: `|---|---|` (dashes, colons, pipes, spaces only, at
/// least one dash).
fn is_md_table_separator(line: &str) -> bool {
    line.starts_with('|')
        && line.contains('-')
        && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Split a run of collected Jira table lines into logical rows. A new row
/// begins at each physical line starting with `|`; any other line continues
/// the previous row's still-open (multi-line) cell and is appended with its
/// newline preserved, so the wrapped text can later be rendered with `<br>`.
fn split_logical_rows(rows: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for &line in rows {
        if out.is_empty() || line.trim_start().starts_with('|') {
            out.push(line.to_string());
        } else {
            let last = out.last_mut().expect("first line always starts a row");
            last.push('\n');
            last.push_str(line);
        }
    }
    out
}

/// Convert a run of Jira table lines to a GFM table. The first logical row's
/// `||` cells make it the header; a `---` separator is synthesized. A cell that
/// spans several physical lines (Jira allows this) is joined with `<br>`, which
/// GFM renders as a line break inside the cell. Tables that cannot be
/// represented as GFM are left verbatim so they round-trip untouched: a
/// data-only table (no `||` header), or an irregular table whose body rows do
/// not all have the header's column count.
fn table_to_md(rows: &[&str]) -> Vec<String> {
    let logical = split_logical_rows(rows);
    let is_header = logical
        .first()
        .is_some_and(|r| r.trim_start().starts_with("||"));
    if is_header {
        let header_cells = split_table_cells(&logical[0], true);
        let n = header_cells.len();
        let regular = logical[1..]
            .iter()
            .all(|r| split_table_cells(r, false).len() == n);
        if !regular {
            return rows.iter().map(|r| r.to_string()).collect();
        }
        let mut out = vec![
            format!("| {} |", header_cells.join(" | ")),
            format!("| {} |", vec!["---"; n].join(" | ")),
        ];
        for r in &logical[1..] {
            let cells = split_table_cells(r, false);
            out.push(format!("| {} |", cells.join(" | ")));
        }
        return out;
    }
    // Header-less Jira data table (rows led by a single `|`). Convert a
    // *regular* one — every row the same column count, ≥2 columns — to GFM,
    // using its first row as the header and prefixing a `DATA_TABLE_MARK` so
    // `md_to_wiki` restores the header-less form. Irregular tables can't be a
    // GFM table, so they stay verbatim and round-trip untouched.
    let n = logical
        .first()
        .map_or(0, |r| split_table_cells(r, false).len());
    let regular = n >= 2
        && logical
            .iter()
            .all(|r| split_table_cells(r, false).len() == n);
    if !regular {
        return rows.iter().map(|r| r.to_string()).collect();
    }
    let mut out = vec![DATA_TABLE_MARK.to_string()];
    for (idx, r) in logical.iter().enumerate() {
        let cells = split_table_cells(r, false);
        out.push(format!("| {} |", cells.join(" | ")));
        if idx == 0 {
            out.push(format!("| {} |", vec!["---"; n].join(" | ")));
        }
    }
    out
}

/// Convert a GFM table (header, separator, data rows) back to Jira markup. A
/// cell containing `<br>` is split back into physical lines, so a multi-line
/// Jira cell is reconstructed exactly.
fn table_to_wiki(rows: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let header = split_md_cells(rows[0]);
    out.push(format!("||{}||", header.join("||")));
    for r in &rows[2..] {
        let cells = split_md_cells(r);
        out.push(format!("|{}|", cells.join("|")));
    }
    out
}

/// Convert a GFM table introduced by [`DATA_TABLE_MARK`] back to a header-less
/// Jira data table (`|…|`, no `||`). The GFM header row was the data table's
/// first row, so it is emitted as an ordinary data row; the `---` separator
/// (`rows[1]`) is dropped. Reverse of the data-only branch in [`table_to_md`].
fn data_table_to_wiki(rows: &[&str]) -> Vec<String> {
    let mut out = vec![format!("|{}|", split_md_cells(rows[0]).join("|"))];
    for r in &rows[2..] {
        out.push(format!("|{}|", split_md_cells(r).join("|")));
    }
    out
}

/// Trim only the ASCII space/tab padding around a table cell, leaving every
/// other whitespace character intact — most importantly a non-breaking space
/// (`\u{a0}`), which Jira authors use as filler in otherwise-empty cells.
/// `str::trim` would strip it as Unicode whitespace and silently empty the
/// cell (`|Owner\u{a0}|\u{a0}|` → `|Owner||`), breaking the round-trip.
fn trim_cell(s: &str) -> &str {
    s.trim_matches(|c| c == ' ' || c == '\t')
}

/// Render one Jira cell's raw text as a GFM cell: convert inline markup on each
/// physical line and join a multi-line cell with `<br>` (GFM cannot hold a
/// literal newline inside a cell).
fn cell_to_md(raw: &str) -> String {
    raw.split('\n')
        .map(|seg| convert_inline_w2m(trim_cell(seg)))
        .collect::<Vec<_>>()
        .join("<br>")
}

/// Reverse of [`cell_to_md`]: split a GFM cell on `<br>` back into physical
/// lines and convert inline markup on each.
fn cell_to_wiki(raw: &str) -> String {
    raw.split("<br>")
        .map(|seg| convert_inline_m2w(trim_cell(seg)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split a Jira table row (possibly spanning several physical lines) into cell
/// contents. `header` rows use `||` as the separator, data rows use `|`.
fn split_table_cells(row: &str, header: bool) -> Vec<String> {
    let sep = if header { "||" } else { "|" };
    row.trim_matches('|').split(sep).map(cell_to_md).collect()
}

fn split_md_cells(row: &str) -> Vec<String> {
    row.trim()
        .trim_matches('|')
        .split('|')
        .map(cell_to_wiki)
        .collect()
}

fn parse_md_code_fence(line: &str) -> Option<String> {
    line.strip_prefix("```").map(|s| s.trim().to_string())
}

// ───────────────────────────── inline spans ─────────────────────────────

// The `regex` crate has no look-around, so word-boundary behaviour comes from
// the `\S…\S` anchors (a span can't open/close on whitespace, matching Jira's
// own rule) plus the shielding discipline below. Every converted span's
// *result* is shielded behind a placeholder so a later pass can never re-match
// it — in particular Markdown's italic `*…*` must not bite into the `*…*` that
// bold `**…**` just produced.
/// Jira "empty macro" emphasis escapes — `{*}`, `{_}`, `{-}`, `{+}`, `{^}`,
/// `{~}` — used to place a bare emphasis delimiter or to break up adjacent
/// markup. They must be shielded verbatim *before* the emphasis passes, or the
/// `*`/`_`/`-` inside them get mistaken for real delimiters and corrupt the
/// line. They round-trip unchanged in both directions.
static ESCAPE_MACRO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{[-*_+^~]\}").unwrap());
/// Wiki attachment/image embed: `!filename.ext!` with an optional `|params`
/// tail (`|thumbnail`, `|width=200`, …). The filename must end in an
/// `.<alnum>` extension so a bare exclamation (`Amazing!`) is never mistaken
/// for an embed.
static W_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!([^!|\r\n]+\.[A-Za-z0-9]+)(?:\|([^!\r\n]*))?!").unwrap());

static W_MONO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{(.+?)\}\}").unwrap());
static W_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]|]+)\|([^\]]+)\]").unwrap());
static W_BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*(\S(?:.*?\S)?)\*").unwrap());
// Jira strikethrough `-text-`. The dashes are only delimiters at a word
// boundary: the opener must follow the start of line or whitespace, the closer
// must precede whitespace or the end. This keeps intra-word hyphens
// (`DEV-Team`, `End-to-End`, `Freigabe-Management`) from being mistaken for a
// strike span. Boundary chars are captured so they can be re-emitted verbatim.
// (A strike hugging punctuation, e.g. `-wrong-,`, is not detected and simply
// round-trips as literal dashes — safe, never corrupting.)
static W_STRIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|\s)-(\S(?:.*?\S)?)-(\s|$)").unwrap());

/// Jira colour macros. Opener `{color:VALUE}` → an HTML `<span style="color:…">`
/// and the bare closer `{color}` → `</span>`. They are handled as *independent*
/// delimiters (not a matched pair) so a colour that opens on one line and closes
/// on another — the common case — still converts, since each line is processed
/// separately. The opener must be tried before the closer (`{color}` is a suffix
/// of `{color:…}` only structurally, but the closer regex requires the `}`
/// immediately after `color`, so it never bites an opener).
static W_COLOR_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{color:([^}]+)\}").unwrap());
static W_COLOR_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{color\}").unwrap());

/// Inline conversion Jira → Markdown. Monospace/link are shielded first so
/// their contents (which may contain `*`/`-`) are never touched by the emphasis
/// passes; each emphasis result is shielded in turn.
///
/// Italic is *not* converted: Jira italic `_x_` is already valid Markdown
/// italic, so leaving it verbatim both renders correctly and sidesteps the
/// triple-`*` ambiguity that `_*x*_` (italic wrapping bold) would otherwise
/// create — it becomes `_**x**_`, which reverses cleanly.
/// Render a wiki image embed as a Markdown image link pointing at the local
/// `attachments/` copy. A filename with spaces is `<>`-wrapped so the URL stays
/// valid; embed params (if any) ride in the link title so they survive back.
fn image_w2m(filename: &str, params: Option<&str>) -> String {
    let url = if filename.contains(' ') {
        format!("<attachments/{filename}>")
    } else {
        format!("attachments/{filename}")
    };
    match params {
        Some(p) if !p.is_empty() => format!("![{filename}]({url} \"{p}\")"),
        _ => format!("![{filename}]({url})"),
    }
}

/// Reverse of [`image_w2m`]: rebuild the wiki `!name!` (or `!name|params!`)
/// embed from the Markdown image's alt text and optional title.
fn image_m2w(alt: &str, params: Option<&str>) -> String {
    match params {
        Some(p) if !p.is_empty() => format!("!{alt}|{p}!"),
        _ => format!("!{alt}!"),
    }
}

fn convert_inline_w2m(text: &str) -> String {
    let mut sh: Vec<String> = Vec::new();
    let s = ESCAPE_MACRO.replace_all(text, |c: &Captures| shield(&mut sh, c[0].to_string()));
    // Image embeds are shielded before every emphasis pass so a filename
    // containing `-` (strike) or the surrounding `!` never gets re-matched.
    let s = W_IMAGE.replace_all(&s, |c: &Captures| {
        shield(&mut sh, image_w2m(&c[1], c.get(2).map(|m| m.as_str())))
    });
    // Colour delimiters are shielded before the emphasis passes so the tag
    // itself is never re-matched, while the text *between* the tags still flows
    // through the remaining conversions.
    let s = W_COLOR_OPEN.replace_all(&s, |c: &Captures| {
        shield(&mut sh, format!("<span style=\"color:{}\">", &c[1]))
    });
    let s = W_COLOR_CLOSE.replace_all(&s, |_: &Captures| shield(&mut sh, "</span>".to_string()));
    let s = W_MONO.replace_all(&s, |c: &Captures| shield(&mut sh, format!("`{}`", &c[1])));
    let s = W_LINK.replace_all(&s, |c: &Captures| {
        shield(&mut sh, format!("[{}]({})", &c[1], &c[2]))
    });
    let s = W_BOLD.replace_all(&s, |c: &Captures| shield(&mut sh, format!("**{}**", &c[1])));
    let s = W_STRIKE.replace_all(&s, |c: &Captures| {
        let tok = shield(&mut sh, format!("~~{}~~", &c[2]));
        format!("{}{tok}{}", &c[1], &c[3])
    });
    unshield(&s, &sh)
}

/// Markdown image embed as emitted by [`image_w2m`]: `![alt](url)` with an
/// optional `<...>`-wrapped URL (for paths with spaces) and an optional
/// `"title"` carrying the Jira embed params. The `alt` text is the attachment
/// filename, which is all we need to rebuild the wiki `!name!` embed.
static M_IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!\[([^\]]*)\]\((?:<[^>]*>|[^)\s]+)(?:\s+"([^"]*)")?\)"#).unwrap()
});

static M_MONO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
static M_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap());
static M_BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(\S(?:.*?\S)?)\*\*").unwrap());
static M_STRIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"~~(\S(?:.*?\S)?)~~").unwrap());
static M_ITALIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*(\S(?:.*?\S)?)\*").unwrap());

/// Reverse of the colour macros: `<span style="color:…">` → `{color:…}` and
/// `</span>` → `{color}`. Only the exact `color:`-style span this converter
/// emits is recognized; any other `<span>` a ticket might carry falls through
/// verbatim.
static M_SPAN_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<span style="color:([^"]*)">"#).unwrap());
static M_SPAN_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"</span>").unwrap());

/// Inline conversion Markdown → Jira. Bold (`**`) is converted and shielded
/// before italic (`*`) runs, so italic never bites into a bold delimiter.
fn convert_inline_m2w(text: &str) -> String {
    let mut sh: Vec<String> = Vec::new();
    let s = ESCAPE_MACRO.replace_all(text, |c: &Captures| shield(&mut sh, c[0].to_string()));
    let s = M_SPAN_OPEN.replace_all(&s, |c: &Captures| {
        shield(&mut sh, format!("{{color:{}}}", &c[1]))
    });
    let s = M_SPAN_CLOSE.replace_all(&s, |_: &Captures| shield(&mut sh, "{color}".to_string()));
    // Image embeds are converted before links so `![alt](url)` is consumed
    // whole rather than having its inner `[alt](url)` eaten by the link pass.
    let s = M_IMAGE.replace_all(&s, |c: &Captures| {
        shield(&mut sh, image_m2w(&c[1], c.get(2).map(|m| m.as_str())))
    });
    let s = M_MONO.replace_all(&s, |c: &Captures| {
        shield(&mut sh, format!("{{{{{}}}}}", &c[1]))
    });
    let s = M_LINK.replace_all(&s, |c: &Captures| {
        shield(&mut sh, format!("[{}|{}]", &c[1], &c[2]))
    });
    let s = M_BOLD.replace_all(&s, |c: &Captures| shield(&mut sh, format!("*{}*", &c[1])));
    let s = M_STRIKE.replace_all(&s, |c: &Captures| shield(&mut sh, format!("-{}-", &c[1])));
    let s = M_ITALIC.replace_all(&s, |c: &Captures| shield(&mut sh, format!("_{}_", &c[1])));
    unshield(&s, &sh)
}

/// Push `value` onto the shield table and return its placeholder token.
fn shield(sh: &mut Vec<String>, value: String) -> String {
    sh.push(value);
    shield_token(sh.len() - 1)
}

/// Placeholder for a shielded inline span. Uses control characters that never
/// occur in issue text so it can't be re-matched by later passes.
fn shield_token(idx: usize) -> String {
    format!("\u{0}{idx}\u{0}")
}

fn unshield(s: &str, shields: &[String]) -> String {
    // Expand in *reverse* index order. Nested spans (e.g. italic wrapping bold)
    // produce an outer shield whose value embeds the inner shield's token; the
    // inner always has the lower index (it was created first). Expanding the
    // outer first re-exposes the inner token, which a later (lower-index)
    // iteration then resolves. Ascending order would leak the inner token.
    let mut out = s.to_string();
    for (idx, val) in shields.iter().enumerate().rev() {
        out = out.replace(&shield_token(idx), val);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md_to_wiki_rejoins_editor_soft_wrapped_lines() {
        // An `$EDITOR` with a text width wraps long lines at the last space. The
        // only space inside an `![alt](url "title")` image link is before the
        // title, so a wrapped image link splits there. Re-joining must reproduce
        // the exact same wiki as the unwrapped form — otherwise an untouched
        // comment reads as edited and save fails with "not authored by you".
        let unwrapped = "here is a screenshot: \\\n![shot.png](attachments/shot.png \"thumbnail\") \\\nand a closing remark.";
        // Editor hard-wrapped the image line at its only space:
        let wrapped = "here is a screenshot: \\\n![shot.png](attachments/shot.png\n\"thumbnail\") \\\nand a closing remark.";
        assert_eq!(
            md_to_wiki(unwrapped),
            md_to_wiki(wrapped),
            "re-wrapped image link must convert to the same wiki"
        );
    }

    #[test]
    fn md_to_wiki_preserves_intentional_hard_breaks() {
        // Lines that end with a `\` hard-break marker stay separate Jira lines;
        // only soft-wrapped (unmarked) continuations are re-joined.
        let md = "line one \\\nline two \\\nline three";
        assert_eq!(md_to_wiki(md), "line one\nline two\nline three");
    }

    /// Hard-wrap each physical line at `width` on the last space at or before
    /// the limit, mimicking an `$EDITOR` with `textwidth` set. Leaves a line
    /// with no earlier space intact (an editor can't break it either).
    fn hard_wrap(md: &str, width: usize) -> String {
        let mut out: Vec<String> = Vec::new();
        for line in md.split('\n') {
            let mut rest = line.to_string();
            loop {
                if rest.chars().count() <= width {
                    out.push(rest);
                    break;
                }
                // Byte index of the last space at or before `width` chars.
                let cut = rest
                    .char_indices()
                    .take(width + 1)
                    .filter(|(_, c)| *c == ' ')
                    .last()
                    .map(|(b, _)| b);
                match cut {
                    Some(b) if b > 0 => {
                        out.push(rest[..b].to_string());
                        rest = rest[b + 1..].to_string();
                    }
                    _ => {
                        out.push(rest);
                        break;
                    }
                }
            }
        }
        out.join("\n")
    }

    #[test]
    fn repro_wrapped_comment_body_roundtrips() {
        // Neutral fixture mirroring a real comment: a titled panel whose body
        // mixes prose (with a bold run), `N)` step lines, and image embeds with
        // long file names — the lines an editor is most likely to wrap.
        let wiki = "\
{panel:title=STEPS|titleBGColor=#f2f2f2}
1) create a booking with several vehicles and drivers:
 !diagram-0001-aaaa-bbbb-cccc-dddd-eeee.png|thumbnail!
planned end: some time in the near future for this run
2) hand out all of the vehicles to the drivers now (/)
 !diagram-0002-aaaa-bbbb-cccc-dddd-ffff.png|thumbnail!

pending (!)
2a) attempt to book one of the vehicles as a restricted user:
option A possible, option B and option C not as expected (!) --> *check this on the other stage once deployed.*
vehicles cannot be found in the requested window (/)
 !diagram-0003-aaaa-bbbb-cccc-dddd-gggg.png|thumbnail!
{panel}";

        let pristine_md = wiki_to_md(wiki);
        let baseline = normalize_ws(&md_to_wiki(&pristine_md));
        for width in [60usize, 66, 70, 72, 76, 80] {
            let wrapped = hard_wrap(&pristine_md, width);
            assert_eq!(
                normalize_ws(&md_to_wiki(&wrapped)),
                baseline,
                "editor-wrapped at width {width} must round-trip to the same wiki"
            );
        }
    }

    #[test]
    fn repro_wrapped_multi_panel_comment_roundtrips() {
        // Closer mirror: three consecutive titled panels, bullet lists whose
        // items carry a plain continuation line, `N)`/`Na)` step prose, a bold
        // run, mentions, and long-name image embeds — the full shape of a real
        // comment body, driven through the same wrap-then-round-trip check.
        let wiki = "\
{panel:title=FRAME|titleBGColor=#f2f2f2}
System: stage-one
User:
first person (restricted role here)
second person (restricted role here)
Pool: the pool owner name
{panel}

{panel:title=STEPS|titleBGColor=#f2f2f2}
1) create a booking with several vehicles and several drivers:
 !diagram-2026-07-20-13-02-21-789.png|thumbnail!
planned end: some later time on the same day for this run
2) hand out all of the vehicles now (/)
 !diagram-2026-07-20-13-03-06-354.png|thumbnail!

pending (!)
2a) attempt to book one of the vehicles as a restricted planner user role:
option A possible, option B unclear and or option C not as expected (!) --> *verify this on the other stage as soon as it is deployed there.*
vehicles cannot be found within the requested window (/)
 !diagram-2026-07-20-13-05-41-031.png|thumbnail!

* after adjusting the requested window to end later than the booking itself:
the vehicle is found (/)
 !diagram-2026-07-20-13-07-36-041.png|thumbnail!

even after widening the granted permissions the vehicle cannot be found (x)
 !diagram-2026-07-20-13-08-39-983.png|thumbnail!

* invoked directly through the vehicle record:
 !diagram-2026-07-20-13-10-56-389.png|thumbnail!
{panel}

{panel:title=NEXT STEPS|titleBGColor=#f2f2f2}
[~alice] unfortunately this still does not fit after all.
the vehicles are not shown to me inside the mobile application.
it might also be the environment, but I set everything up exactly as elsewhere.
{panel}";

        let pristine_md = wiki_to_md(wiki);
        let baseline = normalize_ws(&md_to_wiki(&pristine_md));
        for width in [58usize, 62, 66, 70, 72, 74, 76, 78, 80] {
            let wrapped = hard_wrap(&pristine_md, width);
            let got = normalize_ws(&md_to_wiki(&wrapped));
            assert_eq!(
                got,
                baseline,
                "width {width}: {}",
                first_divergence(&baseline, &got)
            );
        }
    }

    #[test]
    fn repro_double_space_prose_survives_editor_wrap() {
        // Real comments routinely carry stray *internal* double spaces
        // ("option B unclear and  or option C"). When an `$EDITOR`
        // hard-wraps a long line whose break lands *at* such a double space,
        // it drops the trailing space and starts the next fragment at the
        // word; re-joining then re-inserts only a single space. The snapshot
        // (unwrapped) keeps both spaces, so a purely re-wrapped — untouched —
        // comment diverged and read as edited, surfacing the bogus
        // "not authored by you" on save. This must round-trip at every width.
        let wiki = "\
{panel:title=STEPS|titleBGColor=#f2f2f2}
option A possible, option B unclear and  or option C not as expected here (!)
a second long sentence with  a doubled space early and more words trailing after it
{panel}";
        let pristine_md = wiki_to_md(wiki);
        let baseline = normalize_ws(&md_to_wiki(&pristine_md));
        // Widths ≥ 48 keep the 46-char panel-opener heading on one line, so
        // this isolates the wrapped-*prose* case (the two body lines are long
        // enough to wrap across this whole range).
        for width in (48usize..=88).step_by(2) {
            let wrapped = hard_wrap(&pristine_md, width);
            let got = normalize_ws(&md_to_wiki(&wrapped));
            assert_eq!(
                got,
                baseline,
                "width {width}: {}",
                first_divergence(&baseline, &got)
            );
        }
    }

    #[test]
    fn md_to_wiki_terminates_on_orphaned_panel_close() {
        // If an editor hard-wraps a long `## title <!-- panel … -->` opener,
        // the panel collector never matches it and its `<!-- /panel -->` is left
        // orphaned at top level. That line is not a paragraph continuation, so a
        // missing termination guard would spin `md_to_wiki` forever on save.
        // The assertion is simply that this returns at all (and verbatim-emits).
        let mangled =
            "## title <!-- panel\ntitleBGColor=\"#eee\"\n-->\nsome body text\n<!-- /panel -->";
        let wiki = md_to_wiki(mangled);
        assert!(wiki.contains("some body text"));
    }

    #[test]
    fn repro_wrapped_panel_opener_survives_editor_wrap() {
        // A panel whose *title* (plus its `titleBGColor` attribute) makes the
        // Markdown opener `## <title> <!-- panel titleBGColor="…" -->` longer
        // than a typical editor textwidth. When `$EDITOR` hard-wraps that
        // opener, its `<!-- … -->` marker spills across physical lines. Without
        // reassembly `md_to_wiki` no longer recognizes the opener and emits a
        // plain `h2.` plus a stray marker, so a purely re-wrapped (otherwise
        // untouched) comment diverges from its snapshot and reads as edited on
        // save — surfacing the bogus "not authored by you" on a foreign comment.
        // This must round-trip at every width, including narrow ones where the
        // `<!--` itself lands on a follow-on line.
        let wiki = "\
{panel:title=Constraints And Preconditions For This Particular Run|titleBGColor=#f2f2f2}
first line of the panel body describing the environment in some detail
second line continuing the same thought across the wrap boundary here
{panel}";
        // The ticket must be editable in the first place (guard passes).
        assert!(
            roundtrip_diff(wiki).is_none(),
            "fixture must round-trip untouched"
        );
        let pristine_md = wiki_to_md(wiki);
        let baseline = normalize_ws(wiki);
        for width in 20usize..=120 {
            let wrapped = hard_wrap(&pristine_md, width);
            let got = normalize_ws(&md_to_wiki(&wrapped));
            assert_eq!(
                got,
                baseline,
                "width {width}: {}",
                first_divergence(&baseline, &got)
            );
        }
    }

    #[test]
    fn repro_wrapped_titleless_panel_marker_survives_editor_wrap() {
        // The bare (title-less) panel opener `<!-- panel titleBGColor="…" -->`
        // can wrap too; its reassembly must recover the same title-less panel.
        let wiki = "\
{panel:titleBGColor=#f2f2f2}
a title-less panel whose sole attribute makes its opener marker long enough
{panel}";
        assert!(
            roundtrip_diff(wiki).is_none(),
            "fixture must round-trip untouched"
        );
        let pristine_md = wiki_to_md(wiki);
        let baseline = normalize_ws(wiki);
        for width in 20usize..=120 {
            let wrapped = hard_wrap(&pristine_md, width);
            let got = normalize_ws(&md_to_wiki(&wrapped));
            assert_eq!(
                got,
                baseline,
                "width {width}: {}",
                first_divergence(&baseline, &got)
            );
        }
    }

    /// Assert wiki → md → wiki is stable (modulo whitespace) and that the
    /// intermediate Markdown matches expectation.
    fn assert_roundtrip(wiki: &str, expected_md: &str) {
        let md = wiki_to_md(wiki);
        assert_eq!(md, expected_md, "wiki→md mismatch");
        assert!(
            roundtrip_diff(wiki).is_none(),
            "not idempotent: {:?}",
            roundtrip_diff(wiki)
        );
    }

    #[test]
    fn headings() {
        assert_roundtrip("h1. Title", "# Title");
        assert_roundtrip("h3. Sub", "### Sub");
    }

    #[test]
    fn inline_effects() {
        assert_roundtrip("a *bold* word", "a **bold** word");
        // Jira italic `_x_` is already valid Markdown italic → left verbatim.
        assert_roundtrip("an _italic_ word", "an _italic_ word");
        // Italic wrapping bold must survive the round-trip unambiguously.
        assert_roundtrip("an _*emph*_ word", "an _**emph**_ word");
        assert_roundtrip("a -struck- word", "a ~~struck~~ word");
        assert_roundtrip("use {{code}} here", "use `code` here");
    }

    #[test]
    fn links() {
        assert_roundtrip(
            "see [the docs|https://x.y/z]",
            "see [the docs](https://x.y/z)",
        );
    }

    #[test]
    fn image_embeds() {
        // Plain embed → local attachment link.
        assert_roundtrip(
            "see !screenshot.png!",
            "see ![screenshot.png](attachments/screenshot.png)",
        );
        // Params ride in the Markdown title so they survive the round-trip.
        assert_roundtrip(
            "!diagram.png|thumbnail!",
            "![diagram.png](attachments/diagram.png \"thumbnail\")",
        );
        // A filename with a dash must not be eaten by the strike pass.
        assert_roundtrip(
            "!image-2023-12-01.png!",
            "![image-2023-12-01.png](attachments/image-2023-12-01.png)",
        );
        // Spaces in the filename → `<>`-wrapped URL.
        assert_roundtrip(
            "!my capture.png!",
            "![my capture.png](<attachments/my capture.png>)",
        );
    }

    #[test]
    fn bare_exclamation_is_not_an_image() {
        // No extension → not an embed; left verbatim and round-trips.
        assert_roundtrip("Wow! Amazing!", "Wow! Amazing!");
    }

    #[test]
    fn image_heavy_body_normalizes_equal_after_roundtrip() {
        // Regression: an image-heavy comment must survive wiki→md→wiki as
        // `normalize_ws`-equal, because the comment-conflict detector in
        // `edit_with_comments` compares the raw upstream body against this
        // round-tripped baseline. A raw byte-compare once flagged such
        // comments as spurious "modified upstream" conflicts.
        let body = "Here are the findings:\n\n\
                    !overview.png|thumbnail!\n\n\
                    Some prose between the shots.\n\n\
                    * !detail-a.png|width=200!\n\
                    * !detail-b.png!\n\n\
                    That is all.";
        let back = md_to_wiki(&wiki_to_md(body));
        assert_eq!(
            normalize_ws(body),
            normalize_ws(&back),
            "image-heavy body diverged under normalize_ws:\n{body}\n---\n{back}"
        );
        assert!(roundtrip_diff(body).is_none(), "{:?}", roundtrip_diff(body));
    }

    #[test]
    fn bullet_and_numbered_lists() {
        assert_roundtrip("* one\n* two", "- one\n- two");
        assert_roundtrip("* a\n** b", "- a\n  - b");
        assert_roundtrip("# first\n# second", "1. first\n2. second");
    }

    #[test]
    fn ordered_items_are_numbered_sequentially() {
        // Jira writes every ordered item as `#`; the numbers are ours to invent.
        // Emitting `1.` for each one round-trips but reads as a flat list once
        // the buffer is reopened — that reopen is what the user actually sees.
        assert_roundtrip("# one\n# two\n# three", "1. one\n2. two\n3. three");
        // Nested levels count independently; back out and the sibling continues.
        assert_roundtrip("# a\n## a1\n## a2\n# b", "1. a\n  1. a1\n  2. a2\n2. b");
        // A bullet at the same depth interrupts the run.
        assert_roundtrip("# a\n* b\n# c", "1. a\n- b\n1. c");
        // Ordered items under a bullet parent count too, and the mixed `*#`
        // marker run survives (Markdown only indents, so the parent's kind is
        // carried in a marker stack).
        assert_roundtrip("* top\n*# x\n*# y", "- top\n  1. x\n  2. y");
        // A paragraph ends the list, so the next one restarts at 1.
        assert_roundtrip("# a\n# b\n\nprose\n\n# c", "1. a\n2. b\n\nprose\n\n1. c");
    }

    #[test]
    fn mixed_marker_runs_roundtrip() {
        // A bullet nested in an ordered list and vice versa: Jira spells the
        // whole ancestry into the marker run, Markdown only indents.
        assert_roundtrip("# a\n#* b\n#* c\n# d", "1. a\n  - b\n  - c\n2. d");
        assert_roundtrip("* a\n*# b\n* c", "- a\n  1. b\n- c");
        assert_roundtrip("# a\n#* b\n#*# c", "1. a\n  - b\n    1. c");
        // Leaving and re-entering a level keeps the ancestor kinds.
        assert_roundtrip("* a\n*# b\n* c\n*# d", "- a\n  1. b\n- c\n  1. d");
    }

    #[test]
    fn code_block_preserves_verbatim() {
        assert_roundtrip(
            "{code:rust}\nlet x = *y*;\n{code}",
            "```rust\nlet x = *y*;\n```",
        );
    }

    #[test]
    fn noformat_block() {
        assert_roundtrip(
            "{noformat}\nraw *stuff*\n{noformat}",
            "::: noformat\nraw *stuff*\n:::",
        );
    }

    #[test]
    fn titled_panel_becomes_heading_with_marker() {
        // A panel with a title renders as a level-2 heading carrying the
        // remaining attributes in a trailing `<!-- panel … -->` marker.
        let wiki = "{panel:title=USER STORY|titleBGColor=#f2f2f2}\nbla bla\n{panel}";
        let md = wiki_to_md(wiki);
        assert!(
            md.contains("## USER STORY <!-- panel titleBGColor=\"#f2f2f2\" -->"),
            "heading form missing: {md:?}"
        );
        assert!(md.contains("<!-- /panel -->"), "closer missing: {md:?}");
        assert!(!md.contains("{panel"), "raw panel leaked: {md:?}");
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn title_only_panel_omits_marker_attrs() {
        // Only a title → heading + bare `<!-- panel -->` (no attributes left).
        let wiki = "{panel:title=DOR}\ndone\n{panel}";
        let md = wiki_to_md(wiki);
        assert!(
            md.contains("## DOR <!-- panel -->"),
            "heading form missing: {md:?}"
        );
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn untitled_panel_uses_bare_marker() {
        // A panel with no title has no heading — just the marker comment.
        let wiki = "{panel}\ninner *x*\n{panel}";
        let md = wiki_to_md(wiki);
        assert!(md.contains("<!-- panel -->"), "bare marker missing: {md:?}");
        assert!(!md.contains("## "), "unexpected heading: {md:?}");
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn quote_block() {
        assert_roundtrip("{quote}\nquoted\n{quote}", "::: quote\nquoted\n:::");
        assert_roundtrip("bq. a quip", "> a quip");
    }

    #[test]
    fn table_with_header() {
        assert_roundtrip(
            "||H1||H2||\n|a|b|\n|c|d|",
            "| H1 | H2 |\n| --- | --- |\n| a | b |\n| c | d |",
        );
    }

    #[test]
    fn plain_data_table_becomes_gfm_with_marker() {
        // A header-less Jira data table becomes a GFM table whose first row is
        // the header, introduced by the data-table marker so it round-trips
        // back to the `|…|` form.
        assert_roundtrip(
            "|a|b|\n|c|d|",
            "<!-- jira data-table -->\n| a | b |\n| --- | --- |\n| c | d |",
        );
    }

    #[test]
    fn data_only_table_with_bold_pseudo_header_becomes_gfm() {
        // The author used `*bold*` pseudo-header cells in a plain data row and
        // left trailing cells empty (`| | |`). The bold row becomes the real GFM
        // header; the `*bold*` cells must not be rewritten as `_italic_` and the
        // whitespace-only cells must not collapse. Round-trips losslessly back to
        // the header-less form.
        let wiki = "|*Description*|*Category*|*Rating*| | | |\n|a|b|c|d|e|f|";
        let md = wiki_to_md(wiki);
        assert!(
            md.starts_with(DATA_TABLE_MARK),
            "no data-table marker: {md}"
        );
        assert!(
            md.contains("| **Description** |"),
            "bold header not GFM: {md}"
        );
        assert!(md.contains("| --- |"), "no GFM separator: {md}");
        assert!(
            !md.contains("|*Description*|"),
            "raw Jira table leaked: {md}"
        );
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn data_only_table_multiline_color_and_empty_rows_roundtrips() {
        // The shape of a real scoring table: a `*bold*` pseudo-header, cells that
        // wrap across physical lines (multi-line), a `{color}` macro in a cell,
        // fully-empty separator rows (`| | |`), and lone `-` cells. All six
        // columns are regular, so it converts to GFM (multi-line → `<br>`) and
        // round-trips back to the header-less Jira form.
        let wiki = "|*Aspect*|*Class*|*Rating*| | | |\n\
                    |Reach|Value (Team)|few teams (S)|one or\n\
                    several units (M)|several units across\n\
                    areas (L)|{color:#172b4d}all areas (XL){color}|\n\
                    | | | | | | |\n\
                    |Legal requirement|Value (Team)|no (XS)|yes (XXL)|-|-|";
        let md = wiki_to_md(wiki);
        assert!(
            md.contains("<br>"),
            "multi-line cell not joined with <br>: {md}"
        );
        assert!(
            md.contains("<span style=\"color:#172b4d\">"),
            "colour macro not converted in cell: {md}"
        );
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn irregular_data_table_stays_verbatim() {
        // A header-less data table whose rows have different column counts
        // cannot be a GFM table, so it is left verbatim and round-trips.
        let wiki = "|a|b|c|\n|d|e|";
        let md = wiki_to_md(wiki);
        assert_eq!(md, wiki, "irregular data table should stay verbatim");
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn horizontal_rule() {
        assert_roundtrip("----", "---");
    }

    #[test]
    fn colour_becomes_styled_span() {
        // A colour macro converts to an HTML span carrying the colour and back.
        assert_roundtrip(
            "{color:#112233}tinted{color}",
            "<span style=\"color:#112233\">tinted</span>",
        );
        assert_roundtrip(
            "a {color:green}word{color} here",
            "a <span style=\"color:green\">word</span> here",
        );
    }

    #[test]
    fn image_mention_hardbreak_body_guard_and_conflict_agree() {
        // Mirrors the structure of a real image+mention-heavy comment: leading
        // spaces on image lines, `[~key]` mentions, colour spans, blank lines.
        // The guard (roundtrip_diff) and the conflict detector (normalize_ws)
        // must agree — if the guard says "lossless", the conflict detector must
        // see fresh and the round-tripped snapshot as equal.
        let wiki = "*Vorprüfung:*\n\
                    1) Anlage Vorgang auf mehrere Geräte\n\
                     !screenshot-14.png|thumbnail!\n\
                    Suche Gerät: Gerät wird nicht gefunden (/)\n\
                     !screenshot-15.png|thumbnail!\n\n\
                    {color:#FFAB00}*Hinweis:*{color} funktioniert\n\n\
                    siehe auch:\n\
                     !image-2026-01-02-03-04-05-678.png|thumbnail!\n\n\
                    [~somekey] kannst du das prüfen?";
        let back = md_to_wiki(&wiki_to_md(wiki));
        assert!(
            roundtrip_diff(wiki).is_none(),
            "guard flagged the body: {:?}",
            roundtrip_diff(wiki)
        );
        assert_eq!(
            normalize_ws(wiki),
            normalize_ws(&back),
            "guard passed but normalize_ws diverges — conflict detector would \
             still false-positive:\n{wiki}\n---\n{back}"
        );
    }

    #[test]
    fn colour_spans_multiple_lines() {
        // The colour opens on one line and closes on another — each line is
        // converted independently, so it must still round-trip losslessly.
        let wiki = "{color:#445566}first line\nsecond line{color}";
        assert_roundtrip(
            wiki,
            "<span style=\"color:#445566\">first line \\\nsecond line</span>",
        );
    }

    #[test]
    fn colour_wraps_other_inline_markup() {
        // Markup *inside* a colour span still converts (the span only shields
        // its own delimiters, not the wrapped text).
        assert_roundtrip(
            "{color:red}a *bold* bit{color}",
            "<span style=\"color:red\">a **bold** bit</span>",
        );
    }

    #[test]
    fn unhandled_construct_is_flagged() {
        // A macro with no Markdown form, left verbatim, still round-trips as
        // long as it collides with nothing.
        assert!(roundtrip_diff("{anchor:top}").is_none());
    }

    #[test]
    fn mention_passthrough_roundtrips() {
        assert!(roundtrip_diff("ping [~jsmith] please").is_none());
    }

    #[test]
    fn map_3b_body_only_touches_body() {
        // Header + body + CACHE section: only the region between `===` and the
        // CACHE marker is converted; the metadata scaffolding is preserved.
        let buf = "summary: S\nlabels: x\n---\nnumber: ABC-1\n===\n\nh1. Title\n\n* a\n\n#### CACHE / available labels, users & statuses (do not edit) ####\nll_x";
        let md = map_3b_body(buf, wiki_to_md);
        assert!(md.contains("summary: S"), "header lost: {md}");
        assert!(md.contains("# Title"), "body not converted: {md}");
        assert!(md.contains("- a"), "list not converted: {md}");
        assert!(md.contains("#### CACHE"), "cache section lost: {md}");
        assert!(!md.contains("h1."), "wiki heading leaked: {md}");
        // And back again reproduces the original wiki body.
        let wiki = map_3b_body(&md, md_to_wiki);
        assert!(wiki.contains("h1. Title"), "reverse failed: {wiki}");
        assert!(wiki.contains("* a"), "reverse list failed: {wiki}");
    }

    #[test]
    fn empty_macro_escapes_survive() {
        // Jira's `{*}` / `{_}` emphasis escapes must not be eaten by the
        // emphasis passes; they round-trip verbatim.
        assert!(roundtrip_diff("text {*}with{*} escapes and {_}more{_}").is_none());
        assert_roundtrip("a {*}b{*} c", "a {*}b{*} c");
    }

    #[test]
    fn nested_italic_bold_is_unambiguous() {
        // `_*x*_` (italic wrapping bold) → `_**x**_`, which reverses cleanly
        // instead of collapsing into an ambiguous `***x***`.
        assert_roundtrip("_*both*_", "_**both**_");
        assert!(roundtrip_diff("start _*mixed* emphasis_ end").is_none());
    }

    #[test]
    fn block_close_tolerates_leading_whitespace() {
        // A stray leading space before a `{panel}` close (common in text copied
        // into the Jira editor) must still close the panel, not swallow the
        // rest of the document.
        let wiki = "{panel:title=A}\nbody\n {panel}\n{panel:title=B}\nmore\n{panel}";
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn glued_panel_terminator_is_decoupled() {
        // Jira lets a `{panel}` terminator sit glued to the end of the last body
        // line (`* last point {panel}`) and still closes the panel there. The
        // line-based scanner only recognises a standalone `{panel}`, so without
        // the decoupling pre-pass the close is swallowed as list text and the
        // *following* titled panel silently loses its heading. Mirrors the shape
        // of real multi-panel (story / acceptance-criteria) descriptions.
        let wiki = "{panel:title=STORY|titleBGColor=#f2f2f2}\n\
                    As a user I want X\n\
                    * first point\n\
                    * last point {panel}\n\
                    {panel:title=ACCEPTANCE|titleBGColor=#f2f2f2}\n\
                    * criterion one\n\
                    * criterion two {panel}";
        let md = wiki_to_md(wiki);
        assert!(
            md.contains("## STORY <!-- panel titleBGColor=\"#f2f2f2\" -->"),
            "first panel heading missing: {md:?}"
        );
        assert!(
            md.contains("## ACCEPTANCE <!-- panel titleBGColor=\"#f2f2f2\" -->"),
            "second panel heading missing (glued close was swallowed): {md:?}"
        );
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn glued_terminator_after_image_line() {
        // A `{panel}` glued to the end of an image-embed line (`!mock.png!{panel}`)
        // must decouple cleanly — the image still converts and the panel closes.
        let wiki = "{panel:title=SPEC}\n\
                    see the mock\n\
                    !mock.png!{panel}";
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
        let md = wiki_to_md(wiki);
        assert!(
            md.contains("## SPEC <!-- panel -->"),
            "heading missing: {md:?}"
        );
        assert!(
            md.contains("![mock.png](attachments/mock.png)"),
            "image embed lost: {md:?}"
        );
    }

    #[test]
    fn dash_prefixed_prose_is_not_reparsed_as_bullet() {
        // Jira bullets are `*` / `#`; a leading `- ` is plain prose there. But
        // Markdown reads `- ` as a bullet, so `wiki_to_md` must escape such a
        // line (`\- …`) and `md_to_wiki` must strip that escape — otherwise the
        // reverse pass emits a Jira `* …` bullet (plus a stray hard-break) and
        // the round-trip diverges. Mirrors checklist-style prose lines like
        // `- (/) replace the old trigger` seen in real descriptions.
        let wiki = "{panel:title=NOTES}\n\
                    - (/) replace the old reminder trigger\n\
                    - (x) keep the legacy path until Q3\n\
                    {panel}";
        let md = wiki_to_md(wiki);
        assert!(
            md.contains("\\- (/) replace the old reminder trigger"),
            "dash-prefixed prose was not escaped: {md:?}"
        );
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn table_with_multiline_cell_becomes_gfm_with_br() {
        // A Jira table cell may span several physical lines (the row does not
        // end with `|` until the cell closes). GFM has no literal newline inside
        // a cell, so the wrapped text is joined with `<br>` and the table still
        // renders as a proper GFM table — and round-trips back to the multi-line
        // Jira form. Mirrors the multi-line criteria in real "Definition of
        // Ready/Done" tables.
        let wiki = "{panel:title=DoR}\n\
                    ||Criterion||Owner||Status||\n\
                    |First line of a criterion\n\
                    that wraps onto a second physical line|Product Owner|(x)|\n\
                    |A short one|DEV-Team|(/)|\n\
                    {panel}";
        let md = wiki_to_md(wiki);
        assert!(
            md.contains("| First line of a criterion<br>that wraps onto a second physical line | Product Owner | (x) |"),
            "multi-line cell was not joined with <br>: {md:?}"
        );
        assert!(
            md.contains("| --- | --- | --- |"),
            "no GFM separator: {md:?}"
        );
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn dor_table_with_empty_status_and_hyphens_roundtrips() {
        // A "Definition of Ready" table: a `||` header, several body rows whose
        // last "Status" cell is left blank (padded with a bare space → `| |`),
        // casual padding around cells (`Owner |`), a multi-line criterion, and
        // owner cells with intra-word hyphens (`DEV-Team`, `Release-Management`).
        // The table must render as GFM with `<br>` for the wrapped cell, the
        // hyphens must NOT be turned into strikethrough, and the whole thing must
        // round-trip losslessly (cosmetic cell spacing is normalized by the
        // guard). Invented content mirroring the real ticket shape.
        let wiki = "{panel:title=Definition of Ready}\n\
                    ||Criterion||Owner||Status||\n\
                    |The story is prioritised by the Product Owner|Product Owner | |\n\
                    |The story is clear to everyone involved\n\
                    and was refined together|AM + DEV-Team + Release-Management | |\n\
                    |All required accesses are available|DEV-Team | |\n\
                    {panel}";
        let md = wiki_to_md(wiki);
        assert!(
            md.contains("AM + DEV-Team + Release-Management"),
            "intra-word hyphens were mangled (strike?): {md:?}"
        );
        assert!(
            md.contains("<br>and was refined together"),
            "multi-line criterion not joined with <br>: {md:?}"
        );
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn intra_word_hyphen_is_not_strikethrough() {
        // Jira strikethrough is `-text-` only at a word boundary. A compound
        // word must survive untouched, while a genuine spaced strike converts.
        assert_roundtrip("build the DEV-Team roster", "build the DEV-Team roster");
        assert_roundtrip("an End-to-End run", "an End-to-End run");
        assert_roundtrip("a -struck- word", "a ~~struck~~ word");
    }

    #[test]
    fn table_cells_preserve_nbsp_filler() {
        // Jira authors pad otherwise-empty header-table cells with a
        // non-breaking space (`\u{a0}`) so the row renders with height. It is
        // semantically content, but `str::trim` treats it as Unicode whitespace
        // and would empty the cell — silently turning `|Owner\u{a0}|\u{a0}|`
        // into `|Owner||` and breaking the round-trip. Only ASCII space/tab
        // padding may be trimmed. Mirrors the "Definition of Ready" tables of
        // real tickets.
        let wiki = "{panel:title=DoR}\n\
                    ||Criterion||Owner||Status||\n\
                    |Prioritised|Product Owner\u{a0}|\u{a0}|\n\
                    {panel}";
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn panel_with_table_and_colours() {
        // A panel wrapping a header table whose cells carry `{color}` macros —
        // the shape used by the "Definition of Ready" section of real tickets.
        let wiki =
            "{panel:title=DOR}\n||Criterion||Status||\n|{color:#172b4d}done{color}|(/)|\n{panel}";
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn panel_title_padding_before_param_roundtrips() {
        // A stray space between the title value and the `|` param separator
        // (`{panel:title=X |k=v}`). The title becomes a level-2 heading whose
        // trailing whitespace is trimmed on the way to Markdown and cannot be
        // restored, so it must be insignificant to the round-trip guard —
        // attribute padding around `|`/`=` is cosmetic in Jira's panel macro,
        // exactly like table-cell padding.
        let wiki = "{panel:title=Overview |titleBGColor=#eeeeee}\nbody text\n{panel}";
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn overlapping_toggle_emphasis_roundtrips() {
        // Explicit bold/italic toggles `{*}`/`{_}` interleaved with
        // auto-detected `_.._` / `*..*`, overlapping (unbalanced spans). The
        // toggles stay verbatim; only the auto-`*..*` becomes `**..**`. Must
        // round-trip losslessly despite the tangled nesting.
        let wiki = "{*}_open one (_{*}{_}two) and *three* four{_}";
        assert_roundtrip(wiki, "{*}_open one (_{*}{_}two) and **three** four{_}");
    }

    #[test]
    fn single_newline_becomes_hard_break() {
        // Consecutive paragraph lines: Jira renders each newline as a break, so
        // the Markdown carries an explicit trailing `\` hard break, stripped on
        // the way back. A blank line (real paragraph break) gets no marker.
        assert_roundtrip("line one\nline two", "line one \\\nline two");
        assert_roundtrip("para one\n\npara two", "para one\n\npara two");
        // No marker before a block construct — the block already breaks.
        assert_roundtrip("intro\n* item", "intro\n- item");
        assert_roundtrip("intro\nh2. Head", "intro\n## Head");
    }

    #[test]
    fn trailing_backslash_on_last_line_survives() {
        // A literal trailing backslash on a paragraph's final line is not a
        // break marker (no continuation follows) and must round-trip verbatim.
        assert!(roundtrip_diff("ends with a backslash \\\n\nnext para").is_none());
    }

    #[test]
    fn mixed_document() {
        let wiki = "h2. Overview\n\nSome *bold* and _italic_ text.\n\n* a\n* b\n\n{code:java}\nint x = 1;\n{code}";
        assert!(roundtrip_diff(wiki).is_none(), "{:?}", roundtrip_diff(wiki));
    }

    #[test]
    fn header_tables_round_trip() {
        // The plain 3b header renders as two GFM tables and back to the exact
        // same `key: value` / `---` / `===` shape (modulo whitespace).
        let plain = "summary: My title\nlabels: aa, bb\nassignee: someone\n---\nnumber: ABC-1\ntype: Task\nstatus: Open\n===\n\nbody here";
        let md = header_to_md(plain);
        assert!(md.contains("| Field | Value |"), "no table header: {md}");
        assert!(
            md.contains("| summary | My title |"),
            "editable row missing: {md}"
        );
        assert!(
            md.contains("| number | ABC-1 |"),
            "readonly row missing: {md}"
        );
        assert!(!md.contains("summary:"), "plain key leaked: {md}");
        let back = header_from_md(&md);
        assert_eq!(
            normalize_ws(&back),
            normalize_ws(plain),
            "header not idempotent"
        );
    }

    #[test]
    fn header_with_banner_stays_plain() {
        // An error banner in the header (a `#`-line) forces the plain form so the
        // message stays readable — no table conversion.
        let plain = "# ─── ERRORS ───\n# summary is required\nsummary: \nlabels: x\n---\nnumber: ABC-1\n===\n\nbody";
        assert_eq!(header_to_md(plain), plain, "banner header must stay plain");
    }

    #[test]
    fn header_from_md_passes_plain_through() {
        // A plain header (no `|`-table) is returned unchanged, so the save path
        // works whether the user opened with tables or plain (error reopen).
        let plain = "summary: S\n---\nnumber: ABC-1\n===\n\nbody";
        assert_eq!(
            header_from_md(plain),
            plain,
            "plain header must pass through"
        );
    }

    #[test]
    fn header_cell_escapes_pipe() {
        // A value containing a pipe is escaped in the cell and restored intact.
        let plain = "summary: a | b\n---\nnumber: ABC-1\n===\n\nbody";
        let md = header_to_md(plain);
        assert!(
            md.contains("| summary | a \\| b |"),
            "pipe not escaped: {md}"
        );
        let back = header_from_md(&md);
        assert!(back.contains("summary: a | b"), "pipe not restored: {back}");
    }
}
