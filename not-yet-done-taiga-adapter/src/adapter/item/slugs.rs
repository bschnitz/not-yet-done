//! Taiga-specific slug tables (status / user / tag) built on top of the
//! generic `not_yet_done_content::slug::SlugTable`.
//!
//! Prefixes:
//! - `ss-` status (slug source: status name; original: status name)
//! - `uu-` user (slug source: full_name fallback username; original: username)
//! - `tt-` tag (slug source: tag name; original: tag name)

use not_yet_done_content::slug::SlugTable;

use crate::client::{TaigaMember, TaigaStatus};

pub(super) const STATUS_PREFIX: &str = "ss-";
pub(super) const USER_PREFIX: &str = "uu-";
pub(super) const TAG_PREFIX: &str = "tt-";

pub(super) struct TaigaSlugTables {
    pub(super) statuses: SlugTable,
    pub(super) users: SlugTable,
    pub(super) tags: SlugTable,
}

pub(super) fn build_status_table(statuses: &[TaigaStatus]) -> SlugTable {
    SlugTable::build(
        statuses.iter().map(|s| (s.name.clone(), s.name.clone())),
        STATUS_PREFIX,
    )
}

/// Build a user table. `slug_source` is the display name (falls back to
/// username), `original` is always the canonical username — the value used
/// inside the editable buffer once slugs are resolved and the value the
/// PATCH body sends as `assigned_to_id` lookup key.
pub(super) fn build_user_table(members: &[TaigaMember]) -> SlugTable {
    SlugTable::build(
        members.iter().map(|m| {
            let src = if m.full_name.is_empty() {
                m.username.clone()
            } else {
                m.full_name.clone()
            };
            (src, m.username.clone())
        }),
        USER_PREFIX,
    )
}

pub(super) fn build_tag_table(tags: &[String]) -> SlugTable {
    SlugTable::build(
        tags.iter().map(|t| (t.clone(), t.clone())),
        TAG_PREFIX,
    )
}
