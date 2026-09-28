//! CalDAV client: HTTP Basic auth, calendar discovery, and a `calendar-query`
//! `REPORT` that asks the server to expand recurrences into concrete instances.
//!
//! The flow mirrors any CalDAV client (RFC 4791). From the configured `url` —
//! a principal, a calendar-home, or a single collection — we `PROPFIND` our way
//! to the set of calendar collections, then `REPORT` each for the events
//! overlapping the requested window. Discovery is cached after first success:
//! calendars rarely appear or vanish within a session, and the poll loop calls
//! [`CalDavClient::list_events`] repeatedly.

use std::time::Duration;

use chrono::SecondsFormat;
use reqwest::{Method, StatusCode, Url, header};
use tokio::sync::Mutex;

use not_yet_done_calendar_core::{
    CalEvent, CalendarError, CalendarRef, EventDraft, EventPatch, TimeRange,
};
use not_yet_done_content::auth::{CredentialPrompts, CredentialResolver};

use crate::config::{CalDavConfig, DEFAULT_REQUEST_TIMEOUT_SECS};
use crate::ical;

const XML_CT: &str = "application/xml; charset=utf-8";

/// Opt-in diagnostics: set `NYD_DEBUG_CALDAV` (to anything) to trace discovery,
/// per-request HTTP status, and per-calendar event counts to stderr. Off by
/// default and free of any payload — safe to leave in.
fn debug_enabled() -> bool {
    std::env::var_os("NYD_DEBUG_CALDAV").is_some()
}

macro_rules! trace_caldav {
    ($($arg:tt)*) => {
        if debug_enabled() {
            eprintln!("[caldav] {}", format_args!($($arg)*));
        }
    };
}

/// A discovered (or explicitly configured) calendar collection.
#[derive(Debug, Clone)]
struct Calendar {
    url: Url,
    /// Display name, for diagnostics/logging only — the event's "Account"
    /// column is the connection label, set by the caller.
    #[allow(dead_code)]
    name: String,
}

pub(crate) struct CalDavClient {
    http: reqwest::Client,
    base: Url,
    user: Box<dyn CredentialResolver>,
    pass: Box<dyn CredentialResolver>,
    /// Explicit collection URLs from config; when non-empty, discovery is
    /// skipped entirely.
    explicit: Vec<String>,
    /// Discovery cache — `None` until the first successful discovery.
    calendars: Mutex<Option<Vec<Calendar>>>,
}

impl CalDavClient {
    /// `prompts`, when the connection offers one, is what a `script`
    /// credential provider asks the user through — a locked password
    /// store then raises a form in the frontend instead of `gpg`'s own
    /// `pinentry` window.
    pub(crate) fn from_config(
        cfg: &CalDavConfig,
        prompts: Option<&CredentialPrompts>,
    ) -> Result<Self, CalendarError> {
        let base = Url::parse(cfg.url.trim())
            .map_err(|e| CalendarError::Config(format!("invalid url '{}': {e}", cfg.url)))?;
        let secs = cfg
            .request_timeout_secs
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(secs))
            .danger_accept_invalid_certs(cfg.danger_accept_invalid_certs)
            .build()
            .map_err(|e| CalendarError::Other(format!("build HTTP client: {e}")))?;
        let user = cfg
            .username
            .build_resolver_with(prompts)
            .map_err(|e| CalendarError::Config(format!("username provider: {e}")))?;
        let pass = cfg
            .password
            .build_resolver_with(prompts)
            .map_err(|e| CalendarError::Config(format!("password provider: {e}")))?;
        Ok(Self {
            http,
            base,
            user,
            pass,
            explicit: cfg.calendars.clone(),
            calendars: Mutex::new(None),
        })
    }

    /// All events overlapping `range` across every calendar of this connection.
    /// `label` is stamped as each event's `calendar` (the "Account" column).
    pub(crate) async fn list_events(
        &self,
        range: &TimeRange,
        label: &str,
    ) -> Result<Vec<CalEvent>, CalendarError> {
        let calendars = self.calendars().await?;
        trace_caldav!(
            "list_events '{label}' window {} .. {}: {} calendar(s)",
            fmt_ical(range.start),
            fmt_ical(range.end),
            calendars.len()
        );
        let mut out = Vec::new();
        for cal in &calendars {
            let events = self.report_calendar(&cal.url, range, label).await?;
            trace_caldav!("  {} -> {} event(s)", cal.url, events.len());
            out.extend(events);
        }
        trace_caldav!("list_events '{label}': {} event(s) total", out.len());
        Ok(out)
    }

    /// The connection's calendars as write targets: each discovered collection
    /// exposed with its URL as the opaque `id` (what [`create_event`] resolves)
    /// and its display name. CalDAV gives no reliable per-collection writable
    /// flag here, so all are marked writable and any server refusal surfaces on
    /// the actual `PUT`.
    pub(crate) async fn list_calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        let cals = self.calendars().await?;
        Ok(cals
            .into_iter()
            .map(|c| CalendarRef {
                id: c.url.as_str().to_string(),
                name: c.name,
                writable: true,
            })
            .collect())
    }

    /// Create `draft` by `PUT`ting a fresh single-`VEVENT` resource into the
    /// target collection (`calendar_id` = a URL from [`list_calendars`], or the
    /// first discovered calendar when `None`). Returns the event as we wrote it.
    pub(crate) async fn create_event(
        &self,
        calendar_id: Option<&str>,
        draft: &EventDraft,
        label: &str,
    ) -> Result<CalEvent, CalendarError> {
        // Resolve the target collection: an explicit id must be one we know
        // (so a stale/foreign URL can't be written to); otherwise the first.
        let cals = self.calendars().await?;
        let collection = match calendar_id {
            Some(id) => cals
                .iter()
                .find(|c| c.url.as_str() == id)
                .map(|c| c.url.clone())
                .ok_or_else(|| CalendarError::Config(format!("unknown target calendar '{id}'")))?,
            None => cals.first().map(|c| c.url.clone()).ok_or_else(|| {
                CalendarError::Config("no calendar to create the event in".into())
            })?,
        };

        let token = fresh_token();
        let uid = format!("{token}@not-yet-done");
        let ics = ical::to_ics(draft, &uid, chrono::Utc::now());

        // Resource URL = collection + "<token>.ics". Guarantee the collection
        // has a trailing slash first, so `join` appends rather than replacing
        // the last path segment.
        let mut coll_str = collection.as_str().to_string();
        if !coll_str.ends_with('/') {
            coll_str.push('/');
        }
        let base = Url::parse(&coll_str)
            .map_err(|e| CalendarError::Other(format!("bad collection url: {e}")))?;
        let target = base
            .join(&format!("{token}.ics"))
            .map_err(|e| CalendarError::Other(format!("build event url: {e}")))?;

        trace_caldav!("PUT new event {uid} -> {target} ({} bytes)", ics.len());
        self.put_ics(&target, &ics, Precondition::Absent).await?;

        Ok(CalEvent {
            uid,
            calendar: label.to_string(),
            title: draft.title.clone(),
            start: draft.start,
            end: draft.end,
            all_day: draft.all_day,
            location: draft.location.clone(),
            organizer: None,
            show_as: draft.show_as,
            body: draft.body.clone(),
            url: Some(target.as_str().to_string()),
            online_meeting: None,
        })
    }

    /// Delete every resource holding the event whose UID is `uid`. The
    /// resources are found by a `calendar-query` on the UID in each calendar,
    /// since a listed event does not carry the resource it came from; a
    /// recurring event is one resource, so the whole series goes.
    pub(crate) async fn delete_event(&self, uid: &str) -> Result<(), CalendarError> {
        let mut found = Vec::new();
        for cal in &self.calendars().await? {
            found.extend(self.resources_of(&cal.url, uid).await?);
        }
        if found.is_empty() {
            return Err(CalendarError::NotFound(format!(
                "no event with UID '{uid}' on the server"
            )));
        }
        for url in &found {
            trace_caldav!("DELETE event {uid} at {url}");
            self.delete_resource(url).await?;
        }
        Ok(())
    }

    /// Change the event whose UID is `uid` as `patch` says: fetch its
    /// resource, rewrite the properties the patch changes and `PUT` it back
    /// under `If-Match`, so an edit made elsewhere in between is not lost. A
    /// series is refused — its listed events are expanded instances, and
    /// writing one instance's times into the series would move all of them.
    pub(crate) async fn update_event(
        &self,
        uid: &str,
        patch: &EventPatch,
    ) -> Result<(), CalendarError> {
        let mut found = Vec::new();
        for cal in &self.calendars().await? {
            found.extend(self.resources_of(&cal.url, uid).await?);
        }
        let url = match found.as_slice() {
            [] => {
                return Err(CalendarError::NotFound(format!(
                    "no event with UID '{uid}' on the server"
                )));
            }
            [url] => url.clone(),
            _ => {
                return Err(CalendarError::Other(format!(
                    "the event with UID '{uid}' is stored {} times on the server; \
                     not editing any of them",
                    found.len()
                )));
            }
        };
        let (ics, etag) = self.get_resource(&url).await?;
        let ics = ical::apply_patch(&ics, patch, chrono::Utc::now()).map_err(|why| {
            CalendarError::Other(match why {
                ical::PatchRefusal::Recurring => {
                    "a recurring event cannot be edited here: CalDAV stores the series \
                     as one, and the change would move every occurrence"
                        .into()
                }
                ical::PatchRefusal::NoEvent => format!("{url} holds no event"),
            })
        })?;
        trace_caldav!("PUT changed event {uid} -> {url} ({} bytes)", ics.len());
        let precondition = match &etag {
            Some(etag) => Precondition::Matches(etag),
            None => Precondition::None,
        };
        self.put_ics(&url, &ics, precondition).await
    }

    /// `GET` one event resource: its iCalendar text and its `ETag`, if the
    /// server sent one.
    async fn get_resource(&self, url: &Url) -> Result<(String, Option<String>), CalendarError> {
        let resp = self.send_get(url).await?;
        let resp = if resp.status() == StatusCode::UNAUTHORIZED {
            self.user.invalidate().await;
            self.pass.invalidate().await;
            self.send_get(url).await?
        } else {
            resp
        };
        if resp.status() == StatusCode::NOT_FOUND {
            return Err(CalendarError::NotFound(format!("{url} is gone")));
        }
        let etag = resp
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let ics = read_body(url, resp).await?;
        Ok((ics, etag))
    }

    async fn send_get(&self, url: &Url) -> Result<reqwest::Response, CalendarError> {
        let user = self
            .user
            .resolve()
            .await
            .map_err(|e| CalendarError::Auth(e.to_string()))?;
        let pass = self
            .pass
            .resolve()
            .await
            .map_err(|e| CalendarError::Auth(e.to_string()))?;
        self.http
            .get(url.clone())
            .basic_auth(user, Some(pass))
            .send()
            .await
            .map_err(|e| CalendarError::Network(format!("GET {url}: {e}")))
    }

    /// The resources in collection `cal` whose event has the UID `uid`.
    async fn resources_of(&self, cal: &Url, uid: &str) -> Result<Vec<Url>, CalendarError> {
        let uid = xml_escape(uid);
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop><d:getetag/></d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:prop-filter name="UID">
          <c:text-match collation="i;octet">{uid}</c:text-match>
        </c:prop-filter>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#
        );
        let resp = self
            .dav(Method::from_bytes(b"REPORT").unwrap(), cal, "1", &body)
            .await?;
        parse_multistatus(&resp)?
            .into_iter()
            .filter_map(|r| r.href)
            // The collection itself is no event resource.
            .filter(|href| !href.trim().ends_with('/'))
            .map(|href| self.resolve(&href))
            .collect()
    }

    /// `DELETE` one resource, retrying once on a 401 after invalidating the
    /// cached credential. A resource that is already gone counts as deleted.
    async fn delete_resource(&self, url: &Url) -> Result<(), CalendarError> {
        let resp = self.send_delete(url).await?;
        let resp = if resp.status() == StatusCode::UNAUTHORIZED {
            self.user.invalidate().await;
            self.pass.invalidate().await;
            self.send_delete(url).await?
        } else {
            resp
        };
        let status = resp.status();
        trace_caldav!("DELETE {url} -> {status}");
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        let snippet: String = text.chars().take(300).collect();
        let msg = format!("{url} -> {status}: {snippet}");
        Err(
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                CalendarError::Auth(msg)
            } else {
                CalendarError::Network(msg)
            },
        )
    }

    async fn send_delete(&self, url: &Url) -> Result<reqwest::Response, CalendarError> {
        let user = self
            .user
            .resolve()
            .await
            .map_err(|e| CalendarError::Auth(e.to_string()))?;
        let pass = self
            .pass
            .resolve()
            .await
            .map_err(|e| CalendarError::Auth(e.to_string()))?;
        self.http
            .delete(url.clone())
            .basic_auth(user, Some(pass))
            .send()
            .await
            .map_err(|e| CalendarError::Network(format!("DELETE {url}: {e}")))
    }

    /// `PUT` an iCalendar resource under `precondition`, retrying once on a 401
    /// after invalidating the cached credential.
    async fn put_ics(
        &self,
        url: &Url,
        ics: &str,
        precondition: Precondition<'_>,
    ) -> Result<(), CalendarError> {
        let resp = self.send_put(url, ics, precondition).await?;
        let resp = if resp.status() == StatusCode::UNAUTHORIZED {
            self.user.invalidate().await;
            self.pass.invalidate().await;
            self.send_put(url, ics, precondition).await?
        } else {
            resp
        };
        let status = resp.status();
        if status.is_success() {
            trace_caldav!("PUT {url} -> {status}");
            return Ok(());
        }
        if status == StatusCode::PRECONDITION_FAILED {
            return Err(CalendarError::Other(match precondition {
                Precondition::Matches(_) => {
                    "the event was changed on the server in the meantime; \
                     reload and edit it again"
                        .to_string()
                }
                _ => format!("{url} already exists"),
            }));
        }
        let text = resp.text().await.unwrap_or_default();
        let snippet: String = text.chars().take(300).collect();
        let msg = format!("{url} -> {status}: {snippet}");
        Err(
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                CalendarError::Auth(msg)
            } else {
                CalendarError::Network(msg)
            },
        )
    }

    async fn send_put(
        &self,
        url: &Url,
        ics: &str,
        precondition: Precondition<'_>,
    ) -> Result<reqwest::Response, CalendarError> {
        let user = self
            .user
            .resolve()
            .await
            .map_err(|e| CalendarError::Auth(e.to_string()))?;
        let pass = self
            .pass
            .resolve()
            .await
            .map_err(|e| CalendarError::Auth(e.to_string()))?;
        let request = self
            .http
            .put(url.clone())
            .basic_auth(user, Some(pass))
            .header(header::CONTENT_TYPE, "text/calendar; charset=utf-8");
        let request = match precondition {
            Precondition::Absent => request.header(header::IF_NONE_MATCH, "*"),
            Precondition::Matches(etag) => request.header(header::IF_MATCH, etag),
            Precondition::None => request,
        };
        request
            .body(ics.to_string())
            .send()
            .await
            .map_err(|e| CalendarError::Network(format!("PUT {url}: {e}")))
    }

    /// Cached calendar list, discovering on first call (or resolving the
    /// explicit list from config).
    async fn calendars(&self) -> Result<Vec<Calendar>, CalendarError> {
        let mut guard = self.calendars.lock().await;
        if let Some(cals) = guard.as_ref() {
            return Ok(cals.clone());
        }
        let cals = if self.explicit.is_empty() {
            trace_caldav!("discovering calendars from {}", self.base);
            let cals = self.discover().await?;
            for c in &cals {
                trace_caldav!("  discovered calendar: {} ({})", c.url, c.name);
            }
            cals
        } else {
            self.explicit
                .iter()
                .map(|c| {
                    let url = self.resolve(c)?;
                    let name = collection_name(&url);
                    Ok(Calendar { url, name })
                })
                .collect::<Result<Vec<_>, CalendarError>>()?
        };
        if cals.is_empty() {
            return Err(CalendarError::Config(
                "no calendar collections found (check the url / credentials)".into(),
            ));
        }
        *guard = Some(cals.clone());
        Ok(cals)
    }

    /// Walk from the configured `url` to the calendar collections: resolve the
    /// calendar-home, then enumerate its VEVENT-capable children. Short-circuits
    /// if `url` is itself a calendar collection.
    async fn discover(&self) -> Result<Vec<Calendar>, CalendarError> {
        let body = self
            .dav(
                Method::from_bytes(b"PROPFIND").unwrap(),
                &self.base,
                "0",
                PROPFIND_HOME,
            )
            .await?;
        let responses = parse_multistatus(&body)?;

        // If the entry point is itself a calendar collection, use it directly.
        if let Some(r) = responses.first() {
            if r.resourcetypes.iter().any(|t| t == "calendar") {
                return Ok(vec![Calendar {
                    url: self.base.clone(),
                    name: r
                        .displayname
                        .clone()
                        .unwrap_or_else(|| collection_name(&self.base)),
                }]);
            }
        }

        // Otherwise find the calendar-home-set — directly, or via the principal.
        let home = self.resolve_home(&responses).await?;
        let body = self
            .dav(
                Method::from_bytes(b"PROPFIND").unwrap(),
                &home,
                "1",
                PROPFIND_CALENDARS,
            )
            .await?;
        let responses = parse_multistatus(&body)?;

        let mut cals = Vec::new();
        for r in responses {
            if !r.resourcetypes.iter().any(|t| t == "calendar") {
                continue;
            }
            // Keep only calendars that hold events (empty comp-set = accept).
            if !r.comp_names.is_empty()
                && !r
                    .comp_names
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case("VEVENT"))
            {
                continue;
            }
            let Some(href) = r.href else { continue };
            let url = self.resolve(&href)?;
            // Skip the home collection itself echoed back as a response.
            if url == home {
                continue;
            }
            let name = r.displayname.unwrap_or_else(|| collection_name(&url));
            cals.push(Calendar { url, name });
        }
        Ok(cals)
    }

    /// Resolve the calendar-home URL from a Depth-0 PROPFIND response set:
    /// prefer an explicit `calendar-home-set`; else follow
    /// `current-user-principal` and PROPFIND it; else fall back to the base url.
    async fn resolve_home(&self, responses: &[RawResponse]) -> Result<Url, CalendarError> {
        let first = responses.first();
        if let Some(home) = first.and_then(|r| r.calendar_home.as_ref()) {
            return self.resolve(home);
        }
        if let Some(principal) = first.and_then(|r| r.current_user_principal.as_ref()) {
            let purl = self.resolve(principal)?;
            let body = self
                .dav(
                    Method::from_bytes(b"PROPFIND").unwrap(),
                    &purl,
                    "0",
                    PROPFIND_HOME,
                )
                .await?;
            let resp = parse_multistatus(&body)?;
            if let Some(home) = resp.first().and_then(|r| r.calendar_home.as_ref()) {
                return self.resolve(home);
            }
        }
        // Last resort: treat the configured url as the calendar-home itself.
        Ok(self.base.clone())
    }

    /// `REPORT` one calendar collection for events overlapping `range`, asking
    /// the server to expand recurrences to concrete UTC instances.
    async fn report_calendar(
        &self,
        cal: &Url,
        range: &TimeRange,
        label: &str,
    ) -> Result<Vec<CalEvent>, CalendarError> {
        let start = fmt_ical(range.start);
        let end = fmt_ical(range.end);
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <c:calendar-data>
      <c:expand start="{start}" end="{end}"/>
    </c:calendar-data>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{start}" end="{end}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#
        );
        let resp = self
            .dav(Method::from_bytes(b"REPORT").unwrap(), cal, "1", &body)
            .await?;
        let responses = parse_multistatus(&resp)?;
        // A 207 can still carry a failure per resource. Skipping such a
        // response would make an event the server could not hand over look
        // deleted — and a calendar where that is every event look empty.
        if let Some(line) = responses.iter().find_map(RawResponse::failed_status) {
            return Err(CalendarError::Network(format!("REPORT {cal}: {line}")));
        }
        let with_data = responses
            .iter()
            .filter(|r| r.calendar_data.is_some())
            .count();
        trace_caldav!(
            "REPORT {cal}: {} response(s), {with_data} with calendar-data, {} body bytes",
            responses.len(),
            resp.len()
        );
        let mut out = Vec::new();
        let mut parsed_total = 0usize;
        for r in responses {
            let Some(data) = r.calendar_data else {
                continue;
            };
            if debug_enabled() {
                let begins = data.matches("BEGIN:VEVENT").count();
                let vcals = data.matches("BEGIN:VCALENDAR").count();
                // Structural markers only — the first non-empty line of an ICS is
                // `BEGIN:VCALENDAR`, never event content.
                let head: String = data.lines().next().unwrap_or("").chars().take(40).collect();
                trace_caldav!(
                    "  data: {} bytes, {vcals} VCALENDAR, {begins} VEVENT marker(s), head={head:?}",
                    data.len()
                );
            }
            for parsed in ical::parse_events(&data) {
                parsed_total += 1;
                if let Some(ev) = parsed.into_cal_event(label) {
                    out.push(ev);
                }
            }
        }
        trace_caldav!(
            "REPORT {cal}: parsed {parsed_total} VEVENT(s), {} survived into CalEvent",
            out.len()
        );
        Ok(out)
    }

    /// Issue a DAV request with a freshly-resolved Basic credential, retrying
    /// once on a 401 after invalidating the cached credential (so a rotated
    /// password is picked up without a reconnect).
    async fn dav(
        &self,
        method: Method,
        url: &Url,
        depth: &str,
        body: &str,
    ) -> Result<String, CalendarError> {
        let resp = self.send(&method, url, depth, body).await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            self.user.invalidate().await;
            self.pass.invalidate().await;
            let resp = self.send(&method, url, depth, body).await?;
            return read_body(url, resp).await;
        }
        read_body(url, resp).await
    }

    async fn send(
        &self,
        method: &Method,
        url: &Url,
        depth: &str,
        body: &str,
    ) -> Result<reqwest::Response, CalendarError> {
        let user = self.user.resolve().await.map_err(|e| {
            trace_caldav!("username resolve failed: {e}");
            CalendarError::Auth(e.to_string())
        })?;
        let pass = self.pass.resolve().await.map_err(|e| {
            trace_caldav!("password resolve failed: {e}");
            CalendarError::Auth(e.to_string())
        })?;
        trace_caldav!(
            "credentials resolved (user {} bytes, pass {} bytes); sending {method} {url}",
            user.len(),
            pass.len()
        );
        let resp = self
            .http
            .request(method.clone(), url.clone())
            .basic_auth(user, Some(pass))
            .header("Depth", depth)
            .header(header::CONTENT_TYPE, XML_CT)
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| {
                trace_caldav!("{method} {url} transport error: {e}");
                CalendarError::Network(format!("{method} {url}: {e}"))
            })?;
        trace_caldav!("{method} {url} (Depth {depth}) -> {}", resp.status());
        Ok(resp)
    }

    /// Resolve an href (absolute URL or absolute path) against the server root.
    fn resolve(&self, href: &str) -> Result<Url, CalendarError> {
        self.base
            .join(href.trim())
            .map_err(|e| CalendarError::Other(format!("bad href '{href}': {e}")))
    }
}

/// What a `PUT` asks of the resource it writes.
#[derive(Clone, Copy)]
enum Precondition<'a> {
    /// `If-None-Match: *` — a pure create: the server refuses when the
    /// resource somehow exists already.
    Absent,
    /// `If-Match: <etag>` — overwrite only the version that was read.
    Matches(&'a str),
    /// No condition: the server gave no `ETag` to hold it to.
    None,
}

async fn read_body(url: &Url, resp: reqwest::Response) -> Result<String, CalendarError> {
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| CalendarError::Network(format!("read {url}: {e}")))?;
    if !status.is_success() {
        let snippet: String = text.chars().take(300).collect();
        let msg = format!("{url} -> {status}: {snippet}");
        return Err(
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                CalendarError::Auth(msg)
            } else {
                CalendarError::Network(msg)
            },
        );
    }
    Ok(text)
}

/// A collision-resistant token for a new event's UID and resource filename:
/// wall-clock nanoseconds since the epoch plus a per-process monotonic counter,
/// in hex. URL/filename-safe (hex only). Uniqueness need only hold within this
/// process's writes — the `@not-yet-done` UID suffix namespaces them globally.
fn fresh_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{n:x}")
}

/// iCalendar UTC timestamp form the query filters expect: `20240115T090000Z`.
fn fmt_ical(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
        .replace(['-', ':'], "")
}

/// Last non-empty path segment of a collection URL — a decent display name
/// when the server gives none.
fn collection_name(url: &Url) -> String {
    url.path_segments()
        .and_then(|segs| segs.filter(|s| !s.is_empty()).next_back())
        .map(|s| s.to_string())
        .unwrap_or_else(|| url.as_str().to_string())
}

// --- Request bodies ------------------------------------------------------

const PROPFIND_HOME: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:current-user-principal/>
    <d:resourcetype/>
    <d:displayname/>
    <c:calendar-home-set/>
  </d:prop>
</d:propfind>"#;

const PROPFIND_CALENDARS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:resourcetype/>
    <d:displayname/>
    <c:supported-calendar-component-set/>
  </d:prop>
</d:propfind>"#;

// --- Multistatus XML parsing --------------------------------------------

/// The subset of one `<response>` we care about, filled by [`parse_multistatus`].
#[derive(Debug, Default)]
struct RawResponse {
    href: Option<String>,
    resourcetypes: Vec<String>,
    displayname: Option<String>,
    calendar_home: Option<String>,
    current_user_principal: Option<String>,
    comp_names: Vec<String>,
    calendar_data: Option<String>,
    /// Every `<status>` line of the response — its own and each propstat's —
    /// e.g. `HTTP/1.1 404 Not Found`.
    statuses: Vec<String>,
}

impl RawResponse {
    /// The first status line that is not a 2xx, if any.
    fn failed_status(&self) -> Option<&str> {
        self.statuses
            .iter()
            .map(String::as_str)
            .find(|line| !status_is_success(line))
    }
}

/// Whether a `<status>` line (`HTTP/1.1 200 OK`) reports a 2xx. A line that
/// carries no readable code is not one.
fn status_is_success(line: &str) -> bool {
    line.split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| (200..300).contains(&code))
}

/// Take the whole text of a closed element `name` (under `parent`) into
/// `resp`. The ICS payload keeps its raw whitespace (newlines matter); every
/// other leaf field is trimmed and skipped when blank, so pretty-print
/// whitespace between tags never becomes a value.
fn on_text(name: &str, parent: Option<&str>, text: String, resp: &mut RawResponse) {
    if name == "calendar-data" {
        resp.calendar_data.get_or_insert(text);
        return;
    }
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if name == "status" {
        resp.statuses.push(text.to_string());
        return;
    }
    let slot = match (name, parent) {
        ("href", Some("response")) => &mut resp.href,
        ("href", Some("calendar-home-set")) => &mut resp.calendar_home,
        ("href", Some("current-user-principal")) => &mut resp.current_user_principal,
        ("displayname", _) => &mut resp.displayname,
        _ => return,
    };
    slot.get_or_insert_with(|| text.to_string());
}

/// Parse a WebDAV/CalDAV `multistatus` body into per-`response` records. Namespace
/// prefixes vary by server (`D:`/`d:`, `C:`/`cal:`), so everything matches on the
/// element's local name.
fn parse_multistatus(xml: &str) -> Result<Vec<RawResponse>, CalendarError> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    // NB: do NOT trim text. The `<calendar-data>` payload is a raw iCalendar
    // document whose line breaks are significant; quick-xml emits it in chunks
    // split around those newlines, and trimming would strip the newlines at the
    // chunk boundaries — gluing every ICS line into one and breaking the parse.
    // Leaf fields that pick up pretty-print whitespace (href/displayname) are
    // trimmed individually in `on_text` instead.

    let mut out: Vec<RawResponse> = Vec::new();
    let mut cur: Option<RawResponse> = None;
    // Stack of element local names, so a Text/child can look at its parent.
    let mut stack: Vec<String> = Vec::new();
    // The text of the innermost open element, in the chunks and entity
    // references quick-xml splits it into; read when the element closes.
    let mut text = String::new();
    // Whether the document is a multistatus at all. A body that parses as XML
    // without being one — an HTML error or sign-in page served with a 2xx —
    // would otherwise read as "no responses", which to a caller is "no events".
    let mut root_seen = false;

    loop {
        match reader.read_event() {
            Err(e) => return Err(CalendarError::Other(format!("parse multistatus: {e}"))),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                text.clear();
                let name = local_name(e.name().as_ref());
                if !root_seen {
                    expect_multistatus(&name)?;
                    root_seen = true;
                }
                on_open(&name, &e, &stack, &mut cur, &mut out);
                stack.push(name);
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref());
                if !root_seen {
                    expect_multistatus(&name)?;
                    root_seen = true;
                }
                on_open(&name, &e, &stack, &mut cur, &mut out);
                // Empty elements don't nest — treat open+close atomically.
            }
            Ok(Event::End(_)) => {
                let name = stack.pop();
                let text = std::mem::take(&mut text);
                if let (Some(name), Some(resp)) = (name, cur.as_mut()) {
                    on_text(&name, stack.last().map(String::as_str), text, resp);
                }
            }
            Ok(Event::Text(t)) => {
                // `xml_content` decodes the charset + normalises EOLs.
                if let Ok(c) = t.xml_content() {
                    text.push_str(&c);
                }
            }
            Ok(Event::CData(c)) => {
                if let Ok(c) = c.decode() {
                    text.push_str(&c);
                }
            }
            // quick-xml hands each entity reference (`&amp;`, `&#38;`) over as
            // an event of its own, between the text chunks around it.
            Ok(Event::GeneralRef(r)) => {
                if let Ok(Some(ch)) = r.resolve_char_ref() {
                    text.push(ch);
                } else if let Some(entity) = r
                    .decode()
                    .ok()
                    .and_then(|name| quick_xml::escape::resolve_predefined_entity(&name))
                {
                    text.push_str(entity);
                }
            }
            _ => {}
        }
    }
    if !root_seen {
        return Err(CalendarError::Other(
            "the server answered with an empty body instead of a multistatus".into(),
        ));
    }
    // The last response has no following `<response>` to flush it.
    if let Some(last) = cur.take() {
        out.push(last);
    }
    Ok(out)
}

fn expect_multistatus(root: &str) -> Result<(), CalendarError> {
    if root == "multistatus" {
        Ok(())
    } else {
        Err(CalendarError::Other(format!(
            "the server answered with <{root}> instead of a multistatus"
        )))
    }
}

/// Handle an opening (or empty) element: start a new response, record a
/// resourcetype flag, or capture a `comp name=`.
fn on_open(
    name: &str,
    e: &quick_xml::events::BytesStart,
    stack: &[String],
    cur: &mut Option<RawResponse>,
    out: &mut Vec<RawResponse>,
) {
    if name == "response" {
        // Close out the previous response and begin a fresh one.
        if let Some(prev) = cur.take() {
            out.push(prev);
        }
        *cur = Some(RawResponse::default());
    }
    let parent = stack.last().map(String::as_str);
    if parent == Some("resourcetype") {
        if let Some(resp) = cur.as_mut() {
            resp.resourcetypes.push(name.to_string());
        }
    }
    if name == "comp" && parent == Some("supported-calendar-component-set") {
        if let (Some(resp), Some(v)) = (cur.as_mut(), attr(e, b"name")) {
            resp.comp_names.push(v);
        }
    }
}

/// Local (namespace-stripped) name of an element as a `String`.
fn local_name(qname: &[u8]) -> String {
    let local = match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    };
    String::from_utf8_lossy(local).into_owned()
}

/// Read an unnamespaced attribute value by local key.
fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let k = a.key.as_ref();
        let local = match k.iter().position(|&b| b == b':') {
            Some(i) => &k[i + 1..],
            None => k,
        };
        (local == key).then(|| String::from_utf8_lossy(&a.value).into_owned())
    })
}

/// `text` safe to put between XML tags.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_that_is_not_a_multistatus_is_an_error_not_an_empty_answer() {
        let html = "<html><head><title>Sign in</title></head><body/></html>";
        assert!(parse_multistatus(html).is_err());
        assert!(parse_multistatus("").is_err());
        assert!(parse_multistatus("<?xml version=\"1.0\"?>").is_err());
    }

    #[test]
    fn an_empty_multistatus_is_an_empty_answer() {
        let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:"/>"#;
        assert!(parse_multistatus(xml).unwrap().is_empty());
    }

    #[test]
    fn a_failed_status_inside_a_multistatus_is_reported() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/cal/a.ics</d:href>
    <d:propstat>
      <d:prop><c:calendar-data/></d:prop>
      <d:status>HTTP/1.1 503 Service Unavailable</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/cal/b.ics</d:href>
    <d:propstat><d:prop/><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
</d:multistatus>"#;
        let r = parse_multistatus(xml).unwrap();
        assert_eq!(r[0].failed_status(), Some("HTTP/1.1 503 Service Unavailable"));
        assert_eq!(r[1].failed_status(), None);
    }

    #[test]
    fn fmt_ical_is_compact_utc() {
        let dt = chrono::DateTime::parse_from_rfc3339("2024-01-15T09:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(fmt_ical(dt), "20240115T093000Z");
    }

    #[test]
    fn parses_home_and_principal_from_propfind() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/jane@mail.de/</d:href>
    <d:propstat><d:prop>
      <d:current-user-principal><d:href>/principals/jane@mail.de/</d:href></d:current-user-principal>
      <c:calendar-home-set><d:href>/calendars/jane@mail.de/</d:href></c:calendar-home-set>
      <d:resourcetype><d:principal/><d:collection/></d:resourcetype>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let r = parse_multistatus(xml).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].href.as_deref(), Some("/principals/jane@mail.de/"));
        assert_eq!(
            r[0].calendar_home.as_deref(),
            Some("/calendars/jane@mail.de/")
        );
        assert_eq!(
            r[0].current_user_principal.as_deref(),
            Some("/principals/jane@mail.de/")
        );
        assert!(r[0].resourcetypes.iter().any(|t| t == "collection"));
    }

    #[test]
    fn parses_calendar_list_with_comp_set() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/jane@mail.de/private/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Privat</d:displayname>
      <d:resourcetype><d:collection/><cal:calendar/></d:resourcetype>
      <cal:supported-calendar-component-set><cal:comp name="VEVENT"/></cal:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/jane@mail.de/tasks/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Tasks</d:displayname>
      <d:resourcetype><d:collection/><cal:calendar/></d:resourcetype>
      <cal:supported-calendar-component-set><cal:comp name="VTODO"/></cal:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let r = parse_multistatus(xml).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].displayname.as_deref(), Some("Privat"));
        assert!(r[0].comp_names.iter().any(|c| c == "VEVENT"));
        assert!(r[1].comp_names.iter().any(|c| c == "VTODO"));
        assert!(r[1].comp_names.iter().all(|c| c != "VEVENT"));
    }

    #[test]
    fn extracts_calendar_data_from_report() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/jane@mail.de/private/1.ics</d:href>
    <d:propstat><d:prop>
      <c:calendar-data>BEGIN:VCALENDAR
BEGIN:VEVENT
UID:1
DTSTART:20240115T090000Z
SUMMARY:Hi
END:VEVENT
END:VCALENDAR</c:calendar-data>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let r = parse_multistatus(xml).unwrap();
        assert_eq!(r.len(), 1);
        let data = r[0].calendar_data.as_deref().unwrap();
        assert!(data.contains("BEGIN:VEVENT"));
        let events = ical::parse_events(data);
        assert_eq!(events.len(), 1);
    }

    // Regression: real servers (mail.de/SabreDAV) send calendar-data with CRLF
    // line endings, and quick-xml chunks the text around those newlines. Trimming
    // the chunks (the old `trim_text(true)`) stripped every line break, gluing the
    // ICS into one line so `BEGIN:VEVENT` was never recognised. The parser must
    // preserve the payload's newlines verbatim. (A non-raw string so `\r\n` are
    // real CRLFs, unlike the `\n`-only raw-string sample above.)
    #[test]
    fn preserves_crlf_newlines_in_calendar_data() {
        let xml = "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">\r\n\
  <d:response>\r\n\
    <d:href>/calendars/jane@mail.de/private/1.ics</d:href>\r\n\
    <d:propstat><d:prop>\r\n\
      <c:calendar-data>BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:20240115T090000Z\r\nSUMMARY:Hi\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n</c:calendar-data>\r\n\
    </d:prop></d:propstat>\r\n\
  </d:response>\r\n\
</d:multistatus>";
        let r = parse_multistatus(xml).unwrap();
        assert_eq!(r.len(), 1);
        let data = r[0].calendar_data.as_deref().unwrap();
        // The newlines survived: the first line is exactly BEGIN:VCALENDAR, not
        // the whole document glued together.
        assert_eq!(data.lines().next(), Some("BEGIN:VCALENDAR"));
        let events = ical::parse_events(data);
        assert_eq!(events.len(), 1);
        // And the href kept no surrounding pretty-print whitespace.
        assert_eq!(
            r[0].href.as_deref(),
            Some("/calendars/jane@mail.de/private/1.ics")
        );
    }

    // Regression: quick-xml hands `&amp;` over as an event of its own, which the
    // parser used to drop — "Chains &amp; brakes" read back as "Chains  brakes".
    #[test]
    fn keeps_entity_references_and_cdata() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/cal/a&amp;b.ics</d:href>
    <d:propstat><d:prop>
      <d:displayname>Home &amp; garden &#38; more</d:displayname>
      <c:calendar-data>BEGIN:VCALENDAR
BEGIN:VEVENT
UID:1
DTSTART:20240115T090000Z
SUMMARY:Chains &amp; brakes &lt;now&gt;
END:VEVENT
END:VCALENDAR
</c:calendar-data>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/cal/2.ics</d:href>
    <d:propstat><d:prop>
      <c:calendar-data><![CDATA[BEGIN:VCALENDAR
BEGIN:VEVENT
UID:2
DTSTART:20240115T090000Z
SUMMARY:Tea & cake
END:VEVENT
END:VCALENDAR
]]></c:calendar-data>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let r = parse_multistatus(xml).unwrap();
        assert_eq!(r[0].href.as_deref(), Some("/cal/a&b.ics"));
        assert_eq!(r[0].displayname.as_deref(), Some("Home & garden & more"));
        let first = ical::parse_events(r[0].calendar_data.as_deref().unwrap());
        assert_eq!(first[0].summary.as_deref(), Some("Chains & brakes <now>"));
        let second = ical::parse_events(r[1].calendar_data.as_deref().unwrap());
        assert_eq!(second[0].summary.as_deref(), Some("Tea & cake"));
    }
}
