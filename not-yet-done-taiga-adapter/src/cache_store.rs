//! Persistent backing for the Taiga adapter's project-meta cache.
//!
//! Stores statuses / members / tags per project, used by the editable
//! templates to build slug tables. Merge-only: existing rows get their
//! display fields overwritten, new rows inserted, nothing is deleted
//! (renames in Taiga show up as updates, not deletions).
//!
//! Cache scope = `connection_id`, derived from the Taiga base URL via
//! UUID v5 against `NAMESPACE_URL`. Same URL → same id across restarts.
//! The auth-session store ([`crate::auth_session_store`]) shares the
//! same scope key so a single SQLite file holds both per-connection
//! caches.

use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::client::{TaigaMember, TaigaStatus};
use crate::entity::{taiga_project_member, taiga_project_status, taiga_project_tag};

/// Stable cache-scope id derived from the Taiga base URL.
pub fn scope_id_for_url(url: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, url.as_bytes())
}

// ---------------------------------------------------------------------------
// Project-meta cache
// ---------------------------------------------------------------------------

pub async fn load_statuses(
    db: &DatabaseConnection,
    scope_id: Uuid,
    project_id: u64,
    item_type: &str,
) -> Result<Vec<TaigaStatus>, String> {
    taiga_project_status::Entity::find()
        .filter(taiga_project_status::Column::ConnectionId.eq(scope_id))
        .filter(taiga_project_status::Column::ProjectId.eq(project_id as i64))
        .filter(taiga_project_status::Column::ItemType.eq(item_type))
        .all(db)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|m| TaigaStatus {
                    id: m.status_id as u64,
                    name: m.name,
                })
                .collect()
        })
        .map_err(|e| format!("load_statuses: {e}"))
}

pub async fn merge_statuses(
    db: &DatabaseConnection,
    scope_id: Uuid,
    project_id: u64,
    item_type: &str,
    statuses: &[TaigaStatus],
) -> Result<(), String> {
    let txn = db.begin().await.map_err(|e| format!("begin txn: {e}"))?;
    let existing: std::collections::HashMap<i64, taiga_project_status::Model> =
        taiga_project_status::Entity::find()
            .filter(taiga_project_status::Column::ConnectionId.eq(scope_id))
            .filter(taiga_project_status::Column::ProjectId.eq(project_id as i64))
            .filter(taiga_project_status::Column::ItemType.eq(item_type))
            .all(&txn)
            .await
            .map_err(|e| format!("load existing statuses: {e}"))?
            .into_iter()
            .map(|m| (m.status_id, m))
            .collect();

    for s in statuses {
        let sid = s.id as i64;
        if let Some(model) = existing.get(&sid) {
            if model.name == s.name {
                continue;
            }
            let mut am = model.clone().into_active_model();
            am.name = Set(s.name.clone());
            am.update(&txn)
                .await
                .map_err(|e| format!("update status: {e}"))?;
        } else {
            let am = taiga_project_status::ActiveModel {
                connection_id: Set(scope_id),
                project_id: Set(project_id as i64),
                item_type: Set(item_type.to_string()),
                status_id: Set(sid),
                name: Set(s.name.clone()),
                ..taiga_project_status::ActiveModel::new()
            };
            am.insert(&txn)
                .await
                .map_err(|e| format!("insert status: {e}"))?;
        }
    }
    txn.commit().await.map_err(|e| format!("commit txn: {e}"))
}

pub async fn load_members(
    db: &DatabaseConnection,
    scope_id: Uuid,
    project_id: u64,
) -> Result<Vec<TaigaMember>, String> {
    taiga_project_member::Entity::find()
        .filter(taiga_project_member::Column::ConnectionId.eq(scope_id))
        .filter(taiga_project_member::Column::ProjectId.eq(project_id as i64))
        .all(db)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|m| TaigaMember {
                    id: m.user_id as u64,
                    username: m.username,
                    full_name: m.full_name,
                })
                .collect()
        })
        .map_err(|e| format!("load_members: {e}"))
}

pub async fn merge_members(
    db: &DatabaseConnection,
    scope_id: Uuid,
    project_id: u64,
    members: &[TaigaMember],
) -> Result<(), String> {
    let txn = db.begin().await.map_err(|e| format!("begin txn: {e}"))?;
    let existing: std::collections::HashMap<i64, taiga_project_member::Model> =
        taiga_project_member::Entity::find()
            .filter(taiga_project_member::Column::ConnectionId.eq(scope_id))
            .filter(taiga_project_member::Column::ProjectId.eq(project_id as i64))
            .all(&txn)
            .await
            .map_err(|e| format!("load existing members: {e}"))?
            .into_iter()
            .map(|m| (m.user_id, m))
            .collect();

    for u in members {
        let uid = u.id as i64;
        if let Some(model) = existing.get(&uid) {
            if model.username == u.username && model.full_name == u.full_name {
                continue;
            }
            let mut am = model.clone().into_active_model();
            am.username = Set(u.username.clone());
            am.full_name = Set(u.full_name.clone());
            am.update(&txn)
                .await
                .map_err(|e| format!("update member: {e}"))?;
        } else {
            let am = taiga_project_member::ActiveModel {
                connection_id: Set(scope_id),
                project_id: Set(project_id as i64),
                user_id: Set(uid),
                username: Set(u.username.clone()),
                full_name: Set(u.full_name.clone()),
                ..taiga_project_member::ActiveModel::new()
            };
            am.insert(&txn)
                .await
                .map_err(|e| format!("insert member: {e}"))?;
        }
    }
    txn.commit().await.map_err(|e| format!("commit txn: {e}"))
}

pub async fn load_tags(
    db: &DatabaseConnection,
    scope_id: Uuid,
    project_id: u64,
) -> Result<Vec<String>, String> {
    taiga_project_tag::Entity::find()
        .filter(taiga_project_tag::Column::ConnectionId.eq(scope_id))
        .filter(taiga_project_tag::Column::ProjectId.eq(project_id as i64))
        .all(db)
        .await
        .map(|rows| rows.into_iter().map(|m| m.name).collect())
        .map_err(|e| format!("load_tags: {e}"))
}

pub async fn merge_tags(
    db: &DatabaseConnection,
    scope_id: Uuid,
    project_id: u64,
    tags: &[String],
) -> Result<(), String> {
    let txn = db.begin().await.map_err(|e| format!("begin txn: {e}"))?;
    let existing: std::collections::HashSet<String> = taiga_project_tag::Entity::find()
        .filter(taiga_project_tag::Column::ConnectionId.eq(scope_id))
        .filter(taiga_project_tag::Column::ProjectId.eq(project_id as i64))
        .all(&txn)
        .await
        .map_err(|e| format!("load existing tags: {e}"))?
        .into_iter()
        .map(|m| m.name)
        .collect();

    for name in tags {
        if name.is_empty() || existing.contains(name) {
            continue;
        }
        let am = taiga_project_tag::ActiveModel {
            connection_id: Set(scope_id),
            project_id: Set(project_id as i64),
            name: Set(name.clone()),
            ..taiga_project_tag::ActiveModel::new()
        };
        am.insert(&txn)
            .await
            .map_err(|e| format!("insert tag: {e}"))?;
    }
    txn.commit().await.map_err(|e| format!("commit txn: {e}"))
}
