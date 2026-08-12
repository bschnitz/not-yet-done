//! Notification node + helpers — read-only summary of a Taiga
//! web-notification, used as a list-only row in the Notifications subtab.
//!
//! Drill-into is intentionally degenerate: the YAML config is expected to
//! cross-link via `node_id_from: target_id` (handled in `ContentView`)
//! so that opening a notification jumps straight to the underlying
//! ticket. The `Node` impl is provided for completeness when the
//! adapter contract requires `get_by_id`.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::{
    ActionInput, ActionOutcome, ColumnSchema, ContentError, InputSpec, Metadata, MetadataField,
    Node, NodeAction, NodeSummary, NodeType, Result, SortDirection, SortKey,
};

use super::types::notification_type;
use crate::client::{TaigaClient, TaigaNotification, mark_notification_as_read};

pub(super) struct TaigaNotificationNode {
    client: Arc<TaigaClient>,
    composite_id: String,
    label: String,
    notification_id: u64,
    read: bool,
    metadata: Metadata,
}

impl TaigaNotificationNode {
    pub(super) fn new(client: Arc<TaigaClient>, n: TaigaNotification) -> Self {
        Self {
            client,
            composite_id: format!("notification:{}", n.id),
            label: n.obj.subject.clone(),
            notification_id: n.id,
            read: n.read_at.is_some(),
            metadata: build_metadata(&n),
        }
    }
}

#[async_trait]
impl Node for TaigaNotificationNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn node_type(&self) -> &NodeType {
        notification_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    async fn execute(&mut self, action_id: &str, _input: ActionInput) -> Result<ActionOutcome> {
        match action_id {
            "mark_as_read" => {
                mark_notification_as_read(&self.client, self.notification_id)
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                self.read = true;
                Ok(ActionOutcome::Done {
                    message: Some(format!(
                        "Notification #{} marked as read",
                        self.notification_id
                    )),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "notification node has no action `{other}`"
            ))),
        }
    }
}

pub(super) fn parse_notification_id(s: &str) -> Option<u64> {
    s.strip_prefix("notification:").and_then(|n| n.parse().ok())
}

pub(super) fn notification_actions() -> Vec<NodeAction> {
    vec![NodeAction::new(
        "mark_as_read",
        "Mark as read",
        InputSpec::None,
    )]
}

pub(super) fn notification_to_summary(n: &TaigaNotification) -> NodeSummary {
    NodeSummary {
        id: format!("notification:{}", n.id),
        label: n.obj.subject.clone(),
        node_type: notification_type().clone(),
        metadata: build_metadata(n),
        has_children: None,
    }
}

/// The notification list's sortable columns. Every one is carried in the
/// rows built by `build_metadata`; the ordering itself runs through the
/// module's own [`apply_sort`], which compares the typed notification rather
/// than the rendered cell.
pub(super) fn columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("created", "Created").typed("datetime"),
        ColumnSchema::new("event", "Event"),
        ColumnSchema::new("project", "Project"),
        ColumnSchema::new("actor", "Actor"),
        ColumnSchema::new("read", "Read"),
        ColumnSchema::new("subject", "Subject"),
    ]
}

pub(super) fn apply_sort(items: &mut [TaigaNotification], sort: &[SortKey]) -> Vec<SortKey> {
    let effective: Vec<SortKey> = if sort.is_empty() {
        vec![
            SortKey {
                column: "read".into(),
                direction: SortDirection::Asc,
            },
            SortKey {
                column: "created".into(),
                direction: SortDirection::Desc,
            },
        ]
    } else {
        sort.to_vec()
    };
    items.sort_by(|a, b| {
        for k in &effective {
            let ord = sort_value(a, &k.column).cmp(&sort_value(b, &k.column));
            if ord != std::cmp::Ordering::Equal {
                return match k.direction {
                    SortDirection::Asc => ord,
                    SortDirection::Desc => ord.reverse(),
                };
            }
        }
        std::cmp::Ordering::Equal
    });
    effective
}

fn sort_value(n: &TaigaNotification, col: &str) -> String {
    match col {
        "created" => n.created.clone(),
        "event" => n.event.label(),
        "project" => n.project_name.clone(),
        "actor" => n.actor_name.clone(),
        "subject" => n.obj.subject.clone(),
        // Unread sorts before read in Asc; "a" < "z".
        "read" => match n.read_at {
            Some(_) => "z_read".into(),
            None => "a_unread".into(),
        },
        _ => String::new(),
    }
}

fn build_metadata(n: &TaigaNotification) -> Metadata {
    let display_ref = if n.project_slug.is_empty() {
        format!("#{}", n.obj.reference)
    } else {
        format!("{}#{}", n.project_slug, n.obj.reference)
    };
    let target_id = n
        .obj
        .item_type
        .map(|it| format!("{}:{}", it.as_str(), n.obj.id))
        .unwrap_or_default();
    let read_label = if n.read_at.is_some() {
        "read"
    } else {
        "unread"
    };
    Metadata {
        fields: vec![
            simple("event", &n.event.label(), "Event"),
            simple("target_ref", &display_ref, "Ref"),
            simple("subject", &n.obj.subject, "Subject"),
            simple("project", &n.project_name, "Project"),
            simple("actor", &n.actor_name, "Actor"),
            simple("created", &n.created, "Created"),
            simple("read", read_label, "Read"),
            simple("target_id", &target_id, "Target ID"),
        ],
    }
}

fn simple(key: &str, value: &str, label: &str) -> MetadataField {
    MetadataField {
        key: key.into(),
        value: value.into(),
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ItemType, NotificationEvent, NotificationTarget};

    fn synthetic(id: u64, created: &str, read: bool, event: u32) -> TaigaNotification {
        TaigaNotification {
            id,
            event: NotificationEvent::from_code(event),
            created: created.into(),
            read_at: if read {
                Some("2026-01-02T00:00:00+0000".into())
            } else {
                None
            },
            obj: NotificationTarget {
                id: 100 + id,
                reference: id,
                subject: format!("subject-{id}"),
                item_type: Some(ItemType::UserStory),
                raw_content_type: "userstory".into(),
            },
            actor_name: format!("actor-{id}"),
            actor_username: format!("user-{id}"),
            project_name: "Sample".into(),
            project_slug: "sample".into(),
        }
    }

    #[test]
    fn summary_carries_target_id_for_cross_link() {
        let n = synthetic(7, "2026-01-01T00:00:00+0000", false, 5);
        let s = notification_to_summary(&n);
        assert_eq!(s.id, "notification:7");
        let target_id = s
            .metadata
            .fields
            .iter()
            .find(|f| f.key == "target_id")
            .unwrap();
        assert_eq!(target_id.value, "userstory:107");
    }

    #[test]
    fn read_field_reflects_read_at() {
        let unread = synthetic(1, "2026-01-01T00:00:00+0000", false, 1);
        let read = synthetic(2, "2026-01-02T00:00:00+0000", true, 1);
        let su = notification_to_summary(&unread);
        let sr = notification_to_summary(&read);
        let read_of = |s: &NodeSummary| {
            s.metadata
                .fields
                .iter()
                .find(|f| f.key == "read")
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(read_of(&su), "unread");
        assert_eq!(read_of(&sr), "read");
    }

    #[test]
    fn default_sort_is_unread_first_then_created_desc() {
        // Mix of read + unread, intentionally jumbled by created date so the
        // assertions pin the two-key ordering: unread block sorts before read
        // block, and within each block newer items come first.
        let mut items = vec![
            synthetic(1, "2026-02-01T00:00:00+0000", true, 1), // read, middle date
            synthetic(2, "2026-01-01T00:00:00+0000", false, 1), // unread, oldest
            synthetic(3, "2026-03-01T00:00:00+0000", true, 1), // read, newest
            synthetic(4, "2026-02-15T00:00:00+0000", false, 1), // unread, newer
        ];
        let applied = apply_sort(&mut items, &[]);
        // Unread first, newest unread before older unread.
        assert_eq!(items[0].id, 4);
        assert_eq!(items[1].id, 2);
        // Then read, newest read before older read.
        assert_eq!(items[2].id, 3);
        assert_eq!(items[3].id, 1);
        assert_eq!(applied.len(), 2);
        assert_eq!(applied[0].column, "read");
        assert!(matches!(applied[0].direction, SortDirection::Asc));
        assert_eq!(applied[1].column, "created");
        assert!(matches!(applied[1].direction, SortDirection::Desc));
    }

    #[test]
    fn sort_by_read_puts_unread_first_asc() {
        let mut items = vec![
            synthetic(1, "2026-01-01T00:00:00+0000", true, 1),
            synthetic(2, "2026-01-02T00:00:00+0000", false, 1),
            synthetic(3, "2026-01-03T00:00:00+0000", true, 1),
        ];
        let _ = apply_sort(
            &mut items,
            &[SortKey {
                column: "read".into(),
                direction: SortDirection::Asc,
            }],
        );
        assert_eq!(items[0].id, 2);
    }

    #[test]
    fn parse_id_round_trip() {
        assert_eq!(parse_notification_id("notification:42"), Some(42));
        assert_eq!(parse_notification_id("notification:abc"), None);
        assert_eq!(parse_notification_id("userstory:42"), None);
    }

    #[test]
    fn notification_actions_are_static_per_type() {
        // Both read and unread notifications expose `mark_as_read` so
        // the TUI's per-node_type action cache can serve the whole
        // pane after one fetch. The action is idempotent server-side.
        let actions = notification_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "mark_as_read");
        assert!(matches!(actions[0].input, InputSpec::None));
    }
}
