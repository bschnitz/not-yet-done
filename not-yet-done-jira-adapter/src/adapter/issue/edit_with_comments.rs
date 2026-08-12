//! Combined header+comments edit: one buffer holds the issue header in 3b
//! layout plus every existing comment, with `--- add ---` blocks for new
//! comments and `del`/`delete` as a sole-body keyword for deletion.

use std::sync::Arc;

use not_yet_done_content::*;

use crate::client::{JiraClient, JiraComment, JiraIssueDetail};

use super::super::cache::{fetch_comments, fetch_issue, resolve_unknown_mentions};
use super::super::util::{other_err, short_ts};
use super::JiraIssueNode;
use super::markers::{
    ADD_COMMENT_MARKER, DELETE_KEYWORD_DEL, DELETE_KEYWORD_DELETE, FOREIGN_BANNER_END,
    FOREIGN_BANNER_START,
};
use super::slugs::{
    SlugTables, build_slug_tables, canonicalize_labels_via_jira, parse_user_mentions,
    render_user_mentions, resolve_slugs_inplace,
};
use super::template::{
    FieldError, Parsed3b, edit_full_fields, render_3b_from_parsed, render_cache_section,
    strip_banner, strip_cache_section,
};
use super::wiki_md::normalize_ws;

/// Section in the `edit_with_comments` buffer.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum CommentBlockKind {
    /// Existing comment with this id.
    Existing(String),
    /// New-comment block introduced by `--- add ---`.
    Add,
}

/// One parsed comment block from the `edit_with_comments` buffer.
#[derive(Debug)]
pub(super) struct ParsedCommentBlock {
    pub(super) kind: CommentBlockKind,
    /// Body lines, leading/trailing blank lines trimmed.
    pub(super) body: String,
}

/// Result of [`JiraIssueNode::parse_with_comments`]: the 3b header parse
/// plus the comment blocks below.
#[derive(Debug)]
pub(super) struct ParsedWithComments {
    pub(super) header: Parsed3b,
    pub(super) blocks: Vec<ParsedCommentBlock>,
}

/// Outcome we computed for one existing comment in an `edit_with_comments`
/// commit, before any HTTP calls. Aggregated, then either applied (no
/// errors) or surfaced as a `Reopen` banner.
#[derive(Debug)]
enum CommentDecision {
    NoChange,
    /// User edited their own comment — PUT.
    Update {
        id: String,
        new_body: String,
    },
    /// User wrote `del`/`delete` — DELETE.
    Delete {
        id: String,
    },
    /// Foreign user edited; user accepted the new content (user.body == fresh.body).
    AcceptForeign,
    /// Foreign user edited; user kept the snapshot (user.body == snapshot.body).
    /// No write needed; next prepare will pick up the fresh body.
    DropOurs,
}

/// Decide whether a comment was authored by the current user, for the
/// per-author edit/delete gate. Prefers the stable account username (the
/// `name` field, the same value that appears inside `[~name]`) over the
/// display name, which can be ambiguous or reformatted. Falls back to the
/// display name only when either side lacks a username, and defaults to
/// permissive (`true`) when we don't know who we are.
fn comment_is_own(
    author: &str,
    author_key: &str,
    current_display: Option<&str>,
    current_username: Option<&str>,
) -> bool {
    if let Some(me) = current_username {
        if !me.is_empty() && !author_key.is_empty() {
            return author_key == me;
        }
    }
    match current_display {
        Some(u) => author == u,
        None => true,
    }
}

/// Build the per-comment header line for `edit_with_comments`, e.g.
/// `--- @bob 2025-06-01T10:00 (id=10042) ---`.
pub(super) fn render_comment_header(comment: &JiraComment) -> String {
    format!(
        "--- @{author} {ts} (id={id}) ---",
        author = comment.author,
        ts = short_ts(&comment.created),
        id = comment.id,
    )
}

/// Match a `--- @author timestamp (id=NNN) ---` header line and extract the
/// id. Returns `None` for non-matching lines.
pub(super) fn parse_comment_header_id(line: &str) -> Option<&str> {
    let trimmed = line.trim_end();
    let inner = trimmed.strip_prefix("--- ")?.strip_suffix(" ---")?;
    if !inner.starts_with('@') {
        return None;
    }
    let id_open = inner.rfind("(id=")?;
    let id_part = &inner[id_open + 4..];
    let id_close = id_part.find(')')?;
    Some(&id_part[..id_close])
}

/// Free version of `parse_with_comments` callable from tests / from places
/// that don't have a `JiraIssueNode`. Splits the buffer at marker lines
/// (`--- @... (id=N) ---` or `--- add ---`), then runs the existing 3b
/// parser on the header chunk.
pub(super) fn parse_with_comments_text(
    node: &JiraIssueNode,
    text: &str,
) -> std::result::Result<ParsedWithComments, Vec<FieldError>> {
    let text = strip_cache_section(text);
    let text = strip_banner(text);

    // Split into header + sequence of (marker, body-lines).
    let mut header_lines: Vec<&str> = Vec::new();
    let mut blocks_raw: Vec<(CommentBlockKind, Vec<&str>)> = Vec::new();
    let mut current: Option<(CommentBlockKind, Vec<&str>)> = None;
    let mut errors: Vec<FieldError> = Vec::new();

    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let trimmed = line.trim_end();

        if trimmed == ADD_COMMENT_MARKER {
            if let Some(prev) = current.take() {
                blocks_raw.push(prev);
            }
            current = Some((CommentBlockKind::Add, Vec::new()));
            continue;
        }
        if let Some(id) = parse_comment_header_id(trimmed) {
            if let Some(prev) = current.take() {
                blocks_raw.push(prev);
            }
            current = Some((CommentBlockKind::Existing(id.to_string()), Vec::new()));
            continue;
        }

        match current.as_mut() {
            None => header_lines.push(line),
            Some((_, body)) => body.push(line),
        }

        // Within the header, surface the same hard structural errors as
        // parse_3b would (so we can fail fast).
        let _ = lineno;
    }

    if let Some(prev) = current.take() {
        blocks_raw.push(prev);
    }

    let header_text: String = header_lines.join("\n");
    let header = match node.parse_3b(&header_text) {
        Ok(h) => h,
        Err(mut e) => {
            errors.append(&mut e);
            return Err(errors);
        }
    };

    let mut blocks = Vec::with_capacity(blocks_raw.len());
    for (kind, body_lines) in blocks_raw {
        let mut body_lines = body_lines;
        while body_lines.first().is_some_and(|l| l.trim().is_empty()) {
            body_lines.remove(0);
        }
        while body_lines.last().is_some_and(|l| l.trim().is_empty()) {
            body_lines.pop();
        }
        let body = body_lines.join("\n");
        // Empty Add blocks are render-time placeholders — drop them so
        // downstream code never sees a "new comment with no body".
        if matches!(kind, CommentBlockKind::Add) && body.trim().is_empty() {
            continue;
        }
        blocks.push(ParsedCommentBlock { kind, body });
    }

    Ok(ParsedWithComments { header, blocks })
}

// ───────────────── canonical ⇄ markdown comment buffer ─────────────────
//
// `edit_markdown` reuses the whole `edit_with_comments` pipeline but presents
// the buffer in Markdown: the header as two GFM tables, the body as Markdown,
// and each comment under a `### … <!-- jira comment … -->` heading below a
// `## Comments <!-- jira comments section -->` divider. These two functions
// translate between that Markdown shape and the canonical `edit_with_comments`
// buffer (`--- @author ts (id=N) ---` headers, wiki-markup bodies) so the
// classify/diff/apply logic below never has to know which editor produced it.

/// Divider between the issue body and the comment list in the Markdown buffer.
const MD_COMMENTS_SECTION: &str = "## Comments <!-- jira comments section -->";
/// Substring identifying the section divider, tolerant of heading-text edits.
const MD_COMMENTS_SECTION_MARK: &str = "<!-- jira comments section -->";
/// New-comment placeholder heading in the Markdown buffer. The visible
/// `Add Comment` text is cosmetic — `parse_md_comment_heading` keys the Add
/// block off the `<!-- jira comment add -->` marker and ignores the heading
/// text, so a user may rewrite or keep it and the round-trip is unaffected.
const MD_COMMENT_ADD: &str = "### Add Comment <!-- jira comment add -->";

/// Extract the `@author timestamp` prefix from a canonical comment header
/// (`--- @author ts (id=N) ---`), dropping the `--- ` fence and `(id=N)` tail.
fn canonical_header_middle(line: &str) -> String {
    let t = line.trim_end();
    let inner = t
        .strip_prefix("--- ")
        .and_then(|s| s.strip_suffix(" ---"))
        .unwrap_or(t);
    match inner.rfind("(id=") {
        Some(p) => inner[..p].trim().to_string(),
        None => inner.trim().to_string(),
    }
}

/// Match a Markdown comment heading `### <middle> <!-- jira comment (add|id=N) -->`.
/// Returns the block kind plus the `<middle>` author/timestamp text.
fn parse_md_comment_heading(line: &str) -> Option<(CommentBlockKind, String)> {
    let t = line.trim();
    let rest = t.strip_prefix("###")?.trim_start();
    let cmt_start = rest.find("<!--")?;
    let middle = rest[..cmt_start].trim().to_string();
    let inner = rest[cmt_start..]
        .strip_prefix("<!--")?
        .trim()
        .strip_suffix("-->")?
        .trim()
        .strip_prefix("jira comment")?
        .trim();
    if inner == "add" {
        Some((CommentBlockKind::Add, middle))
    } else {
        inner
            .strip_prefix("id=")
            .map(|id| (CommentBlockKind::Existing(id.trim().to_string()), middle))
    }
}

/// Join body lines, dropping leading/trailing blank lines.
fn join_trim(mut lines: Vec<&str>) -> String {
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Split the comment region of a *canonical* buffer into
/// `(kind, middle, body)` blocks, keyed off the `--- add ---` /
/// `--- @author ts (id=N) ---` header lines.
fn split_canonical_blocks(lines: &[&str]) -> Vec<(CommentBlockKind, String, String)> {
    let mut blocks: Vec<(CommentBlockKind, String, String)> = Vec::new();
    let mut cur: Option<(CommentBlockKind, String, Vec<&str>)> = None;
    let flush = |cur: Option<(CommentBlockKind, String, Vec<&str>)>,
                 blocks: &mut Vec<(CommentBlockKind, String, String)>| {
        if let Some((k, m, b)) = cur {
            blocks.push((k, m, join_trim(b)));
        }
    };
    for line in lines {
        let t = line.trim_end();
        if t == ADD_COMMENT_MARKER {
            flush(cur.take(), &mut blocks);
            cur = Some((CommentBlockKind::Add, String::new(), Vec::new()));
        } else if let Some(id) = parse_comment_header_id(t) {
            flush(cur.take(), &mut blocks);
            cur = Some((
                CommentBlockKind::Existing(id.to_string()),
                canonical_header_middle(t),
                Vec::new(),
            ));
        } else if let Some((_, _, body)) = cur.as_mut() {
            body.push(line);
        }
    }
    flush(cur.take(), &mut blocks);
    blocks
}

/// Split the comment region of a *Markdown* buffer into `(kind, middle, body)`
/// blocks, keyed off the `### … <!-- jira comment … -->` headings.
fn split_md_comment_blocks(lines: &[&str]) -> Vec<(CommentBlockKind, String, String)> {
    let mut blocks: Vec<(CommentBlockKind, String, String)> = Vec::new();
    let mut cur: Option<(CommentBlockKind, String, Vec<&str>)> = None;
    let flush = |cur: Option<(CommentBlockKind, String, Vec<&str>)>,
                 blocks: &mut Vec<(CommentBlockKind, String, String)>| {
        if let Some((k, m, b)) = cur {
            blocks.push((k, m, join_trim(b)));
        }
    };
    for line in lines {
        if let Some((kind, middle)) = parse_md_comment_heading(line) {
            flush(cur.take(), &mut blocks);
            cur = Some((kind, middle, Vec::new()));
        } else if let Some((_, _, body)) = cur.as_mut() {
            body.push(line);
        }
    }
    flush(cur.take(), &mut blocks);
    blocks
}

/// Convert a canonical `edit_with_comments` buffer to the Markdown shape used
/// by `edit_markdown`: header → GFM tables, body → Markdown, and each comment
/// under a `### … <!-- jira comment … -->` heading below the section divider.
/// The trailing CACHE section is preserved verbatim. With no comment markers
/// (no `--- add ---`, no `(id=)` headers) this degrades to plain body-only
/// Markdown, matching the pre-comments `edit_markdown` behaviour.
pub(super) fn comments_canonical_to_md(canonical: &str) -> String {
    use super::markers::CACHE_MARKER;
    use super::wiki_md::{header_to_md, map_3b_body, wiki_to_md};

    let lines: Vec<&str> = canonical.split('\n').collect();
    let cache_idx = lines
        .iter()
        .position(|l| l.trim_end() == CACHE_MARKER)
        .unwrap_or(lines.len());
    let first_marker = lines[..cache_idx].iter().position(|l| {
        let t = l.trim_end();
        t == ADD_COMMENT_MARKER || parse_comment_header_id(t).is_some()
    });

    let Some(first_marker) = first_marker else {
        return header_to_md(&map_3b_body(canonical, wiki_to_md));
    };

    let header_body = lines[..first_marker].join("\n");
    let header_md = header_to_md(&map_3b_body(&header_body, wiki_to_md));
    let blocks = split_canonical_blocks(&lines[first_marker..cache_idx]);

    let mut out: Vec<String> = Vec::new();
    out.push(header_md.trim_end().to_string());
    out.push(String::new());
    out.push(MD_COMMENTS_SECTION.to_string());
    for (kind, middle, body) in &blocks {
        out.push(String::new());
        match kind {
            CommentBlockKind::Add => out.push(MD_COMMENT_ADD.to_string()),
            CommentBlockKind::Existing(id) => out.push(if middle.is_empty() {
                format!("### <!-- jira comment id={id} -->")
            } else {
                format!("### {middle} <!-- jira comment id={id} -->")
            }),
        }
        if !body.trim().is_empty() {
            out.push(String::new());
            out.push(wiki_to_md(body));
        }
    }
    if cache_idx < lines.len() {
        out.push(String::new());
        out.extend(lines[cache_idx..].iter().map(|s| s.to_string()));
    }
    out.join("\n")
}

/// Reverse of [`comments_canonical_to_md`]: turn the Markdown `edit_markdown`
/// buffer back into a canonical `edit_with_comments` buffer the classify/diff
/// pipeline understands. With no `## Comments` divider this degrades to the
/// plain body-only reverse.
pub(super) fn comments_md_to_canonical(md: &str) -> String {
    use super::markers::CACHE_MARKER;
    use super::wiki_md::{header_from_md, map_3b_body, md_to_wiki, rejoin_wrapped_html_markers};

    // An editor may have hard-wrapped a long structural marker (a comment
    // heading, the section divider, a panel opener) across several lines;
    // stitch every such `<!-- … -->` back onto one line before parsing so a
    // purely re-wrapped buffer keeps its comment/section structure intact.
    let md = rejoin_wrapped_html_markers(md);
    // The trailing read-only CACHE divider is a plain `#### … ####` heading
    // (no HTML marker), so the pass above leaves it alone; reassemble it here
    // too, or a narrow editor wrap would fold the whole CACHE block into the
    // last comment's body and misclassify it as an edit.
    let md = rejoin_wrapped_cache_marker(&md);
    let md = md.as_str();
    let lines: Vec<&str> = md.split('\n').collect();
    let cache_idx = lines
        .iter()
        .position(|l| l.trim_end() == CACHE_MARKER)
        .unwrap_or(lines.len());
    let section_idx = lines[..cache_idx]
        .iter()
        .position(|l| l.trim().contains(MD_COMMENTS_SECTION_MARK));

    let Some(section_idx) = section_idx else {
        return map_3b_body(&header_from_md(md), md_to_wiki);
    };

    let header_body_md = lines[..section_idx].join("\n");
    let header_body_wiki = map_3b_body(&header_from_md(&header_body_md), md_to_wiki);
    let blocks = split_md_comment_blocks(&lines[section_idx + 1..cache_idx]);

    let mut out: Vec<String> = Vec::new();
    out.push(header_body_wiki.trim_end().to_string());
    for (kind, middle, body) in &blocks {
        out.push(String::new());
        match kind {
            CommentBlockKind::Add => out.push(ADD_COMMENT_MARKER.to_string()),
            CommentBlockKind::Existing(id) => {
                // The canonical parser requires the header to start with `@`,
                // so re-attach it if the user blanked the author.
                let m = if middle.is_empty() {
                    "@?".to_string()
                } else if middle.starts_with('@') {
                    middle.clone()
                } else {
                    format!("@{middle}")
                };
                out.push(format!("--- {m} (id={id}) ---"));
            }
        }
        if !body.trim().is_empty() {
            out.push(String::new());
            out.push(md_to_wiki(body));
        }
    }
    if cache_idx < lines.len() {
        out.push(String::new());
        out.extend(lines[cache_idx..].iter().map(|s| s.to_string()));
    }
    out.join("\n")
}

/// Reassemble an editor-wrapped CACHE divider back onto one line.
///
/// The divider is the fixed [`CACHE_MARKER`] string; a narrow editor wrap can
/// split it across several physical lines. We match strictly against the known
/// constant (a run of lines whose space-join is a growing prefix of it), so
/// this can never merge unrelated content — only the real, wrapped marker.
fn rejoin_wrapped_cache_marker(md: &str) -> String {
    use super::markers::CACHE_MARKER;
    let lines: Vec<&str> = md.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        let is_fragment =
            t != CACHE_MARKER && t.starts_with("#### CACHE") && CACHE_MARKER.starts_with(t);
        if !is_fragment {
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        // `t` is a strict prefix of the marker — join following fragments
        // until the whole marker is reconstructed (or the run diverges).
        let mut joined = t.to_string();
        let mut j = i + 1;
        let mut matched = false;
        while j < lines.len() {
            let nxt = lines[j].trim();
            if nxt.is_empty() {
                break;
            }
            joined.push(' ');
            joined.push_str(nxt);
            j += 1;
            if joined == CACHE_MARKER {
                matched = true;
                break;
            }
            if !CACHE_MARKER.starts_with(&joined) {
                break;
            }
        }
        if matched {
            out.push(CACHE_MARKER.to_string());
            i = j;
        } else {
            out.push(lines[i].to_string());
            i += 1;
        }
    }
    out.join("\n")
}

/// True if `body` is a sole-body delete keyword — case-insensitive,
/// no other non-blank lines.
pub(super) fn is_delete_keyword(body: &str) -> bool {
    let mut non_blank = body.lines().filter(|l| !l.trim().is_empty());
    let first = match non_blank.next() {
        Some(l) => l.trim().to_ascii_lowercase(),
        None => return false,
    };
    if non_blank.next().is_some() {
        return false;
    }
    first == DELETE_KEYWORD_DEL || first == DELETE_KEYWORD_DELETE
}

/// DELETE a Jira comment. Wrapper around the client method that converts
/// the raw String error into the `ContentError` layer used by the adapter.
async fn delete_comment(client: &Arc<JiraClient>, issue_key: &str, comment_id: &str) -> Result<()> {
    client
        .delete_comment(issue_key, comment_id)
        .await
        .map_err(other_err)
}

impl JiraIssueNode {
    /// Render the buffer for `edit_with_comments`: 3b header, then every
    /// comment newest→oldest, each preceded by `--- @author ts (id=N) ---`.
    /// The user adds new comments by writing `--- add ---` followed by a body.
    pub(super) fn render_with_comments(
        &self,
        editable_fields: &[String],
        detail: &JiraIssueDetail,
        comments: &[JiraComment],
        tables: &SlugTables,
    ) -> String {
        // Render the 3b header without the CACHE section — we append it
        // once at the very end after the comment list.
        let mut out = self.render_3b_full(editable_fields, detail, None, None, false, tables);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');

        out.push_str(ADD_COMMENT_MARKER);
        out.push('\n');
        out.push('\n');

        let mut sorted: Vec<&JiraComment> = comments.iter().collect();
        sorted.sort_by(|a, b| b.created.cmp(&a.created));

        for c in sorted {
            out.push_str(&render_comment_header(c));
            out.push('\n');
            out.push('\n');
            let body = render_user_mentions(c.body.trim_end(), &tables.users);
            out.push_str(&body);
            out.push('\n');
            out.push('\n');
        }

        out.push_str(&render_cache_section(tables));
        out
    }

    /// Parse a buffer produced by `render_with_comments`. See
    /// [`parse_with_comments_text`] for the splitting rules.
    pub(super) fn parse_with_comments(
        &self,
        text: &str,
    ) -> std::result::Result<ParsedWithComments, Vec<FieldError>> {
        parse_with_comments_text(self, text)
    }

    /// End-to-end edit flow for `edit_with_comments`. The user edits the
    /// issue header *and* every existing comment in one buffer, with `--- add ---`
    /// blocks for new comments and `del`/`delete` as a sole-body keyword to
    /// remove an own comment. Foreign comment edits (someone else changed
    /// the body upstream while we were editing) surface as a `Reopen` with
    /// upstream content restored and a banner listing the user's would-be
    /// edits so they can re-apply.
    pub(super) async fn execute_edit_with_comments(
        &mut self,
        text: &str,
        original_text: &str,
        version: &str,
    ) -> Result<ActionOutcome> {
        let editable_fields = edit_full_fields();

        // (a) Parse user buffer.
        let mut user = match self.parse_with_comments(text) {
            Ok(p) => p,
            Err(errs) => {
                return Ok(ActionOutcome::Reopen {
                    content: self.render_with_errors(text, &errs),
                    new_version: None,
                });
            }
        };

        // (b) Parse original snapshot (what the user opened with). Errors
        // here would mean we generated a malformed buffer in `prepare` —
        // surface as plain error.
        let mut snapshot = self.parse_with_comments(original_text).map_err(|errs| {
            other_err(format!(
                "internal: original buffer failed to re-parse ({} error(s))",
                errs.len()
            ))
        })?;

        // (b2) Translate slugs / mentions back to their wire form, so the
        // diff and the PUT/POST work in the same encoding the server uses.
        let tables = build_slug_tables(&self.cache);
        canonicalize_labels_via_jira(&self.client, &self.cache, &tables, &mut user.header).await;
        let mut header_errors = self.validate_3b(&user.header, &editable_fields);
        resolve_slugs_inplace(&mut user.header, &tables, &mut header_errors);

        let mut mention_errors: Vec<FieldError> = Vec::new();
        for block in &mut user.blocks {
            match parse_user_mentions(&block.body, &tables.users) {
                Ok(b) => block.body = b,
                Err(slug) => mention_errors.push(FieldError {
                    message: format!("unknown user mention `@{slug}` in comment"),
                }),
            }
        }
        // Snapshot is server-authored — should always round-trip cleanly.
        // Errors here would be a slug we couldn't resolve; keep the original
        // so the diff just sees a different value (which surfaces as an edit).
        for block in &mut snapshot.blocks {
            if let Ok(b) = parse_user_mentions(&block.body, &tables.users) {
                block.body = b;
            }
        }

        if !header_errors.is_empty() || !mention_errors.is_empty() {
            let mut all = header_errors;
            all.extend(mention_errors);
            return Ok(ActionOutcome::Reopen {
                content: self.render_with_errors(text, &all),
                new_version: None,
            });
        }

        // (d) Re-fetch fresh upstream comments + issue.
        let fresh_issue = fetch_issue(&self.client, &self.cache, &self.key)
            .await
            .map_err(other_err)?;
        let fresh_comments = fetch_comments(&self.client, &self.cache, &self.key)
            .await
            .map_err(other_err)?;
        // Pre-resolve any `[~KEY]` mention we don't yet know about — keeps
        // `render_user_mentions` synchronous and ensures the foreign-reopen
        // banner lands with proper @uu_slugs everywhere.
        let mut mention_sources: Vec<&str> =
            fresh_comments.iter().map(|c| c.body.as_str()).collect();
        for block in &user.blocks {
            mention_sources.push(block.body.as_str());
        }
        resolve_unknown_mentions(&self.client, &self.cache, &mention_sources).await;
        // Identity for the per-author "is this comment mine?" gate. We fetch
        // both the display name and the stable account username: display names
        // can be ambiguous or reformatted, so we prefer matching the username
        // (the `name` field, same value that appears inside `[~name]`) and only
        // fall back to the display name when either side lacks a username.
        let current_user = self.client.current_user().await.ok().map(|s| s.to_string());
        let current_username = self
            .client
            .current_username()
            .await
            .ok()
            .map(|s| s.to_string());
        let is_own_comment = |c: &JiraComment| -> bool {
            comment_is_own(
                &c.author,
                &c.author_key,
                current_user.as_deref(),
                current_username.as_deref(),
            )
        };

        // Build a snapshot id→body map and a fresh id→comment map for quick lookup.
        let snap_by_id: std::collections::HashMap<&str, &str> = snapshot
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                CommentBlockKind::Existing(id) => Some((id.as_str(), b.body.as_str())),
                CommentBlockKind::Add => None,
            })
            .collect();
        let fresh_by_id: std::collections::HashMap<&str, &JiraComment> =
            fresh_comments.iter().map(|c| (c.id.as_str(), c)).collect();

        // (e) Classify each comment block in the user buffer.
        let mut decisions: Vec<CommentDecision> = Vec::new();
        let mut adds: Vec<String> = Vec::new();
        let mut errors: Vec<(String, String, String)> = Vec::new(); // (id, message, restore_body)

        for block in &user.blocks {
            match &block.kind {
                CommentBlockKind::Add => {
                    adds.push(block.body.clone());
                }
                CommentBlockKind::Existing(id) => {
                    let user_body = block.body.trim();
                    let snapshot_body = snap_by_id.get(id.as_str()).copied().unwrap_or("");
                    let fresh = fresh_by_id.get(id.as_str()).copied();

                    let is_own = match fresh {
                        Some(c) => is_own_comment(c),
                        // Comment vanished upstream — owner check moot, error below.
                        None => false,
                    };
                    let is_delete = is_delete_keyword(user_body);

                    let Some(fresh) = fresh else {
                        // Comment was deleted upstream while we edited.
                        if normalize_ws(user_body) == normalize_ws(snapshot_body) {
                            // We didn't touch it either — silently drop.
                            decisions.push(CommentDecision::DropOurs);
                        } else {
                            errors.push((
                                id.clone(),
                                format!("comment {id} was deleted upstream — your edit is lost"),
                                user_body.to_string(),
                            ));
                        }
                        continue;
                    };

                    // Compare with the same whitespace normalization the
                    // round-trip guard uses, not a raw `.trim()`. In markdown
                    // mode the snapshot baseline is round-tripped
                    // (`md→wiki(wiki→md(upstream))`) while `fresh.body` is raw
                    // upstream wiki; the round-trip is only guaranteed stable
                    // modulo whitespace, so a raw compare would flag unchanged
                    // comments as foreign edits (e.g. image lines the editor
                    // re-wrapped).
                    let snap_norm = normalize_ws(snapshot_body);
                    let foreign_changed = normalize_ws(&fresh.body) != snap_norm;
                    let user_changed = normalize_ws(user_body) != snap_norm;

                    match (foreign_changed, user_changed) {
                        (false, false) => decisions.push(CommentDecision::NoChange),
                        (false, true) => {
                            if is_delete {
                                if !is_own {
                                    errors.push((
                                        id.clone(),
                                        format!(
                                            "cannot delete comment {id}: not authored by you ({})",
                                            fresh.author
                                        ),
                                        snapshot_body.to_string(),
                                    ));
                                    continue;
                                }
                                decisions.push(CommentDecision::Delete { id: id.clone() });
                            } else if !is_own {
                                errors.push((
                                    id.clone(),
                                    format!(
                                        "cannot edit comment {id}: not authored by you ({})",
                                        fresh.author
                                    ),
                                    snapshot_body.to_string(),
                                ));
                            } else {
                                decisions.push(CommentDecision::Update {
                                    id: id.clone(),
                                    new_body: user_body.to_string(),
                                });
                            }
                        }
                        (true, false) => decisions.push(CommentDecision::DropOurs),
                        (true, true) => {
                            if normalize_ws(user_body) == normalize_ws(&fresh.body) {
                                // User explicitly accepted upstream (e.g. after a prior reopen).
                                decisions.push(CommentDecision::AcceptForeign);
                            } else {
                                // True conflict: user has their own change AND someone else changed
                                // the comment upstream. Restore upstream, banner the user's edit.
                                errors.push((
                                    id.clone(),
                                    format!(
                                        "comment {id} was modified upstream by {} — your edit kept below",
                                        fresh.author
                                    ),
                                    user_body.to_string(),
                                ));
                            }
                        }
                    }
                }
            }
        }

        // (f) Removed comments: in snapshot but not in user buffer (and not in errors).
        for (snap_id, _) in &snap_by_id {
            let still_present = user
                .blocks
                .iter()
                .any(|b| matches!(&b.kind, CommentBlockKind::Existing(id) if id == snap_id));
            if !still_present {
                let restore = fresh_by_id
                    .get(snap_id)
                    .map(|c| c.body.clone())
                    .unwrap_or_default();
                errors.push((
                    snap_id.to_string(),
                    format!("comment {snap_id} was removed — use `del` (sole body) to delete"),
                    restore,
                ));
            }
        }

        // (g) If any structural / foreign-edit errors: reopen with restored buffer + banner.
        if !errors.is_empty() {
            // Async status-aware tables so the reopened header shows the `ss_`
            // status slug and the CACHE legend matches the transition menu.
            let display_tables = self.slug_tables(&fresh_issue).await;
            let content = self.render_foreign_reopen(
                &user,
                &fresh_issue,
                &fresh_comments,
                &errors,
                &display_tables,
            );
            return Ok(ActionOutcome::Reopen {
                content,
                new_version: Some(fresh_issue.updated.clone()),
            });
        }

        // (h) Apply comment changes: PUT, DELETE, POST.
        let issue_key = self.key.clone();
        for d in &decisions {
            match d {
                CommentDecision::Update { id, new_body } => {
                    self.client
                        .update_comment(&issue_key, id, new_body)
                        .await
                        .map_err(other_err)?;
                }
                CommentDecision::Delete { id } => {
                    delete_comment(&self.client, &issue_key, id).await?;
                }
                _ => {}
            }
        }
        for body in &adds {
            self.client
                .add_comment(&issue_key, body)
                .await
                .map_err(other_err)?;
        }

        // (i) Apply issue-header changes via the existing edit_full pipeline.
        // Reconstruct a 3b-only buffer from the user's header parse + body.
        // Use the fresh-issue tables so the slugs match what `parse_3b` /
        // `resolve_slugs_inplace` will see when execute_edit_full re-parses.
        let fresh_tables = build_slug_tables(&self.cache);
        let header_3b = render_3b_from_parsed(
            &user.header,
            &edit_full_fields(),
            &fresh_issue,
            &fresh_tables,
        );
        let header_outcome = self
            .execute_edit_full(&header_3b, version, Some(original_text))
            .await?;

        // Compose final message.
        let n_updates = decisions
            .iter()
            .filter(|d| matches!(d, CommentDecision::Update { .. }))
            .count();
        let n_deletes = decisions
            .iter()
            .filter(|d| matches!(d, CommentDecision::Delete { .. }))
            .count();
        let n_adds = adds.len();

        let comment_msg = match (n_updates, n_deletes, n_adds) {
            (0, 0, 0) => String::new(),
            _ => format!(", comments: +{n_adds} ~{n_updates} -{n_deletes}",),
        };

        Ok(match header_outcome {
            ActionOutcome::Done { message } => ActionOutcome::Done {
                message: Some(format!(
                    "{}{comment_msg}",
                    message.unwrap_or_else(|| format!("{issue_key} updated"))
                )),
            },
            ActionOutcome::NoChanges if comment_msg.is_empty() => ActionOutcome::NoChanges,
            ActionOutcome::NoChanges => ActionOutcome::Done {
                message: Some(format!("{issue_key} unchanged{comment_msg}")),
            },
            other => other,
        })
    }

    /// Re-render the buffer for a `Reopen` cycle: user's header preserved,
    /// each comment restored from `fresh` (or kept as user's body for adds),
    /// banner listing all errors at the top.
    fn render_foreign_reopen(
        &self,
        user: &ParsedWithComments,
        fresh_issue: &JiraIssueDetail,
        fresh_comments: &[JiraComment],
        errors: &[(String, String, String)],
        tables: &SlugTables,
    ) -> String {
        // Header: re-render the 3b layout with the user's editable values
        // and body, but read-only fields refreshed from fresh_issue.
        let header = render_3b_from_parsed(&user.header, &edit_full_fields(), fresh_issue, &tables);

        // Comments: rebuild newest→oldest from fresh_comments (so removed
        // ones come back), preserving user-add blocks.
        let mut sorted: Vec<&JiraComment> = fresh_comments.iter().collect();
        sorted.sort_by(|a, b| b.created.cmp(&a.created));

        let mut out = String::new();
        out.push_str(FOREIGN_BANNER_START);
        out.push('\n');
        for (_, msg, would_be) in errors {
            out.push_str(&format!("# • {msg}\n"));
            if !would_be.is_empty() {
                for line in would_be.lines() {
                    out.push_str(&format!("#   > {line}\n"));
                }
            }
        }
        out.push_str(FOREIGN_BANNER_END);
        out.push('\n');

        out.push_str(&header);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');

        // Re-emit user adds at the top so they're easy to find/edit on retry.
        for block in &user.blocks {
            if matches!(block.kind, CommentBlockKind::Add) && !block.body.trim().is_empty() {
                out.push_str(ADD_COMMENT_MARKER);
                out.push('\n');
                out.push('\n');
                let body = render_user_mentions(block.body.trim_end(), &tables.users);
                out.push_str(&body);
                out.push('\n');
                out.push('\n');
            }
        }

        for c in sorted {
            out.push_str(&render_comment_header(c));
            out.push('\n');
            out.push('\n');
            let body = render_user_mentions(c.body.trim_end(), &tables.users);
            out.push_str(&body);
            out.push('\n');
            out.push('\n');
        }

        out.push_str(&render_cache_section(&tables));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::comment_is_own;

    #[test]
    fn own_by_username_ignores_display_name_mismatch() {
        // The stable username matches even though the display name differs
        // (renamed profile) — the comment is still ours.
        assert!(comment_is_own(
            "Alice B.",
            "abecker",
            Some("Alice Becker"),
            Some("abecker"),
        ));
    }

    #[test]
    fn foreign_by_username_despite_shared_display_name() {
        // Two people share a display name; the username disambiguates them so
        // a colleague's comment is not treated as ours.
        assert!(!comment_is_own(
            "Alex Smith",
            "asmith2",
            Some("Alex Smith"),
            Some("asmith1"),
        ));
    }

    #[test]
    fn falls_back_to_display_name_when_username_missing() {
        // Jira returned no author username; the display name decides.
        assert!(comment_is_own("Carol", "", Some("Carol"), Some("cjones")));
        assert!(!comment_is_own("Dave", "", Some("Carol"), Some("cjones")));
    }

    #[test]
    fn permissive_when_identity_unknown() {
        // We couldn't resolve who we are → don't block the user.
        assert!(comment_is_own("Anyone", "anyone", None, None));
    }

    // --- editor-wrap robustness of the comment/CACHE structure ---
    //
    // A user's editor may hard-wrap long lines on save. Structural lines — the
    // comment headings and the trailing CACHE divider — must survive that so
    // the comment classifier does not mistake a purely re-wrapped buffer for
    // an edit of a foreign comment. All fixtures below are invented.

    use super::super::markers::CACHE_MARKER;
    use super::{
        CommentBlockKind, comments_md_to_canonical, rejoin_wrapped_cache_marker,
        split_canonical_blocks,
    };

    /// Editor hard-wrap: break each line at the last space ≤ width, dropping
    /// the break space (the common `$EDITOR`/`gq` behaviour).
    fn hard_wrap(md: &str, width: usize) -> String {
        let mut out: Vec<String> = Vec::new();
        for line in md.split('\n') {
            let mut rest = line.to_string();
            loop {
                if rest.chars().count() <= width {
                    out.push(rest);
                    break;
                }
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

    /// id → canonical (wiki) body for the comment region of a canonical buffer.
    fn comment_bodies(canonical: &str) -> std::collections::HashMap<String, String> {
        let lines: Vec<&str> = canonical.split('\n').collect();
        let cache_idx = lines
            .iter()
            .position(|l| l.trim_end() == CACHE_MARKER)
            .unwrap_or(lines.len());
        let mut m = std::collections::HashMap::new();
        for (kind, _middle, body) in split_canonical_blocks(&lines[..cache_idx]) {
            if let CommentBlockKind::Existing(id) = kind {
                m.insert(id, body);
            }
        }
        m
    }

    /// A synthetic edit buffer: body, a comments section with one existing
    /// comment whose heading is deliberately long, and the read-only CACHE
    /// divider followed by a scaffold block.
    fn synthetic_buffer() -> String {
        format!(
            "Some description body.\n\
             \n\
             ## Comments <!-- jira comments section -->\n\
             \n\
             ### @averylongdisplayname_department_extern 2022-11-09T10:00 <!-- jira comment id=987654 -->\n\
             \n\
             A short comment body that must round-trip unchanged.\n\
             \n\
             {CACHE_MARKER}\n\
             h1. labels: ll_alpha, ll_beta, ll_gamma\n\
             h1. statuses: ss_open, ss_done"
        )
    }

    #[test]
    fn wrapped_comment_heading_survives_editor_wrap() {
        let buf = synthetic_buffer();
        let baseline = comment_bodies(&comments_md_to_canonical(&buf));
        assert!(
            baseline.contains_key("987654"),
            "fixture must have the comment"
        );
        for width in [40usize, 50, 60, 66, 72, 80] {
            let wrapped = comments_md_to_canonical(&hard_wrap(&buf, width));
            let got = comment_bodies(&wrapped);
            assert!(
                got.contains_key("987654"),
                "width {width}: comment heading wrap lost the comment"
            );
        }
    }

    #[test]
    fn wrapped_cache_divider_not_absorbed_into_last_comment() {
        let buf = synthetic_buffer();
        let base_body = comment_bodies(&comments_md_to_canonical(&buf))
            .remove("987654")
            .unwrap();
        for width in [40usize, 50, 60] {
            // These widths wrap the 66-char CACHE divider.
            let got = comment_bodies(&comments_md_to_canonical(&hard_wrap(&buf, width)));
            let body = got.get("987654").expect("comment present");
            assert!(
                !body.contains("labels:") && !body.contains("statuses:"),
                "width {width}: CACHE block leaked into comment body"
            );
            assert_eq!(
                super::super::wiki_md::normalize_ws(body),
                super::super::wiki_md::normalize_ws(&base_body),
                "width {width}: comment body changed by wrapping"
            );
        }
    }

    #[test]
    fn rejoin_wrapped_cache_marker_reassembles_only_the_marker() {
        // A wrapped marker is rejoined; unrelated `#### CACHE …` prose is not.
        let wrapped = "#### CACHE / available labels, users &\nstatuses (do not edit) ####";
        assert_eq!(rejoin_wrapped_cache_marker(wrapped), CACHE_MARKER);
        let whole = format!("{CACHE_MARKER}\nrest");
        assert_eq!(rejoin_wrapped_cache_marker(&whole), whole);
        let unrelated = "#### CACHE OF THE ATLANTIC\nis a documentary";
        assert_eq!(rejoin_wrapped_cache_marker(unrelated), unrelated);
    }
}
