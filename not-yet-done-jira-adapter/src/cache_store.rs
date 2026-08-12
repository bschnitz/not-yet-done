//! Persistent backing for the in-memory `JiraCache`.
//!
//! Reuses the `jira_label` / `jira_user` entities. Each cache scope is
//! identified by a UUID derived from the Jira URL (UUID v5 against
//! `NAMESPACE_URL`), so multiple Jira instances coexist cleanly.
//!
//! The cache is **merge-only**: nothing is ever removed for the active
//! scope. Existing rows get their display fields overwritten on a re-merge
//! (display names change, e.g. after a user rename); brand-new rows get
//! inserted. `cleanup_orphans` is the one exception — it sweeps rows whose
//! `connection_id` does not match the active scope, wiping legacy data
//! left behind by removed code paths.

use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, Set, TransactionTrait,
};
use uuid::Uuid;

use super::client::JiraUser;
use crate::entity::{jira_label, jira_user, workflow_edge};

/// Stable cache-scope id derived from the Jira URL. Same URL → same id
/// across restarts and across processes.
pub fn scope_id_for_url(url: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, url.as_bytes())
}

pub async fn load_labels(db: &DatabaseConnection, scope_id: Uuid) -> Result<Vec<String>, String> {
    jira_label::Entity::find()
        .filter(jira_label::Column::ConnectionId.eq(scope_id))
        .all(db)
        .await
        .map(|rows| rows.into_iter().map(|m| m.name).collect())
        .map_err(|e| format!("load_labels: {e}"))
}

pub async fn load_users(db: &DatabaseConnection, scope_id: Uuid) -> Result<Vec<JiraUser>, String> {
    jira_user::Entity::find()
        .filter(jira_user::Column::ConnectionId.eq(scope_id))
        .all(db)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|m| JiraUser {
                    name: m.username,
                    display_name: m.display_name,
                    email_address: m.email,
                })
                .collect()
        })
        .map_err(|e| format!("load_users: {e}"))
}

/// Insert any label not already present for `scope_id`. Existing rows are
/// untouched (labels are bare strings — there's nothing to update).
pub async fn merge_labels(
    db: &DatabaseConnection,
    scope_id: Uuid,
    labels: &[String],
) -> Result<(), String> {
    if labels.is_empty() {
        return Ok(());
    }
    let txn = db.begin().await.map_err(|e| format!("begin txn: {e}"))?;

    let existing: std::collections::HashSet<String> = jira_label::Entity::find()
        .filter(jira_label::Column::ConnectionId.eq(scope_id))
        .all(&txn)
        .await
        .map_err(|e| format!("load existing labels: {e}"))?
        .into_iter()
        .map(|m| m.name)
        .collect();

    for name in labels {
        if name.is_empty() || existing.contains(name) {
            continue;
        }
        let model = jira_label::ActiveModel {
            connection_id: Set(scope_id),
            name: Set(name.clone()),
            ..jira_label::ActiveModel::new()
        };
        model
            .insert(&txn)
            .await
            .map_err(|e| format!("insert label: {e}"))?;
    }

    txn.commit().await.map_err(|e| format!("commit txn: {e}"))
}

/// Merge users by `(scope_id, username)`. New rows are inserted; existing
/// rows have their `display_name` / `normalized` / `email` overwritten with
/// the new values. Users are never deleted.
pub async fn merge_users(
    db: &DatabaseConnection,
    scope_id: Uuid,
    users: &[JiraUser],
) -> Result<(), String> {
    if users.is_empty() {
        return Ok(());
    }
    let txn = db.begin().await.map_err(|e| format!("begin txn: {e}"))?;

    let existing: std::collections::HashMap<String, jira_user::Model> = jira_user::Entity::find()
        .filter(jira_user::Column::ConnectionId.eq(scope_id))
        .all(&txn)
        .await
        .map_err(|e| format!("load existing users: {e}"))?
        .into_iter()
        .map(|m| (m.username.clone(), m))
        .collect();

    for u in users {
        if u.name.is_empty() {
            continue;
        }
        let normalized = slug::slugify(&u.display_name);
        if let Some(model) = existing.get(&u.name) {
            let mut am: jira_user::ActiveModel = model.clone().into_active_model();
            am.display_name = Set(u.display_name.clone());
            am.normalized = Set(normalized);
            am.email = Set(u.email_address.clone());
            am.update(&txn)
                .await
                .map_err(|e| format!("update user: {e}"))?;
        } else {
            let model = jira_user::ActiveModel {
                connection_id: Set(scope_id),
                username: Set(u.name.clone()),
                display_name: Set(u.display_name.clone()),
                normalized: Set(normalized),
                email: Set(u.email_address.clone()),
                ..jira_user::ActiveModel::new()
            };
            model
                .insert(&txn)
                .await
                .map_err(|e| format!("insert user: {e}"))?;
        }
    }

    txn.commit().await.map_err(|e| format!("commit txn: {e}"))
}

/// Sweep cached rows whose `connection_id` is not the active scope.
/// One-shot cleanup of legacy rows written by code paths that have since
/// been removed (e.g. the old `run_sync` keyed by `jira_connection.id`
/// instead of by `scope_id_for_url`). Returns `(users_deleted, labels_deleted)`.
pub async fn cleanup_orphans(
    db: &DatabaseConnection,
    scope_id: Uuid,
) -> Result<(usize, usize), String> {
    let txn = db.begin().await.map_err(|e| format!("begin txn: {e}"))?;

    let users_deleted = jira_user::Entity::delete_many()
        .filter(jira_user::Column::ConnectionId.ne(scope_id))
        .exec(&txn)
        .await
        .map_err(|e| format!("delete orphan users: {e}"))?
        .rows_affected as usize;

    let labels_deleted = jira_label::Entity::delete_many()
        .filter(jira_label::Column::ConnectionId.ne(scope_id))
        .exec(&txn)
        .await
        .map_err(|e| format!("delete orphan labels: {e}"))?
        .rows_affected as usize;

    txn.commit().await.map_err(|e| format!("commit txn: {e}"))?;
    Ok((users_deleted, labels_deleted))
}

/// One observed workflow edge: `from_status -> transition -> to_status`
/// inside a project + issue-type. The transition picker writes one of
/// these per available transition every time it runs against an issue;
/// the planner then enumerates multi-hop paths over the accumulated set.
#[derive(Debug, Clone)]
pub struct WorkflowEdgeRow {
    pub project_key: String,
    pub issuetype_id: String,
    pub from_status_id: String,
    pub from_status_name: String,
    pub transition_id: String,
    pub transition_name: String,
    pub to_status_id: String,
    pub to_status_name: String,
    pub required_fields: Vec<String>,
}

/// Upsert workflow edges for `scope_id`. Existing rows are overwritten
/// (transition rename, new required-field state, refreshed `last_seen`);
/// brand-new rows get inserted. Rows with any empty id field are
/// skipped — workflow recording only stores fully-keyed observations.
pub async fn merge_workflow_edges(
    db: &DatabaseConnection,
    scope_id: Uuid,
    edges: &[WorkflowEdgeRow],
    now_unix: i64,
) -> Result<(), String> {
    if edges.is_empty() {
        return Ok(());
    }
    let txn = db.begin().await.map_err(|e| format!("begin txn: {e}"))?;
    for e in edges {
        if e.project_key.is_empty()
            || e.issuetype_id.is_empty()
            || e.from_status_id.is_empty()
            || e.transition_id.is_empty()
            || e.to_status_id.is_empty()
        {
            continue;
        }
        let required_json = serde_json::to_string(&e.required_fields)
            .map_err(|err| format!("serialize required_fields: {err}"))?;
        let existing = workflow_edge::Entity::find_by_id((
            scope_id,
            e.project_key.clone(),
            e.issuetype_id.clone(),
            e.from_status_id.clone(),
            e.transition_id.clone(),
        ))
        .one(&txn)
        .await
        .map_err(|err| format!("find workflow_edge: {err}"))?;
        if let Some(model) = existing {
            let mut am: workflow_edge::ActiveModel = model.into_active_model();
            am.from_status_name = Set(e.from_status_name.clone());
            am.transition_name = Set(e.transition_name.clone());
            am.to_status_id = Set(e.to_status_id.clone());
            am.to_status_name = Set(e.to_status_name.clone());
            am.required_fields = Set(required_json);
            am.last_seen_unix = Set(now_unix);
            am.update(&txn)
                .await
                .map_err(|err| format!("update workflow_edge: {err}"))?;
        } else {
            let model = workflow_edge::ActiveModel {
                connection_id: Set(scope_id),
                project_key: Set(e.project_key.clone()),
                issuetype_id: Set(e.issuetype_id.clone()),
                from_status_id: Set(e.from_status_id.clone()),
                transition_id: Set(e.transition_id.clone()),
                from_status_name: Set(e.from_status_name.clone()),
                transition_name: Set(e.transition_name.clone()),
                to_status_id: Set(e.to_status_id.clone()),
                to_status_name: Set(e.to_status_name.clone()),
                required_fields: Set(required_json),
                last_seen_unix: Set(now_unix),
            };
            model
                .insert(&txn)
                .await
                .map_err(|err| format!("insert workflow_edge: {err}"))?;
        }
    }
    txn.commit().await.map_err(|e| format!("commit txn: {e}"))
}

/// Load every recorded edge in the (`scope_id`, project, issuetype)
/// triple. Used by the planner to enumerate reachable status paths.
pub async fn load_workflow_edges(
    db: &DatabaseConnection,
    scope_id: Uuid,
    project_key: &str,
    issuetype_id: &str,
) -> Result<Vec<WorkflowEdgeRow>, String> {
    let rows = workflow_edge::Entity::find()
        .filter(workflow_edge::Column::ConnectionId.eq(scope_id))
        .filter(workflow_edge::Column::ProjectKey.eq(project_key))
        .filter(workflow_edge::Column::IssuetypeId.eq(issuetype_id))
        .all(db)
        .await
        .map_err(|e| format!("load workflow_edges: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|m| WorkflowEdgeRow {
            project_key: m.project_key,
            issuetype_id: m.issuetype_id,
            from_status_id: m.from_status_id,
            from_status_name: m.from_status_name,
            transition_id: m.transition_id,
            transition_name: m.transition_name,
            to_status_id: m.to_status_id,
            to_status_name: m.to_status_name,
            required_fields: serde_json::from_str(&m.required_fields).unwrap_or_default(),
        })
        .collect())
}
