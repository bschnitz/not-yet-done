# Plan: EditSession refactor

## Motivation

Today `app/editor.rs` knows about `ContentAdapter`, `Node`, `MetadataChange`,
`ContentError::Conflict`, `parse_editor_output`, `editor_template`,
`update_metadata`, `EditorProcessResult`, and a 9-variant `EditorAction` enum
with a ~100-line match. That is a layering violation — adapter internals leak
into the editor-orchestration layer.

The App should not know what gets edited. It should hold an opaque handle,
hand it the saved buffer when the editor closes, and act on a tiny outcome
enum. Every edit flow (Jira ticket, task, query, script, notes, …) implements
the same trait.

## Layer split

| Knowledge                                       | Lives on    |
| ----------------------------------------------- | ----------- |
| Template format (e.g. Jira 3b: `---` / `===`)   | **Node**    |
| Parser for saved buffer                         | **Node**    |
| Validation rules                                | **Node**    |
| Error-banner rendering (in the buffer's syntax) | **Node**    |
| Conflict-buffer rendering                       | **Node**    |
| Backend writes (HTTP / DB)                      | **Node**    |
| Editor lifecycle, reopen loop, subprocess       | **App**     |
| Polymorphism between edit kinds                 | **Session** |

The Node owns _everything_ format-specific. Only the Node knows what its own
buffer looks like, so only the Node can sensibly parse it back, validate it,
or render errors / conflicts inside it. Each adapter (Jira today, GitLab /
Notion / … later) is free to design a totally different buffer format with
different validation rules and error syntax — the abstraction does not force
a shared format.

The Session is a thin polymorphic shim. It holds the adapter / node-id /
version state between reopens, and translates `EditResult` (from Node) to
`CommitOutcome` (for App). It contains no format knowledge.

The App holds `Box<dyn EditSession>`, calls four trait methods, knows nothing
about content adapters.

## Trait contracts

### EditSession (TUI side)

```rust
// not-yet-done-tui/src/edit_session/mod.rs

#[async_trait]
pub trait EditSession: Send + Sync {
    /// Initial buffer that gets written to the temp file.
    fn template(&self) -> &str;

    /// File suffix for $EDITOR syntax highlighting (".md", ".yaml", …).
    fn suffix(&self) -> &str;

    /// Subprocess closed; saved buffer is `text`.
    async fn commit(&mut self, text: &str) -> CommitOutcome;

    /// Optional: invoked on intermediate saves (detached-editor live-reload).
    async fn live_apply(&mut self, _text: &str) {}
}

pub enum CommitOutcome {
    Done      { message: Option<String> },
    Reopen    { content: String },
    Cancelled { message: Option<String> },
    FollowUp(FollowUp),
}

pub enum FollowUp {
    PromptShortcut { scope: String, name: String, query: String },
}
```

### ContentNode (content side, edit-related additions)

```rust
trait ContentNode {
    // existing — stays:
    fn metadata(&self) -> &Metadata;
    fn content(&self) -> Option<&dyn ContentRef>;
    fn content_mut(&mut self) -> Option<&mut dyn ContentMut>;
    fn version(&self) -> Option<&str>;
    async fn update_metadata(&mut self, changes: &[MetadataChange], version: Option<&str>) -> Result<()>;

    // edit flow — owned end-to-end by the node:
    async fn editor_template(&self, fields: &[String]) -> Result<String>;

    /// End-to-end edit processing: parse, validate, write, conflict-detect.
    /// Only the node knows its own buffer format, so all of these steps live
    /// here. The session merely calls this and maps the result.
    async fn process_edit(
        &self,
        text: &str,
        version: &str,
        fields: &[String],
    ) -> Result<EditResult, ContentError>;
}

pub enum EditResult {
    /// Persisted. Optional notification text for the user.
    Success { new_version: String, message: Option<String> },

    /// Validation error OR conflict. The node has already produced a buffer
    /// the user can resolve in the editor (with whatever banners /
    /// in-format markers make sense for its syntax).
    /// `new_version` is set if a conflict re-fetch advanced the version.
    Reopen  { content: String, new_version: Option<String> },

    /// Nothing changed — no need to roundtrip.
    NoChanges,
}
```

`parse_editor_output` and `EditorOutput` go away as public types — they are
now implementation details inside `process_edit`.

## App-side simplification

Before:

```rust
pub pending_editor_action: Option<EditorAction>,   // 9 variants
```

After:

```rust
pub pending_session: Option<Box<dyn EditSession>>,
```

`open_session`:

```rust
pub fn open_session(&mut self, session: Box<dyn EditSession>) -> EditorRequest {
    if self.editor_busy() { … return EditorRequest::None; }
    let suffix: &'static str = Box::leak(session.suffix().to_string().into_boxed_str());
    let content = session.template().to_string();
    self.pending_session = Some(session);
    /* dispatch inline / launch / detached as today */
}
```

`process_editor_content`:

```rust
pub async fn process_editor_content(&mut self, text: &str) -> Option<String> {
    let mut session = self.pending_session.take()?;
    match session.commit(text).await {
        CommitOutcome::Done      { message } => { if let Some(m)=message { self.notify(m); } None }
        CommitOutcome::Cancelled { message } => { if let Some(m)=message { self.notify(m); } None }
        CommitOutcome::Reopen    { content } => {
            self.pending_session = Some(session);
            Some(content)
        }
        CommitOutcome::FollowUp(f) => { self.handle_follow_up(f); None }
    }
}
```

`process_editor_live_save` collapses to:

```rust
if let Some(s) = self.pending_session.as_mut() { s.live_apply(&content).await; }
```

That's it. ~100 lines of match → ~10 lines. `EditorAction` and
`EditorProcessResult` go away.

## Example: JiraIssueEditSession (the shim)

```rust
pub struct JiraIssueEditSession {
    adapter:         Arc<dyn ContentAdapter>,
    node_id:         String,
    editable_fields: Vec<String>,
    version:         String,
    template:        String,            // built once via node.editor_template()
}

impl JiraIssueEditSession {
    pub async fn new(
        adapter: Arc<dyn ContentAdapter>,
        node_id: String,
        editable_fields: Vec<String>,
    ) -> Result<Self, ContentError> {
        let node = adapter.get_by_id(&node_id).await?;
        let template = node.editor_template(&editable_fields).await?;
        let version = node.version().unwrap_or("").to_string();
        Ok(Self { adapter, node_id, editable_fields, version, template })
    }
}

#[async_trait]
impl EditSession for JiraIssueEditSession {
    fn template(&self) -> &str { &self.template }
    fn suffix(&self)   -> &str { ".md" }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        let node = match self.adapter.get_by_id(&self.node_id).await {
            Ok(n)  => n,
            Err(e) => return CommitOutcome::Cancelled { message: Some(format!("{e}")) },
        };
        match node.process_edit(text, &self.version, &self.editable_fields).await {
            Ok(EditResult::Success { new_version, message }) => {
                self.version = new_version;
                CommitOutcome::Done { message }
            }
            Ok(EditResult::Reopen { content, new_version }) => {
                if let Some(v) = new_version { self.version = v; }
                CommitOutcome::Reopen { content }
            }
            Ok(EditResult::NoChanges) => CommitOutcome::Cancelled { message: Some("No changes".into()) },
            Err(e) => CommitOutcome::Cancelled { message: Some(format!("{e}")) },
        }
    }
}
```

The session is the same shape for every content-backed edit. Only the
`process_edit` implementation in the node differs.

## Example: JiraIssueNode::process_edit (where the format lives)

```rust
async fn process_edit(
    &self,
    text: &str,
    version: &str,
    fields: &[String],
) -> Result<EditResult, ContentError> {
    // (a) Parse the 3b layout. Node-internal knowledge.
    let parsed = match self.parse(text) {
        Ok(p)  => p,
        Err(e) => return Ok(EditResult::Reopen {
            content: self.render_with_errors(text, &[e.into()]),
            new_version: None,
        }),
    };

    // (b) Validate (Jira-specific rules).
    let mut errs = Vec::new();
    if parsed.editable.get("summary").map_or(true, |s| s.trim().is_empty()) {
        errs.push(FieldError::field("summary", "must not be empty"));
    }
    if !errs.is_empty() {
        return Ok(EditResult::Reopen {
            content: self.render_with_errors(text, &errs),
            new_version: None,
        });
    }

    // (c) Diff: only roundtrip if anything actually changed.
    let metadata_changes = self.diff_metadata(&parsed.editable);
    let body_changed     = parsed.body != self.original_body();
    if metadata_changes.is_empty() && !body_changed {
        return Ok(EditResult::NoChanges);
    }

    // (d) Write body (with conflict detection).
    let mut new_version = version.to_string();
    if body_changed {
        match self.write_content(&parsed.body, version).await {
            Ok(v) => new_version = v,
            Err(ContentError::Conflict(c)) => {
                let fresh = self.adapter().get_by_id(&self.id()).await?;
                return Ok(EditResult::Reopen {
                    content: self.render_conflict(text, &fresh),
                    new_version: Some(c.remote_version),
                });
            }
            Err(e) => return Err(e),
        }
    }

    // (e) Write metadata.
    if !metadata_changes.is_empty() {
        match self.write_metadata(&metadata_changes, &new_version).await {
            Ok(v) => new_version = v,
            Err(ContentError::Conflict(c)) => {
                let fresh = self.adapter().get_by_id(&self.id()).await?;
                return Ok(EditResult::Reopen {
                    content: self.render_conflict(text, &fresh),
                    new_version: Some(c.remote_version),
                });
            }
            Err(e) => return Err(e),
        }
    }

    Ok(EditResult::Success { new_version, message: Some("Saved".into()) })
}
```

`self.parse`, `self.render_with_errors`, `self.render_conflict`,
`self.diff_metadata` are private helpers inside `JiraIssueNode`. Each
adapter writes its own — there is no shared toolkit, because the format
is what differs and helpers couldn't span formats without becoming
dishonest.

## Sessions to migrate

| Today's `EditorAction` variant | New session struct                       | Notes                                                                                               |
| ------------------------------ | ---------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `ContentEdit { … }`            | `JiraIssueEditSession`                   | First migration target. Drives `JiraIssueNode::process_edit`. Uses 3b layout in node impl.          |
| `EditTask` + `CreateTask`      | `TaskEditSession { mode: Edit/Create }`  | Wraps the existing `editor_templates::*` parsers — those stay where they are, session only invokes. |
| `TaskNotes`                    | `TaskNotesEditSession`                   | No validation, just save.                                                                           |
| `Restructure`                  | `RestructureEditSession`                 | Subtree edit. `process_edit`-like logic stays in `tree_edit::*`.                                    |
| `QueryFilter` (tasks)          | `QueryFilterSession { kind: Tasks }`     | Has `live_apply` + may emit FollowUp::PromptShortcut.                                               |
| `TrackingQueryFilter`          | `QueryFilterSession { kind: Trackings }` | Same shape.                                                                                         |
| `ContentQueryEdit`             | `ContentQueryEditSession`                | Has `live_apply` + may emit FollowUp::PromptShortcut.                                               |
| `ContentChildCreate`           | `ContentChildCreateSession`              | E.g. add Jira comment via `node.create_child(...)`.                                                 |
| `TrackingScript`               | `TrackingScriptEditSession`              | Edits a file on disk. Validation lives in the script-format helper.                                 |
| `TrackingScriptOutput`         | `TrackingScriptOutputSession`            | Read-only. `commit` is a no-op (`Done { message: None }`).                                          |

For each session, the parsing/validation/error-rendering/conflict-rendering
stays where it conceptually belongs:

- Content-adapter-backed sessions → on the `ContentNode` impl
  (`process_edit`).
- Task sessions → in `editor_templates::*` and `tree_edit::*` (already
  there today).
- Query sessions → in `query_filter::*`.
- Script session → minimal; just text save.

## Phases

### Phase 1 — Plumbing

- Create `not-yet-done-tui/src/edit_session/mod.rs` with the trait,
  `CommitOutcome`, `FollowUp`. Unit-test the trait contract with a tiny
  fake session.
- Add `pending_session: Option<Box<dyn EditSession>>` to App **alongside**
  `pending_editor_action`. New code path is opt-in; old path keeps working.
- Add `App::open_session()`. `process_editor_content` checks
  `pending_session` first, falls through to existing dispatcher.
- `cargo build --release` clean.

### Phase 2 — Migrate ContentEdit (Jira)

- Extend `ContentNode` trait with `process_edit` and `EditResult`. Default
  impl can call existing `parse_editor_output` for backwards-compat during
  migration.
- Implement `process_edit` on `JiraIssueNode` with the current parser,
  validator, conflict logic moved into private node methods.
- Create `JiraIssueEditSession` — the thin shim above.
- ContentView dispatches `OpenEditSession` instead of `OpenContentEditor`.
- Remove `EditorAction::ContentEdit`, `process_content_edit` from
  `app/editor.rs`. Remove `EditorAction::ContentChildCreate` if migrated
  in lockstep, otherwise keep for Phase 5.
- Verify: edit a Jira ticket, success path, error path, conflict path.

### Phase 3 — 3b header layout

- Update `JiraIssueNode::editor_template` → 3b layout
  (editable section, then `---`, then read-only section, then `===`,
  then body).
- Implement parser + validator inside `JiraIssueNode::process_edit`:
  - both markers present in correct order
  - `summary` present, non-empty
  - no unknown editable keys before `---`
- Implement `JiraIssueNode::render_with_errors` for the new format
  (e.g. `# ─── ERRORS ───\n# • summary must not be empty\n` prepended
  above the editable section).
- Implement `JiraIssueNode::render_conflict` for the new format.

### Phase 4 — Migrate TaskEdit

- `TaskEditSession` (covers Create + Edit; constructor takes `mode`).
- Reuses `editor_templates::parse_*` directly — those parsers already do
  the validation and produce error banners; the session just maps
  `ParseResult` → `EditResult` (or directly to `CommitOutcome`, since
  task edit doesn't go through ContentNode).
- TasksView dispatches sessions; `EditorProcessResult` is removed.

### Phase 5 — Migrate remaining variants

- TaskNotes, QueryFilter, TrackingQueryFilter, TrackingScript,
  TrackingScriptOutput, ContentChildCreate, ContentQueryEdit, Restructure.
- After Restructure (the last and trickiest) lands: delete the
  `EditorAction` enum and `EditorProcessResult` enum entirely.

### Phase 6 — Cleanup

- Remove `process_editor_result` + `EditorProcessResult` from views.
- Remove `pending_editor_action`.
- App's `editor.rs` shrinks to: `open_session`, `process_editor_content`,
  `process_editor_live_save`, `editor_busy`, plus the existing
  detached-poll loop.

### Phase 7 — Verify

- `cargo build --release` clean.
- `cargo test --release -p not-yet-done-tui` (current 143 + new toolkit
  tests) green.
- `cargo install --path not-yet-done-tui --offline`.
- Manual smoke: each migrated flow tested by user (one per phase).

## Constraints / open questions

1. **Where does the session get constructed?**
   Two options:
   - **(a) View constructs**: ContentView builds `JiraIssueEditSession`
     directly. Pro: trait stays at the boundary; views own their session
     types. Con: views must do async (sessions need `.new(...).await`).
   - **(b) View emits descriptor; App constructs**:
     `ViewRequest::OpenEditSession(SessionDescriptor)` where
     `SessionDescriptor` is a small enum the App turns into a session.
     Pro: views remain sync. Con: a centralised enum reintroduces a
     mini-`EditorAction`.
   - Decision: **(b)** for Phase 1; revisit in Phase 6 if descriptor enum
     becomes a maintenance pain.

2. **`live_apply`**: Today `apply_content_query_live` lives in App. Move
   it into `ContentQueryEditSession::live_apply`. The App's poll loop
   calls `session.live_apply(text).await` whenever the detached editor
   reports an intermediate save.

3. **`Restructure`**: edits a whole subtree, persisted via
   `tree_edit::apply_changes`. Fits the trait, but `commit` will be
   substantial. Migrate last.

4. **`PromptShortcut` follow-up**: today triggers
   `awaiting_favorite_shortcut` directly. With sessions, the session
   returns `CommitOutcome::FollowUp(FollowUp::PromptShortcut { … })` and
   the App sets `awaiting_favorite_shortcut` in `handle_follow_up`. The
   modal-key route stays on the App.

5. **No shared toolkit.** Parsing, validation, error-banner rendering,
   conflict-buffer rendering all live on the node (or in the existing
   helpers like `editor_templates::*`, `tree_edit::*`,
   `query_filter::*`). Each format does its own thing. If two adapters
   later turn out to write structurally identical helpers, _then_
   extract — not before. Three similar lines beat a premature
   abstraction.

## Verification checklist

- [ ] `pending_editor_action` field gone, `EditorAction` enum gone.
- [ ] `EditorProcessResult` gone.
- [ ] `app/editor.rs` < 200 lines (was ~1000).
- [ ] No `ContentAdapter`, `MetadataChange`, `ContentError`, or
      `parse_editor_output` referenced from `app/editor.rs`.
- [ ] All existing flows tested manually after migration:
      task create / edit / notes / restructure / query (tasks &
      trackings) / Jira issue edit / Jira comment create / tracking
      script edit & output / content query edit (with shortcut prompt
      and live-reload).
- [ ] All tests pass.
- [ ] `cargo install --path not-yet-done-tui --offline` succeeds.
