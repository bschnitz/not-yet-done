//! The persistent event store: every successful fetch of a connection is
//! written here, so the events outlive the process that fetched them.
//!
//! That decouples *syncing* (network, possibly an interactive sign-in) from
//! *reading*: a one-shot `nyd adapter calendar ls` serves what the last sync
//! stored instead of an empty cold cache, and a connection configured as
//! `fetch: on_demand` is only ever touched by an explicit sync — every other
//! reader works off the store.
//!
//! Default backing store: a private SQLite file under
//! `~/.local/share/not_yet_done/calendar-cache.sqlite`; the adapter YAML's
//! `db.url` points it elsewhere (any SeaORM URL, e.g. a Postgres server). Rows
//! are scoped by adapter instance id, so instances can share one database.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, Condition, Database, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, TransactionTrait,
};

use not_yet_done_calendar_core::{CalEvent, ShowAs, TimeRange};

use crate::adapter::MergedEvent;
use crate::entity::{calendar_event, calendar_sync};

/// Rows per `INSERT` — keeps a large fetch under SQLite's bound-variable cap.
const INSERT_CHUNK: usize = 500;

pub(crate) fn default_sqlite_url() -> Result<String, String> {
    let dir: PathBuf = dirs::data_local_dir()
        .ok_or_else(|| "cannot resolve XDG data-local dir".to_string())?
        .join("not_yet_done");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join("calendar-cache.sqlite");
    Ok(format!("sqlite://{}?mode=rwc", path.display()))
}

/// Open a SeaORM connection and sync this crate's entity schema (scoped to
/// `entity::*`, so a shared database keeps its other tables).
pub(crate) async fn connect(db_url: &str) -> Result<DatabaseConnection, DbErr> {
    let db = Database::connect(db_url).await?;
    db.get_schema_registry("not_yet_done_calendar_adapter::entity::*")
        .sync(&db)
        .await?;
    Ok(db)
}

/// What the store knows about one connection's syncing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyncState {
    pub(crate) connection: String,
    /// Bumped on every store of this connection's events.
    pub(crate) revision: i64,
    pub(crate) synced_at: Option<DateTime<Utc>>,
    pub(crate) attempted_at: DateTime<Utc>,
    pub(crate) last_error: Option<String>,
}

/// One adapter instance's view of the store.
pub(crate) struct EventStore {
    db: Arc<DatabaseConnection>,
    scope: String,
}

impl EventStore {
    pub(crate) fn new(db: Arc<DatabaseConnection>, scope: impl Into<String>) -> Self {
        Self {
            db,
            scope: scope.into(),
        }
    }

    /// Store a successful fetch of `connection` that covered `range`: its
    /// stored events overlapping `range` are replaced by `events`, events that
    /// ended before `keep_from` (or before `range`, whichever is earlier) are
    /// dropped so the store does not grow without bound, and the connection's
    /// sync row records the success. One transaction — a concurrent reader
    /// sees the old slice or the new one, never a half-written mix.
    pub(crate) async fn save(
        &self,
        connection: &str,
        range: &TimeRange,
        keep_from: DateTime<Utc>,
        events: &[MergedEvent],
        now: DateTime<Utc>,
    ) -> Result<i64, DbErr> {
        let txn = self.db.begin().await?;
        let prune_before = keep_from.min(range.start).timestamp();
        calendar_event::Entity::delete_many()
            .filter(calendar_event::Column::Scope.eq(&self.scope))
            .filter(calendar_event::Column::Connection.eq(connection))
            .filter(
                Condition::any()
                    .add(
                        Condition::all()
                            .add(calendar_event::Column::StartUnix.lt(range.end.timestamp()))
                            .add(calendar_event::Column::EndUnix.gte(range.start.timestamp())),
                    )
                    .add(calendar_event::Column::EndUnix.lt(prune_before)),
            )
            .exec(&txn)
            .await?;

        for chunk in events.chunks(INSERT_CHUNK) {
            let rows = chunk.iter().map(|m| self.to_row(connection, m));
            calendar_event::Entity::insert_many(rows)
                .on_conflict(
                    OnConflict::columns([
                        calendar_event::Column::Scope,
                        calendar_event::Column::GlobalId,
                    ])
                    .update_columns([
                        calendar_event::Column::Connection,
                        calendar_event::Column::Uid,
                        calendar_event::Column::Calendar,
                        calendar_event::Column::Title,
                        calendar_event::Column::StartUnix,
                        calendar_event::Column::EndUnix,
                        calendar_event::Column::AllDay,
                        calendar_event::Column::Location,
                        calendar_event::Column::Organizer,
                        calendar_event::Column::ShowAs,
                        calendar_event::Column::Body,
                        calendar_event::Column::Url,
                    ])
                    .to_owned(),
                )
                .exec(&txn)
                .await?;
        }

        let revision = self.previous_revision(&txn, connection).await? + 1;
        self.upsert_sync(
            &txn,
            calendar_sync::ActiveModel {
                scope: Set(self.scope.clone()),
                connection: Set(connection.to_string()),
                revision: Set(revision),
                synced_at_unix: Set(Some(now.timestamp())),
                range_start_unix: Set(Some(range.start.timestamp())),
                range_end_unix: Set(Some(range.end.timestamp())),
                attempted_at_unix: Set(now.timestamp()),
                last_error: Set(None),
            },
            &[
                calendar_sync::Column::Revision,
                calendar_sync::Column::SyncedAtUnix,
                calendar_sync::Column::RangeStartUnix,
                calendar_sync::Column::RangeEndUnix,
                calendar_sync::Column::AttemptedAtUnix,
                calendar_sync::Column::LastError,
            ],
        )
        .await?;
        txn.commit().await?;
        Ok(revision)
    }

    /// Record a failed attempt for `connection`. The stored events and the
    /// last success stay untouched — a reader keeps serving them and can tell
    /// from [`SyncState::last_error`] that they are no longer being refreshed.
    pub(crate) async fn record_failure(
        &self,
        connection: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), DbErr> {
        let txn = self.db.begin().await?;
        let revision = self.previous_revision(&txn, connection).await?;
        self.upsert_sync(
            &txn,
            calendar_sync::ActiveModel {
                scope: Set(self.scope.clone()),
                connection: Set(connection.to_string()),
                revision: Set(revision),
                synced_at_unix: Set(None),
                range_start_unix: Set(None),
                range_end_unix: Set(None),
                attempted_at_unix: Set(now.timestamp()),
                last_error: Set(Some(error.to_string())),
            },
            &[
                calendar_sync::Column::AttemptedAtUnix,
                calendar_sync::Column::LastError,
            ],
        )
        .await?;
        txn.commit().await
    }

    /// Every connection's sync state for this scope, by connection id.
    pub(crate) async fn sync_states(&self) -> Result<Vec<SyncState>, DbErr> {
        let rows = calendar_sync::Entity::find()
            .filter(calendar_sync::Column::Scope.eq(&self.scope))
            .all(self.db.as_ref())
            .await?;
        let mut states: Vec<SyncState> = rows
            .into_iter()
            .map(|r| SyncState {
                connection: r.connection,
                revision: r.revision,
                synced_at: r.synced_at_unix.map(from_unix),
                attempted_at: from_unix(r.attempted_at_unix),
                last_error: r.last_error,
            })
            .collect();
        states.sort_by(|a, b| a.connection.cmp(&b.connection));
        Ok(states)
    }

    /// The stored events of one connection.
    pub(crate) async fn load(&self, connection: &str) -> Result<Vec<MergedEvent>, DbErr> {
        let rows = calendar_event::Entity::find()
            .filter(calendar_event::Column::Scope.eq(&self.scope))
            .filter(calendar_event::Column::Connection.eq(connection))
            .all(self.db.as_ref())
            .await?;
        Ok(rows.into_iter().map(from_row).collect())
    }

    /// Whether any event of `connection` is stored.
    pub(crate) async fn has_events(&self, connection: &str) -> Result<bool, DbErr> {
        Ok(calendar_event::Entity::find()
            .filter(calendar_event::Column::Scope.eq(&self.scope))
            .filter(calendar_event::Column::Connection.eq(connection))
            .one(self.db.as_ref())
            .await?
            .is_some())
    }

    async fn previous_revision(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        connection: &str,
    ) -> Result<i64, DbErr> {
        Ok(
            calendar_sync::Entity::find_by_id((self.scope.clone(), connection.to_string()))
                .one(txn)
                .await?
                .map(|r| r.revision)
                .unwrap_or(0),
        )
    }

    async fn upsert_sync(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        row: calendar_sync::ActiveModel,
        update: &[calendar_sync::Column],
    ) -> Result<(), DbErr> {
        calendar_sync::Entity::insert(row)
            .on_conflict(
                OnConflict::columns([calendar_sync::Column::Scope, calendar_sync::Column::Connection])
                    .update_columns(update.iter().copied())
                    .to_owned(),
            )
            .exec(txn)
            .await?;
        Ok(())
    }

    fn to_row(&self, connection: &str, m: &MergedEvent) -> calendar_event::ActiveModel {
        let e = &m.event;
        calendar_event::ActiveModel {
            scope: Set(self.scope.clone()),
            global_id: Set(m.global_id.clone()),
            connection: Set(connection.to_string()),
            uid: Set(e.uid.clone()),
            calendar: Set(e.calendar.clone()),
            title: Set(e.title.clone()),
            start_unix: Set(e.start.timestamp()),
            end_unix: Set(e.end.timestamp()),
            all_day: Set(e.all_day),
            location: Set(e.location.clone()),
            organizer: Set(e.organizer.clone()),
            show_as: Set(e.show_as.as_str().to_string()),
            body: Set(e.body.clone()),
            url: Set(e.url.clone()),
        }
    }
}

fn from_unix(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap_or_default()
}

fn from_row(r: calendar_event::Model) -> MergedEvent {
    MergedEvent {
        global_id: r.global_id,
        account: r.calendar.clone(),
        event: CalEvent {
            uid: r.uid,
            calendar: r.calendar,
            title: r.title,
            start: from_unix(r.start_unix),
            end: from_unix(r.end_unix),
            all_day: r.all_day,
            location: r.location,
            organizer: r.organizer,
            show_as: ShowAs::from_token(&r.show_as),
            body: r.body,
            url: r.url,
        },
    }
}
