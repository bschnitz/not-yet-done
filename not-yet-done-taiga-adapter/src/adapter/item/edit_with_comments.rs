//! `edit_with_comments` action — same buffer as `edit_full` plus inline
//! existing comments and `--- add ---` blocks for new ones. `del`/`delete`
//! as the sole body of a comment block deletes that comment.
//!
//! Per the design discussion: no per-comment conflict detection. The user
//! can only edit their own comments (server enforces with 403); they take
//! responsibility for not stomping on parallel edits to their own.

use not_yet_done_content::*;

use crate::client::{TaigaComment, delete_comment, edit_comment, fetch_comments};

use super::TaigaItemNode;
use super::edit_full::{build_tables, edit_full_fields};
use super::slugs::build_user_table;
use super::template::{
    self, FieldError, Parsed3b, render_3b, render_with_errors,
};

const ADD_COMMENT_MARKER: &str = "--- add ---";
const DELETE_KEYWORD_DEL: &str = "del";
const DELETE_KEYWORD_DELETE: &str = "delete";

#[derive(Debug, Clone)]
enum CommentBlockKind {
    Existing(String),
    Add,
}

#[derive(Debug)]
struct ParsedCommentBlock {
    kind: CommentBlockKind,
    body: String,
}

#[derive(Debug)]
struct ParsedWithComments {
    header: Parsed3b,
    blocks: Vec<ParsedCommentBlock>,
}

fn render_comment_header(c: &TaigaComment) -> String {
    let ts = short_ts(&c.created);
    format!("--- @{} {ts} (id={}) ---", c.author, c.id)
}

/// Trim the time component off ISO timestamps; keep up to minute precision.
fn short_ts(ts: &str) -> String {
    if let Some((date, rest)) = ts.split_once('T') {
        let time_part = rest.split('.').next().unwrap_or(rest);
        let time_part = time_part.split('+').next().unwrap_or(time_part);
        let time_part = time_part.trim_end_matches('Z');
        let parts: Vec<&str> = time_part.split(':').collect();
        let hm = if parts.len() >= 2 {
            format!("{}:{}", parts[0], parts[1])
        } else {
            time_part.to_string()
        };
        return format!("{date} {hm}");
    }
    ts.to_string()
}

/// Parse a `--- @author ts (id=...) ---` line; return the id (anything
/// between `id=` and `)`).
fn parse_comment_header_id(line: &str) -> Option<&str> {
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

fn is_delete_keyword(body: &str) -> bool {
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

fn parse_with_comments(text: &str) -> std::result::Result<ParsedWithComments, Vec<FieldError>> {
    let text = template::strip_cache_section(text);
    let text = template::strip_banner(text);

    let mut header_lines: Vec<&str> = Vec::new();
    let mut blocks_raw: Vec<(CommentBlockKind, Vec<&str>)> = Vec::new();
    let mut current: Option<(CommentBlockKind, Vec<&str>)> = None;

    for line in text.lines() {
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
    }
    if let Some(prev) = current.take() {
        blocks_raw.push(prev);
    }

    let header_text = header_lines.join("\n");
    let header = template::parse_3b(&header_text)?;

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
        if matches!(kind, CommentBlockKind::Add) && body.trim().is_empty() {
            continue;
        }
        blocks.push(ParsedCommentBlock { kind, body });
    }

    Ok(ParsedWithComments { header, blocks })
}

impl TaigaItemNode {
    pub(super) async fn prepare_edit_with_comments(&self) -> Result<EditorPrep> {
        let statuses = self
            .client
            .ensure_statuses(self.detail.project_id, self.detail.item_type)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let members = self
            .client
            .ensure_members(self.detail.project_id)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let tags = self
            .client
            .ensure_tags(self.detail.project_id)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let tables = build_tables(&statuses, &members, &tags);

        let comments = fetch_comments(&self.client, self.detail.item_type, self.detail.id)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;

        let mut out = render_3b(
            &edit_full_fields(),
            &self.detail,
            &tables,
            None,
            None,
            false,
        );
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');

        out.push_str(ADD_COMMENT_MARKER);
        out.push('\n');
        out.push('\n');

        let mut sorted: Vec<&TaigaComment> = comments.iter().collect();
        sorted.sort_by(|a, b| b.created.cmp(&a.created));
        for c in sorted {
            out.push_str(&render_comment_header(c));
            out.push('\n');
            out.push('\n');
            out.push_str(c.body.trim_end());
            out.push('\n');
            out.push('\n');
        }

        out.push_str(&template::render_cache_section(&tables));

        Ok(EditorPrep {
            template: out,
            version: self.detail.version.to_string(),
            suffix: ".md".into(),
        })
    }

    pub(super) async fn execute_edit_with_comments(
        &mut self,
        text: &str,
        original_text: &str,
        version: &str,
    ) -> Result<ActionOutcome> {
        // 1. Parse buffer.
        let user = match parse_with_comments(text) {
            Ok(p) => p,
            Err(errs) => {
                return Ok(ActionOutcome::Reopen {
                    content: render_with_errors(text, &errs),
                    new_version: None,
                });
            }
        };
        let snapshot = parse_with_comments(original_text).map_err(|errs| {
            ContentError::Other(
                format!(
                    "internal: original buffer failed to re-parse ({} error(s))",
                    errs.len()
                )
                .into(),
            )
        })?;

        // 2. Snapshot map: id → body for diffing existing-comment edits.
        let snap_by_id: std::collections::HashMap<&str, &str> = snapshot
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                CommentBlockKind::Existing(id) => Some((id.as_str(), b.body.as_str())),
                CommentBlockKind::Add => None,
            })
            .collect();

        // Members table so `@uu-slug` mentions in comment bodies resolve to
        // Taiga's wire `@username` form (the same slug system as the assignee
        // field). Existing comments render with `@username` already, so the
        // resolved body compares cleanly against the snapshot when unchanged.
        let members = self
            .client
            .ensure_members(self.detail.project_id)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let users = build_user_table(&members);

        // 3. Apply per-comment ops first (independent of item version).
        let mut comment_errors: Vec<String> = Vec::new();
        let mut n_updates = 0usize;
        let mut n_deletes = 0usize;

        for block in &user.blocks {
            let CommentBlockKind::Existing(id) = &block.kind else {
                continue;
            };
            let user_body = block.body.trim();
            let snap_body = snap_by_id.get(id.as_str()).copied().unwrap_or("").trim();
            if is_delete_keyword(user_body) {
                if let Err(e) = delete_comment(
                    &self.client,
                    self.detail.item_type,
                    self.detail.id,
                    id,
                )
                .await
                {
                    comment_errors.push(format!("delete {id}: {e}"));
                } else {
                    n_deletes += 1;
                }
            } else {
                let resolved = template::resolve_user_mentions(user_body, &users);
                if resolved != snap_body {
                    if let Err(e) = edit_comment(
                        &self.client,
                        self.detail.item_type,
                        self.detail.id,
                        id,
                        &resolved,
                    )
                    .await
                    {
                        comment_errors.push(format!("edit {id}: {e}"));
                    } else {
                        n_updates += 1;
                    }
                }
            }
        }

        // 4. Collect new-comment bodies (mentions resolved to `@username`).
        let adds: Vec<String> = user
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                CommentBlockKind::Add => {
                    let body = b.body.trim();
                    if body.is_empty() {
                        None
                    } else {
                        Some(template::resolve_user_mentions(body, &users))
                    }
                }
                CommentBlockKind::Existing(_) => None,
            })
            .collect();

        // 5. Reuse `execute_edit_full_inner` for the header + comment-add
        //    PATCHes. We synthesise a 3b-only buffer from the user's parsed
        //    header so the existing diff/parse path works.
        let header_3b = self
            .header_only_buffer_from(&user.header, original_text)
            .await?;
        let header_outcome = self
            .execute_edit_full_inner(&header_3b, &header_3b, version, Some(&adds))
            .await?;

        let comment_errs_str = if comment_errors.is_empty() {
            String::new()
        } else {
            format!(" (errors: {})", comment_errors.join("; "))
        };
        let comment_msg = if n_updates + n_deletes + adds.len() > 0 {
            format!(
                ", comments: +{} ~{} -{}",
                adds.len(),
                n_updates,
                n_deletes,
            )
        } else {
            String::new()
        };

        Ok(match header_outcome {
            ActionOutcome::Done { message } => ActionOutcome::Done {
                message: Some(format!(
                    "{}{comment_msg}{comment_errs_str}",
                    message.unwrap_or_default()
                )),
            },
            ActionOutcome::NoChanges if comment_msg.is_empty() => ActionOutcome::NoChanges,
            ActionOutcome::NoChanges => ActionOutcome::Done {
                message: Some(format!("unchanged{comment_msg}{comment_errs_str}")),
            },
            other => other,
        })
    }

    /// Re-render a 3b-only buffer from the user's parsed header. Re-uses
    /// the current detail + tables so that the buffer round-trips through
    /// `parse_3b` cleanly when `execute_edit_full_inner` re-parses it.
    async fn header_only_buffer_from(
        &self,
        header: &Parsed3b,
        _original_text: &str,
    ) -> Result<String> {
        let statuses = self
            .client
            .ensure_statuses(self.detail.project_id, self.detail.item_type)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let members = self
            .client
            .ensure_members(self.detail.project_id)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let tags = self
            .client
            .ensure_tags(self.detail.project_id)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let tables = build_tables(&statuses, &members, &tags);
        Ok(template::render_3b_from_parsed(
            header,
            &edit_full_fields(),
            &self.detail,
            &tables,
        ))
    }
}
