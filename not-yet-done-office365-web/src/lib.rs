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
//! is to keep a real (headless) browser session alive and drive it.
//!
//! This crate isolates all of that behind a clean seam:
//!
//! - [`MsOfficeWeb::session`] hands out a [`SessionHandle`] for an account,
//!   sharing one browser session across all consumers that pass the same
//!   `account_key` (see [`SessionConfig`]).
//! - [`SessionHandle::calendar`] (and, later, `mail()` etc.) return typed
//!   domain APIs — [`CalendarApi`] today.
//! - The browser itself runs as an **out-of-process Playwright sidecar**; the
//!   Rust side only speaks a small newline-delimited JSON protocol to it.
//!
//! The crate deliberately depends on nothing from this workspace's adapter or
//! content layers, so it stays reusable for any Office 365 web surface.

mod calendar;
mod dto;
mod error;
mod registry;
mod session;
mod sidecar;

pub use calendar::CalendarApi;
pub use dto::{MsCalEvent, MsShowAs, MsTimeRange};
pub use error::MsOfficeError;
pub use registry::MsOfficeWeb;
pub use session::{
    LoadStatus, LoginCredentials, LoginState, PromptKind, SessionConfig, SessionHandle,
    SessionPrompt, SidecarConfig,
};
