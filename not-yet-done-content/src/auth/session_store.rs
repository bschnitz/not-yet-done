//! Pluggable storage for derived session credentials (JWTs, login cookies, …).
//!
//! The orchestrator hands the store an opaque blob plus a creation
//! timestamp; the policy in [`SessionCachePolicy`](super::SessionCachePolicy)
//! decides how to interpret the timestamp. The blob itself stays opaque —
//! adapters serialize whatever they need (token, refresh, expiry, …)
//! into it.
//!
//! [`InMemorySessionStore`] covers the volatile case
//! (`SessionCachePolicy::None`) and tests. Adapters that need disk
//! persistence implement [`SessionStore`] against their own database
//! — see the Taiga migration in Phase 3.

use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::RwLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEntry {
    /// Adapter-specific session payload (typically JSON: token, refresh,
    /// expiry, …). Opaque to the orchestrator.
    pub blob: String,
    /// Wall-clock time when the session was issued. Used by TTL policies.
    pub created_at: SystemTime,
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn load(&self) -> Option<SessionEntry>;
    async fn save(&self, entry: SessionEntry);
    async fn delete(&self);
}

#[derive(Default)]
pub struct InMemorySessionStore {
    inner: RwLock<Option<SessionEntry>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn load(&self) -> Option<SessionEntry> {
        self.inner.read().await.clone()
    }

    async fn save(&self, entry: SessionEntry) {
        *self.inner.write().await = Some(entry);
    }

    async fn delete(&self) {
        *self.inner.write().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_store_roundtrip() {
        let s = InMemorySessionStore::new();
        assert!(s.load().await.is_none());

        let entry = SessionEntry {
            blob: "synthetic-token".into(),
            created_at: SystemTime::UNIX_EPOCH,
        };
        s.save(entry.clone()).await;
        assert_eq!(s.load().await, Some(entry));

        s.delete().await;
        assert!(s.load().await.is_none());
    }

    #[tokio::test]
    async fn in_memory_store_overwrites_on_save() {
        let s = InMemorySessionStore::new();
        s.save(SessionEntry {
            blob: "a".into(),
            created_at: SystemTime::UNIX_EPOCH,
        })
        .await;
        s.save(SessionEntry {
            blob: "b".into(),
            created_at: SystemTime::UNIX_EPOCH,
        })
        .await;
        assert_eq!(s.load().await.unwrap().blob, "b");
    }
}
