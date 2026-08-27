//! Read-only Markdown rendering of one item — the body `export_workspace`
//! writes to `ticket.md`.
//!
//! Deliberately *not* the 3b edit template ([`super::template`]): that is a
//! round-trippable buffer (slugged values, `---`/`===` markers, a trailing
//! completions block) and every one of those traits is noise in a document
//! meant to be read. Here values are rendered as the user knows them and the
//! markers are gone.
//!
//! Taiga descriptions and comments are already Markdown, so — unlike Jira —
//! nothing is converted on the way; the only rewriting is turning absolute
//! attachment URLs into the `attachments/<name>` paths the exported folder
//! carries, so the document renders offline and without a token.

use not_yet_done_content::download::safe_attachment_name;

use crate::client::{TaigaAttachment, TaigaComment};

use super::ItemDetail;
use super::edit_with_comments::short_ts;

/// Heading that opens the comment section. Carries an HTML marker like the
/// Jira export's so a tool can find the section without matching prose.
const COMMENTS_SECTION: &str = "## Comments <!-- taiga comments section -->";

/// Heading that opens the attachment list, marked like the comment section.
const ATTACHMENTS_SECTION: &str = "## Attachments <!-- taiga attachments section -->";

/// Extensions a browser renders inline, so those attachments are embedded
/// instead of merely linked.
const IMAGE_EXT: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "svg"];

/// A link target usable in Markdown. Attachment names keep their spaces and
/// brackets, which would end the target early, so anything but a plain path is
/// wrapped in the angle-bracket form pandoc accepts.
fn link_target(local: &str) -> String {
    if local.contains([' ', '(', ')', '<', '>']) {
        format!("<{local}>")
    } else {
        local.to_string()
    }
}

fn is_image(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, ext)| IMAGE_EXT.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// The display form of the item's reference — `<project-slug>#<ref>`, or
/// `#<ref>` when the detail payload carried no project slug.
pub(super) fn display_ref(detail: &ItemDetail) -> String {
    match &detail.project_slug {
        Some(slug) if !slug.is_empty() => format!("{slug}#{}", detail.r#ref),
        _ => format!("#{}", detail.r#ref),
    }
}

/// Filesystem-safe key for the workspace folder: the display ref with the `#`
/// dropped, so `proj#42` becomes `proj-42`.
pub(super) fn workspace_key(detail: &ItemDetail) -> String {
    display_ref(detail)
        .replace('#', "-")
        .trim_matches('-')
        .to_string()
}

fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn render_table(rows: &[(&str, String)]) -> Vec<String> {
    let mut out = vec!["| Field | Value |".to_string(), "| --- | --- |".to_string()];
    for (k, v) in rows {
        out.push(format!("| {} | {} |", escape_cell(k), escape_cell(v)));
    }
    out
}

/// Replace every absolute attachment URL in `text` with the local path the
/// exported folder uses. Both the cache-busted `url` and its query-free form
/// are matched, because Taiga hands out either depending on the endpoint.
fn localize_attachments(text: &str, attachments: &[TaigaAttachment]) -> String {
    let mut out = text.to_string();
    for a in attachments {
        let local = link_target(&format!("attachments/{}", safe_attachment_name(&a.name)));
        let bare = a.url.split('?').next().unwrap_or(&a.url).to_string();
        // Longest first: replacing the bare form first would leave the query
        // string of the full form dangling behind the local path.
        if a.url != bare {
            out = out.replace(&a.url, &local);
        }
        out = out.replace(&bare, &local);
    }
    out
}

/// The attachment list — every file the workspace carries that the body does
/// not already show. Taiga keeps attachments beside the text rather than in
/// it, so without this section the exported files would be invisible in the
/// preview; an attachment the description already embeds is skipped so it is
/// not rendered twice.
fn render_attachments(attachments: &[TaigaAttachment], body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in attachments {
        let local = format!("attachments/{}", safe_attachment_name(&a.name));
        if body.contains(&local) {
            continue;
        }
        let target = link_target(&local);
        out.push(String::new());
        if is_image(&a.name) {
            out.push(format!("![{}]({})", a.name, target));
        } else {
            out.push(format!("- [{}]({})", a.name, target));
        }
    }
    if out.is_empty() {
        return out;
    }
    let mut section = vec![String::new(), ATTACHMENTS_SECTION.to_string()];
    section.extend(out);
    section
}

/// The whole document: two header tables, the description, and every comment
/// under a `### @author <when>` heading.
pub(super) fn item_markdown(
    detail: &ItemDetail,
    comments: &[TaigaComment],
    attachments: &[TaigaAttachment],
) -> String {
    let mut out: Vec<String> = Vec::new();
    out.extend(render_table(&[
        ("subject", detail.subject.clone()),
        ("status", detail.status.clone()),
        ("assignee", detail.assignees.join(", ")),
        ("tags", detail.tags.join(", ")),
    ]));
    out.push(String::new());
    out.extend(render_table(&[
        ("ref", display_ref(detail)),
        ("type", detail.item_type.as_str().to_string()),
        ("creator", detail.creator.clone()),
        ("modified", detail.modified.clone().unwrap_or_default()),
    ]));
    out.push(String::new());
    out.push(localize_attachments(&detail.description, attachments));

    let mut body_so_far = out.join("\n");

    if !comments.is_empty() {
        out.push(String::new());
        out.push(COMMENTS_SECTION.to_string());
        for c in comments {
            out.push(String::new());
            out.push(format!(
                "### @{} {} <!-- taiga comment id={} -->",
                c.author,
                short_ts(&c.created),
                c.id
            ));
            if !c.body.trim().is_empty() {
                out.push(String::new());
                let body = localize_attachments(&c.body, attachments);
                body_so_far.push('\n');
                body_so_far.push_str(&body);
                out.push(body);
            }
        }
    }

    // Appended last so the links sit below the prose that may already carry
    // them; the skip check needs the whole body, comments included.
    out.extend(render_attachments(attachments, &body_so_far));

    let mut text = out.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ItemType;

    fn detail() -> ItemDetail {
        ItemDetail {
            item_type: ItemType::Task,
            id: 7,
            r#ref: 42,
            project_id: 1,
            project_slug: Some("demo".into()),
            subject: "A | piped subject".into(),
            description: "Body with ![shot](https://taiga.invalid/media/a.png?v=1)".into(),
            status: "In progress".into(),
            assignees: vec!["Alice".into()],
            assignee_usernames: vec!["alice".into()],
            creator: "Bob".into(),
            tags: vec!["frontend".into()],
            modified: Some("2026-01-02T03:04:05+0000".into()),
            version: 3,
            parent_user_story_id: None,
            parent_user_story_subject: None,
        }
    }

    fn attachment() -> TaigaAttachment {
        TaigaAttachment {
            id: 1,
            name: "a.png".into(),
            size: 10,
            description: String::new(),
            created_date: "2026-01-01T00:00:00+0000".into(),
            modified_date: "2026-01-01T00:00:00+0000".into(),
            owner: 1,
            url: "https://taiga.invalid/media/a.png?v=1".into(),
            thumbnail_url: None,
        }
    }

    #[test]
    fn header_tables_escape_pipes_and_carry_both_blocks() {
        let md = item_markdown(&detail(), &[], &[]);
        assert!(md.contains("| subject | A \\| piped subject |"));
        assert!(md.contains("| ref | demo#42 |"));
        assert!(md.contains("| tags | frontend |"));
        // No edit-template markers and no completions block in a document.
        assert!(!md.contains("\n===\n"));
        assert!(!md.contains("COMPLETIONS"));
    }

    #[test]
    fn attachment_urls_become_local_paths() {
        let md = item_markdown(&detail(), &[], &[attachment()]);
        assert!(md.contains("![shot](attachments/a.png)"));
        assert!(!md.contains("taiga.invalid"));
    }

    #[test]
    fn comments_get_a_section_only_when_present() {
        assert!(!item_markdown(&detail(), &[], &[]).contains("## Comments"));
        let c = TaigaComment {
            id: "c1".into(),
            author: "Alice".into(),
            author_username: Some("alice".into()),
            created: "2026-01-02T03:04:05+0000".into(),
            body: "looks good".into(),
        };
        let md = item_markdown(&detail(), std::slice::from_ref(&c), &[]);
        assert!(md.contains("## Comments <!-- taiga comments section -->"));
        assert!(md.contains("### @Alice 2026-01-02 03:04 <!-- taiga comment id=c1 -->"));
        assert!(md.contains("looks good"));
    }

    #[test]
    fn attachments_section_lists_only_unreferenced_files() {
        let mut doc = TaigaAttachment {
            name: "memo.pdf".into(),
            ..attachment()
        };
        doc.id = 2;
        doc.url = "https://taiga.invalid/media/memo.pdf?v=1".into();
        // The description embeds `a.png`, so only the PDF is listed.
        let md = item_markdown(&detail(), &[], &[attachment(), doc]);
        assert!(md.contains("## Attachments <!-- taiga attachments section -->"));
        assert!(md.contains("- [memo.pdf](attachments/memo.pdf)"));
        assert_eq!(md.matches("attachments/a.png").count(), 1);
    }

    #[test]
    fn image_attachments_are_embedded_and_absent_ones_add_no_section() {
        let mut img = attachment();
        img.name = "shot.PNG".into();
        img.url = "https://taiga.invalid/media/shot.PNG?v=1".into();
        let md = item_markdown(&detail(), &[], &[img]);
        assert!(md.contains("![shot.PNG](attachments/shot.PNG)"));
        assert!(!item_markdown(&detail(), &[], &[]).contains("## Attachments"));
    }

    #[test]
    fn spacey_names_use_the_angle_bracket_target() {
        let mut a = attachment();
        a.name = "Memo Buchhaltung.pdf".into();
        a.url = "https://taiga.invalid/media/memo.pdf?v=1".into();
        let md = item_markdown(&detail(), &[], &[a]);
        assert!(md.contains("- [Memo Buchhaltung.pdf](<attachments/Memo Buchhaltung.pdf>)"));
    }

    #[test]
    fn workspace_key_is_filesystem_safe() {
        assert_eq!(workspace_key(&detail()), "demo-42");
        let mut d = detail();
        d.project_slug = None;
        assert_eq!(workspace_key(&d), "42");
    }
}
