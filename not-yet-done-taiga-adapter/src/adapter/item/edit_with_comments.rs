//! `edit_with_comments` action — same buffer as `edit_full` plus inline
//! existing comments and `--- add ---` blocks for new ones. `del`/`delete`
//! as the sole body of a comment block deletes that comment.
//!
//! Per the design discussion: no per-comment conflict detection. The user
//! can only edit their own comments — Taiga answers a foreign edit with a
//! 403, so the buffer marks those comments [`NOT_YOURS_MARKER`] and the
//! execute path rejects them locally instead of asking. Deletion stays
//! open: Taiga also lets project admins delete a foreign comment, so that
//! call keeps going to the server, which is the authority. For their own
//! comments the user takes responsibility for not stomping on parallel
//! edits.

use not_yet_done_content::*;

use crate::client::{TaigaComment, delete_comment, edit_comment, fetch_comments};

use super::TaigaItemNode;
use super::edit_full::{build_tables, edit_full_fields};
use super::slugs::build_user_table;
use super::template::{self, FieldError, Parsed3b, render_3b, render_with_errors};

const ADD_COMMENT_MARKER: &str = "--- add ---";
const DELETE_KEYWORD_DEL: &str = "del";
const DELETE_KEYWORD_DELETE: &str = "delete";

#[derive(Debug, Clone)]
enum CommentBlockKind {
    /// `foreign` mirrors the [`NOT_YOURS_MARKER`] of the header line.
    Existing {
        id: String,
        foreign: bool,
    },
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

fn render_comment_header(c: &TaigaComment, me: Option<&str>) -> String {
    let ts = short_ts(&c.created);
    let mark = if is_foreign(c, me) {
        format!(" {NOT_YOURS_MARKER}")
    } else {
        String::new()
    };
    format!("--- @{} {ts}{mark} (id={}) ---", c.author, c.id)
}

/// A comment belongs to somebody else when its authoritative username
/// differs from the authenticated one. Unknown on either side → treat it
/// as ours and let the server stay the authority (a 403 still surfaces),
/// rather than blocking an edit the user is allowed to make.
fn is_foreign(c: &TaigaComment, me: Option<&str>) -> bool {
    match (c.author_username.as_deref(), me) {
        (Some(author), Some(me)) if !author.is_empty() && !me.is_empty() => author != me,
        _ => false,
    }
}

/// Compare comment bodies without the whitespace an editor rewrites on
/// save. Trailing blanks per line, CRLF and the outer blank lines are
/// cosmetic; anything else — blank lines between paragraphs, leading
/// indentation of a code block — is the user's text and stays
/// significant.
fn normalize_body(s: &str) -> String {
    let mut lines: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Trim the time component off ISO timestamps; keep up to minute precision.
pub(super) fn short_ts(ts: &str) -> String {
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

/// Parse a `--- @author ts [not yours] (id=...) ---` line; return the id
/// (anything between `id=` and `)`) and whether the header carries the
/// [`NOT_YOURS_MARKER`]. A buffer rendered by an older binary has no
/// marker — then `foreign` is false and the server decides as before.
fn parse_comment_header(line: &str) -> Option<(&str, bool)> {
    let trimmed = line.trim_end();
    let inner = trimmed.strip_prefix("--- ")?.strip_suffix(" ---")?;
    if !inner.starts_with('@') {
        return None;
    }
    let id_open = inner.rfind("(id=")?;
    let id_part = &inner[id_open + 4..];
    let id_close = id_part.find(')')?;
    let foreign = inner[..id_open].contains(NOT_YOURS_MARKER);
    Some((&id_part[..id_close], foreign))
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
        if let Some((id, foreign)) = parse_comment_header(trimmed) {
            if let Some(prev) = current.take() {
                blocks_raw.push(prev);
            }
            current = Some((
                CommentBlockKind::Existing {
                    id: id.to_string(),
                    foreign,
                },
                Vec::new(),
            ));
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
        // Identity for the "is this comment mine?" gate. Cached from the
        // login session; an error here only costs the marker, so the edit
        // path stays usable and the server decides.
        let me = self
            .client
            .current_username()
            .await
            .ok()
            .map(|s| s.to_string());

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
            out.push_str(&render_comment_header(c, me.as_deref()));
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
            file_path: None,
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

        // 2. Snapshot map: id → (body, foreign) for diffing existing-comment
        //    edits. Ownership is read from the snapshot, not from the user
        //    buffer, so deleting the marker by hand does not unlock an edit
        //    the server would refuse anyway.
        let snap_by_id: std::collections::HashMap<&str, (&str, bool)> = snapshot
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                CommentBlockKind::Existing { id, foreign } => {
                    Some((id.as_str(), (b.body.as_str(), *foreign)))
                }
                CommentBlockKind::Add => None,
            })
            .collect();

        // Members table so `@uu_slug` mentions in comment bodies resolve to
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
            let CommentBlockKind::Existing {
                id,
                foreign: block_foreign,
            } = &block.kind
            else {
                continue;
            };
            let user_body = block.body.trim();
            let (snap_body, foreign) = snap_by_id
                .get(id.as_str())
                .copied()
                .map(|(body, foreign)| (body.trim(), foreign))
                .unwrap_or(("", *block_foreign));
            if is_delete_keyword(user_body) {
                if let Err(e) =
                    delete_comment(&self.client, self.detail.item_type, self.detail.id, id).await
                {
                    comment_errors.push(format!("delete {id}: {e}"));
                } else {
                    n_deletes += 1;
                }
            } else {
                let resolved = template::resolve_user_mentions(user_body, &users);
                // Normalized compare: an editor that strips trailing
                // whitespace on save must not count as an edit — on a
                // foreign comment that alone bought a doomed 403.
                if normalize_body(&resolved) != normalize_body(snap_body) {
                    if foreign {
                        // Taiga answers this with a 403 while the item
                        // PATCH still goes through — say so here instead
                        // of relaying a server error nobody can act on.
                        comment_errors.push(format!(
                            "edit {id}: not authored by you — Taiga only lets a comment's author edit it"
                        ));
                    } else if let Err(e) = edit_comment(
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
                CommentBlockKind::Existing { .. } => None,
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
            format!(", comments: +{} ~{} -{}", adds.len(), n_updates, n_deletes,)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// All fixtures below are invented.
    fn comment(id: &str, author: &str, username: Option<&str>) -> TaigaComment {
        TaigaComment {
            id: id.into(),
            author: author.into(),
            author_username: username.map(|s| s.to_string()),
            created: "2024-03-04T09:15:00+0000".into(),
            body: "body".into(),
        }
    }

    #[test]
    fn own_comment_header_has_no_marker() {
        let c = comment("aaa-111", "Robin Vega", Some("rvega"));
        let line = render_comment_header(&c, Some("rvega"));
        assert_eq!(line, "--- @Robin Vega 2024-03-04 09:15 (id=aaa-111) ---");
        assert_eq!(parse_comment_header(&line), Some(("aaa-111", false)));
    }

    #[test]
    fn foreign_comment_header_is_marked_and_round_trips() {
        let c = comment("bbb-222", "Sam Okoro", Some("sokoro"));
        let line = render_comment_header(&c, Some("rvega"));
        assert_eq!(
            line,
            "--- @Sam Okoro 2024-03-04 09:15 [not yours] (id=bbb-222) ---"
        );
        assert_eq!(parse_comment_header(&line), Some(("bbb-222", true)));
    }

    #[test]
    fn unknown_identity_stays_unmarked_so_the_server_decides() {
        let c = comment("ccc-333", "System", None);
        assert!(!is_foreign(&c, Some("rvega")));
        assert!(!is_foreign(&comment("d", "Sam", Some("sokoro")), None));
    }

    #[test]
    fn header_without_marker_parses_as_own() {
        assert_eq!(
            parse_comment_header("--- @Sam Okoro 2024-03-04 09:15 (id=bbb-222) ---"),
            Some(("bbb-222", false))
        );
    }

    #[test]
    fn parsed_blocks_carry_the_marker() {
        let buf = "subject: hi\n\
                   ---\n\
                   ===\n\
                   the description\n\
                   --- @Sam Okoro 2024-03-04 09:15 [not yours] (id=bbb-222) ---\n\
                   theirs\n\
                   --- @Robin Vega 2024-03-04 09:16 (id=aaa-111) ---\n\
                   mine\n";
        let parsed = parse_with_comments(buf).expect("parses");
        let kinds: Vec<(String, bool)> = parsed
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                CommentBlockKind::Existing { id, foreign } => Some((id.clone(), *foreign)),
                CommentBlockKind::Add => None,
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("bbb-222".to_string(), true),
                ("aaa-111".to_string(), false)
            ]
        );
    }

    #[test]
    fn trailing_whitespace_and_crlf_are_not_an_edit() {
        assert_eq!(
            normalize_body("one   \r\ntwo\t\r\n"),
            normalize_body("one\ntwo")
        );
        assert_eq!(normalize_body("\n\ntext \n\n"), normalize_body("text"));
    }

    #[test]
    fn real_text_changes_still_differ() {
        assert_ne!(normalize_body("one\ntwo"), normalize_body("one\nTwo"));
        // A blank line between paragraphs is the user's text, not cosmetics.
        assert_ne!(normalize_body("one\ntwo"), normalize_body("one\n\ntwo"));
        // Leading indentation is significant (code blocks).
        assert_ne!(normalize_body("    code"), normalize_body("code"));
    }
}
