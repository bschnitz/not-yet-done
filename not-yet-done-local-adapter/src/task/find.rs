//! The tasks adapter's `find` action: a tree-wide search whose every
//! parameter is optional and narrows the hit set.
//!
//! The search runs in memory over the [`ForestSnapshot`] the adapter already
//! holds, so tags, the notes marker and the parent chain are at hand without
//! another query, and instants are compared as instants rather than as the
//! text the store keeps them in. Notes are read from disk only when a `text`
//! condition asks for them (`in=notes|both`).
//!
//! The parameters are declared on the action ([`find_params`]), so `help`
//! lists them and a frontend rejects a typo instead of ignoring it.

use std::collections::HashSet;

use chrono::{DateTime, Local, NaiveTime, TimeZone, Utc};
use not_yet_done_content::{ActionArgs, NodeHit, ParamSpec};
use not_yet_done_task_core::entity::task;
use uuid::Uuid;

use super::{ForestSnapshot, TaskRow};

/// Hit cap when the invocation names none. The adapter picks it, not the
/// caller: the `truncated` flag on the answer says when it bit.
pub(super) const DEFAULT_LIMIT: i64 = 200;

/// The parameters `find` declares, in the order `help` prints them.
pub(super) fn find_params() -> Vec<ParamSpec> {
    vec![
        ParamSpec::text(
            "text",
            "words that must all occur (case-insensitive); `id:<uuid>` picks one task. \
             Falls back to the action's free text (TUI prompt, CLI --text)",
        ),
        ParamSpec::text("in", "where `text` is looked for: description (default), notes, both"),
        ParamSpec::list(
            "status",
            "any of todo, in_progress, done, cancelled; `open` = todo + in_progress",
        ),
        ParamSpec::list("tag", "tag names the task must all carry"),
        ParamSpec::list("not_tag", "tag names the task must carry none of"),
        ParamSpec::text("due", "due within this day (or at this instant), e.g. today, friday"),
        ParamSpec::text("due_from", "due at or after this date, e.g. today, 2026-10-01"),
        ParamSpec::text(
            "due_to",
            "due at or before this date; a plain day counts whole, e.g. today, end of next week",
        ),
        ParamSpec::bool("has_due", "true: only tasks with a due date; false: only without"),
        ParamSpec::bool("overdue", "true: open tasks whose due date has passed"),
        ParamSpec::text("not_before_from", "not_before at or after this date"),
        ParamSpec::text("not_before_to", "not_before at or before this date (whole day)"),
        ParamSpec::bool("actionable", "true: not_before unset or reached; false: not yet startable"),
        ParamSpec::text("created_from", "created at or after this date"),
        ParamSpec::text("created_to", "created at or before this date (whole day)"),
        ParamSpec::text("updated_from", "updated at or after this date"),
        ParamSpec::text("updated_to", "updated at or before this date (whole day)"),
        ParamSpec::text("tracked_from", "last tracked at or after this date"),
        ParamSpec::text("tracked_to", "last tracked at or before this date (whole day)"),
        ParamSpec::text(
            "under",
            "only this subtree: a task id, or a task's description (case-insensitive) — \
             a `/`-separated description path pins it, e.g. Work/Acme",
        ),
        ParamSpec::int("priority_min", "priority at least this"),
        ParamSpec::int("priority_max", "priority at most this"),
        ParamSpec::bool("recurring", "true: only recurring tasks; false: only one-off tasks"),
        ParamSpec::bool("has_notes", "true: only tasks with a notes file; false: only without"),
        ParamSpec::bool("include_deleted", "also deleted tasks (default false)"),
        ParamSpec::text(
            "sort",
            "tree (default), due, not_before, created, updated, tracked, priority, description; \
             a leading `-` reverses",
        ),
        ParamSpec::int("limit", "maximum number of hits (default 200)"),
    ]
}

/// Where a `text` condition looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextScope {
    Description,
    Notes,
    Both,
}

/// An inclusive instant range; either end may be open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Range {
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

impl Range {
    fn is_open(&self) -> bool {
        self.from.is_none() && self.to.is_none()
    }

    /// Whether `at` lies in the range. A task without the date never does
    /// once the range constrains anything.
    fn admits(&self, at: Option<DateTime<Utc>>) -> bool {
        if self.is_open() {
            return true;
        }
        let Some(at) = at else { return false };
        self.from.is_none_or(|f| at >= f) && self.to.is_none_or(|t| at <= t)
    }
}

/// How the hits are ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortKey {
    Tree,
    Due,
    NotBefore,
    Created,
    Updated,
    Tracked,
    Priority,
    Description,
}

/// The parsed invocation: every field `None`/empty means "no condition".
#[derive(Debug)]
pub(super) struct FindQuery {
    id: Option<Uuid>,
    words: Vec<String>,
    scope: TextScope,
    statuses: Option<HashSet<String>>,
    tags: Vec<String>,
    not_tags: Vec<String>,
    due: Range,
    has_due: Option<bool>,
    overdue: Option<bool>,
    not_before: Range,
    actionable: Option<bool>,
    created: Range,
    updated: Range,
    tracked: Range,
    under: Option<String>,
    priority_min: Option<i64>,
    priority_max: Option<i64>,
    recurring: Option<bool>,
    has_notes: Option<bool>,
    include_deleted: bool,
    sort: SortKey,
    descending: bool,
    pub(super) limit: usize,
    now: DateTime<Utc>,
}

impl FindQuery {
    /// Read the invocation. `text` is the action's free text (`ctx.text`),
    /// used when no `text` argument is given — the TUI prompt sends it that
    /// way. `now` anchors relative dates and `overdue`/`actionable`.
    pub(super) fn parse<Tz>(
        args: &ActionArgs,
        text: Option<&str>,
        now: DateTime<Tz>,
    ) -> Result<Self, String>
    where
        Tz: TimeZone,
        Tz::Offset: Copy,
    {
        let raw_text = args.text("text").or_else(|| text.map(str::to_string));
        let raw_text = raw_text.as_deref().map(str::trim).unwrap_or("");
        let (id, words) = match raw_text.strip_prefix("id:") {
            Some(rest) => (
                Some(
                    Uuid::parse_str(rest.trim())
                        .map_err(|_| format!("`id:` expects a task uuid, got `{}`", rest.trim()))?,
                ),
                Vec::new(),
            ),
            None => (
                None,
                raw_text.split_whitespace().map(str::to_lowercase).collect(),
            ),
        };

        let scope = match args.text("in").as_deref().map(str::trim) {
            None | Some("") | Some("description") => TextScope::Description,
            Some("notes") => TextScope::Notes,
            Some("both") => TextScope::Both,
            Some(other) => {
                return Err(format!("in: expected description, notes or both, got `{other}`"));
            }
        };

        let statuses = match items(args, "status") {
            list if list.is_empty() => None,
            list => Some(parse_statuses(&list)?),
        };

        let due_day = match args.text("due") {
            Some(p) => {
                let at = resolve(&p, now, "due")?;
                Range {
                    from: Some(at),
                    to: Some(whole_day_end(at, now)),
                }
            }
            None => Range::default(),
        };
        let due = intersect(due_day, range(args, "due", now)?);

        let (sort, descending) = parse_sort(args.text("sort").as_deref())?;
        let limit = match args.int("limit") {
            Some(n) if n < 1 => return Err(format!("limit must be at least 1, got {n}")),
            Some(n) => n as usize,
            None => DEFAULT_LIMIT as usize,
        };

        Ok(Self {
            id,
            words,
            scope,
            statuses,
            tags: lowered(items(args, "tag")),
            not_tags: lowered(items(args, "not_tag")),
            due,
            has_due: args.bool("has_due"),
            overdue: args.bool("overdue"),
            not_before: range(args, "not_before", now)?,
            actionable: args.bool("actionable"),
            created: range(args, "created", now)?,
            updated: range(args, "updated", now)?,
            tracked: range(args, "tracked", now)?,
            under: args
                .text("under")
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty()),
            priority_min: args.int("priority_min"),
            priority_max: args.int("priority_max"),
            recurring: args.bool("recurring"),
            has_notes: args.bool("has_notes"),
            include_deleted: args.bool("include_deleted").unwrap_or(false),
            sort,
            descending,
            limit,
            now: now.with_timezone(&Utc),
        })
    }

    /// Whether the query needs the notes files read.
    fn reads_notes(&self) -> bool {
        !self.words.is_empty() && self.scope != TextScope::Description
    }

    /// Every condition that only needs the row itself.
    fn admits_row(&self, row: &TaskRow) -> bool {
        let t = &row.task;
        if t.deleted && !self.include_deleted {
            return false;
        }
        if self.id.is_some_and(|id| id != t.id) {
            return false;
        }
        if let Some(statuses) = &self.statuses
            && !statuses.contains(status_key(&t.status))
        {
            return false;
        }
        if !self.tags.is_empty() || !self.not_tags.is_empty() {
            let names: HashSet<String> = row
                .tag_names
                .split(", ")
                .filter(|s| !s.is_empty())
                .map(str::to_lowercase)
                .collect();
            if !self.tags.iter().all(|tag| names.contains(tag))
                || self.not_tags.iter().any(|tag| names.contains(tag))
            {
                return false;
            }
        }
        if self.has_due.is_some_and(|want| want != t.due.is_some()) || !self.due.admits(t.due) {
            return false;
        }
        if let Some(want) = self.overdue {
            let is = is_open(&t.status) && t.due.is_some_and(|d| d < self.now);
            if want != is {
                return false;
            }
        }
        if !self.not_before.admits(t.not_before) {
            return false;
        }
        if let Some(want) = self.actionable {
            let is = t.not_before.is_none_or(|nb| nb <= self.now);
            if want != is {
                return false;
            }
        }
        if !self.created.admits(Some(t.created_at))
            || !self.updated.admits(Some(t.updated_at))
            || !self.tracked.admits(t.last_tracked_at)
        {
            return false;
        }
        let priority = i64::from(t.priority);
        if self.priority_min.is_some_and(|m| priority < m)
            || self.priority_max.is_some_and(|m| priority > m)
        {
            return false;
        }
        if self.recurring.is_some_and(|want| want != t.recur.is_some())
            || self.has_notes.is_some_and(|want| want != row.has_notes)
        {
            return false;
        }
        true
    }

    /// The `text` condition. `notes` is the task's notes text, read only
    /// when [`Self::reads_notes`] asked for it.
    fn admits_text(&self, row: &TaskRow, notes: Option<&str>) -> bool {
        if self.words.is_empty() {
            return true;
        }
        let description = row.task.description.to_lowercase();
        let notes = notes.map(str::to_lowercase).unwrap_or_default();
        self.words.iter().all(|w| match self.scope {
            TextScope::Description => description.contains(w),
            TextScope::Notes => notes.contains(w),
            TextScope::Both => description.contains(w) || notes.contains(w),
        })
    }
}

/// Run `query` over `snapshot`. `matches` is the pane's active query as a
/// set of matching ids (`None` = no query): a hit outside it is one the
/// tree never shows, so offering it would strand the caller's expand walk.
/// Returns the hits and whether the limit cut them short.
pub(super) fn run(
    snapshot: &ForestSnapshot,
    query: &FindQuery,
    matches: Option<&HashSet<Uuid>>,
) -> Result<(Vec<NodeHit>, bool), String> {
    let subtree = match &query.under {
        Some(under) => Some(resolve_under(snapshot, under)?),
        None => None,
    };
    let all_tasks: Vec<task::Model> = if query.reads_notes() {
        snapshot.by_id.values().map(|r| r.task.clone()).collect()
    } else {
        Vec::new()
    };

    let mut rows: Vec<&TaskRow> = snapshot
        .by_id
        .iter()
        .filter(|(id, _)| matches.is_none_or(|m| m.contains(id)))
        .filter(|(id, _)| subtree.as_ref().is_none_or(|s| s.contains(id)))
        .map(|(_, row)| row)
        .filter(|row| query.admits_row(row))
        .filter(|row| {
            let notes = (query.reads_notes() && row.has_notes)
                .then(|| crate::notes::read_notes(&row.task, &all_tasks));
            query.admits_text(row, notes.as_deref())
        })
        .collect();

    sort_rows(snapshot, &mut rows, query.sort, query.descending);
    let truncated = rows.len() > query.limit;
    rows.truncate(query.limit);
    let hits = rows.into_iter().map(|row| hit(snapshot, row)).collect();
    Ok((hits, truncated))
}

/// One hit: the id path the TUI expands along, plus the cells a listing
/// shows next to the label.
fn hit(snapshot: &ForestSnapshot, row: &TaskRow) -> NodeHit {
    let t = &row.task;
    let id = t.id;
    let parents: Vec<String> = snapshot
        .path_to(id)
        .iter()
        .rev()
        .skip(1)
        .rev()
        .filter_map(|p| Uuid::parse_str(p).ok())
        .filter_map(|p| snapshot.by_id.get(&p))
        .map(|r| r.task.description.clone())
        .collect();
    let mut columns = vec![
        ("status".to_string(), status_key(&t.status).to_string()),
        ("due".to_string(), local_stamp(t.due)),
        ("not_before".to_string(), local_stamp(t.not_before)),
        ("tags".to_string(), row.tag_names.clone()),
        ("parents".to_string(), parents.join(" / ")),
    ];
    if t.deleted {
        columns.push(("deleted".to_string(), "true".to_string()));
    }
    NodeHit {
        path: snapshot.path_to(id),
        label: t.description.clone(),
        group_key: String::new(),
        columns,
    }
}

fn sort_rows(snapshot: &ForestSnapshot, rows: &mut [&TaskRow], key: SortKey, descending: bool) {
    // Tree order is the default and the tie-breaker of every other key:
    // the root-first chain of creation-ordered sibling positions.
    let tree_key = |row: &TaskRow| tree_position(snapshot, row.task.id);
    rows.sort_by_cached_key(|row| tree_key(row));
    if key == SortKey::Tree {
        if descending {
            rows.reverse();
        }
        return;
    }
    // A stable sort on the secondary key keeps tree order among equals.
    // Tasks lacking the date sort last in either direction.
    rows.sort_by(|a, b| {
        let (a, b) = (&a.task, &b.task);
        let date = |x: Option<DateTime<Utc>>, y: Option<DateTime<Utc>>| match (x, y) {
            (Some(x), Some(y)) if descending => y.cmp(&x),
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        let plain = |o: std::cmp::Ordering| if descending { o.reverse() } else { o };
        match key {
            SortKey::Tree => std::cmp::Ordering::Equal,
            SortKey::Due => date(a.due, b.due),
            SortKey::NotBefore => date(a.not_before, b.not_before),
            SortKey::Created => date(Some(a.created_at), Some(b.created_at)),
            SortKey::Updated => date(Some(a.updated_at), Some(b.updated_at)),
            SortKey::Tracked => date(a.last_tracked_at, b.last_tracked_at),
            SortKey::Priority => plain(a.priority.cmp(&b.priority)),
            SortKey::Description => {
                plain(a.description.to_lowercase().cmp(&b.description.to_lowercase()))
            }
        }
    });
}

/// The positions of `id` and its ancestors among their siblings, root
/// first — sorting by it yields the order the tree renders in.
fn tree_position(snapshot: &ForestSnapshot, id: Uuid) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut cur = Some(id);
    while let Some(c) = cur {
        let parent = snapshot.by_id.get(&c).and_then(|r| r.parent);
        let pos = snapshot
            .children
            .get(&parent)
            .and_then(|sibs| sibs.iter().position(|s| *s == c))
            .unwrap_or(usize::MAX);
        chain.push(pos);
        cur = parent;
    }
    chain.reverse();
    chain
}

/// The ids of the subtree `under` names, the named task included.
///
/// `under` is a task id, or a description: a single one matches any task
/// with that description, a `/`-separated path matches tasks whose
/// description chain ends in it. Ambiguity is fine — the union of all
/// matching subtrees is searched — but naming nothing is an error, so a
/// typo does not read as "no hits".
fn resolve_under(snapshot: &ForestSnapshot, under: &str) -> Result<HashSet<Uuid>, String> {
    let roots: Vec<Uuid> = match Uuid::parse_str(under) {
        Ok(id) if snapshot.by_id.contains_key(&id) => vec![id],
        Ok(id) => return Err(format!("under: no task with id {id}")),
        Err(_) => {
            let wanted: Vec<String> = under
                .split('/')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            snapshot
                .by_id
                .keys()
                .copied()
                .filter(|id| description_chain_ends_with(snapshot, *id, &wanted))
                .collect()
        }
    };
    if roots.is_empty() {
        return Err(format!("under: no task named `{under}`"));
    }
    let mut out = HashSet::new();
    let mut stack = roots;
    while let Some(id) = stack.pop() {
        if out.insert(id)
            && let Some(kids) = snapshot.children.get(&Some(id))
        {
            stack.extend(kids.iter().copied());
        }
    }
    Ok(out)
}

/// Whether the descriptions from some ancestor down to `id` equal `wanted`
/// (lower-cased, root-most first).
fn description_chain_ends_with(snapshot: &ForestSnapshot, id: Uuid, wanted: &[String]) -> bool {
    let mut cur = Some(id);
    for want in wanted.iter().rev() {
        let Some(row) = cur.and_then(|c| snapshot.by_id.get(&c)) else {
            return false;
        };
        if row.task.description.trim().to_lowercase() != *want {
            return false;
        }
        cur = row.parent;
    }
    !wanted.is_empty()
}

fn parse_statuses(list: &[String]) -> Result<HashSet<String>, String> {
    let mut out = HashSet::new();
    for s in list {
        match s.trim() {
            "open" => {
                out.insert("todo".to_string());
                out.insert("in_progress".to_string());
            }
            k @ ("todo" | "in_progress" | "done" | "cancelled") => {
                out.insert(k.to_string());
            }
            other => {
                return Err(format!(
                    "status: expected todo, in_progress, done, cancelled or open, got `{other}`"
                ));
            }
        }
    }
    Ok(out)
}

fn parse_sort(raw: Option<&str>) -> Result<(SortKey, bool), String> {
    let raw = raw.map(str::trim).unwrap_or("");
    let (descending, name) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    let key = match name {
        "" | "tree" => SortKey::Tree,
        "due" => SortKey::Due,
        "not_before" => SortKey::NotBefore,
        "created" => SortKey::Created,
        "updated" => SortKey::Updated,
        "tracked" => SortKey::Tracked,
        "priority" => SortKey::Priority,
        "description" => SortKey::Description,
        other => {
            return Err(format!(
                "sort: expected tree, due, not_before, created, updated, tracked, priority \
                 or description, got `{other}`"
            ));
        }
    };
    Ok((key, descending))
}

/// The `<prefix>_from` / `<prefix>_to` pair as a range. A `_to` that names
/// a plain day (resolves to local midnight) covers that whole day.
fn range<Tz>(args: &ActionArgs, prefix: &str, now: DateTime<Tz>) -> Result<Range, String>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let from_key = format!("{prefix}_from");
    let to_key = format!("{prefix}_to");
    let from = match args.text(&from_key) {
        Some(p) => Some(resolve(&p, now, &from_key)?),
        None => None,
    };
    let to = match args.text(&to_key) {
        Some(p) => Some(whole_day_end(resolve(&p, now, &to_key)?, now)),
        None => None,
    };
    Ok(Range { from, to })
}

fn intersect(a: Range, b: Range) -> Range {
    Range {
        from: a.from.max(b.from),
        to: match (a.to, b.to) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        },
    }
}

fn resolve<Tz>(phrase: &str, now: DateTime<Tz>, key: &str) -> Result<DateTime<Utc>, String>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    natural_date::resolve_datetime(phrase, now)
        .ok_or_else(|| format!("{key}: `{phrase}` is not a date"))
}

/// An instant at local midnight stands for its whole day as an upper
/// bound: `due_to=today` keeps a task due tonight. Any other instant is
/// meant as written.
fn whole_day_end<Tz>(at: DateTime<Utc>, now: DateTime<Tz>) -> DateTime<Utc>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let local = at.with_timezone(&now.timezone());
    if local.time() != NaiveTime::MIN {
        return at;
    }
    let next = local.date_naive().succ_opt().unwrap_or(local.date_naive());
    now.timezone()
        .from_local_datetime(&next.and_time(NaiveTime::MIN))
        .earliest()
        .map(|d| d.with_timezone(&Utc) - chrono::Duration::nanoseconds(1))
        .unwrap_or(at)
}

/// A list argument's items. Each item is also split on commas: the CLI
/// can only hand over `--arg status=todo,done` as one text, a YAML
/// binding may send a real list — both mean the same.
fn items(args: &ActionArgs, key: &str) -> Vec<String> {
    args.list(key)
        .unwrap_or_default()
        .iter()
        .flat_map(|item| item.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn lowered(list: Vec<String>) -> Vec<String> {
    list.into_iter().map(|s| s.to_lowercase()).collect()
}

fn status_key(status: &task::TaskStatus) -> &'static str {
    match status {
        task::TaskStatus::Todo => "todo",
        task::TaskStatus::InProgress => "in_progress",
        task::TaskStatus::Done => "done",
        task::TaskStatus::Cancelled => "cancelled",
    }
}

fn is_open(status: &task::TaskStatus) -> bool {
    matches!(status, task::TaskStatus::Todo | task::TaskStatus::InProgress)
}

/// `2026-10-05 14:00` in local time, or empty.
fn local_stamp(at: Option<DateTime<Utc>>) -> String {
    at.map(|d| d.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}
