//! Ad-hoc end-to-end smoke for the CalDAV backend against a real server.
//!
//! The calendar *adapter* serves from a background-warmed cache and never
//! blocks on a fetch, so a one-shot CLI `ls` reads an empty cache and returns
//! before the first network round-trip — useless for verifying this backend.
//! This example bypasses the adapter and awaits [`CalendarBackend::list_events`]
//! directly, so the fetch actually completes and its result is printed.
//!
//! No secrets or endpoints are baked in — everything comes from the
//! environment, so this file carries no real data and is safe to commit:
//!
//! ```sh
//! CALDAV_URL='https://server/principals/user'   \
//! CALDAV_USER='user@example.com'                 \
//! CALDAV_PASS_CMD='pass show path/to/password'   \
//! NYD_DEBUG_CALDAV=1                              \
//!   cargo run -p not-yet-done-calendar-caldav --example smoke
//! ```
//!
//! Optional: `CALDAV_PAST_DAYS` / `CALDAV_FUTURE_DAYS` (default 3650 each) widen
//! the window; `CALDAV_CALENDARS` (comma-separated) pins explicit collections
//! and skips discovery.

use std::sync::Arc;

use chrono::{Duration, Utc};
use not_yet_done_calendar_caldav::CalDavBackendFactory;
use not_yet_done_calendar_core::{CalendarBackendFactory, TimeRange};
use not_yet_done_content::{HostContext, InMemoryHostBus};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = env("CALDAV_URL")?;
    let user = env("CALDAV_USER")?;
    let pass_cmd = env("CALDAV_PASS_CMD")?;
    let past: i64 = opt("CALDAV_PAST_DAYS")
        .as_deref()
        .unwrap_or("3650")
        .parse()?;
    let future: i64 = opt("CALDAV_FUTURE_DAYS")
        .as_deref()
        .unwrap_or("3650")
        .parse()?;

    // Build the backend's YAML config from env. `calendars:` only when pinned.
    let mut config = format!(
        "url: {url}\n\
         username:\n  type: literal\n  value: {user}\n\
         password:\n  type: command\n  script: {pass_cmd}\n"
    );
    if let Some(cals) = opt("CALDAV_CALENDARS") {
        config.push_str("calendars:\n");
        for c in cals.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            config.push_str(&format!("  - {c}\n"));
        }
    }

    let ctx = HostContext {
        event_bus: Arc::new(InMemoryHostBus::default()),
        anonymize: false,
    };
    let backend = CalDavBackendFactory::new().create("smoke", &config, &ctx)?;

    let now = Utc::now();
    let range = TimeRange {
        start: now - Duration::days(past),
        end: now + Duration::days(future),
    };
    eprintln!("[smoke] window {} .. {}", range.start, range.end);

    let events = backend.list_events(&range).await?;
    println!("[smoke] {} event(s) returned", events.len());
    for (i, ev) in events.iter().enumerate().take(50) {
        println!(
            "  {:>3}. {} .. {}  {}{}",
            i + 1,
            ev.start.to_rfc3339(),
            ev.end.to_rfc3339(),
            if ev.all_day { "[all-day] " } else { "" },
            ev.title,
        );
    }
    if events.len() > 50 {
        println!("  … and {} more", events.len() - 50);
    }

    // Field coverage — how many events carry each optional field. Counts only
    // (never values), so it stays privacy-safe while showing whether a column
    // like "organizer" would actually be populated for this server.
    let n = events.len().max(1);
    let cover =
        |name: &str, c: usize| println!("  {name}: {c}/{} ({}%)", events.len(), c * 100 / n);
    println!("[smoke] field coverage:");
    cover(
        "organizer",
        events.iter().filter(|e| e.organizer.is_some()).count(),
    );
    cover(
        "location",
        events.iter().filter(|e| e.location.is_some()).count(),
    );
    cover("body", events.iter().filter(|e| e.body.is_some()).count());
    cover("url", events.iter().filter(|e| e.url.is_some()).count());
    Ok(())
}

fn env(key: &str) -> Result<String, String> {
    std::env::var(key).map_err(|_| format!("missing required env var {key}"))
}

fn opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}
