//! `find_filtered` on trackings: the `has_live_successor` predicate over the
//! move chain (`predecessor_id`).
//!
//! A move (split, edit) soft-deletes a tracking and inserts successors that
//! point back at it, so the fixture builds chains by hand: insert, then
//! soft-delete everything that was moved on.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use sea_orm::{ConnectionTrait, Database, DbBackend, Schema};
use shaku::HasComponent;
use uuid::Uuid;

use not_yet_done_task_core::entity::{task, tracking};
use not_yet_done_task_core::filter::query_filter;
use not_yet_done_task_core::module::TaskDomainModule;
use not_yet_done_task_core::repository::{
    ProjectRepositoryImpl, ProjectRepositoryImplParameters, TagRepositoryImpl,
    TagRepositoryImplParameters, TaskRepository, TaskRepositoryImpl, TaskRepositoryImplParameters,
    TrackingRepository, TrackingRepositoryImpl, TrackingRepositoryImplParameters,
};

struct Fixture {
    tasks: Arc<dyn TaskRepository>,
    trackings: Arc<dyn TrackingRepository>,
    task_id: Uuid,
}

async fn setup() -> Fixture {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("in-memory SQLite");
    let schema = Schema::new(DbBackend::Sqlite);
    db.execute(&schema.create_table_from_entity(task::Entity))
        .await
        .expect("create task table");
    db.execute(&schema.create_table_from_entity(tracking::Entity))
        .await
        .expect("create tracking table");

    let module = TaskDomainModule::builder()
        .with_component_parameters::<TaskRepositoryImpl>(TaskRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .with_component_parameters::<ProjectRepositoryImpl>(ProjectRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .with_component_parameters::<TagRepositoryImpl>(TagRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .with_component_parameters::<TrackingRepositoryImpl>(TrackingRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .build();

    let tasks: Arc<dyn TaskRepository> = module.resolve();
    let trackings: Arc<dyn TrackingRepository> = module.resolve();
    let task_id = tasks
        .insert("Work".into(), None, None, None)
        .await
        .expect("insert task")
        .id;
    Fixture {
        tasks,
        trackings,
        task_id,
    }
}

impl Fixture {
    /// Insert a tracking, optionally as successor of `predecessor`.
    async fn track(&self, hour: u32, predecessor: Option<Uuid>) -> Uuid {
        let at = Utc.with_ymd_and_hms(2026, 9, 1, hour, 0, 0).unwrap();
        self.trackings
            .insert(self.task_id, at, predecessor)
            .await
            .expect("insert tracking")
            .id
    }

    async fn delete(&self, id: Uuid) {
        self.trackings
            .soft_delete_keeping_times(id)
            .await
            .expect("soft delete");
    }

    async fn query(&self, body: &str) -> HashSet<Uuid> {
        let parsed = query_filter::parse(body).expect("parse query");
        self.trackings
            .find_filtered(&parsed.expr)
            .await
            .expect("find_filtered")
            .into_iter()
            .map(|t| t.id)
            .collect()
    }
}

/// Every chain shape the predicate has to tell apart.
struct Chains {
    /// Never moved, alive.
    plain: Uuid,
    /// Never moved, deleted.
    plain_deleted: Uuid,
    /// Moved twice, the last move alive: `a ← b ← c`.
    twice: [Uuid; 3],
    /// Moved twice, the last move deleted too.
    twice_dead: [Uuid; 3],
    /// Split, one of the two parts still alive.
    split: [Uuid; 3],
    /// Split, both parts deleted.
    split_dead: [Uuid; 3],
}

async fn seed(f: &Fixture) -> Chains {
    let plain = f.track(1, None).await;

    let plain_deleted = f.track(2, None).await;
    f.delete(plain_deleted).await;

    let a = f.track(3, None).await;
    let b = f.track(3, Some(a)).await;
    let c = f.track(3, Some(b)).await;
    f.delete(a).await;
    f.delete(b).await;

    let d = f.track(4, None).await;
    let e = f.track(4, Some(d)).await;
    let g = f.track(4, Some(e)).await;
    for id in [d, e, g] {
        f.delete(id).await;
    }

    let s = f.track(5, None).await;
    let s1 = f.track(5, Some(s)).await;
    let s2 = f.track(6, Some(s)).await;
    f.delete(s).await;
    f.delete(s1).await;

    let t = f.track(7, None).await;
    let t1 = f.track(7, Some(t)).await;
    let t2 = f.track(8, Some(t)).await;
    for id in [t, t1, t2] {
        f.delete(id).await;
    }

    Chains {
        plain,
        plain_deleted,
        twice: [a, b, c],
        twice_dead: [d, e, g],
        split: [s, s1, s2],
        split_dead: [t, t1, t2],
    }
}

#[tokio::test]
async fn has_live_successor_marks_every_tracking_above_a_live_one() {
    let f = setup().await;
    let c = seed(&f).await;

    let got = f.query("query: [has_live_successor, true]\n").await;
    let want: HashSet<Uuid> = [c.twice[0], c.twice[1], c.split[0]].into();
    assert_eq!(got, want);
}

#[tokio::test]
async fn has_live_successor_false_is_the_complement() {
    let f = setup().await;
    let c = seed(&f).await;

    let got = f.query("query: [has_live_successor, false]\n").await;
    let mut want: HashSet<Uuid> =
        [c.plain, c.plain_deleted, c.twice[2], c.split[1], c.split[2]].into();
    want.extend(c.twice_dead);
    want.extend(c.split_dead);
    assert_eq!(got, want);
}

/// The query the predicate exists for: every tracking once, without its
/// moves — the original of a live chain, plus what was never moved.
#[tokio::test]
async fn originals_without_their_moves() {
    let f = setup().await;
    let c = seed(&f).await;

    let got = f
        .query(
            "query:\n  and:\n    - [predecessor_id, is_null]\n    - or:\n        - [deleted, =, false]\n        - [has_live_successor, true]\n",
        )
        .await;
    let want: HashSet<Uuid> = [c.plain, c.twice[0], c.split[0]].into();
    assert_eq!(got, want);
}

#[tokio::test]
async fn has_live_successor_needs_a_bool() {
    let f = setup().await;
    let parsed = query_filter::parse("query: [has_live_successor, yes please]\n").unwrap();
    let err = f.trackings.find_filtered(&parsed.expr).await.unwrap_err();
    assert!(err.to_string().contains("true or false"), "{err}");
}

/// Tasks have no move chain: the predicate is a load error there, not an
/// empty result.
#[tokio::test]
async fn task_queries_reject_has_live_successor() {
    let f = setup().await;
    let parsed = query_filter::parse("query: [has_live_successor, true]\n").unwrap();
    let err = f.tasks.find_filtered(&parsed.expr).await.unwrap_err();
    assert!(err.to_string().contains("has_live_successor"), "{err}");
}
