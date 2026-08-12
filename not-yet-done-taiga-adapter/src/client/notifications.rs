//! Web-notifications endpoints.
//!
//! Listing: `GET /api/v1/web-notifications?page=N`. Wrapper shape is
//! `{ objects: [...], total: N }`. Default page size is around 30 — the
//! adapter walks pages serially up to [`MAX_PAGES`] to keep latency
//! bounded.
//!
//! Mark-as-read: `PATCH /api/v1/web-notifications/<id>/set-as-read`
//! with an empty JSON object as body. The hyphenated path is
//! mandatory (`set_as_read` returns 404), and POST returns 500 (the
//! handler is wired only to PATCH; what the Taiga web UI also sends).

use not_yet_done_content::http_log;
use serde::Deserialize;
use serde_json::json;

use super::TaigaClient;
use super::query::ItemType;

const MAX_PAGES: u32 = 20;

/// One notification, normalized for the adapter layer.
#[derive(Debug, Clone)]
pub struct TaigaNotification {
    pub id: u64,
    pub event: NotificationEvent,
    pub created: String,
    /// `None` = unread; `Some` = ISO-8601 timestamp at which the user
    /// marked it read.
    pub read_at: Option<String>,
    /// Linked Taiga item — the userstory/issue/task/epic the event is about.
    pub obj: NotificationTarget,
    /// Originator (the person whose action triggered the notification).
    pub actor_name: String,
    pub actor_username: String,
    pub project_name: String,
    pub project_slug: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationEvent {
    /// `assigned you to`
    Assigned,
    /// `has commented on`
    Commented,
    /// `mentioned you in a comment on`
    Mentioned,
    /// Numeric event types Taiga returns that we don't have a specific
    /// label for yet. Surfaced as `event(<n>)` in the UI.
    Other(u32),
}

impl NotificationEvent {
    pub fn from_code(code: u32) -> Self {
        match code {
            1 => Self::Assigned,
            5 => Self::Commented,
            6 => Self::Mentioned,
            other => Self::Other(other),
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Assigned => "assigned".to_string(),
            Self::Commented => "commented".to_string(),
            Self::Mentioned => "mentioned".to_string(),
            Self::Other(n) => format!("event({n})"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NotificationTarget {
    /// Taiga's internal numeric id (used for API calls).
    pub id: u64,
    /// Public reference number shown in the UI as `#42`.
    pub reference: u64,
    pub subject: String,
    /// Item kind. `None` if Taiga returned a content_type string we don't
    /// recognize (still rendered, but cross-link to ticket editor will be
    /// disabled for this row).
    pub item_type: Option<ItemType>,
    /// Raw content_type string from Taiga (e.g. "userstory", "issue").
    /// Kept for display when `item_type` is None.
    pub raw_content_type: String,
}

// ── HTTP-side DTOs ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct ListResponse {
    objects: Vec<RawNotification>,
    #[serde(default)]
    total: u64,
}

#[derive(Deserialize)]
struct RawNotification {
    id: u64,
    event_type: u32,
    created: String,
    #[serde(default)]
    read: Option<String>,
    data: RawData,
}

#[derive(Deserialize)]
struct RawData {
    /// Optional because Taiga occasionally emits project-scoped events
    /// (bulk operations, membership changes) that have no concrete linked
    /// item. Such rows are filtered out in [`map_one`] — our list view is
    /// item-centric and `target_id` cross-link relies on this field.
    #[serde(default)]
    obj: Option<RawObj>,
    #[serde(default)]
    user: Option<RawUser>,
    #[serde(default)]
    project: Option<RawProject>,
}

#[derive(Deserialize)]
struct RawObj {
    id: u64,
    #[serde(rename = "ref")]
    reference: u64,
    subject: String,
    content_type: String,
}

#[derive(Deserialize)]
struct RawUser {
    #[serde(default)]
    name: String,
    #[serde(default)]
    username: String,
}

#[derive(Deserialize)]
struct RawProject {
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: String,
}

// ── Client API ───────────────────────────────────────────────────────

/// One page's worth of notifications plus the metadata the caller needs
/// to render pagination footers or decide whether to keep walking.
/// `raw_count` is the number of rows the server returned (before our
/// `obj`-filter), `total` is the total row count across all pages (from
/// the response wrapper).
pub struct NotificationPage {
    pub items: Vec<TaigaNotification>,
    pub raw_count: u64,
    pub total: u64,
}

/// Fetch one page. Single HTTP round-trip. `page_size` is sent as a
/// query parameter so the server slices the result the way the caller
/// expects (Taiga's default of 30 applies if `page_size` is `None`).
pub async fn fetch_notifications_page(
    client: &TaigaClient,
    page: u32,
    page_size: Option<u32>,
) -> Result<NotificationPage, String> {
    let url = match page_size {
        Some(sz) => format!(
            "{}/api/v1/web-notifications?page={page}&page_size={sz}",
            client.base_url
        ),
        None => format!("{}/api/v1/web-notifications?page={page}", client.base_url),
    };
    let headers = client.auth_headers()?;
    http_log::log_request("GET", &url);
    let resp = client
        .send_retrying("GET", &url, || {
            client.http.get(&url).headers(headers.clone())
        })
        .await?;
    let resp = http_log::check_status("GET", &url, resp).await?;
    let body = resp
        .text()
        .await
        .map_err(|e| format!("web-notifications body: {e}"))?;
    let parsed: ListResponse = serde_json::from_str(&body).map_err(|e| {
        let snippet = body
            .lines()
            .nth(e.line().saturating_sub(1))
            .map(|l| {
                let col = e.column().saturating_sub(1);
                let start = col.saturating_sub(40);
                let end = (col + 40).min(l.len());
                format!("…{}…", &l[start..end])
            })
            .unwrap_or_default();
        format!(
            "web-notifications parse at line {}, col {}: {} | near: {}",
            e.line(),
            e.column(),
            e,
            snippet
        )
    })?;
    let raw_count = parsed.objects.len() as u64;
    let mapped: Vec<TaigaNotification> = parsed.objects.into_iter().filter_map(map_one).collect();
    Ok(NotificationPage {
        items: mapped,
        raw_count,
        total: parsed.total,
    })
}

fn map_one(raw: RawNotification) -> Option<TaigaNotification> {
    // No `obj` → project-scoped event (bulk action, membership change).
    // We can't render or cross-link it in an item-centric list, so drop it.
    let obj = raw.data.obj?;
    let item_type = ItemType::parse(&obj.content_type).ok();
    let actor = raw.data.user.unwrap_or(RawUser {
        name: String::new(),
        username: String::new(),
    });
    let project = raw.data.project.unwrap_or(RawProject {
        name: String::new(),
        slug: String::new(),
    });
    Some(TaigaNotification {
        id: raw.id,
        event: NotificationEvent::from_code(raw.event_type),
        created: raw.created,
        read_at: raw.read,
        obj: NotificationTarget {
            id: obj.id,
            reference: obj.reference,
            subject: obj.subject,
            item_type,
            raw_content_type: obj.content_type,
        },
        actor_name: actor.name,
        actor_username: actor.username,
        project_name: project.name,
        project_slug: project.slug,
    })
}

/// Fetch all notification pages serially, capped at [`MAX_PAGES`]. Order
/// matches Taiga's response order (which is roughly newest-first; the
/// adapter applies the actual configured sort afterwards).
///
/// Termination is driven by the wrapper's `total` field: walk until the
/// raw row count fetched matches `total`. Asking for a page beyond the
/// last one returns 404 with a confusing error ("Page is not 'last'…"),
/// which we don't want to bubble up — hence the explicit `total` check
/// instead of an empty-page heuristic.
pub async fn fetch_all_web_notifications(
    client: &TaigaClient,
) -> Result<Vec<TaigaNotification>, String> {
    let mut out = Vec::new();
    let mut raw_seen: u64 = 0;
    for page in 1..=MAX_PAGES {
        let chunk = fetch_notifications_page(client, page, None).await?;
        raw_seen += chunk.raw_count;
        out.extend(chunk.items);
        // Stop once we've seen all rows, or when the page came back empty
        // (defensive: shouldn't happen if total is reported correctly).
        if chunk.raw_count == 0 || raw_seen >= chunk.total {
            break;
        }
    }
    Ok(out)
}

/// Mark one notification as read. Idempotent — Taiga returns 200 even if
/// the notification was already marked read.
pub async fn mark_notification_as_read(client: &TaigaClient, id: u64) -> Result<(), String> {
    let url = format!(
        "{}/api/v1/web-notifications/{id}/set-as-read",
        client.base_url,
    );
    let headers = client.auth_headers()?;
    let payload = json!({});
    http_log::log_request("PATCH", &url);
    let resp = client
        .send_retrying("PATCH", &url, || {
            client
                .http
                .patch(&url)
                .headers(headers.clone())
                .json(&payload)
        })
        .await?;
    http_log::check_status("PATCH", &url, resp).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_code_mapping() {
        assert_eq!(NotificationEvent::from_code(1), NotificationEvent::Assigned);
        assert_eq!(
            NotificationEvent::from_code(5),
            NotificationEvent::Commented
        );
        assert_eq!(
            NotificationEvent::from_code(6),
            NotificationEvent::Mentioned
        );
        match NotificationEvent::from_code(99) {
            NotificationEvent::Other(99) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn event_labels() {
        assert_eq!(NotificationEvent::Assigned.label(), "assigned");
        assert_eq!(NotificationEvent::Commented.label(), "commented");
        assert_eq!(NotificationEvent::Mentioned.label(), "mentioned");
        assert_eq!(NotificationEvent::from_code(42).label(), "event(42)");
    }

    #[test]
    fn map_one_synthetic() {
        // Fully invented payload — exercises mapping without touching real
        // Taiga data.
        let raw: RawNotification = serde_json::from_value(serde_json::json!({
            "id": 7,
            "event_type": 5,
            "created": "2026-01-01T00:00:00+0000",
            "read": null,
            "data": {
                "obj": { "id": 11, "ref": 3, "subject": "Demo ticket", "content_type": "userstory" },
                "user": { "name": "Pat Example", "username": "pat" },
                "project": { "name": "Sample", "slug": "sample" }
            }
        })).unwrap();
        let n = map_one(raw).expect("notification with obj should map");
        assert_eq!(n.id, 7);
        assert_eq!(n.event, NotificationEvent::Commented);
        assert!(n.read_at.is_none());
        assert_eq!(n.obj.reference, 3);
        assert_eq!(n.obj.item_type, Some(ItemType::UserStory));
        assert_eq!(n.actor_username, "pat");
        assert_eq!(n.project_slug, "sample");
    }

    #[test]
    fn map_one_drops_notification_without_obj() {
        // Project-scoped events (bulk ops, membership changes) come without
        // a `data.obj` field — they should be silently dropped instead of
        // crashing the whole list parse.
        let raw: RawNotification = serde_json::from_value(serde_json::json!({
            "id": 99,
            "event_type": 5,
            "created": "2026-01-01T00:00:00+0000",
            "read": null,
            "data": {
                "user": { "name": "Pat Example", "username": "pat" },
                "project": { "name": "Sample", "slug": "sample" }
            }
        }))
        .unwrap();
        assert!(map_one(raw).is_none());
    }

    #[test]
    fn map_one_unknown_content_type() {
        let raw: RawNotification = serde_json::from_value(serde_json::json!({
            "id": 1,
            "event_type": 1,
            "created": "2026-01-01T00:00:00+0000",
            "read": "2026-01-02T00:00:00+0000",
            "data": {
                "obj": { "id": 2, "ref": 9, "subject": "x", "content_type": "wiki_page" },
                "user": null,
                "project": null
            }
        }))
        .unwrap();
        let n = map_one(raw).expect("notification with obj should map");
        assert!(n.obj.item_type.is_none());
        assert_eq!(n.obj.raw_content_type, "wiki_page");
        assert_eq!(n.read_at.as_deref(), Some("2026-01-02T00:00:00+0000"));
        assert_eq!(n.actor_name, "");
        assert_eq!(n.project_name, "");
    }
}
