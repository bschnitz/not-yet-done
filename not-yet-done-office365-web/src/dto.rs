//! Protocol-neutral, Office-365-flavoured data types returned by the domain
//! APIs. These are the crate's public vocabulary; a consumer (e.g. a calendar
//! backend) maps them onto its own domain DTOs.
//!
//! The `Deserialize` impls match the JSON the sidecar emits (camelCase), so a
//! sidecar response body can be parsed straight into these types.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Half-open time window `[start, end)` a listing is scoped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MsTimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl MsTimeRange {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self { start, end }
    }
}

/// How the organiser marks the time an event occupies (the Outlook/Exchange
/// `showAs` / free-busy status). Unknown tokens degrade to [`MsShowAs::Unknown`]
/// rather than failing the whole listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MsShowAs {
    Free,
    Tentative,
    Busy,
    Oof,
    WorkingElsewhere,
    #[default]
    #[serde(other)]
    Unknown,
}

/// One calendar event as seen through the Office 365 web surface. `start`/`end`
/// are UTC instants.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MsCalEvent {
    /// Source-stable event id.
    pub id: String,
    #[serde(default)]
    pub subject: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub is_all_day: bool,
    #[serde(default)]
    pub show_as: MsShowAs,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub organizer: Option<String>,
    /// Plain-text body preview, if the sidecar captured one.
    #[serde(default)]
    pub body_preview: Option<String>,
    /// Web link to open the event in the source app, if any.
    #[serde(default)]
    pub web_link: Option<String>,
}
