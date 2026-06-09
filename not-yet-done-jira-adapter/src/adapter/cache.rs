//! In-memory + persistent cache for one Jira scope (labels + users), plus
//! the API-fetch wrappers that mirror every result into the cache.
//!
//! The cache is merge-only: nothing is ever evicted at runtime. Labels
//! accumulate as a sorted set; users are keyed by Jira-username.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::cache_store;
use crate::client::{JiraClient, JiraComment, JiraIssueDetail, JiraUser};

/// Merge-only cache for one Jira scope. Labels accumulate as a sorted set;
/// users are keyed by Jira-username and re-mergers overwrite display fields.
/// Nothing is ever evicted at runtime — the cache only grows.
///
/// Holds the persistence handle (`db` + `scope_id`) so any callsite with a
/// `&Mutex<JiraCache>` can write through via the [`persist_users`] /
/// [`persist_labels`] helpers without threading those plumbing values
/// separately through every node type.
pub(super) struct JiraCache {
    labels: BTreeSet<String>,
    users: HashMap<String, JiraUser>,
    db: Option<Arc<DatabaseConnection>>,
    scope_id: Uuid,
}

impl JiraCache {
    pub(super) fn new(db: Option<Arc<DatabaseConnection>>, scope_id: Uuid) -> Self {
        Self {
            labels: BTreeSet::new(),
            users: HashMap::new(),
            db,
            scope_id,
        }
    }

    pub(super) fn labels_snapshot(&self) -> Vec<String> {
        self.labels.iter().cloned().collect()
    }

    pub(super) fn users_snapshot(&self) -> Vec<JiraUser> {
        self.users.values().cloned().collect()
    }

    pub(super) fn user_by_name(&self, name: &str) -> Option<JiraUser> {
        self.users.get(name).cloned()
    }

    /// Insert any label not already present. Returns the labels that were
    /// actually new (callers persist just those).
    pub(super) fn merge_labels<I: IntoIterator<Item = String>>(&mut self, labels: I) -> Vec<String> {
        let mut new = Vec::new();
        for l in labels {
            if l.is_empty() {
                continue;
            }
            if self.labels.insert(l.clone()) {
                new.push(l);
            }
        }
        new
    }

    /// Upsert users by `name`. Existing entries get their `display_name`
    /// and `email_address` overwritten; new entries get inserted. Returns
    /// the entries that were genuinely new or changed (callers persist
    /// just those).
    pub(super) fn merge_users<I: IntoIterator<Item = JiraUser>>(&mut self, users: I) -> Vec<JiraUser> {
        let mut changed = Vec::new();
        for u in users {
            if u.name.is_empty() {
                continue;
            }
            let differs = match self.users.get(&u.name) {
                Some(existing) => {
                    existing.display_name != u.display_name
                        || existing.email_address != u.email_address
                }
                None => true,
            };
            if differs {
                self.users.insert(u.name.clone(), u.clone());
                changed.push(u);
            }
        }
        changed
    }
}

/// Merge `users` into the in-memory cache, then write any genuinely
/// new/changed entries to the persistent store. Brief lock window (sync
/// merge), DB I/O happens after the lock is dropped.
pub(super) async fn persist_users(cache: &Mutex<JiraCache>, users: Vec<JiraUser>) {
    let (changed, db, scope_id) = {
        let mut c = cache.lock().unwrap();
        let changed = c.merge_users(users);
        (changed, c.db.clone(), c.scope_id)
    };
    if changed.is_empty() {
        return;
    }
    if let Some(db) = db {
        if let Err(e) = cache_store::merge_users(&db, scope_id, &changed).await {
            eprintln!("nyd: persisting Jira users cache failed: {e}");
        }
    }
}

/// Pure projection of an issue's user references (assignee, reporter,
/// creator) into `JiraUser` records suitable for cache merging. Empty keys
/// are skipped; missing display names fall back to the key.
fn issue_users(detail: &JiraIssueDetail) -> Vec<JiraUser> {
    let mut users = Vec::new();
    for (key, display) in [
        (&detail.assignee_key, &detail.assignee),
        (&detail.reporter_key, &detail.reporter),
        (&detail.creator_key, &detail.creator),
    ] {
        if key.is_empty() {
            continue;
        }
        let display_name = if display.is_empty() { key.clone() } else { display.clone() };
        users.push(JiraUser {
            name: key.clone(),
            display_name,
            email_address: None,
        });
    }
    users
}

/// Harvest every user reference embedded in an issue (assignee, reporter,
/// creator) and its labels into the cache. Used as a side-effect of every
/// `get_issue` so the cache passively grows with browsing — no separate
/// "load all users" step required.
async fn merge_issue_into_cache(cache: &Mutex<JiraCache>, detail: &JiraIssueDetail) {
    let users = issue_users(detail);
    if !users.is_empty() {
        persist_users(cache, users).await;
    }
    if !detail.labels.is_empty() {
        persist_labels(cache, detail.labels.clone()).await;
    }
}

/// Harvest every comment author into the cache. Companion to
/// [`merge_issue_into_cache`].
async fn merge_comments_into_cache(cache: &Mutex<JiraCache>, comments: &[JiraComment]) {
    let users: Vec<JiraUser> = comments
        .iter()
        .filter(|c| !c.author_key.is_empty())
        .map(|c| {
            let display_name = if c.author.is_empty() {
                c.author_key.clone()
            } else {
                c.author.clone()
            };
            JiraUser {
                name: c.author_key.clone(),
                display_name,
                email_address: None,
            }
        })
        .collect();
    if !users.is_empty() {
        persist_users(cache, users).await;
    }
}

/// Wrapper around `JiraClient::get_issue` that mirrors every user/label
/// reference into the cache before returning the issue.
pub(super) async fn fetch_issue(
    client: &JiraClient,
    cache: &Mutex<JiraCache>,
    key: &str,
) -> std::result::Result<JiraIssueDetail, String> {
    let detail = client.get_issue(key).await?;
    merge_issue_into_cache(cache, &detail).await;
    Ok(detail)
}

/// Wrapper around `JiraClient::get_comments` that mirrors every author
/// into the cache before returning the comment list.
pub(super) async fn fetch_comments(
    client: &JiraClient,
    cache: &Mutex<JiraCache>,
    key: &str,
) -> std::result::Result<Vec<JiraComment>, String> {
    let comments = client.get_comments(key).await?;
    merge_comments_into_cache(cache, &comments).await;
    Ok(comments)
}

/// Extract the KEY token from each `[~KEY]` occurrence in `text`.
/// Stops at `]` and skips empty / multi-line keys (which can't be valid).
fn extract_mention_keys(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        loop {
            let idx = rest.find("[~")?;
            let after = &rest[idx + 2..];
            let end = after.find(']')?;
            let key = &after[..end];
            rest = &after[end + 1..];
            if !key.is_empty() && !key.contains(['\n', '[', '~']) {
                return Some(key);
            }
        }
    })
}

/// Walk every `[~KEY]` mention in `texts`, look up unknown KEYs via the
/// Jira API, and merge them into the cache. Lookup failures are silent
/// (the KEY then stays verbatim in the rendered output, matching the
/// pre-existing behavior for unresolved mentions).
pub(super) async fn resolve_unknown_mentions(
    client: &JiraClient,
    cache: &Mutex<JiraCache>,
    texts: &[&str],
) {
    let mut all_keys: BTreeSet<String> = BTreeSet::new();
    for text in texts {
        for key in extract_mention_keys(text) {
            all_keys.insert(key.to_string());
        }
    }
    if all_keys.is_empty() {
        return;
    }

    let unknown: Vec<String> = {
        let c = cache.lock().unwrap();
        all_keys
            .into_iter()
            .filter(|k| c.user_by_name(k).is_none())
            .collect()
    };

    for key in unknown {
        if let Ok(user) = client.get_user_by_name(&key).await {
            persist_users(cache, vec![user]).await;
        }
    }
}

/// Snapshot `(db, scope_id)` out of the cache for callers that need to
/// hit the persistent store directly (e.g. workflow-edge recording,
/// which has its own row shape and doesn't merge into in-memory state).
/// Returns `None` when the cache is configured without a backing DB.
pub(super) fn db_handle(
    cache: &Mutex<JiraCache>,
) -> Option<(Arc<DatabaseConnection>, Uuid)> {
    let c = cache.lock().unwrap();
    c.db.clone().map(|db| (db, c.scope_id))
}

/// Companion to [`persist_users`] for label strings.
pub(super) async fn persist_labels(cache: &Mutex<JiraCache>, labels: Vec<String>) {
    let (new, db, scope_id) = {
        let mut c = cache.lock().unwrap();
        let new = c.merge_labels(labels);
        (new, c.db.clone(), c.scope_id)
    };
    if new.is_empty() {
        return;
    }
    if let Some(db) = db {
        if let Err(e) = cache_store::merge_labels(&db, scope_id, &new).await {
            eprintln!("nyd: persisting Jira labels cache failed: {e}");
        }
    }
}

/// Synchronously hydrate the in-memory cache from the persisted store and
/// sweep orphan rows left behind by removed code paths. Uses
/// `block_in_place` to bridge sync construction with async DB I/O —
/// requires a multi-threaded runtime. On `current_thread` runtimes (or
/// outside Tokio entirely) the call is a no-op and the cache stays empty;
/// it then accumulates entries the first time issues / users are loaded.
pub(super) fn hydrate_from_db(
    cache: &Arc<Mutex<JiraCache>>,
    db: &Arc<DatabaseConnection>,
    scope_id: Uuid,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else { return; };
    if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
        return;
    }

    let (labels, users, orphans) = tokio::task::block_in_place(|| {
        handle.block_on(async {
            let labels = cache_store::load_labels(db, scope_id).await.unwrap_or_default();
            let users = cache_store::load_users(db, scope_id).await.unwrap_or_default();
            let orphans = cache_store::cleanup_orphans(db, scope_id).await.ok();
            (labels, users, orphans)
        })
    });

    if let Some((u, l)) = orphans {
        if u + l > 0 {
            eprintln!(
                "nyd: cleaned up {u} orphan jira_user and {l} orphan jira_label row(s) \
                 from previous schema"
            );
        }
    }

    let mut c = cache.lock().unwrap();
    c.merge_labels(labels);
    c.merge_users(users);
}
