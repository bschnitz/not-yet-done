//! Thin Microsoft Graph REST client: resolve a bearer token, page through
//! `/me/calendarView`, map events to [`CalEvent`].

use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};
use reqwest::StatusCode;
use serde::Deserialize;

use not_yet_done_calendar_core::{
    CalEvent, CalendarError, CalendarRef, EventDraft, ShowAs, TimeRange,
};
use not_yet_done_content::auth::CredentialResolver;

use crate::config::{DEFAULT_REQUEST_TIMEOUT_SECS, MsGraphConfig};

const DEFAULT_BASE_URL: &str = "https://graph.microsoft.com";

/// Fields we ask Graph for — keeps the payload lean and the parse total.
const SELECT: &str = "id,subject,start,end,isAllDay,showAs,bodyPreview,webLink,location,organizer";

pub(crate) struct MsGraphClient {
    http: reqwest::Client,
    base_url: String,
    resolver: Box<dyn CredentialResolver>,
}

impl MsGraphClient {
    pub(crate) fn from_config(cfg: MsGraphConfig) -> Result<Self, CalendarError> {
        let request_secs = cfg
            .request_timeout_secs
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
        let base_url = cfg
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let resolver = cfg
            .token
            .build_resolver()
            .map_err(|e| CalendarError::Config(format!("token provider: {e}")))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(request_secs))
            .build()
            .map_err(|e| CalendarError::Other(format!("build HTTP client: {e}")))?;
        Ok(Self {
            http,
            base_url,
            resolver,
        })
    }

    /// All events overlapping `range` in the mailbox's default calendar.
    /// Pages through `@odata.nextLink` until exhausted.
    pub(crate) async fn calendar_view(
        &self,
        range: &TimeRange,
        label: &str,
    ) -> Result<Vec<CalEvent>, CalendarError> {
        let start = range.start.format("%Y-%m-%dT%H:%M:%SZ");
        let end = range.end.format("%Y-%m-%dT%H:%M:%SZ");
        let mut url = format!(
            "{base}/v1.0/me/calendarView?startDateTime={start}&endDateTime={end}\
             &$select={SELECT}&$top=100&$orderby=start/dateTime",
            base = self.base_url,
        );

        let mut out = Vec::new();
        loop {
            let body = self.authed_get(&url).await?;
            let page: GraphResponse = serde_json::from_str(&body)
                .map_err(|e| CalendarError::Other(format!("parse calendarView: {e}")))?;
            for ev in page.value {
                out.push(map_event(ev, label)?);
            }
            match page.next_link {
                Some(next) if !next.is_empty() => url = next,
                _ => break,
            }
        }
        Ok(out)
    }

    /// The mailbox's calendars as write targets (`GET /me/calendars`). `canEdit`
    /// carries straight through to [`CalendarRef::writable`].
    pub(crate) async fn list_calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        let url = format!(
            "{}/v1.0/me/calendars?$select=id,name,canEdit",
            self.base_url
        );
        let body = self.authed_get(&url).await?;
        let page: CalendarsResponse = serde_json::from_str(&body)
            .map_err(|e| CalendarError::Other(format!("parse calendars: {e}")))?;
        Ok(page
            .value
            .into_iter()
            .map(|c| CalendarRef {
                id: c.id,
                name: c.name.unwrap_or_default(),
                writable: c.can_edit.unwrap_or(true),
            })
            .collect())
    }

    /// Create `draft` via `POST /me/calendars/{id}/events` (or `/me/events` for
    /// the default calendar when `calendar_id` is `None`). Returns the created
    /// event as Graph echoes it back. A 403 is surfaced as a scope hint, since
    /// the common cause is the app registration lacking `Calendars.ReadWrite`.
    pub(crate) async fn create_event(
        &self,
        calendar_id: Option<&str>,
        draft: &EventDraft,
        label: &str,
    ) -> Result<CalEvent, CalendarError> {
        let url = match calendar_id {
            Some(id) => format!("{}/v1.0/me/calendars/{}/events", self.base_url, id),
            None => format!("{}/v1.0/me/events", self.base_url),
        };
        let payload = build_event_json(draft);
        let body = serde_json::to_string(&payload)
            .map_err(|e| CalendarError::Other(format!("encode event: {e}")))?;

        let created = self.authed_post(&url, &body).await?;
        let ev: GraphEvent = serde_json::from_str(&created)
            .map_err(|e| CalendarError::Other(format!("parse created event: {e}")))?;
        map_event(ev, label)
    }

    /// GET with a freshly-resolved bearer token. A 401 forces the credential
    /// provider to re-resolve (an `az`-minted token expires ~hourly) and the
    /// request is retried once — so a long-lived TUI session keeps working
    /// past token expiry without a reconnect.
    async fn authed_get(&self, url: &str) -> Result<String, CalendarError> {
        let token = self.resolve_token().await?;
        let resp = self.send(url, &token).await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            self.resolver.invalidate().await;
            let token = self.resolve_token().await?;
            let resp = self.send(url, &token).await?;
            return read_body(url, resp).await;
        }
        read_body(url, resp).await
    }

    async fn resolve_token(&self) -> Result<String, CalendarError> {
        self.resolver
            .resolve()
            .await
            .map_err(|e| CalendarError::Auth(e.to_string()))
    }

    async fn send(&self, url: &str, token: &str) -> Result<reqwest::Response, CalendarError> {
        self.http
            .get(url)
            .bearer_auth(token)
            // Ask Graph to return all datetimes in UTC so we can parse them
            // offset-free and anchor to `Utc` directly.
            .header("Prefer", "outlook.timezone=\"UTC\"")
            .send()
            .await
            .map_err(|e| CalendarError::Network(format!("GET {url}: {e}")))
    }

    /// POST a JSON body with a freshly-resolved bearer token, retrying once on a
    /// 401 (same rationale as [`authed_get`](MsGraphClient::authed_get)). A 403
    /// is translated into an actionable `Calendars.ReadWrite` scope hint.
    async fn authed_post(&self, url: &str, body: &str) -> Result<String, CalendarError> {
        let token = self.resolve_token().await?;
        let resp = self.send_post(url, &token, body).await?;
        let resp = if resp.status() == StatusCode::UNAUTHORIZED {
            self.resolver.invalidate().await;
            let token = self.resolve_token().await?;
            self.send_post(url, &token, body).await?
        } else {
            resp
        };
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| CalendarError::Network(format!("read {url}: {e}")))?;
        if status.is_success() {
            return Ok(text);
        }
        let snippet: String = text.chars().take(300).collect();
        if status == StatusCode::FORBIDDEN {
            return Err(CalendarError::Auth(format!(
                "Graph refused the write ({status}). The app registration likely \
                 lacks the delegated `Calendars.ReadWrite` scope — grant/consent it, \
                 then retry. Server said: {snippet}"
            )));
        }
        Err(CalendarError::Network(format!(
            "POST {url} -> {status}: {snippet}"
        )))
    }

    async fn send_post(
        &self,
        url: &str,
        token: &str,
        body: &str,
    ) -> Result<reqwest::Response, CalendarError> {
        self.http
            .post(url)
            .bearer_auth(token)
            .header("Prefer", "outlook.timezone=\"UTC\"")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| CalendarError::Network(format!("POST {url}: {e}")))
    }
}

/// Build the Graph `POST /events` JSON body from an [`EventDraft`]. Times go out
/// in UTC (`timeZone: "UTC"`); an all-day event carries date-midnight bounds
/// with `isAllDay: true` (Graph requires the end to be the exclusive next day).
fn build_event_json(draft: &EventDraft) -> serde_json::Value {
    use serde_json::json;

    let (start_s, end_s) = if draft.all_day {
        let start_date = draft.start.date_naive();
        let mut end_date = draft.end.date_naive();
        if end_date <= start_date {
            end_date = start_date + chrono::Duration::days(1);
        }
        (
            format!("{start_date}T00:00:00"),
            format!("{end_date}T00:00:00"),
        )
    } else {
        (
            draft.start.format("%Y-%m-%dT%H:%M:%S").to_string(),
            draft.end.format("%Y-%m-%dT%H:%M:%S").to_string(),
        )
    };

    let mut obj = json!({
        "subject": draft.title,
        "start": { "dateTime": start_s, "timeZone": "UTC" },
        "end": { "dateTime": end_s, "timeZone": "UTC" },
        "isAllDay": draft.all_day,
        "showAs": show_as_graph(draft.show_as),
    });
    let map = obj.as_object_mut().expect("json object");
    if let Some(loc) = draft
        .location
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        map.insert("location".into(), json!({ "displayName": loc }));
    }
    if let Some(body) = draft
        .body
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        map.insert(
            "body".into(),
            json!({ "contentType": "text", "content": body }),
        );
    }
    obj
}

/// [`ShowAs`] → Graph `showAs` token (the write mirror of [`map_show_as`]).
fn show_as_graph(show_as: ShowAs) -> &'static str {
    match show_as {
        ShowAs::Free => "free",
        ShowAs::Tentative => "tentative",
        ShowAs::Busy => "busy",
        ShowAs::OutOfOffice => "oof",
        ShowAs::WorkingElsewhere => "workingElsewhere",
        ShowAs::Unknown => "busy",
    }
}

async fn read_body(url: &str, resp: reqwest::Response) -> Result<String, CalendarError> {
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| CalendarError::Network(format!("read {url}: {e}")))?;
    if !status.is_success() {
        let snippet: String = text.chars().take(300).collect();
        return Err(CalendarError::Network(format!(
            "GET {url} -> {status}: {snippet}"
        )));
    }
    Ok(text)
}

// --- Graph JSON shapes ---------------------------------------------------

#[derive(Deserialize)]
struct GraphResponse {
    #[serde(default)]
    value: Vec<GraphEvent>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

#[derive(Deserialize)]
struct CalendarsResponse {
    #[serde(default)]
    value: Vec<GraphCalendar>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphCalendar {
    id: String,
    name: Option<String>,
    can_edit: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphEvent {
    id: String,
    subject: Option<String>,
    start: GraphDateTime,
    end: GraphDateTime,
    #[serde(default)]
    is_all_day: bool,
    show_as: Option<String>,
    body_preview: Option<String>,
    web_link: Option<String>,
    location: Option<GraphLocation>,
    organizer: Option<GraphRecipient>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphDateTime {
    date_time: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphLocation {
    display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphRecipient {
    email_address: Option<GraphEmail>,
}

#[derive(Deserialize)]
struct GraphEmail {
    name: Option<String>,
    address: Option<String>,
}

fn map_event(ev: GraphEvent, label: &str) -> Result<CalEvent, CalendarError> {
    Ok(CalEvent {
        start: parse_graph_dt(&ev.start.date_time)?,
        end: parse_graph_dt(&ev.end.date_time)?,
        uid: ev.id,
        calendar: label.to_string(),
        title: ev.subject.unwrap_or_default(),
        all_day: ev.is_all_day,
        location: ev
            .location
            .and_then(|l| l.display_name)
            .filter(|s| !s.trim().is_empty()),
        organizer: ev
            .organizer
            .and_then(|o| o.email_address)
            .and_then(|e| e.name.filter(|s| !s.trim().is_empty()).or(e.address)),
        show_as: map_show_as(ev.show_as.as_deref()),
        body: ev.body_preview.filter(|s| !s.trim().is_empty()),
        url: ev.web_link.filter(|s| !s.trim().is_empty()),
    })
}

/// Graph (with `Prefer: outlook.timezone="UTC"`) returns e.g.
/// `2024-01-15T09:00:00.0000000` — no offset, seven fractional digits.
fn parse_graph_dt(s: &str) -> Result<DateTime<Utc>, CalendarError> {
    let s = s.trim();
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .map(|ndt| ndt.and_utc())
        .map_err(|e| CalendarError::Other(format!("bad datetime '{s}': {e}")))
}

fn map_show_as(raw: Option<&str>) -> ShowAs {
    match raw.unwrap_or("").to_ascii_lowercase().as_str() {
        "free" => ShowAs::Free,
        "tentative" => ShowAs::Tentative,
        "busy" => ShowAs::Busy,
        "oof" => ShowAs::OutOfOffice,
        "workingelsewhere" => ShowAs::WorkingElsewhere,
        _ => ShowAs::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_graph_datetime_with_and_without_fraction() {
        let a = parse_graph_dt("2024-01-15T09:30:00.0000000").unwrap();
        assert_eq!(
            a.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "2024-01-15T09:30:00Z"
        );
        let b = parse_graph_dt("2024-01-15T09:30:00").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn maps_show_as_case_insensitively() {
        assert_eq!(map_show_as(Some("Busy")).as_str(), "busy");
        assert_eq!(map_show_as(Some("oof")).as_str(), "oof");
        assert_eq!(map_show_as(None).as_str(), "unknown");
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn builds_timed_event_json_with_utc_and_optional_fields() {
        let draft = EventDraft {
            title: "Sprint planning".into(),
            start: utc("2024-01-15T09:00:00Z"),
            end: utc("2024-01-15T10:00:00Z"),
            all_day: false,
            location: Some("Room 1".into()),
            body: Some("agenda".into()),
            show_as: ShowAs::Busy,
        };
        let v = build_event_json(&draft);
        assert_eq!(v["subject"], "Sprint planning");
        assert_eq!(v["start"]["dateTime"], "2024-01-15T09:00:00");
        assert_eq!(v["start"]["timeZone"], "UTC");
        assert_eq!(v["isAllDay"], false);
        assert_eq!(v["showAs"], "busy");
        assert_eq!(v["location"]["displayName"], "Room 1");
        assert_eq!(v["body"]["content"], "agenda");
    }

    #[test]
    fn builds_all_day_event_with_next_day_end() {
        let draft = EventDraft {
            title: "Holiday".into(),
            start: utc("2024-03-01T00:00:00Z"),
            end: utc("2024-03-01T00:00:00Z"),
            all_day: true,
            location: None,
            body: None,
            show_as: ShowAs::Free,
        };
        let v = build_event_json(&draft);
        assert_eq!(v["isAllDay"], true);
        assert_eq!(v["start"]["dateTime"], "2024-03-01T00:00:00");
        // End rolls to the exclusive next day.
        assert_eq!(v["end"]["dateTime"], "2024-03-02T00:00:00");
        assert_eq!(v["showAs"], "free");
        assert!(v.get("location").is_none(), "omitted when empty");
    }

    #[test]
    fn maps_event_prefers_organizer_name_then_address() {
        let json = r#"{
            "id": "AAA",
            "subject": "Sprint planning",
            "start": {"dateTime": "2024-01-15T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2024-01-15T10:00:00.0000000", "timeZone": "UTC"},
            "isAllDay": false,
            "showAs": "busy",
            "bodyPreview": "  agenda  ",
            "webLink": "https://outlook.office365.com/x",
            "location": {"displayName": "Room 1"},
            "organizer": {"emailAddress": {"name": "Alice", "address": "alice@example.invalid"}}
        }"#;
        let ev: GraphEvent = serde_json::from_str(json).unwrap();
        let mapped = map_event(ev, "Work").unwrap();
        assert_eq!(mapped.uid, "AAA");
        assert_eq!(mapped.calendar, "Work");
        assert_eq!(mapped.title, "Sprint planning");
        assert_eq!(mapped.organizer.as_deref(), Some("Alice"));
        assert_eq!(mapped.location.as_deref(), Some("Room 1"));
        // bodyPreview passes through verbatim; display-time trimming is the
        // adapter's concern, not the client's.
        assert_eq!(mapped.body.as_deref(), Some("  agenda  "));
        assert!(!mapped.all_day);
    }
}
