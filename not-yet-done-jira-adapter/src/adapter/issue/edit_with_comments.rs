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
    ADD_COMMENT_MARKER, DELETE_KEYWORD_DEL, DELETE_KEYWORD_DELETE,
    FOREIGN_BANNER_END, FOREIGN_BANNER_START,
};
use super::slugs::{
    build_slug_tables, parse_user_mentions, render_user_mentions,
    resolve_slugs_inplace,
};
use super::template::{
    FieldError, Parsed3b, edit_full_fields, render_3b_from_parsed,
    render_cache_section, strip_banner, strip_cache_section,
};

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
    Update { id: String, new_body: String },
    /// User wrote `del`/`delete` — DELETE.
    Delete { id: String },
    /// Foreign user edited; user accepted the new content (user.body == fresh.body).
    AcceptForeign,
    /// Foreign user edited; user kept the snapshot (user.body == snapshot.body).
    /// No write needed; next prepare will pick up the fresh body.
    DropOurs,
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
async fn delete_comment(
    client: &Arc<JiraClient>,
    issue_key: &str,
    comment_id: &str,
) -> Result<()> {
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
    ) -> String {
        let tables = build_slug_tables(&self.cache);
        // Render the 3b header without the CACHE section — we append it
        // once at the very end after the comment list.
        let mut out = self.render_3b_full(editable_fields, detail, None, None, false);
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

        out.push_str(&render_cache_section(&tables));
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
        // banner lands with proper @uu-slugs everywhere.
        let mut mention_sources: Vec<&str> =
            fresh_comments.iter().map(|c| c.body.as_str()).collect();
        for block in &user.blocks {
            mention_sources.push(block.body.as_str());
        }
        resolve_unknown_mentions(&self.client, &self.cache, &mention_sources).await;
        let current_user = self.client.current_user().await.ok().map(|s| s.to_string());

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

                    let is_own = match (current_user.as_deref(), fresh) {
                        (Some(u), Some(c)) => c.author == u,
                        // Permissive default when we don't know the user.
                        (None, _) => true,
                        // Comment vanished upstream — owner check moot, error below.
                        (_, None) => false,
                    };
                    let is_delete = is_delete_keyword(user_body);

                    let Some(fresh) = fresh else {
                        // Comment was deleted upstream while we edited.
                        if user_body == snapshot_body {
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

                    let foreign_changed = fresh.body.trim() != snapshot_body.trim();
                    let user_changed = user_body != snapshot_body.trim();

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
                            if user_body == fresh.body.trim() {
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
            let still_present = user.blocks.iter().any(|b| matches!(&b.kind, CommentBlockKind::Existing(id) if id == snap_id));
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
            let content = self.render_foreign_reopen(&user, &fresh_issue, &fresh_comments, &errors);
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
            self.client.add_comment(&issue_key, body).await.map_err(other_err)?;
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
            _ => format!(
                ", comments: +{n_adds} ~{n_updates} -{n_deletes}",
            ),
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
    ) -> String {
        // Header: re-render the 3b layout with the user's editable values
        // and body, but read-only fields refreshed from fresh_issue.
        let tables = build_slug_tables(&self.cache);
        let header = render_3b_from_parsed(
            &user.header,
            &edit_full_fields(),
            fresh_issue,
            &tables,
        );

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
