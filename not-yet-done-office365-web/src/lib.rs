//! Authenticated Office 365 **web** session, exposed as typed, app-agnostic
//! APIs.
//!
//! # Why this crate exists
//!
//! Some Office 365 tenants can only be reached through the browser: the Graph
//! API is blocked by device-compliance Conditional Access, and the data plane
//! (Outlook on the web / Exchange) authenticates every call with a short-lived
//! MSAL bearer token that is minted *inside* the browser — plain cookies are
//! not enough. The only durable, unattended way in from an unmanaged machine
//! is to keep a real (hidden) browser session alive and drive it.
//!
//! This crate isolates all of that behind a clean seam:
//!
//! - [`MsOfficeWeb::session`] hands out a [`SessionHandle`] for an account,
//!   sharing one browser session across all consumers that pass the same
//!   `account_key` (see [`SessionConfig`]).
//! - [`SessionHandle::calendar`] (and, later, `mail()` etc.) return typed
//!   domain APIs — [`CalendarApi`] today.
//! - The browser is a **`drunken-browser` child process**, one per account,
//!   driven over its control socket. What it does in the page is a *flow* on
//!   the browser's own shelf; this crate opens the flow, runs it with the
//!   range as its data, relays what the run says and asks to whoever attends
//!   the session ([`SessionPrompt`]), and reads what the run yielded.
//!
//! The crate deliberately depends on nothing from this workspace's adapter or
//! content layers, so it stays reusable for any Office 365 web surface.

mod browser;
mod calendar;
mod dto;
mod error;
mod registry;
mod session;

pub use calendar::CalendarApi;
pub use dto::{MsCalEvent, MsShowAs, MsTimeRange};
pub use error::MsOfficeError;
pub use registry::MsOfficeWeb;
pub use session::{
    Answers, BrowserConfig, LoadStatus, PromptKind, SessionConfig, SessionHandle, SessionPrompt,
};
