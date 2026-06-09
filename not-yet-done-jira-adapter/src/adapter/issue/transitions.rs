//! Workflow-transition support that lives on the issue node:
//! recording observed edges into the cache and enumerating multi-hop
//! paths over the accumulated edges.
//!
//! The recorder runs as a fire-and-forget side effect of
//! `picker_options("transition")` — failures are logged but never
//! propagated, because a degraded cache must not stop the user from
//! performing the direct transition.
//!
//! Path enumeration is a depth-bounded BFS over the recorded edges:
//! simple paths only (no status revisits inside a single chain), and
//! self-loops are skipped in traversal even when they exist as edges.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use not_yet_done_content::{ActionOption, ActionOutcome, Result};

use crate::cache_store::{self, WorkflowEdgeRow};
use crate::client::{JiraIssueDetail, JiraTransition};

use super::JiraIssueNode;
use super::super::cache::{db_handle, fetch_issue};
use super::super::util::other_err;

/// Hop limit for multi-hop enumeration. Picked empirically — Jira
/// workflows in practice are 3–5 statuses wide, so anything beyond
/// four hops is more confusing in the picker than it is useful.
const MAX_PATH_DEPTH: usize = 4;

/// One enumerated chain through the recorded workflow graph. Every
/// vector has the same length as the number of hops; `from_status_*`
/// is the starting status (not stored on each hop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowPath {
    pub from_status_id: String,
    pub from_status_name: String,
    pub transition_ids: Vec<String>,
    pub transition_names: Vec<String>,
    pub status_ids: Vec<String>,
    pub status_names: Vec<String>,
    /// Names of fields any hop in the chain reports as required. The
    /// chain executor uses this to refuse paths it can't auto-fill.
    pub required_fields: Vec<String>,
}

impl WorkflowPath {
    /// Comma-separated transition ids, in execution order.
    pub(crate) fn value(&self) -> String {
        self.transition_ids.join(",")
    }
}

/// Enumerate every simple path of length `1..=max_depth` reachable
/// from `start_status_id` over the given edges. Self-loops are recorded
/// as edges (they exist in real workflows) but excluded from path
/// traversal — they never produce a useful chain. Output is sorted by
/// (hop count asc, terminal status-name alpha) so direct transitions
/// surface before multi-hop chains in the picker.
pub(crate) fn enumerate_paths(
    edges: &[WorkflowEdgeRow],
    start_status_id: &str,
    max_depth: usize,
) -> Vec<WorkflowPath> {
    if max_depth == 0 || start_status_id.is_empty() {
        return Vec::new();
    }
    let mut adjacency: HashMap<&str, Vec<&WorkflowEdgeRow>> = HashMap::new();
    let mut start_name = "";
    for e in edges {
        if e.from_status_id == e.to_status_id {
            continue;
        }
        adjacency.entry(e.from_status_id.as_str()).or_default().push(e);
        if e.from_status_id == start_status_id {
            start_name = e.from_status_name.as_str();
        }
    }

    let mut out: Vec<WorkflowPath> = Vec::new();
    let mut visited_status: Vec<String> = vec![start_status_id.to_string()];
    let mut path: Vec<&WorkflowEdgeRow> = Vec::new();
    dfs(
        &adjacency,
        start_status_id,
        max_depth,
        &mut visited_status,
        &mut path,
        &mut out,
        start_name,
    );

    out.sort_by(|a, b| {
        a.transition_ids
            .len()
            .cmp(&b.transition_ids.len())
            .then_with(|| {
                a.status_names
                    .last()
                    .map(String::as_str)
                    .unwrap_or("")
                    .cmp(b.status_names.last().map(String::as_str).unwrap_or(""))
            })
    });
    out
}

fn dfs<'a>(
    adjacency: &HashMap<&'a str, Vec<&'a WorkflowEdgeRow>>,
    current: &str,
    remaining: usize,
    visited_status: &mut Vec<String>,
    path: &mut Vec<&'a WorkflowEdgeRow>,
    out: &mut Vec<WorkflowPath>,
    start_name: &str,
) {
    if remaining == 0 {
        return;
    }
    let Some(neighbors) = adjacency.get(current) else { return; };
    for edge in neighbors {
        if visited_status.iter().any(|s| s == &edge.to_status_id) {
            continue;
        }
        path.push(edge);
        visited_status.push(edge.to_status_id.clone());

        out.push(materialize_path(path, visited_status[0].as_str(), start_name));

        dfs(
            adjacency,
            edge.to_status_id.as_str(),
            remaining - 1,
            visited_status,
            path,
            out,
            start_name,
        );

        visited_status.pop();
        path.pop();
    }
}

fn materialize_path(
    edges: &[&WorkflowEdgeRow],
    start_id: &str,
    start_name: &str,
) -> WorkflowPath {
    let mut transition_ids = Vec::with_capacity(edges.len());
    let mut transition_names = Vec::with_capacity(edges.len());
    let mut status_ids = Vec::with_capacity(edges.len());
    let mut status_names = Vec::with_capacity(edges.len());
    let mut required_fields: Vec<String> = Vec::new();
    for e in edges {
        transition_ids.push(e.transition_id.clone());
        transition_names.push(e.transition_name.clone());
        status_ids.push(e.to_status_id.clone());
        status_names.push(e.to_status_name.clone());
        for f in &e.required_fields {
            if !required_fields.iter().any(|x| x == f) {
                required_fields.push(f.clone());
            }
        }
    }
    let resolved_start_name = edges
        .first()
        .map(|e| e.from_status_name.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(start_name);
    WorkflowPath {
        from_status_id: start_id.to_string(),
        from_status_name: resolved_start_name.to_string(),
        transition_ids,
        transition_names,
        status_ids,
        status_names,
        required_fields,
    }
}

impl JiraIssueNode {
    /// Project key prefix of `PROJ-123` — the segment before the first
    /// `-`. Returns `""` for malformed keys; the recorder skips those.
    fn project_key(&self) -> &str {
        match self.key.find('-') {
            Some(idx) => &self.key[..idx],
            None => "",
        }
    }

    /// Build `WorkflowEdgeRow`s from the just-observed transition list
    /// against the current issue detail. Returns an empty Vec when any
    /// composite-key piece is missing (which causes the caller to
    /// silently skip recording and fall back to direct-only options).
    fn build_observed_edges(
        &self,
        detail: &JiraIssueDetail,
        transitions: &[JiraTransition],
    ) -> Vec<WorkflowEdgeRow> {
        let project = self.project_key();
        if project.is_empty()
            || detail.status_id.is_empty()
            || detail.issue_type_id.is_empty()
        {
            return Vec::new();
        }
        transitions
            .iter()
            .map(|t| WorkflowEdgeRow {
                project_key: project.to_string(),
                issuetype_id: detail.issue_type_id.clone(),
                from_status_id: detail.status_id.clone(),
                from_status_name: detail.status.clone(),
                transition_id: t.id.clone(),
                transition_name: t.name.clone(),
                to_status_id: t.to_status_id.clone(),
                to_status_name: t.to_status.clone(),
                required_fields: t.required_fields.clone(),
            })
            .collect()
    }

    /// Persist `edges` into the workflow-edge cache. Quietly bails when
    /// the cache has no backing DB or when the write fails — recording
    /// is a snowball side effect, never a hard requirement.
    async fn persist_observed_edges(&self, edges: &[WorkflowEdgeRow]) {
        if edges.is_empty() {
            return;
        }
        let Some((db, scope_id)) = db_handle(&self.cache) else { return; };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Err(e) = cache_store::merge_workflow_edges(&db, scope_id, edges, now).await {
            eprintln!("nyd: persisting jira workflow_edges failed: {e}");
        }
    }

    /// Build the transition picker's options for the current issue.
    /// Records the observed direct edges into the cache as a side
    /// effect, then enumerates simple paths up to `MAX_PATH_DEPTH` over
    /// the union of (cached edges, just-observed edges).
    ///
    /// Label format: only the terminal status name, with a trailing `*`
    /// when the path is multi-hop (so the user sees *which* status they
    /// land in, without the picker drowning in intermediate steps). Paths
    /// are deduped by terminal status; since enumeration sorts by hop
    /// count ascending, the shortest route to a given status wins — a
    /// direct edge collapses any multi-hop chain to the same target.
    pub(super) async fn transition_options(
        &self,
        detail: &JiraIssueDetail,
        transitions: &[JiraTransition],
    ) -> Vec<ActionOption> {
        let observed = self.build_observed_edges(detail, transitions);
        self.persist_observed_edges(&observed).await;

        let project = self.project_key();
        let cached = if !project.is_empty() && !detail.issue_type_id.is_empty() {
            if let Some((db, scope_id)) = db_handle(&self.cache) {
                cache_store::load_workflow_edges(&db, scope_id, project, &detail.issue_type_id)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let combined = merge_edges(cached, observed);
        let paths = enumerate_paths(&combined, &detail.status_id, MAX_PATH_DEPTH);

        if paths.is_empty() {
            // No status_id, empty cache, or unknown start — fall back to the
            // raw direct transitions so the picker is never empty when the
            // server returned at least one option.
            return fallback_options(transitions);
        }

        paths_to_options(&paths)
    }

    /// Execute a comma-separated chain of transition IDs sequentially.
    /// On success: refreshes `self.detail` and returns
    /// `ActionOutcome::Done` with the final status name.
    ///
    /// On failure mid-chain: refreshes `self.detail` (so the user sees
    /// the partial-success state) and surfaces a `ContentError::Other`
    /// indicating which step failed and why. Earlier successful hops
    /// are preserved server-side — they aren't rolled back, mirroring
    /// the agreed semantics: "bei required fields mit fehler abbrechen,
    /// aber denk an den refresh".
    pub(super) async fn execute_transition_chain(
        &mut self,
        chain: &str,
    ) -> Result<ActionOutcome> {
        let ids: Vec<String> = chain
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        if ids.is_empty() {
            return Ok(ActionOutcome::NoChanges);
        }
        let total = ids.len();
        for (idx, tid) in ids.iter().enumerate() {
            if let Err(e) = self.client.do_transition(&self.key, tid).await {
                let new_status = self.refresh_detail_status().await;
                return Err(other_err(format!(
                    "Chain stopped at step {}/{} (now in {}): {}",
                    idx + 1,
                    total,
                    new_status,
                    e,
                )));
            }
        }
        let final_status = self.refresh_detail_status().await;
        Ok(ActionOutcome::Done {
            message: Some(format!("{} → {}", self.key, final_status)),
        })
    }

    /// Re-fetch the issue detail and replace the cached copy. Returns
    /// the latest status name, or "unknown" if the re-fetch failed —
    /// callers use this in user-facing chain-result messages.
    async fn refresh_detail_status(&mut self) -> String {
        match fetch_issue(&self.client, &self.cache, &self.key)
            .await
            .map_err(other_err)
        {
            Ok(detail) => {
                let status = detail.status.clone();
                self.replace_detail(detail);
                status
            }
            Err(_) => "unknown".to_string(),
        }
    }
}

/// Picker labels from enumerated paths: terminal status name only, with
/// a `*` suffix for multi-hop chains. Deduped by terminal status — since
/// paths are pre-sorted by hop count ascending, the shortest route to a
/// given status always wins (a direct edge shadows any chain to the
/// same target).
fn paths_to_options(paths: &[WorkflowPath]) -> Vec<ActionOption> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut options = Vec::with_capacity(paths.len());
    for p in paths {
        let Some(terminal) = p.status_names.last() else { continue };
        if !seen.insert(terminal.clone()) {
            continue;
        }
        let label = if p.transition_ids.len() > 1 {
            format!("{}*", terminal)
        } else {
            terminal.clone()
        };
        options.push(ActionOption { label, value: p.value() });
    }
    options
}

/// Fallback when path enumeration yields nothing (no status_id, empty
/// cache, or unknown start). Uses the raw server-returned transitions
/// (all direct, so no `*` suffix) and dedupes by terminal status — first
/// transition to a status wins.
fn fallback_options(transitions: &[JiraTransition]) -> Vec<ActionOption> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut options = Vec::with_capacity(transitions.len());
    for t in transitions {
        let terminal = if t.to_status.is_empty() { &t.name } else { &t.to_status };
        if !seen.insert(terminal.clone()) {
            continue;
        }
        options.push(ActionOption { label: terminal.clone(), value: t.id.clone() });
    }
    options
}

/// Merge two edge lists, keyed by the composite PK
/// (`project_key`, `issuetype_id`, `from_status_id`, `transition_id`).
/// Values in `overrides` win — used so the just-observed snapshot wins
/// over the (potentially stale, e.g. renamed transition) cached row.
fn merge_edges(
    base: Vec<WorkflowEdgeRow>,
    overrides: Vec<WorkflowEdgeRow>,
) -> Vec<WorkflowEdgeRow> {
    let key = |e: &WorkflowEdgeRow| {
        (
            e.project_key.clone(),
            e.issuetype_id.clone(),
            e.from_status_id.clone(),
            e.transition_id.clone(),
        )
    };
    let mut map: HashMap<_, WorkflowEdgeRow> = HashMap::new();
    for e in base {
        map.insert(key(&e), e);
    }
    for e in overrides {
        map.insert(key(&e), e);
    }
    map.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: (&str, &str), to: (&str, &str), tid: &str, tname: &str) -> WorkflowEdgeRow {
        WorkflowEdgeRow {
            project_key: "PROJ".into(),
            issuetype_id: "10001".into(),
            from_status_id: from.0.into(),
            from_status_name: from.1.into(),
            transition_id: tid.into(),
            transition_name: tname.into(),
            to_status_id: to.0.into(),
            to_status_name: to.1.into(),
            required_fields: Vec::new(),
        }
    }

    #[test]
    fn enumerate_returns_direct_then_two_hop() {
        let edges = vec![
            edge(("1", "To Do"), ("2", "In Progress"), "21", "Start"),
            edge(("2", "In Progress"), ("3", "Done"), "31", "Finish"),
        ];
        let paths = enumerate_paths(&edges, "1", 4);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].status_names, vec!["In Progress".to_string()]);
        assert_eq!(paths[0].value(), "21");
        assert_eq!(
            paths[1].status_names,
            vec!["In Progress".to_string(), "Done".to_string()]
        );
        assert_eq!(paths[1].value(), "21,31");
        assert_eq!(paths[1].from_status_name, "To Do");
    }

    #[test]
    fn self_loops_are_not_traversed() {
        let edges = vec![
            edge(("1", "To Do"), ("1", "To Do"), "21", "Stay"),
            edge(("1", "To Do"), ("2", "Done"), "31", "Close"),
        ];
        let paths = enumerate_paths(&edges, "1", 4);
        assert_eq!(paths.len(), 1, "self-loop must not produce a path");
        assert_eq!(paths[0].value(), "31");
    }

    #[test]
    fn cycles_are_blocked_by_visited_set() {
        let edges = vec![
            edge(("1", "A"), ("2", "B"), "12", "x"),
            edge(("2", "B"), ("1", "A"), "21", "y"),
        ];
        let paths = enumerate_paths(&edges, "1", 4);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].status_ids, vec!["2".to_string()]);
    }

    #[test]
    fn depth_limit_caps_chain_length() {
        let edges = vec![
            edge(("1", "A"), ("2", "B"), "12", "x"),
            edge(("2", "B"), ("3", "C"), "23", "y"),
            edge(("3", "C"), ("4", "D"), "34", "z"),
        ];
        let paths = enumerate_paths(&edges, "1", 2);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[1].status_ids, vec!["2".to_string(), "3".to_string()]);
    }

    #[test]
    fn required_fields_union_across_hops() {
        let mut e1 = edge(("1", "A"), ("2", "B"), "12", "x");
        e1.required_fields = vec!["resolution".into()];
        let mut e2 = edge(("2", "B"), ("3", "C"), "23", "y");
        e2.required_fields = vec!["resolution".into(), "comment".into()];
        let edges = vec![e1, e2];
        let paths = enumerate_paths(&edges, "1", 4);
        let two_hop = paths.iter().find(|p| p.transition_ids.len() == 2).unwrap();
        assert_eq!(
            two_hop.required_fields,
            vec!["resolution".to_string(), "comment".to_string()]
        );
    }

    #[test]
    fn sorted_by_depth_then_name() {
        let edges = vec![
            edge(("1", "A"), ("2", "Zzz"), "12", "z"),
            edge(("1", "A"), ("3", "Aaa"), "13", "a"),
        ];
        let paths = enumerate_paths(&edges, "1", 4);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].status_names, vec!["Aaa".to_string()]);
        assert_eq!(paths[1].status_names, vec!["Zzz".to_string()]);
    }

    #[test]
    fn empty_when_start_unknown() {
        let edges = vec![edge(("1", "A"), ("2", "B"), "12", "x")];
        assert!(enumerate_paths(&edges, "99", 4).is_empty());
        assert!(enumerate_paths(&edges, "", 4).is_empty());
        assert!(enumerate_paths(&edges, "1", 0).is_empty());
    }

    #[test]
    fn options_show_terminal_only_with_star_for_multi_hop() {
        let edges = vec![
            edge(("1", "To Do"), ("2", "In Progress"), "21", "Start"),
            edge(("2", "In Progress"), ("3", "Done"), "31", "Finish"),
            edge(("1", "To Do"), ("4", "Cancelled"), "41", "Cancel"),
        ];
        let paths = enumerate_paths(&edges, "1", 4);
        let options = paths_to_options(&paths);
        let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
        assert!(labels.contains(&"In Progress"), "direct hop has no star");
        assert!(labels.contains(&"Cancelled"), "direct hop has no star");
        assert!(labels.contains(&"Done*"), "multi-hop has star");
    }

    #[test]
    fn options_dedupe_by_terminal_status_shortest_wins() {
        // Two ways to reach Done: a direct edge (21) and a 2-hop chain
        // (12 → 23). Picker should show only the direct route, no star.
        let edges = vec![
            edge(("1", "Open"), ("3", "Done"), "21", "Resolve"),
            edge(("1", "Open"), ("2", "In Review"), "12", "Review"),
            edge(("2", "In Review"), ("3", "Done"), "23", "Approve"),
        ];
        let paths = enumerate_paths(&edges, "1", 4);
        let options = paths_to_options(&paths);
        let done = options.iter().filter(|o| o.label.starts_with("Done")).count();
        assert_eq!(done, 1, "only one entry for terminal status Done");
        let done_opt = options.iter().find(|o| o.label.starts_with("Done")).unwrap();
        assert_eq!(done_opt.label, "Done", "direct edge wins, no star");
        assert_eq!(done_opt.value, "21");
    }

    #[test]
    fn fallback_options_dedupe_by_terminal_no_star() {
        let transitions = vec![
            JiraTransition {
                id: "21".into(),
                name: "Resolve".into(),
                to_status_id: "3".into(),
                to_status: "Done".into(),
                required_fields: Vec::new(),
            },
            JiraTransition {
                id: "22".into(),
                name: "Close".into(),
                to_status_id: "3".into(),
                to_status: "Done".into(),
                required_fields: Vec::new(),
            },
            JiraTransition {
                id: "31".into(),
                name: "Reopen".into(),
                to_status_id: "1".into(),
                to_status: "Open".into(),
                required_fields: Vec::new(),
            },
        ];
        let options = fallback_options(&transitions);
        assert_eq!(options.len(), 2, "Done deduped");
        assert_eq!(options[0].label, "Done");
        assert_eq!(options[0].value, "21", "first transition wins");
        assert_eq!(options[1].label, "Open");
    }
}
