//! sea-orm-backed [`SessionStore`] for the Stoat adapter.
//!
//! Persists the orchestrator's opaque session blob — adapter-side JSON
//! containing the `X-Session-Token` plus the resolved user identity — in
//! the `auth_session` table, keyed by the connection's `scope_id`
//! (UUID v5 of the base URL). The password is never stored.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, DatabaseConnection, EntityTrait, IntoActiveModel, Set, TransactionTrait,
};
use uuid::Uuid;

use not_yet_done_content::{SessionEntry, SessionStore};

use crate::entity::auth_session;

pub struct SqlAuthSessionStore {
    db: Arc<DatabaseConnection>,
    scope_id: Uuid,
}

impl SqlAuthSessionStore {
    pub fn new(db: Arc<DatabaseConnection>, scope_id: Uuid) -> Self {
        Self { db, scope_id }
    }

    fn unix_seconds(t: SystemTime) -> i64 {
        t.duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    fn from_unix(secs: i64) -> SystemTime {
        if secs >= 0 {
            SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64)
        } else {
            SystemTime::UNIX_EPOCH
        }
    }
}

#[async_trait]
impl SessionStore for SqlAuthSessionStore {
    async fn load(&self) -> Option<SessionEntry> {
        let row = auth_session::Entity::find_by_id(self.scope_id)
            .one(self.db.as_ref())
            .await
            .ok()
            .flatten()?;
        Some(SessionEntry {
            blob: row.blob,
            created_at: Self::from_unix(row.created_at_unix),
        })
    }

    async fn save(&self, entry: SessionEntry) {
        let unix = Self::unix_seconds(entry.created_at);
        let txn = match self.db.begin().await {
            Ok(t) => t,
            Err(_) => return,
        };
        let existing = auth_session::Entity::find_by_id(self.scope_id)
            .one(&txn)
            .await
            .ok()
            .flatten();
        match existing {
            Some(model) => {
                let mut am = model.into_active_model();
                am.blob = Set(entry.blob);
                am.created_at_unix = Set(unix);
                let _ = am.update(&txn).await;
            }
            None => {
                let am = auth_session::ActiveModel {
                    connection_id: Set(self.scope_id),
                    blob: Set(entry.blob),
                    created_at_unix: Set(unix),
                };
                let _ = am.insert(&txn).await;
            }
        }
        let _ = txn.commit().await;
    }

    async fn delete(&self) {
        let _ = auth_session::Entity::delete_by_id(self.scope_id)
            .exec(self.db.as_ref())
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::Database;

    async fn fresh_db() -> Arc<DatabaseConnection> {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory");
        db.get_schema_registry("not_yet_done_stoat_adapter::entity::*")
            .sync(&db)
            .await
            .expect("schema sync");
        Arc::new(db)
    }

    #[tokio::test]
    async fn roundtrip_save_load_delete() {
        let db = fresh_db().await;
        let scope = Uuid::new_v4();
        let store = SqlAuthSessionStore::new(db.clone(), scope);

        assert!(store.load().await.is_none());

        let entry = SessionEntry {
            blob: r#"{"token":"synthetic-session-token"}"#.into(),
            created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(123_456),
        };
        store.save(entry.clone()).await;
        let loaded = store.load().await.expect("present");
        assert_eq!(loaded.blob, entry.blob);
        assert_eq!(loaded.created_at, entry.created_at);

        store.delete().await;
        assert!(store.load().await.is_none());
    }

    #[tokio::test]
    async fn save_overwrites_existing_row() {
        let db = fresh_db().await;
        let scope = Uuid::new_v4();
        let store = SqlAuthSessionStore::new(db.clone(), scope);

        store
            .save(SessionEntry {
                blob: "first".into(),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(100),
            })
            .await;
        store
            .save(SessionEntry {
                blob: "second".into(),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(200),
            })
            .await;
        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.blob, "second");
    }
}
