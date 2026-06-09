# Plan: Content-Actions Unification

## Motivation

Today the action system has three disjoint mechanisms for "a thing the user
can trigger":

1. **Hardcoded TUI verbs** (`edit`, `create`, `navigate`, `reload`,
   `query_edit`, `delete`) — closed set, each with its own optional config
   block in `ActionDef`. New verbs require TUI code.
2. **`CustomAction` from the adapter** (`transition`) — open, but limited
   to "one input value or none", no editor templates, no structured input.
3. **View-internal actions** (`fuzzy_filter`, `search`) — never touch the
   adapter, pure client-side state.

`edit` is conceptually closer to `transition` than to `fuzzy_filter`, but in
the current model the opposite is true: edit/transition share no machinery,
edit/fuzzy_filter share `ActionDef`.

The unification: nodes declare their own actions (one of which is "edit"),
the TUI dispatches uniformly, YAML binds keys to action ids. View-internal
actions stay TUI-internal — they don't belong on a node.

A second motivation: a concrete new feature, `edit_with_comments` (Shift+e
in the action bar), exposes a Jira issue plus all comments inline as one
editable buffer. Multiple edit-flavors per node is exactly what the new
shape buys us.

## Layer split

| Knowledge                                                 | Lives on        |
| --------------------------------------------------------- | --------------- |
| Which actions a node has, with input shape                | **Node**        |
| Action execution (parse buffer, write, errors, conflict)  | **Node**        |
| Picker option fetch (e.g. transition list)                | **Node**        |
| Editor lifecycle, reopen loop, picker UI                  | **App / View**  |
| Polymorphism between action kinds                         | **Action enum** |
| YAML → action id, key, label override                     | **View config** |
| View-level operations (fuzzy filter, refresh, query menu) | **ContentView** |

Adapters and nodes own the open set. The TUI knows a fixed handful of
`InputSpec` and `ActionOutcome` variants and does not need to grow when a
new adapter is added.

## New types in `not-yet-done-content`

```rust
pub struct NodeAction {
    pub id: String,           // "edit_full", "edit_with_comments", "delete", "transition"
    pub label: String,        // for action bar / status bar
    pub input: InputSpec,
}

pub enum InputSpec {
    /// No user input — fire and forget (e.g. delete, reload-style actions).
    None,

    /// Multi-line editor buffer. Adapter renders the template via
    /// `prepare()`, parses the result inside `execute()`.
    Editor,

    /// Picker over a closed or dynamically-fetched option list.
    /// Picker options are returned by `picker_options(action_id)`.
    Picker,
}

pub enum ActionInput {
    None,
    Edited { text: String, original: String, version: String },
    Picked(String),  // value of the selected ActionOption
}

pub enum ActionOutcome {
    /// Persisted. Optional notification.
    Done { message: Option<String> },

    /// Validation / conflict / partial failure — fresh buffer to reopen,
    /// adapter has rendered banners in its own syntax.
    Reopen { content: String, new_version: Option<String> },

    /// Nothing changed.
    NoChanges,

    /// Action created/navigated to a new node (e.g. create_child returns
    /// the new node id). Optional — caller decides whether to drill down.
    Navigate { node_id: String, node_type: NodeType },
}
```

### `Node` trait — new shape

```rust
#[async_trait]
pub trait Node: Send + Sync {
    fn id(&self) -> &str;
    fn label(&self) -> &str;
    fn node_type(&self) -> &NodeType;
    fn metadata(&self) -> &Metadata;
    fn children_types(&self) -> Vec<NodeType>;

    async fn list(&self, params: ListParams) -> Result<ListResult>;
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>>;

    fn content(&self) -> Option<&dyn Content>;

    /// Actions this node supports right now. May depend on instance state
    /// (e.g. only show "delete" if user has permission, only show
    /// "edit_with_comments" on issues that have comments — adapter's call).
    fn actions(&self) -> Vec<NodeAction>;

    /// Prepare the input for an Editor-shape action: render the initial
    /// template plus the version token at template time.
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        let _ = action_id;
        Err(ContentError::NotSupported("prepare not supported".into()))
    }

    /// Picker options for a Picker-shape action.
    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        let _ = action_id;
        Ok(vec![])
    }

    /// Execute the action with the user's input. Adapter-specific.
    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome>;
}

pub struct EditorPrep {
    pub template: String,
    pub version: String,
    pub suffix: String,  // ".jira", ".md", …
}
```

### Removed from `Node`

- `editor_template`
- `parse_editor_output`
- `process_edit`
- `update_metadata`
- `delete`
- `create_child`
- `content_mut` (was only ever called from default `process_edit`)

### Removed from `ContentAdapter`

- `custom_actions`
- `action_options`
- `execute_action`

### Kept on `ContentAdapter`

- `adapter_type`, `root`, `get_by_id`, `capabilities`, `schema` —
  capabilities and schema stay because they're cross-cutting introspection,
  not action dispatch.

### Removed types

- `EditorOutput`, `EditResult`, `MetadataChange`, `CustomAction`

## YAML changes

Before:

```yaml
- name: edit
  key: e
  type: edit
  edit:
    content: true
    metadata: [summary]
- name: transition
  key: t
  type: custom
  custom_action: transition
```

After:

```yaml
- id: edit_full
  key: e
- id: edit_with_comments
  key: E
- id: transition
  key: t
```

`name`, `type`, `edit`, `custom_action`, `navigate_to`, `query_template`
fields drop. View-internal actions (`fuzzy_filter`, `query_edit`, `reload`)
keep their existing schema — they don't go through the node.

`ActionDef::shows_in_action_bar()` becomes data-driven: `InputSpec::Editor`
and `InputSpec::Picker` show in the action bar; `None` only in the status
bar. Override flag `hide_from_bar` stays.

## TUI dispatch

`ContentView` action dispatch:

1. User presses key → look up YAML `ActionDef` by key.
2. Distinguish view-internal (fuzzy_filter, query_edit, reload, navigate)
   from node-level. View-internal handled inline as today.
3. Node-level: get current node, find `NodeAction` by id, branch on
   `input`:
   - `None`: call `node.execute(id, ActionInput::None)` directly.
   - `Editor`: call `node.prepare(id)` → open editor (new
     `NodeActionEditSession` replaces `JiraIssueEditSession`).
   - `Picker`: call `node.picker_options(id)` → show picker → on select
     call `node.execute(id, ActionInput::Picked(value))`.
4. Handle `ActionOutcome` uniformly: Done → notify + reload list,
   Reopen → keep editor open with new content, NoChanges → notify,
   Navigate → drill down.

`NodeActionEditSession` is the new unified edit session. Holds
`adapter: Arc<dyn ContentAdapter>`, `node_id: String`, `action_id: String`,
plus snapshot of `template`/`version` from `prepare()`. On commit, calls
`get_by_id` → `execute(action_id, ActionInput::Edited { … })`. Maps
`ActionOutcome` → `CommitOutcome`. Replaces today's `JiraIssueEditSession`.

`ContentChildCreateSession` becomes a special case of
`NodeActionEditSession` over the action `create_<child_type>` on the
parent node (Jira: `create_comment`, `create_attachment` if supported).

## Phases

### Phase 1 — Big-bang trait refactor (one logical change, behaviorally equivalent)

Single commit, all crates compile, all tests green at the end. No behavior
change — same actions, same buffers, same outcomes.

1.1. **`not-yet-done-content/src/lib.rs`**: define new types
(`NodeAction`, `InputSpec`, `ActionInput`, `ActionOutcome`, `EditorPrep`).
Rewrite `Node` and `ContentAdapter` traits per "Removed/Kept" sections.
Delete obsolete types.

1.2. **`not-yet-done-content/src/mock.rs`**: rewrite `MockNode` and
`MockAdapter` against new trait. Each `MockNodeData` declares actions.
The 5 self-tests get rewritten to drive `actions()`/`execute()`.

1.3. **`not-yet-done-core/src/jira/adapter.rs`**: rewrite all four node
types.

- `JiraRoot::actions()`: empty for now (root has no actions). Listing is
  not an action — it's the ambient `list()` call.
- `JiraIssueNode::actions()`: returns `edit_full` (`InputSpec::Editor`),
  `delete` (`InputSpec::None`), `transition` (`InputSpec::Picker`),
  `create_comment` (`InputSpec::Editor`).
- `JiraCommentNode::actions()`: `edit_full`, `delete`.
- `JiraAttachmentNode::actions()`: empty (read-only).

`execute()` per node dispatches by `action_id`, routing to the existing
private helpers (3-way merge for issue, PUT for comment, DELETE,
transitions). The big diff is wiring, not logic — keep helpers, change
entry points.

`prepare()` for Editor-shape actions consolidates today's
`editor_template` + `version` access.

`picker_options()` for `transition` calls the existing transition fetch.

The 28 inline `#[cfg(test)]` tests rewrite minimally — they call
`prepare()` instead of `editor_template()`, `execute()` instead of
`process_edit()`. Same assertions.

1.4. **`not-yet-done-tui/src/edit_session/`**: replace
`jira_issue.rs` and `content_child_create.rs` with one new
`node_action.rs` containing `NodeActionEditSession`. Old files deleted.
The seven other sessions (Task, TaskNotes, TrackingScript,
TrackingScriptOutput, Restructure, three QueryFilter) are unchanged —
they don't touch `Node`.

1.5. **`not-yet-done-tui/src/views/content_view.rs`**: rewrite action
dispatch. View-internal branch unchanged. Node-action branch reads
`actions()`/`prepare()`/`picker_options()`/`execute()` per spec above.
The 22 unit tests using MockAdapter get rewritten to drive node actions.

1.6. **`not-yet-done-tui/src/config/view_config.rs`**: simplify
`ActionDef`. Drop `edit: EditConfig`, `navigate_to`, `query_template`,
`custom_action` fields and the `EditConfig` struct. Add `id: String`
(replacing `name` for action lookup; `name` becomes `label` override).
`shows_in_action_bar` becomes data-driven via the resolved
`InputSpec` (looked up at dispatch time).

1.7. **YAML configs**: update `docs/examples/views/jira.yaml` to new
schema. User's `~/.config/not_yet_done/views/jira.yaml` needs the same
edit — flag this in the migration commit message.

1.8. **Build & smoke**: full `cargo build --release`, `cargo test`,
`cargo install --path not-yet-done-tui --offline`, manual run-through of
existing smoke tests in `docs/smoke-tests-edit-session.md` (Jira section

- Content tab — they all should still pass unchanged).

### Phase 2 — `edit_with_comments` action

One commit, isolated to Jira adapter + smoke tests.

2.1. **Current-user fetch**:

- `JiraClient::current_user()` → `GET /rest/api/2/myself`, returns
  `{ account_id, display_name, name }`.
- Cached on `JiraCache` (existing struct), TTL similar to labels/users.
- `JiraAdapter` exposes `current_account_id()` for nodes to compare.

  2.2. **`JiraIssueNode::actions()`** adds:

```rust
NodeAction {
    id: "edit_with_comments".into(),
    label: "edit + comments".into(),
    input: InputSpec::Editor,
}
```

2.3. **Buffer format** — extending the 3b layout:

```
--- meta ---
summary: …
=== read-only ===
status: …
key: …
=== body ===
Issue body in Jira-Wiki…
=== Comments below this line — edit, "del" to delete, "--- add ---" to create ===

--- [12345] alice — 2026-04-30 14:23 -----------------------------
Latest comment body…

--- [12300] bob — 2026-04-29 09:11 -------------------------------
Earlier comment body…
```

Comments newest → oldest. `[12345]` is the comment id. Header line is
load-bearing for the parser; if user mangles it, the block becomes
unidentifiable and the parser will treat it as foreign-touched (see 2.7).

2.4. **`JiraIssueNode::prepare()`** for `edit_with_comments`:

- Render the existing 3b issue buffer.
- Append `=== Comments below this line ===` separator.
- Fetch comments via existing `JiraClient::get_comments(key)`.
- Render each comment block.
- Snapshot for change detection (see 2.5).

`EditorPrep::template` is the full buffer. `version` is the issue's own
`updated` token (existing). Comment-level versions are not part of the
issue version — handled per 2.6.

2.5. **Original-state snapshot — lives on the node**.
`JiraIssueNode` gains a private `comment_snapshot:
Vec<CommentSnapshot>` set during `prepare()`:

```rust
struct CommentSnapshot {
    id: String,
    author_account_id: String,
    body_hash: u64,  // FxHash or similar over normalized body
}
```

`prepare()` populates this on the node instance. `execute()` consumes
it. Since the same node instance handles both calls in the
`NodeActionEditSession`, the snapshot survives. (If a re-fetch is needed
on Reopen, `prepare()` is called again and the snapshot is rebuilt —
fresh.)

Body normalization for hashing: the same `normalize_blank_lines`
already used for the issue body diff.

2.6. **`JiraIssueNode::execute()` for `edit_with_comments`** —
processing order:

1. **Section split** at `=== Comments below this line ===`. Missing →
   reopen with banner "comment section marker missing — restored",
   buffer is the `prepare()` output (with original comments, plus user's
   edits to the issue body if any are still recoverable; safe fallback:
   re-render from scratch).

2. **Issue part** — reuse existing 3b parser + diffy 3-way merge. Same
   conflict handling as today's `edit_full`.

3. **Comment section parse** → list of `ParsedBlock`:

```rust
enum ParsedBlock {
    Existing { id: String, body: String },
    Add(String),
    Garbled(String),  // header unparseable
}
```

4. **For each `Existing` block**:

- Look up snapshot by id.
- If body normalized hash == snapshot hash → no-op.
- If body trimmed == "del" or "delete" (case-insensitive):
  - own comment → DELETE call.
  - foreign comment → restore + foreign-error (see 2.7).
- Else (body changed):
  - own comment → PUT call.
  - foreign comment → restore + foreign-error (see 2.7).
- If snapshot not found (id not in original list, possibly user invented
  one) → treat as garbled, restore section.

5. **For each `Add` block** with non-empty body → POST new comment via
   `JiraClient::add_comment`. Position-independent.

6. **For each `Garbled` block** → counts as "section was tampered with",
   restore + section-tampered banner (similar to marker-missing).

7. **Block missing** (id in snapshot, no `Existing` for it in buffer):
   silently ignored. Comment will reappear on next render.

8. **Comment-write failures**: each PUT/POST/DELETE wraps its error.
   Per-comment errors collect into a single Reopen with banners; issue
   write either succeeded or failed independently. This means the issue
   body can succeed while two comments fail — the Reopen buffer reflects
   the post-issue-write state plus error banners for the failed
   comments.

9. **Conflicts on comments**: explicitly NOT detected (per spec). Each
   PUT uses no `expected_version` and last-writer-wins.

2.7. **Foreign-edit / foreign-delete restore**: when the buffer contains
edits to comments not authored by the current user:

- Render that comment in the Reopen buffer with its **original** body.
- After the `# ─── ERRORS ───` banner at the top, append a section per
  rejected change:

```
# ─── ERRORS ───
# • Cannot edit comment by alice (you are bob)
# • Cannot delete comment by carol (you are bob)

# Your text below — copy it into a new "--- add ---" block if you want
# to keep it as a new comment of your own.
#
# === Rejected edit to alice's comment [12345] ===
# (your version of the body)
#
# === Rejected delete of carol's comment [12300] ===

# (rest of the buffer — issue body, comments restored)
```

Comment lines (`#`-prefixed) are stripped on the next save by an
existing `strip_error_banner`-style helper extended for this case.

2.8. **Smoke tests** — extend `docs/smoke-tests-edit-session.md` with
a new "Jira edit_with_comments" section:

- Open issue with `E`, sees comments newest → oldest, edit own comment
  → save → comment updated.
- Edit own comment, change one word → updated, others untouched.
- Write `del` in own comment block → comment deleted.
- Add `--- add ---` block at the end with body → new comment created.
- Edit foreign comment → reopen with restore + error banner.
- Delete (`del`) foreign comment → reopen with restore + error banner.
- Delete the `=== Comments below ===` marker → reopen with banner +
  restored state.
- Mix: own edit + foreign edit + new add → own + new applied, foreign
  rejected with restore.
- Combined with issue body edit — all paths cooperate.

## Out of scope (deferred to "Schmerz konkret")

- `InputSpec::OneLine` (single-line input with autocomplete).
- `InputSpec::MultiSelect`.
- `InputSpec::StructuredForm`.
- Comment-level conflict detection.
- Bulk actions (multiple selected items).
- Action gating by capabilities (e.g. hide `delete` if read-only) —
  trivial later via `actions()` returning a filtered list.

## Migration notes

- User's `~/.config/not_yet_done/views/jira.yaml` needs manual update
  to the new YAML schema (see Phase 1.7). Will be flagged in the
  Phase 1 commit message and in any release note.
- Adapter authors (currently only Jira) lose `editor_template` etc. and
  implement `actions()`/`prepare()`/`execute()` instead. Mock adapter
  serves as reference implementation.
- The architecture rule from `project_edit_session_refactor.md` — Node
  owns format knowledge, Session is a shim — carries over: `prepare()`
  / `execute()` are still strictly node-side.
