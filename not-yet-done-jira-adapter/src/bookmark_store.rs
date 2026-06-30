//! sea-orm-backed [`BookmarkStore`] for the Jira adapter.
//!
//! Hides sea-orm entirely behind the adapter-agnostic [`BookmarkStore`]
//! trait from `not-yet-done-content`: the adapter and node code depend only
//! on the trait, never on the `jira_bookmark` table or sea-orm types. Rows
//! are keyed by the connection's `scope_id` (UUID v5 of the base URL), so a
//! bookmark belongs to the Jira server and is shared across every view-file
//! instance pointing at that server.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use not_yet_done_content::{Bookmark, BookmarkStore, ContentError, Result};

use crate::entity::jira_bookmark;

pub struct SqlBookmarkStore {
    db: Arc<DatabaseConnection>,
    scope_id: Uuid,
}

impl SqlBookmarkStore {
    pub fn new(db: Arc<DatabaseConnection>, scope_id: Uuid) -> Self {
        Self { db, scope_id }
    }

    /// Render stored unix seconds as the RFC3339 stamp the `BookmarkStore`
    /// contract (and the bookmarks view's `DateTime` sort) expects.
    fn rfc3339(unix: i64) -> String {
        chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default()
    }

    fn boxed(e: impl std::error::Error + Send + Sync + 'static) -> ContentError {
        ContentError::Other(Box::new(e))
    }
}

#[async_trait]
impl BookmarkStore for SqlBookmarkStore {
    async fn list(&self) -> Result<Vec<Bookmark>> {
        let rows = jira_bookmark::Entity::find()
            .filter(jira_bookmark::Column::ConnectionId.eq(self.scope_id))
            .order_by_asc(jira_bookmark::Column::BookmarkedAtUnix)
            .order_by_asc(jira_bookmark::Column::IssueKey)
            .all(self.db.as_ref())
            .await
            .map_err(Self::boxed)?;
        Ok(rows
            .into_iter()
            .map(|r| Bookmark {
                id: r.issue_key,
                bookmarked_at: Self::rfc3339(r.bookmarked_at_unix),
            })
            .collect())
    }

    async fn contains(&self, id: &str) -> Result<bool> {
        let found = jira_bookmark::Entity::find_by_id((self.scope_id, id.to_string()))
            .one(self.db.as_ref())
            .await
            .map_err(Self::boxed)?;
        Ok(found.is_some())
    }

    async fn toggle(&self, id: &str) -> Result<bool> {
        let existing = jira_bookmark::Entity::find_by_id((self.scope_id, id.to_string()))
            .one(self.db.as_ref())
            .await
            .map_err(Self::boxed)?;
        if existing.is_some() {
            jira_bookmark::Entity::delete_by_id((self.scope_id, id.to_string()))
                .exec(self.db.as_ref())
                .await
                .map_err(Self::boxed)?;
            Ok(false)
        } else {
            jira_bookmark::ActiveModel {
                connection_id: Set(self.scope_id),
                issue_key: Set(id.to_string()),
                bookmarked_at_unix: Set(chrono::Utc::now().timestamp()),
            }
            .insert(self.db.as_ref())
            .await
            .map_err(Self::boxed)?;
            Ok(true)
        }
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
        db.get_schema_registry("not_yet_done_jira_adapter::entity::*")
            .sync(&db)
            .await
            .expect("schema sync");
        Arc::new(db)
    }

    #[tokio::test]
    async fn empty_store_lists_nothing() {
        let store = SqlBookmarkStore::new(fresh_db().await, Uuid::new_v4());
        assert!(store.list().await.unwrap().is_empty());
        assert!(!store.contains("PROJ-1").await.unwrap());
    }

    #[tokio::test]
    async fn toggle_adds_then_removes() {
        let store = SqlBookmarkStore::new(fresh_db().await, Uuid::new_v4());
        assert!(store.toggle("PROJ-1").await.unwrap()); // now bookmarked
        assert!(store.contains("PROJ-1").await.unwrap());
        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "PROJ-1");
        assert!(!all[0].bookmarked_at.is_empty());

        assert!(!store.toggle("PROJ-1").await.unwrap()); // removed
        assert!(!store.contains("PROJ-1").await.unwrap());
        assert!(store.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn separate_scopes_dont_collide() {
        let db = fresh_db().await;
        let store_a = SqlBookmarkStore::new(db.clone(), Uuid::new_v4());
        let store_b = SqlBookmarkStore::new(db.clone(), Uuid::new_v4());

        store_a.toggle("PROJ-1").await.unwrap();
        assert!(store_a.contains("PROJ-1").await.unwrap());
        assert!(!store_b.contains("PROJ-1").await.unwrap());
        assert!(store_b.list().await.unwrap().is_empty());
    }
}
