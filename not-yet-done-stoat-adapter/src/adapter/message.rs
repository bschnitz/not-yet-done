//! `stoat:message` — leaf node carrying a single message body.
//!
//! Built two ways: in bulk by [`StoatChannelNode::list`](super::channel)
//! from a `messages` page, and singly by
//! [`StoatAdapter::get_by_id`](super::StoatAdapter) for the preview path
//! (which re-fetches the body via `GET …/messages/{id}`). Node ids are
//! **composite** — `<channel_id>/msg/<message_id>` — so `get_by_id` can
//! recover the channel a bare ULID would not encode.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;

use not_yet_done_content::{
    ActionInput, ActionOutcome, Content, ContentError, EditorPrep, HintPlacement, InputSpec,
    Metadata, MetadataField, Node, NodeAction, NodeType, Result,
};

use super::mentions;
use super::other_err;
use super::types::message_type;
use crate::client::{MessageView, StoatClient};

/// Build the composite node id the tree uses for a message.
pub(super) fn composite_id(channel_id: &str, message_id: &str) -> String {
    format!("{channel_id}/msg/{message_id}")
}

/// Split a composite id back into `(channel_id, message_id)`. Returns
/// `None` for ids that don't carry the `/msg/` marker.
pub(super) fn split_composite(id: &str) -> Option<(&str, &str)> {
    id.split_once("/msg/")
}

/// Format a ULID-derived millisecond timestamp as `YYYY-MM-DD HH:MM`
/// (UTC). Falls back to an empty string for messages whose id wasn't a
/// decodable ULID.
fn format_ts(ms: Option<u64>) -> String {
    match ms.and_then(|ms| DateTime::from_timestamp_millis(ms as i64)) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        None => String::new(),
    }
}

/// A small, curated set of unicode reactions offered by the `react`
/// picker. The Revolt API accepts any unicode emoji (or custom-emoji id),
/// but a short list keeps the picker usable; the `value` is the emoji
/// itself, sent verbatim (percent-encoded) on the reaction endpoint.
const REACTION_EMOJI: &[(&str, &str)] = &[
    ("👍 thumbs up", "👍"),
    ("❤️ heart", "❤️"),
    ("😂 joy", "😂"),
    ("🎉 tada", "🎉"),
    ("😮 wow", "😮"),
    ("😢 sad", "😢"),
    ("🙏 thanks", "🙏"),
    ("✅ check", "✅"),
];

/// Actions a single message exposes. Static per node_type (the
/// deterministic-per-type contract the hint resolver relies on) — edits
/// and deletes of other users' messages are rejected by the server at
/// execute time, not filtered here.
pub(super) fn message_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("edit_message", "edit", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar),
        NodeAction::new("delete_message", "delete", InputSpec::None),
        NodeAction::new("react", "react", InputSpec::Picker),
    ]
}

pub(super) struct StoatMessageNode {
    client: Arc<StoatClient>,
    channel_id: String,
    message_id: String,
    composite_id: String,
    label: String,
    /// Raw body, with `<@ID>` mention codes intact — the single source of
    /// truth for the edit round-trip and `Content::read`. Display forms
    /// (`label`, the `content` metadata field) are derived from it.
    content_body: String,
    /// Server-scoped `id → username` map for resolving mentions.
    users: Arc<HashMap<String, String>>,
    metadata: Metadata,
}

impl StoatMessageNode {
    pub(super) fn new(
        client: Arc<StoatClient>,
        view: MessageView,
        users: Arc<HashMap<String, String>>,
    ) -> Self {
        let composite_id = composite_id(&view.channel_id, &view.id);
        let timestamp = format_ts(view.timestamp_ms);
        // Display body: `<@ID>` → `@username` (unknown ids kept raw).
        let display_body = mentions::render_display(&view.content, &users);
        let mut fields = vec![
            MetadataField {
                key: "author".into(),
                value: view.author_name.clone(),
                display_label: "Author".into(),
                editable: false,
                allowed_values: None,
            },
            MetadataField {
                key: "time".into(),
                value: timestamp,
                display_label: "Time".into(),
                editable: false,
                allowed_values: None,
            },
            MetadataField {
                key: "author_id".into(),
                value: view.author_id,
                display_label: "Author ID".into(),
                editable: false,
                allowed_values: None,
            },
            // Display body (with newlines) so a view column can render it
            // as multi-line markdown. Mentions are resolved to `@username`;
            // the `label` below stays collapsed for tree / single-line /
            // search use. This is the unflattened source a `markdown: true`
            // column reads via `column_value`.
            MetadataField {
                key: "content".into(),
                value: display_body.clone(),
                display_label: "Body".into(),
                editable: false,
                allowed_values: None,
            },
        ];
        if view.edited {
            fields.push(MetadataField {
                key: "edited".into(),
                value: "yes".into(),
                display_label: "Edited".into(),
                editable: false,
                allowed_values: None,
            });
        }
        // The list column shows `content`; collapse newlines so a
        // multi-line message stays a single table row (the full body is
        // available via preview / `content()`). Built from the display
        // body so the row shows `@username`, not the raw `<@ID>`.
        let label = display_body.replace('\n', " ");
        Self {
            client,
            channel_id: view.channel_id,
            message_id: view.id,
            composite_id,
            label,
            content_body: view.content,
            users,
            metadata: Metadata { fields },
        }
    }
}

#[async_trait]
impl Node for StoatMessageNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn node_type(&self) -> &NodeType {
        message_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }

    fn actions(&self) -> Vec<NodeAction> {
        message_actions()
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            // Edit opens the body with mentions as `@uu-…` slugs plus the
            // CACHE section. No header is stripped — chat messages are
            // Markdown and may legitimately begin with `#`.
            "edit_message" => {
                let table = mentions::user_table(&self.users);
                let mut template = mentions::render_slugs(&self.content_body, &table);
                template.push_str(&mentions::cache_section(&table));
                Ok(EditorPrep {
                    template,
                    // Revolt messages carry no optimistic-concurrency token;
                    // we don't guard against concurrent edits (low-conflict,
                    // and the API offers no etag/if-match).
                    version: String::new(),
                    suffix: ".md".into(),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "prepare: unknown action {other}"
            ))),
        }
    }

    async fn picker_options(&self, action_id: &str) -> Result<Vec<not_yet_done_content::ActionOption>> {
        match action_id {
            "react" => Ok(REACTION_EMOJI
                .iter()
                .map(|(label, value)| not_yet_done_content::ActionOption {
                    label: (*label).to_string(),
                    value: (*value).to_string(),
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("edit_message", ActionInput::Edited { text, .. }) => {
                // Drop the CACHE section, then translate `@uu-slug`
                // mentions back to the wire `<@ID>` form before saving.
                let body = mentions::strip_cache_section(&text);
                let table = mentions::user_table(&self.users);
                let new_content = mentions::parse_slugs(body.trim_end(), &table)
                    .map_err(|slug| other_err(format!("unknown mention slug @{slug}")))?;
                let new_content = new_content.trim_end().to_string();
                if new_content == self.content_body.trim_end() {
                    return Ok(ActionOutcome::NoChanges);
                }
                if new_content.is_empty() {
                    return Err(ContentError::Other(
                        "refusing to save an empty message (use delete instead)".into(),
                    ));
                }
                self.client
                    .edit_message(&self.channel_id, &self.message_id, &new_content)
                    .await
                    .map_err(other_err)?;
                self.content_body = new_content;
                Ok(ActionOutcome::Done {
                    message: Some("Message edited".into()),
                })
            }
            ("delete_message", ActionInput::None) => {
                self.client
                    .delete_message(&self.channel_id, &self.message_id)
                    .await
                    .map_err(other_err)?;
                Ok(ActionOutcome::Done {
                    message: Some("Message deleted".into()),
                })
            }
            ("react", ActionInput::Picked(emoji)) => {
                self.client
                    .add_reaction(&self.channel_id, &self.message_id, &emoji)
                    .await
                    .map_err(other_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Reacted {emoji}")),
                })
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unsupported action/input for {other}"
            ))),
        }
    }
}

#[async_trait]
impl Content for StoatMessageNode {
    fn node_type(&self) -> &NodeType {
        message_type()
    }

    fn version(&self) -> Option<&str> {
        None
    }

    async fn read(&self) -> Result<Vec<u8>> {
        // The preview/content pane is human-facing, so resolve mentions to
        // `@username` here too (the editor takes the slugged form via
        // `prepare`, not this path).
        Ok(mentions::render_display(&self.content_body, &self.users).into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> Arc<StoatClient> {
        // No HTTP is performed in these tests — we only inspect the
        // synchronous surface (actions, prepare, picker_options).
        StoatClient::from_session(
            "https://chat.example.invalid",
            crate::client::StoatSession {
                token: "synthetic".into(),
                user_id: "U0".into(),
                session_id: "S0".into(),
                session_name: "test".into(),
            },
        )
        .expect("client")
    }

    /// Empty mention map — for views without `<@…>` codes, display is
    /// identity so the existing assertions hold unchanged.
    fn no_users() -> Arc<HashMap<String, String>> {
        Arc::new(HashMap::new())
    }

    fn sample_view() -> MessageView {
        MessageView {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            channel_id: "C1".into(),
            content: "line one\nline two".into(),
            author_id: "U1".into(),
            author_name: "alice".into(),
            edited: true,
            timestamp_ms: Some(1469918176385),
        }
    }

    #[test]
    fn composite_id_roundtrips() {
        let id = composite_id("C1", "M1");
        assert_eq!(id, "C1/msg/M1");
        assert_eq!(split_composite(&id), Some(("C1", "M1")));
        assert_eq!(split_composite("no-marker"), None);
    }

    #[test]
    fn node_flattens_label_and_keeps_body() {
        let node = StoatMessageNode::new(test_client(), sample_view(), no_users());
        assert_eq!(node.id(), "C1/msg/01ARZ3NDEKTSV4RRFFQ69G5FAV");
        // Newlines collapsed for the table row …
        assert_eq!(node.label(), "line one line two");
        // … but the body keeps them for preview.
        assert_eq!(node.content_body, "line one\nline two");
    }

    #[test]
    fn metadata_carries_author_time_and_edited() {
        let node = StoatMessageNode::new(test_client(), sample_view(), no_users());
        let m = node.metadata();
        assert_eq!(m.fields[0].key, "author");
        assert_eq!(m.fields[0].value, "alice");
        assert_eq!(m.fields[1].key, "time");
        assert_eq!(m.fields[1].value, "2016-07-30 22:36");
        assert!(m.fields.iter().any(|f| f.key == "edited"));
    }

    #[test]
    fn metadata_content_field_keeps_raw_newlines() {
        // The `content` field is the unflattened source a `markdown: true`
        // column reads — it must keep newlines that `label` collapses.
        let node = StoatMessageNode::new(test_client(), sample_view(), no_users());
        let content = node
            .metadata()
            .fields
            .iter()
            .find(|f| f.key == "content")
            .expect("content metadata field");
        assert_eq!(content.value, "line one\nline two");
        assert!(!content.editable);
    }

    #[tokio::test]
    async fn content_reads_full_body() {
        let node = StoatMessageNode::new(test_client(), sample_view(), no_users());
        let body = node.content().unwrap().read().await.unwrap();
        assert_eq!(String::from_utf8(body).unwrap(), "line one\nline two");
    }

    #[test]
    fn declares_edit_delete_react_actions() {
        let node = StoatMessageNode::new(test_client(), sample_view(), no_users());
        let actions = node.actions();
        let ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["edit_message", "delete_message", "react"]);
        assert!(matches!(actions[0].input, InputSpec::Editor));
        assert!(matches!(actions[1].input, InputSpec::None));
        assert!(matches!(actions[2].input, InputSpec::Picker));
    }

    #[tokio::test]
    async fn prepare_edit_returns_raw_body_unwrapped() {
        // No header is added — Markdown messages may start with `#`, which
        // a header-strip would eat. The template is the verbatim body.
        let node = StoatMessageNode::new(test_client(), sample_view(), no_users());
        let prep = node.prepare("edit_message").await.unwrap();
        assert_eq!(prep.template, "line one\nline two");
        assert_eq!(prep.suffix, ".md");
        assert!(prep.version.is_empty());
    }

    #[tokio::test]
    async fn unchanged_edit_is_a_noop() {
        let mut node = StoatMessageNode::new(test_client(), sample_view(), no_users());
        let outcome = node
            .execute(
                "edit_message",
                ActionInput::Edited {
                    text: "line one\nline two".into(),
                    original: "line one\nline two".into(),
                    version: String::new(),
                },
            )
            .await
            .unwrap();
        // Same content → no network call, no change.
        assert!(matches!(outcome, ActionOutcome::NoChanges));
    }

    #[tokio::test]
    async fn react_offers_emoji_options() {
        let node = StoatMessageNode::new(test_client(), sample_view(), no_users());
        let opts = node.picker_options("react").await.unwrap();
        assert_eq!(opts.len(), REACTION_EMOJI.len());
        assert_eq!(opts[0].value, "👍");
        // A non-react action surfaces no options.
        assert!(node.picker_options("edit_message").await.unwrap().is_empty());
    }

    fn users_with_alice() -> Arc<HashMap<String, String>> {
        let mut m = HashMap::new();
        m.insert("01AAA".to_string(), "alice".to_string());
        Arc::new(m)
    }

    fn mention_view() -> MessageView {
        MessageView {
            id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            channel_id: "C1".into(),
            content: "hi <@01AAA>".into(),
            author_id: "U1".into(),
            author_name: "bob".into(),
            edited: false,
            timestamp_ms: Some(1469918176385),
        }
    }

    #[test]
    fn display_resolves_mention_in_label_and_content() {
        let node = StoatMessageNode::new(test_client(), mention_view(), users_with_alice());
        // Label and the markdown `content` field both show `@alice`.
        assert_eq!(node.label(), "hi @alice");
        let content = node
            .metadata()
            .fields
            .iter()
            .find(|f| f.key == "content")
            .unwrap();
        assert_eq!(content.value, "hi @alice");
    }

    #[tokio::test]
    async fn prepare_edit_renders_mention_as_slug_with_cache() {
        let node = StoatMessageNode::new(test_client(), mention_view(), users_with_alice());
        let prep = node.prepare("edit_message").await.unwrap();
        // The wire `<@ID>` becomes a `@uu-…` slug in the buffer …
        assert!(prep.template.starts_with("hi @uu-alice"));
        // … and the CACHE section advertises it.
        assert!(prep.template.contains("@uu-alice"));
        assert!(prep.template.contains(mentions::CACHE_MARKER));
    }

    #[tokio::test]
    async fn unchanged_edit_roundtrips_slug_back_to_code() {
        // Editing with the slug form (and the CACHE section appended) must
        // resolve back to the original `<@ID>` body → a no-op.
        let mut node = StoatMessageNode::new(test_client(), mention_view(), users_with_alice());
        let prep = node.prepare("edit_message").await.unwrap();
        let outcome = node
            .execute(
                "edit_message",
                ActionInput::Edited {
                    text: prep.template.clone(),
                    original: prep.template,
                    version: String::new(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ActionOutcome::NoChanges));
    }

    #[tokio::test]
    async fn edit_rejects_unknown_mention_slug() {
        let mut node = StoatMessageNode::new(test_client(), mention_view(), users_with_alice());
        let result = node
            .execute(
                "edit_message",
                ActionInput::Edited {
                    text: "ping @uu-ghost".into(),
                    original: "hi <@01AAA>".into(),
                    version: String::new(),
                },
            )
            .await;
        match result {
            Err(e) => assert!(format!("{e}").contains("uu-ghost")),
            Ok(_) => panic!("expected an error for the unknown slug"),
        }
    }
}
