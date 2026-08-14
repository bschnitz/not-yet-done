# Confluence Adapter — Plan

Status: **in progress** — R-1 (7ea2e25), CF-0 (a763ea6), CF-1 (27d862a)
committed locally; CF-2a/b next.
Tracking memory: `project_confluence_adapter.md`.

## Goal

Add a `ContentAdapter` for Atlassian Confluence Server / Data-Center
that exposes Spaces → Pages (recursively) as a tree in the TUI, with
full CRUD on pages, attachments and comments. Authentication piggy-
backs on the existing `AuthOrchestrator` (cookie mechanism, like Jira).

## Scope (this plan)

- **Variant:** Confluence Server / Data-Center 8.x / 9.x.
  Identified from the URL shape `/<context>/spaces/<KEY>/pages/<id>/<title>`
  combined with a `/<context>/` deployment context-path (Cloud uses
  `/wiki/...` on `*.atlassian.net`). Atlassian-Crowd-SSO cookies on the
  host (`JSESSIONID` for `/<context>`, `crowd.token_key`,
  `atlassian.xsrf.token`) confirm a self-hosted DC install.
- **Auth:** cookie-based (re-use Jira's `Cookie` `AuthMechanism` with
  `command`/`literal`/`env` source). PAT-style basic-auth optional later.
- **Tree:** single recursive branch.
  Root → Space → (recursively) Page → Page → … Each page additionally
  exposes Attachments and Comments as sibling child-types.
- **CRUD:** Read first, Write after Read is solid. Page edit (with
  conflict detection), create (under parent or space-root), delete
  (trash → purge), attachment up/download, comment add/edit/delete.
- **Search:** CQL-based saved queries (analog to JQL for Jira).

## Out of scope (deliberate, possible follow-ups)

- Confluence Cloud (different endpoints, `/wiki/api/v2/...`).
- Whiteboards, Databases, Smart Links (new Cloud features).
- Page hierarchies move / reorder (`PUT /content/{id}` with `ancestors`
  — possible later, not in first pass).
- Page-version history browsing / diff.
- Restrictions / page permissions edit.
- Live preview while editing (wiki-markup ↔ storage-format conversion).
- Watchers / notifications.

## Design decisions

### D1 — Base on Jira adapter (not Taiga)

Reasons (see `adapter-survey-2026-06-02.md` for detail):

- Jira's auth is **cookie-based**; Confluence DC behind the same
  Atlassian Crowd SSO uses the **same cookie idiom**. Jira's
  `AuthBridge` + `CookieMechanism` plug straight in.
- Jira's **lazy hydration** (`OnceCell` per node) is the right shape
  for Confluence — page trees can be large and page bodies are heavy.
  Taiga's eager-on-construction pattern would cost an extra fetch
  per row.
- Confluence has **no query-variable use-case** comparable to Taiga's
  `${var:default}`; we'd inherit complexity we don't need.
- Jira's **3-section template renderer** (`3b`) for issue edit is a
  near-drop-in for the page-body edit buffer (header / storage / comments).

What we **adopt from Taiga**:

- **`ProjectMetaCache`-style per-space metadata cache** for space-level
  labels, members, status (Confluence has page-labels + last-modifier
  user — same pattern fits).
- **Attachment-upload action** (Taiga has the more recent / cleaner
  file-picker integration).

### D2 — Node-type model

| Node                    | Purpose              | Actions                                                                                                 |
| ----------------------- | -------------------- | ------------------------------------------------------------------------------------------------------- |
| `confluence:root`       | Adapter root         | (none)                                                                                                  |
| `confluence:space`      | One space            | `open-in-browser`, `cql-search`                                                                         |
| `confluence:page`       | A page (recursive)   | `edit`, `edit-with-comments`, `create-child`, `clone`, `delete`, `open-in-browser`, `upload-attachment` |
| `confluence:comment`    | Comment on a page    | `edit`, `delete`                                                                                        |
| `confluence:attachment` | Attachment on a page | `download`, `delete`                                                                                    |

Pages reference each other recursively — the validator change from
DSF (db-script-folders) for variable-depth node ids should already
cover this. If not, a small extension follows.

### D3 — Composite IDs

`NodeRef` shape:

```
confluence/<instance>/space/<KEY>
confluence/<instance>/space/<KEY>/page/<id>
confluence/<instance>/space/<KEY>/page/<id>/page/<id>/...
confluence/<instance>/space/<KEY>/page/<id>/.../comment/<id>
confluence/<instance>/space/<KEY>/page/<id>/.../attachment/<id>
```

Page-id alone is globally unique in Confluence (the URL drops the
intermediate parents). The full chain in our `NodeRef` keeps the tree-
walker shape consistent with what `multi-tree-continuation` (MT-1)
expects — even though here it's _single_ recursion, the chain serves
as our breadcrumb path for `:focus-node` and link resolution.

### D4 — HTTP client

Own `ConfluenceClient` in `not-yet-done-confluence-adapter/src/client/`,
shaped after `JiraClient`:

- per-concern submodules (`spaces.rs`, `pages.rs`, `attachments.rs`,
  `comments.rs`, `search.rs`, `user.rs`)
- `reqwest::Client` with 30 s default timeout
- XSRF: `X-Atlassian-Token: no-check` header on every `POST`/`PUT`/`DELETE`
  (Confluence enforces this on write endpoints; see the reference script
  below — Atlassian accepts both `no-check` and `nocheck`, we stay with the
  proven hyphenated form)
- pagination via `start` / `limit` params (server-side, native), default
  `limit=50`
- `http_log::log_request()` for debugging

**Working reference (manual, outside the repo):** the user script
`~/data/conf-edit` confirms the edit flow against the real
server (read `?expand=body.storage,version,title` → format → edit in
nvim → PUT with `version.number+1`, `Content-Type: application/json`,
`X-Atlassian-Token: no-check`). The endpoint shapes and headers in our
plan agree with it. Live probe 2026-06-02 against the real
instance: `/user/current`, `/space?limit=1`, `/content/{id}?expand=...`,
`/content/{id}/child/{page|attachment|comment}` all HTTP 200.
Confluence version: **9.2.19 Server/DC**.

Endpoint summary (Server REST `/rest/api/`):

| Capability          | Method | Path                                                                      |
| ------------------- | ------ | ------------------------------------------------------------------------- |
| List spaces         | GET    | `/space?limit=N&start=N&expand=...`                                       |
| Get space           | GET    | `/space/{KEY}?expand=homepage`                                            |
| Top-level pages     | GET    | `/space/{KEY}/content/page?start=&limit=`                                 |
| Child pages         | GET    | `/content/{id}/child/page?start=&limit=`                                  |
| Page detail         | GET    | `/content/{id}?expand=body.storage,version,ancestors,metadata.labels`     |
| Create page         | POST   | `/content` (body: type/title/space/body.storage/ancestors)                |
| Update page         | PUT    | `/content/{id}` (must increment `version.number`)                         |
| Delete page         | DELETE | `/content/{id}?status=current` (→ trash) then `?status=trashed` (→ purge) |
| Page attachments    | GET    | `/content/{id}/child/attachment`                                          |
| Upload attachment   | POST   | `/content/{id}/child/attachment` (multipart) + XSRF header                |
| Download attachment | GET    | `/content/{attId}/download`                                               |
| Page comments       | GET    | `/content/{id}/child/comment?expand=body.storage,version`                 |
| Add comment         | POST   | `/content` with `type=comment, container={id}`                            |
| Edit comment        | PUT    | `/content/{commentId}` (version-incr.)                                    |
| Delete comment      | DELETE | `/content/{commentId}`                                                    |
| CQL search          | GET    | `/content/search?cql=...&start=&limit=&expand=`                           |
| Current user        | GET    | `/user/current`                                                           |

### D5 — Page body format

Confluence uses three serializations: `storage` (XHTML-like, canonical
authoring format), `view` (rendered HTML), `wiki` (legacy textile-ish).

- **For our edit buffer:** fetch + write `body.storage`. Stable,
  diff-friendly, round-trip-safe. The storage value arrives from the server
  as a one-liner without whitespace — unusable for editing.
- **Pretty-print trick** (taken from `~/data/conf-edit`):
  wrap the value in `<root>...</root>`, run it through an XML formatter
  (`xmllint --format`), then strip the root-tag wrapper again.
  This works because `body.storage` is a valid XML fragment
  (Atlassian's own namespaces such as `<ac:*>` / `<ri:*>` are allowed,
  but because of the undeclared prefixes they may need `xmllint
--recover` or a custom pretty-printer).
- **Change detection before the PUT**: store the original in formatted form
  as well, and compare only after the formatter round trip. Otherwise every
  edit would count as "changed", because formatting itself does not round
  trip byte-exactly.
- **No conversion to/from Markdown.** Rabbit hole. Users edit storage
  XHTML directly. The README documents that. (The Confluence wiki-markup
  grammar in `~/data/projects/confluence_wiki` is separate —
  optionally much later for a Markdown-bridge follow-up feature.)
- **Editor suffix `.xml`** on the tempfile, so that nvim picks up XML syntax
  highlighting and possibly tree-sitter XML.

### D6 — Conflict handling on `PUT /content/{id}`

Confluence requires `version.number = current + 1`. If two clients save
concurrently, the second `PUT` fails (409 / 409-equivalent error in
`statusCode`/`message`).

The user's reference script (`conf-edit`) does **not** do this — it PUTs
blindly with `version+1` and accepts last-write-wins. For our
adapter we want to be one step better:

PUT-body shape (verified with the reference script):

```json
{
  "version": { "number": <stashed+1> },
  "type": "page",
  "title": "<title>",
  "body": {
    "storage": { "value": "<edited xhtml>", "representation": "storage" }
  }
}
```

Strategy:

1. On `open-for-edit`: fetch `body.storage` + `version.number` + `title`,
   stash all three in the `EditSession`.
2. On `commit`: PUT with `version.number = stashed + 1`.
3. On `409`: re-fetch the latest, do a `diffy` 3-way merge (as in Jira), put
   conflict markers into the buffer and a warning into the status bar. The user
   merges → commit again.

### D7 — CQL → saved queries

Map ContentQuery to CQL syntactically, similar to JQL for Jira:

- Saved query body is a CQL string (`space=DEMO AND label=docs`).
- Adapter implements `SavedQueryStore` via `FsSavedQueryStore` (same
  as Jira/Taiga).
- No `${var}` substitution in first cut — defer until proven needed.

### D8 — Auth-blob shape (auth_session table)

```jsonc
// In auth_session.data for confluence instance
{ "cookie": "JSESSIONID=...; crowd.token_key=...; atlassian.xsrf.token=..." }
```

Same shape as Jira's cookie blob. Session validation: `GET /user/current`
(cheap). On `401`/`302→login`, `invalidate_session()` clears the blob;
the orchestrator re-runs the configured `cookie.command`.

### D9 — Adapter-instance config (YAML)

`~/.config/not_yet_done/user-confluence.yaml`:

```yaml
adapter: confluence
instance: wiki
url: https://wiki.example.org/confluence # base URL incl. context-path
auth:
  mechanism: cookie
  cookie:
    command: ["/path/to/get-cookie.sh", "wiki"] # writes "JSESSIONID=...; crowd.token_key=..." to stdout
db:
  path: ~/.local/share/not_yet_done/confluence/wiki.sqlite
manual_connect: true # don't auto-connect on startup
```

The `cookie.command` is the user's existing pattern from Jira (e.g.
extract from qutebrowser sqlite or browser-extension export).

### D10 — TUI integration

- New tab key `6` for Confluence (1–5 are taken — Tasks, Trackings,
  Jira, Taiga, Postgres).
- View YAML at `~/.config/not_yet_done/views/confluence.yaml`:
  - root subview: spaces list
  - drill into space → pages list
  - recursive ChildDef on `confluence:page` (drill into page → child
    pages + attachments + comments siblings)
- Saved queries (CQL) per view via existing `q` menu.

## Phase list

The order is deliberately read-first, write-last. Every phase goes into
its own commit and is smoke-testable locally.

### Up front (before CF-0) — decided

- **R-1: pull `sort_serde` out of the Jira and Taiga adapters into
  `not-yet-done-content`.** **Decided (user, 2026-06-02): up front, before
  CF-0.** Trivial (~30 LoC, identical implementation in both). It removes the
  duplicate immediately, and Confluence builds on the shared variant from the
  start.
- **R-2: `not-yet-done-adapter-common` as a new crate.** It would absorb
  `HttpClientBuilder` (auth-header injection, timeout default, http*log
  integration), `SlugTable<T>`, `TemplateRenderer` (3b format).
  A bigger rebuild. **Not now — build the Confluence adapter with deliberate
  duplication of the Jira pattern first, then do R-2 as a follow-up refactor
  across all three adapters.** Rationale: two data points (Jira/Taiga)
  are thin for a good trait design; three adapters give us the
  material for a \_real* abstraction instead of a "two were similar
  so it must be a pattern" fallacy.

### CF-0 — anchor

- `docs/plan-confluence-adapter.md` (this document).
- `memory/project_confluence_adapter.md` as the tracking memory.
- `memory/adapter_survey_2026_06_02.md` with the refactor candidates from
  the survey (see the appendix below).
- A commit anchor for the compaction.

### CF-1 — crate scaffold

- New workspace member `not-yet-done-confluence-adapter`.
- Skeleton files: `Cargo.toml`, `src/lib.rs`, `src/adapter/mod.rs`,
  `src/client/mod.rs`, `src/config.rs`, `src/factory.rs`,
  `src/auth_bridge.rs`, `src/db.rs`.
- The adapter implements the `ContentAdapter` trait with minimal stubs
  (`root()` returns an empty list, everything else `Other("not implemented")`).
- The factory is registered in
  `not-yet-done-tui/src/main.rs::build_adapter_factories()`.
- Tab key `6` in the TUI; the tab shows an empty list, nothing crashes.
- `cargo build --release` + `cargo test --release` + install + commit.

### CF-2a — DB + entities + ConfluenceClient stub

Pure "dumb plumbing", no auth logic yet. It should compile on its own
and stay installable.

- SeaORM entities `auth_session` + `view_sort_state` (1:1 as in Jira).
- `db.rs` with `default_sqlite_url()` (path
  `~/.local/share/not_yet_done/confluence-cache.sqlite`) and
  `connect()` with `get_schema_registry(...).sync()`.
- `auth_session_store.rs` with `SqlAuthSessionStore` (as in Jira,
  blob = JSON `{cookie: "..."}`).
- `cache_store.rs` with `scope_id_for_url()`.
- `ConfluenceClient::new(base_url, cookie_header, accept_invalid_certs)`:
  reqwest setup, 30 s timeout, a `Cookie` header + `X-Atlassian-Token: no-check`
  as default headers. A single method: `current_user() -> Result<JsonValue>`
  as a health probe. The per-concern submodules are not there yet.
- `config.rs` extended: `auth: AuthSpec`, `accept_invalid_certs: bool`,
  `db: Option<DbConfig>`, `manual_connect: bool`. Update the tests.
- The factory does not pump any of this into the adapter yet (the adapter
  constructor additionally receives `Arc<DatabaseConnection>` + scope_id, but
  auth stays external). The adapter stays minimal — the root remains empty.
- Smoke: `cargo build --release` + `cargo test --release -p
not-yet-done-confluence-adapter` green, the TUI installable.

### CF-2b — AuthBridge + session validation

- `AuthBridge` as in Jira:
  - the `Cookie` mechanism accepts a `command`/`literal`/`env` source.
  - a cached `ConfluenceClient` instance behind `RwLock<Option<Arc<...>>>`.
  - `validate_session()` calls `current_user()` and returns a `bool`.
  - `invalidate_session()` clears the blob in the `auth_session` table.
- The factory wires the AuthBridge in and hands it to the adapter.
- `submit_credentials()` is unused (the cookie comes from outside via the
  command).
- Smoke: a view YAML with `adapter: confluence` plus a real cookie command,
  the tab opens, the banner shows `Ready` (or `Session invalid` if the
  cookie has expired). The first real bytes on the wire against a
  real instance.

### CF-3 — spaces

- `ConfluenceClient::list_spaces(start, limit)` → `Vec<SpaceMeta>`.
- `ConfluenceSpaceNode` (`confluence:space`) with `id()`, `label()`,
  `metadata()`, `actions()` (initially: `open-in-browser`).
- `ConfluenceRootNode::list()` paginates over `list_spaces`.
- The view YAML shows the spaces list in the root tab.
- Smoke: tab `6` shows the spaces, ESC/q closes.

### CF-4 — page tree (read-only, recursive)

- `ConfluenceClient::list_top_pages(space_key, start, limit)`.
- `ConfluenceClient::list_child_pages(parent_id, start, limit)`.
- `ConfluencePageNode` with `list()` → the child-pages call depending on
  the position in the tree (top level via `space.content.page`, otherwise via
  `content/{id}/child/page`).
- A recursive ChildDef in the view YAML (reusing the DSF mechanism).
- Smoke: spaces → enter → top pages → enter → child pages → several
  levels deep.

### CF-5 — page detail (read-only)

- `ConfluenceClient::get_page(id)` with `expand=body.storage,version,ancestors,metadata.labels`.
- `ConfluencePageNode::content()` renders `body.storage` into an `ItemDetail`
  (detail pane). The storage format is XHTML-like; the first cut shows it
  raw with `xml`/`html` syntax highlighting.
- Cache: `OnceCell<PageDetail>` per node (lazy hydration, the Jira pattern).
- Smoke: `p` (preview toggle) on a page shows `body.storage`.

### CF-6 — attachments (read-only)

- `ConfluencePageNode::children_types()` contains `confluence:attachment`.
- `ConfluenceClient::list_attachments(page_id)`.
- `ConfluenceAttachmentNode` with a `download` action (opens via xdg-open
  after downloading into a tempfile).
- Smoke: page → drill → attachment list; `d` downloads + opens.

### CF-7 — comments (read-only)

- `ConfluencePageNode::children_types()` extended by `confluence:comment`.
- `ConfluenceClient::list_comments(page_id)`.
- `ConfluenceCommentNode` shows body + author + timestamp.
- Smoke: a page with comments → drill → comments list.

### CF-8 — saved queries (CQL)

- `ConfluenceAdapter::saved_query_store()` → `FsSavedQueryStore`
  under `<instance_data_dir>/queries/`.
- The view configuration supports the `q` menu; apply calls
  `ConfluenceClient::cql_search(cql, start, limit)`.
- Ship a first example query (`saved/recent-pages.yaml` with
  `lastModified > now('-7d') ORDER BY lastModified DESC`).
- Smoke: `q` opens the menu, apply shows the CQL results.

### CF-9 — page edit (write 1)

- An `EditSession` for `confluence:page`, as in Jira's issue edit:
  - opens `body.storage` in a tempfile with an `.html` suffix
  - stashes `version.number`
  - on commit: `PUT /content/{id}` with `version.number + 1`
  - on `409`: re-fetch, `diffy` merge, conflict markers in the buffer
- The action `e` invokes the `EditSession`.
- Smoke: `e` on a page → the tempfile opens → edit → save → the page
  is updated in the browser.

### CF-10 — page create (write 2)

- A `confluence:space` action `create-page` (Shift+A or `a`),
  a `confluence:page` action `create-child`.
- A template file with an empty `<p></p>` plus a header block for the title.
- `POST /content` with `type=page`, `space.key`, and `ancestors=[{id:...}]`
  where applicable.
- Smoke: space → `a` → title + body → save → the new page appears.

### CF-11 — page delete (write 3)

- The action `D` (capital) on a page → confirm popup → `DELETE
/content/{id}` (status=current → into the trash).
- Optionally: a second `D` → `DELETE /content/{id}?status=trashed`
  (permanent). For now only the trash step; purge is documented but not
  exposed (safety).
- Smoke: `D` → confirm → the page is gone from the list.

### CF-12 — comments CRUD (write 4)

- A `confluence:page` action `add-comment` (`c`) opens an empty editor.
- `confluence:comment` actions `edit` (`e`), `delete` (`D`).
- `POST /content` with `type=comment, container={page_id}`.
- `PUT /content/{commentId}` as in the page edit.
- Smoke: all three actions once each.

### CF-13 — attachment upload (write 5)

- A `confluence:page` action `upload-attachment` (`A`) opens the FilePicker
  (the Taiga pattern, already in `not-yet-done-tui/src/widgets/file_picker.rs`).
- `POST /content/{id}/child/attachment` multipart + `X-Atlassian-Token: nocheck`.
- Smoke: `A` → FilePicker → pick a file → it appears in the attachment list.

### CF-14 — clone page

- As in the Jira/Taiga clone action (pattern: `y` to mark,
  `p` to paste — or a direct clone action with a title prompt).
- `GET /content/{id}` → `POST /content` with the same body and a new title.
- Smoke: clone within the same space, clone into another space.

### CF-15 — docs + final smoke + install + commit

- A README section "Confluence Adapter".
- Extend `docs/smoke-tests.md` (Confluence block).
- Ship a view YAML with all the sub-tabs as the default.
- `cargo build --release` + `cargo test --release` + install.
- A bundled commit or per-phase commits — depending on the user's preference.

## Open questions (to be clarified before CF-1)

1. **Cookie source:** **Clarified (user, 2026-06-02): the same Crowd SSO,
   the same cookie pool as Jira.** Reuse the existing script,
   possibly with a `--path` / `--service` parameter that filters the cookies for
   the right subpath.
2. **DB cache schema:** **Clarified (user, 2026-06-02):** CF-1 starts
   with only `auth_session` + `view_sort_state`. A user cache along the lines of
   `JiraCache` (`Arc<Mutex<...>>` for a `confluence_user` table) will be
   added later, **when it makes sense** — at the earliest at
   CF-7 (comments need author resolution) or CF-12 (comments CRUD
   with mention autocomplete). Labels stay page-local, no
   cache of their own is planned.
3. **Storage-format editor:** **Approach (user, 2026-06-02): evaluate ad hoc
   at CF-9** — test the round-trip behaviour against a real page
   (mixed XHTML with Atlassian namespaces such as `<ac:structured-macro>`,
   `<ri:user>` etc.). If `xmllint --format` breaks nothing,
   it stays raw; otherwise write a custom pretty-printer.
4. **CQL examples:** **Clarified: post-compact** — once we reach CF-8,
   we settle the defaults against the real workflow.

## Appendix: refactor candidates from the adapter survey

(Full analysis: `memory/adapter_survey_2026_06_02.md`.)

Quick wins (before or directly after Confluence):

1. Pull `sort_serde` out of the two adapters into `not-yet-done-content`.
2. Define the `view_sort_state` SeaORM entity as shared instead of duplicating
   it twice.

Bigger refactors (better after Confluence, because then there are 3 data
points):

3. `not-yet-done-adapter-common` with `HttpClientBuilder` (auth-header
   injection, timeout, http_log, retry-on-401).
4. `SlugTable<T>` + a `SlugResolver` trait (Jira: `ll-`/`uu-`; Taiga:
   `ss-`/`uu-`/`tt-`; Confluence will need labels + users too).
5. `TemplateRenderer` for the 3b format (header/body/comments).
6. A `diffy`-based 3-way merge as a shared utility.
7. A composite-ID codec (`<parent>/<type>/<child>` parser & builder).

Adapter registration:

8. Instead of a hardcoded HashMap in `main.rs::build_adapter_factories()`,
   an `inventory::collect!`-based registry pattern — adapter
   crates register themselves, and `main.rs` does not have to be touched
   when a new one is added. Small, but worth it.
