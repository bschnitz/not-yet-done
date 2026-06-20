//! CRUD for the global `link` table.
//!
//! Each row represents a directed link `source_ref → target_ref`.
//! The repository owns no traversal logic — resolving / navigating
//! to a node is the [`not_yet_done_content::LinkRoute`] contract,
//! which lives elsewhere.

use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use shaku::Component;
use uuid::Uuid;

use not_yet_done_content::NodeRef;

use crate::entity::link::{self, ActiveModel};
use crate::error::CoreError;

#[async_trait]
pub trait LinkRepository: shaku::Interface {
    /// Create a link `source → target`. Idempotent: if the same
    /// directed pair already exists the existing row is returned
    /// instead of duplicating it.
    async fn create(
        &self,
        source: &NodeRef,
        target: &NodeRef,
    ) -> Result<link::Model, CoreError>;

    async fn find_by_id(&self, id: Uuid) -> Result<Option<link::Model>, CoreError>;

    /// Links pointing *out of* `source`. Newest first.
    async fn outgoing(&self, source: &NodeRef) -> Result<Vec<link::Model>, CoreError>;

    /// Links pointing *to* `target`. Newest first.
    async fn incoming(&self, target: &NodeRef) -> Result<Vec<link::Model>, CoreError>;

    /// Count of outgoing + incoming for `node`. Used by the "has
    /// links" indicator column to avoid loading the rows themselves.
    async fn count_for(&self, node: &NodeRef) -> Result<u64, CoreError>;

    async fn delete(&self, id: Uuid) -> Result<(), CoreError>;

    /// Every link row. Used by the bulk-prune command to walk every
    /// ref and test it against the routing chain.
    async fn list_all(&self) -> Result<Vec<link::Model>, CoreError>;
}

#[derive(Component)]
#[shaku(interface = LinkRepository)]
pub struct LinkRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

#[async_trait]
impl LinkRepository for LinkRepositoryImpl {
    async fn create(
        &self,
        source: &NodeRef,
        target: &NodeRef,
    ) -> Result<link::Model, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let source_s = source.as_str().to_string();
        let target_s = target.as_str().to_string();

        if let Some(existing) = link::Entity::find()
            .filter(link::Column::SourceRef.eq(&source_s))
            .filter(link::Column::TargetRef.eq(&target_s))
            .one(db)
            .await?
        {
            return Ok(existing);
        }

        let model = ActiveModel {
            source_ref: Set(source_s),
            target_ref: Set(target_s),
            ..ActiveModel::new()
        };
        Ok(model.insert(db).await?)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<link::Model>, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(link::Entity::find_by_id(id).one(db).await?)
    }

    async fn outgoing(&self, source: &NodeRef) -> Result<Vec<link::Model>, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(link::Entity::find()
            .filter(link::Column::SourceRef.eq(source.as_str()))
            .order_by_desc(link::Column::CreatedAt)
            .all(db)
            .await?)
    }

    async fn incoming(&self, target: &NodeRef) -> Result<Vec<link::Model>, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(link::Entity::find()
            .filter(link::Column::TargetRef.eq(target.as_str()))
            .order_by_desc(link::Column::CreatedAt)
            .all(db)
            .await?)
    }

    async fn count_for(&self, node: &NodeRef) -> Result<u64, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let s = node.as_str();
        Ok(link::Entity::find()
            .filter(
                Condition::any()
                    .add(link::Column::SourceRef.eq(s))
                    .add(link::Column::TargetRef.eq(s)),
            )
            .count(db)
            .await?)
    }

    async fn delete(&self, id: Uuid) -> Result<(), CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        link::Entity::delete_by_id(id).exec(db).await?;
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<link::Model>, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(link::Entity::find()
            .order_by_desc(link::Column::CreatedAt)
            .all(db)
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Database, DbBackend, Schema};

    async fn setup() -> (LinkRepositoryImpl, DatabaseConnection) {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        let schema = Schema::new(DbBackend::Sqlite);
        db.execute(&schema.create_table_from_entity(link::Entity))
            .await
            .expect("create link table");
        let repo = LinkRepositoryImpl { db: Some(db.clone()) };
        (repo, db)
    }

    fn nref(s: &str) -> NodeRef {
        NodeRef::parse(s).expect("valid")
    }

    #[tokio::test]
    async fn create_and_find_round_trip() {
        let (repo, _db) = setup().await;
        let src = nref("jira/prod/PROJ-1");
        let tgt = nref("tasks/abc-123");
        let row = repo.create(&src, &tgt).await.unwrap();
        assert_eq!(row.source_ref, "jira/prod/PROJ-1");
        assert_eq!(row.target_ref, "tasks/abc-123");

        let by_id = repo.find_by_id(row.id).await.unwrap().unwrap();
        assert_eq!(by_id, row);
    }

    #[tokio::test]
    async fn create_is_idempotent_per_directed_pair() {
        let (repo, _db) = setup().await;
        let src = nref("jira/prod/PROJ-1");
        let tgt = nref("tasks/abc");
        let a = repo.create(&src, &tgt).await.unwrap();
        let b = repo.create(&src, &tgt).await.unwrap();
        assert_eq!(a.id, b.id, "duplicate create returns existing row");
        assert_eq!(repo.list_all().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reverse_direction_is_a_separate_row() {
        let (repo, _db) = setup().await;
        let a = nref("jira/prod/PROJ-1");
        let b = nref("tasks/abc");
        let ab = repo.create(&a, &b).await.unwrap();
        let ba = repo.create(&b, &a).await.unwrap();
        assert_ne!(ab.id, ba.id);
        assert_eq!(repo.list_all().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn outgoing_and_incoming() {
        let (repo, _db) = setup().await;
        let hub = nref("jira/prod/HUB-1");
        let a = nref("tasks/aaa");
        let b = nref("tasks/bbb");
        repo.create(&hub, &a).await.unwrap();
        repo.create(&hub, &b).await.unwrap();
        repo.create(&a, &hub).await.unwrap();

        let out = repo.outgoing(&hub).await.unwrap();
        assert_eq!(out.len(), 2);
        for row in &out {
            assert_eq!(row.source_ref, "jira/prod/HUB-1");
        }

        let inc = repo.incoming(&hub).await.unwrap();
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0].source_ref, "tasks/aaa");
    }

    #[tokio::test]
    async fn count_covers_both_directions() {
        let (repo, _db) = setup().await;
        let hub = nref("jira/prod/HUB-1");
        let a = nref("tasks/aaa");
        let b = nref("tasks/bbb");
        repo.create(&hub, &a).await.unwrap();
        repo.create(&hub, &b).await.unwrap();
        repo.create(&a, &hub).await.unwrap();
        assert_eq!(repo.count_for(&hub).await.unwrap(), 3);
        assert_eq!(repo.count_for(&a).await.unwrap(), 2);
        assert_eq!(repo.count_for(&b).await.unwrap(), 1);
        assert_eq!(repo.count_for(&nref("tasks/lonely")).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let (repo, _db) = setup().await;
        let src = nref("jira/prod/PROJ-1");
        let tgt = nref("tasks/abc");
        let row = repo.create(&src, &tgt).await.unwrap();
        repo.delete(row.id).await.unwrap();
        assert!(repo.find_by_id(row.id).await.unwrap().is_none());
        assert!(repo.list_all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn outgoing_orders_newest_first() {
        let (repo, _db) = setup().await;
        let src = nref("jira/prod/PROJ-1");
        let old = repo.create(&src, &nref("tasks/older")).await.unwrap();
        // Ensure later row has strictly newer created_at on fast systems.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let new = repo.create(&src, &nref("tasks/newer")).await.unwrap();
        let out = repo.outgoing(&src).await.unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, new.id);
        assert_eq!(out[1].id, old.id);
    }
}
