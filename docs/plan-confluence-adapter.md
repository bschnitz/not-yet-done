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

Reasons (siehe `adapter-survey-2026-06-02.md` für Detail):

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
  (Confluence enforces this on write endpoints; siehe Reference-Script
  unten — Atlassian akzeptiert `no-check` und `nocheck`, wir bleiben beim
  bewährten Hyphen-Format)
- pagination via `start` / `limit` params (server-side, native), default
  `limit=50`
- `http_log::log_request()` for debugging

**Working reference (manuell, ausserhalb des Repos):** das User-Skript
`~/data/conf-edit` bestätigt den Edit-Flow gegen den realen
Server (Read `?expand=body.storage,version,title` → format → edit in
nvim → PUT mit `version.number+1`, `Content-Type: application/json`,
`X-Atlassian-Token: no-check`). Endpoint-Shapes + Header in unserem
Plan stimmen damit überein. Live-Probe 2026-06-02 gegen die echte
Instanz: `/user/current`, `/space?limit=1`, `/content/{id}?expand=...`,
`/content/{id}/child/{page|attachment|comment}` alle HTTP 200.
Confluence-Version: **9.2.19 Server/DC**.

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
  diff-freundlich, round-trip-sicher. Storage-Wert kommt vom Server
  als one-liner ohne Whitespace — unbenutzbar zum editieren.
- **Pretty-Print-Trick** (aus `~/data/conf-edit` übernommen):
  Wert in `<root>...</root>` wrappen, durch XML-Formatter laufen
  lassen (`xmllint --format`), Root-Tag-Wrapper anschließend wieder
  strippen. Funktioniert, weil `body.storage` valides XML-Fragment
  ist (Atlassian-eigene Namespaces wie `<ac:*>` / `<ri:*>` sind erlaubt,
  brauchen aber wegen nicht-deklarierter Prefixes evtl. `xmllint
--recover` oder einen Custom-Pretty-Printer).
- **Change-Detection vor PUT**: Original AUCH formatiert speichern,
  Vergleich erst nach dem Formatter-Roundtrip. Sonst würde jeder
  Edit als „geändert" gelten, weil das Formatieren selbst nicht
  byte-exakt round-trippt.
- **No conversion to/from Markdown.** Rabbit hole. Users edit storage
  XHTML direkt. README dokumentiert das. (Confluence-Wiki-Markup-
  Grammar in `~/data/projects/confluence_wiki` ist separat —
  optional viel später für ein Markdown-Bridge-Folge-Feature.)
- **Editor-Suffix `.xml`** im tempfile, damit nvim XML-Syntax-Highlight
  - ggf. Tree-Sitter-XML zieht.

### D6 — Conflict handling on `PUT /content/{id}`

Confluence requires `version.number = current + 1`. If two clients save
concurrently, the second `PUT` fails (409 / 409-equivalent error in
`statusCode`/`message`).

Das User-Reference-Skript (`conf-edit`) macht das **nicht** — es PUTet
blind mit `version+1` und akzeptiert Last-Write-Wins. Für unseren
Adapter wollen wir eine Stufe besser sein:

PUT-Body shape (vom Reference-Skript verifiziert):

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
   stash alle drei in `EditSession`.
2. On `commit`: PUT mit `version.number = stashed + 1`.
3. On `409`: re-fetch latest, `diffy`-3-way-merge (analog Jira), Konflikt-
   Marker in den Buffer, Status-Bar-Warning. User merged → erneut commit.

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

- New tab key `6` for Confluence (1–5 sind belegt — Tasks, Trackings,
  Jira, Taiga, Postgres).
- View YAML at `~/.config/not_yet_done/views/confluence.yaml`:
  - root subview: spaces list
  - drill into space → pages list
  - recursive ChildDef on `confluence:page` (drill into page → child
    pages + attachments + comments siblings)
- Saved queries (CQL) per view via existing `q` menu.

## Phasen-Liste

Reihenfolge ist absichtlich Read-zuerst, Write-zuletzt. Jede Phase
kommt in einen eigenen Commit, ist lokal smoke-bar.

### Vorab (vor CF-0) — entschieden

- **R-1: `sort_serde` aus Jira- und Taiga-Adapter in `not-yet-done-content`
  ziehen.** **Entschieden (User 2026-06-02): vorab, vor CF-0.**
  Trivial (~30 LoC, identische Implementierung in beiden). Reduziert
  Duplikat sofort und Confluence baut von Anfang an auf der shared
  Variante auf.
- **R-2: `not-yet-done-adapter-common` als neues Crate.** Aufnahme von
  `HttpClientBuilder` (auth-header injection, timeout-default, http*log
  integration), `SlugTable<T>`, `TemplateRenderer` (3b-Format).
  Größerer Umbau. **Nicht jetzt — Confluence-Adapter erstmal mit
  bewusster Duplizierung des Jira-Patterns, dann R-2 als Folge-Refactor
  über alle drei Adapter.** Begründung: zwei Datenpunkte (Jira/Taiga)
  sind dünn für ein gutes Trait-Design; drei Adapter geben uns das
  Material für eine \_echte* Abstraktion statt eines "zwei waren ähnlich
  also ist es ein Pattern"-Fehlschlusses.

### CF-0 — Anchor

- `docs/plan-confluence-adapter.md` (dieses Dokument).
- `memory/project_confluence_adapter.md` als Tracking-Memory.
- `memory/adapter_survey_2026_06_02.md` mit den Refactor-Kandidaten aus
  dem Survey (siehe Anhang unten).
- Commit-Anker für compact.

### CF-1 — Crate-Scaffold

- Neues Workspace-Member `not-yet-done-confluence-adapter`.
- Skelett-Files: `Cargo.toml`, `src/lib.rs`, `src/adapter/mod.rs`,
  `src/client/mod.rs`, `src/config.rs`, `src/factory.rs`,
  `src/auth_bridge.rs`, `src/db.rs`.
- Adapter implementiert `ContentAdapter`-Trait mit minimal-stubs
  (`root()` gibt leere Liste, alles andere `Other("not implemented")`).
- Factory registriert in `not-yet-done-tui/src/main.rs::build_adapter_factories()`.
- Tab-Key `6` in der TUI; Tab zeigt leere Liste, nichts crashed.
- `cargo build --release` + `cargo test --release` + install + commit.

### CF-2a — DB + Entities + ConfluenceClient-Stub

Reines „dumb plumbing", noch keine Auth-Logik. Soll für sich kompilieren
und installierbar bleiben.

- Sea-ORM Entities `auth_session` + `view_sort_state` (1:1 wie Jira).
- `db.rs` mit `default_sqlite_url()` (Pfad
  `~/.local/share/not_yet_done/confluence-cache.sqlite`) und
  `connect()` mit `get_schema_registry(...).sync()`.
- `auth_session_store.rs` mit `SqlAuthSessionStore` (analog Jira,
  Blob = JSON `{cookie: "..."}`).
- `cache_store.rs` mit `scope_id_for_url()`.
- `ConfluenceClient::new(base_url, cookie_header, accept_invalid_certs)`:
  reqwest-Setup, 30 s Timeout, `Cookie`-Header + `X-Atlassian-Token: no-check`
  als Default-Header. Eine einzige Methode: `current_user() -> Result<JsonValue>`
  als Health-Probe. Per-Concern-Submodule sind noch nicht da.
- `config.rs` erweitert: `auth: AuthSpec`, `accept_invalid_certs: bool`,
  `db: Option<DbConfig>`, `manual_connect: bool`. Tests aktualisieren.
- Factory pumpt nichts davon noch in den Adapter (Adapter-Konstruktor
  bekommt zusätzlich Arc<DatabaseConnection> + scope_id, aber Auth
  bleibt extern). Adapter bleibt minimal — Root weiter leer.
- Smoke: `cargo build --release` + `cargo test --release -p
not-yet-done-confluence-adapter` grün, TUI installierbar.

### CF-2b — AuthBridge + Session-Validation

- `AuthBridge` analog Jira:
  - `Cookie` mechanism akzeptiert `command`/`literal`/`env` source.
  - Cached `ConfluenceClient`-Instance hinter `RwLock<Option<Arc<...>>>`.
  - `validate_session()` ruft `current_user()` auf, gibt `bool`.
  - `invalidate_session()` löscht Blob in `auth_session`-Tabelle.
- Factory wired AuthBridge ein, übergibt an Adapter.
- `submit_credentials()` ungebraucht (Cookie kommt extern via command).
- Smoke: View-YAML mit `adapter: confluence` + echtem Cookie-Command,
  Tab öffnet, Banner zeigt `Ready` (oder `Session invalid` falls
  Cookie abgelaufen ist). Erstes echtes Bytes-on-the-Wire gegen
  reale Instanz.

### CF-3 — Spaces

- `ConfluenceClient::list_spaces(start, limit)` → `Vec<SpaceMeta>`.
- `ConfluenceSpaceNode` (`confluence:space`) mit `id()`, `label()`,
  `metadata()`, `actions()` (initial: `open-in-browser`).
- `ConfluenceRootNode::list()` paginiert über `list_spaces`.
- View-YAML zeigt Spaces-Liste in Root-Tab.
- Smoke: Tab `6` zeigt Spaces, ESC/q schliesst.

### CF-4 — Page-Tree (Read-only, rekursiv)

- `ConfluenceClient::list_top_pages(space_key, start, limit)`.
- `ConfluenceClient::list_child_pages(parent_id, start, limit)`.
- `ConfluencePageNode` mit `list()` → child-pages-Aufruf je nach
  Position im Tree (top-level via `space.content.page`, sonst via
  `content/{id}/child/page`).
- Recursive ChildDef im View-YAML (DSF-Mechanismus weiterverwenden).
- Smoke: Spaces → enter → Top-Pages → enter → Child-Pages → mehrere
  Ebenen tief.

### CF-5 — Page-Detail (Read-only)

- `ConfluenceClient::get_page(id)` mit `expand=body.storage,version,ancestors,metadata.labels`.
- `ConfluencePageNode::content()` rendert `body.storage` in `ItemDetail`
  (Detail-Pane). Storage-Format ist XHTML-ähnlich; first cut zeigt es
  roh mit Syntax-Highlighting `xml`/`html`.
- Cache: `OnceCell<PageDetail>` pro Node (lazy hydration, Jira-Pattern).
- Smoke: `p` (preview-toggle) auf einer Page zeigt `body.storage`.

### CF-6 — Attachments (Read-only)

- `ConfluencePageNode::children_types()` enthält `confluence:attachment`.
- `ConfluenceClient::list_attachments(page_id)`.
- `ConfluenceAttachmentNode` mit `download`-Action (öffnet via xdg-open
  nach Download in tempfile).
- Smoke: Page → drill → Attachment-Liste; `d` lädt + öffnet.

### CF-7 — Comments (Read-only)

- `ConfluencePageNode::children_types()` ergänzt um `confluence:comment`.
- `ConfluenceClient::list_comments(page_id)`.
- `ConfluenceCommentNode` zeigt Body + Author + Timestamp.
- Smoke: Page mit Kommentaren → drill → Comments-Liste.

### CF-8 — Saved Queries (CQL)

- `ConfluenceAdapter::saved_query_store()` → `FsSavedQueryStore`
  unter `<instance_data_dir>/queries/`.
- View-Konfiguration unterstützt `q`-Menü; Apply ruft
  `ConfluenceClient::cql_search(cql, start, limit)`.
- Eine erste Beispiel-Query mitliefern (`saved/recent-pages.yaml` mit
  `lastModified > now('-7d') ORDER BY lastModified DESC`).
- Smoke: `q` öffnet Menü, Apply zeigt CQL-Resultate.

### CF-9 — Page-Edit (Write 1)

- `EditSession` für `confluence:page` analog Jira's Issue-Edit:
  - öffnet `body.storage` in tempfile mit `.html` Suffix
  - stash `version.number`
  - on commit: `PUT /content/{id}` mit `version.number + 1`
  - on `409`: re-fetch, `diffy` merge, Konflikt-Marker im Buffer
- Action `e` ruft `EditSession`.
- Smoke: `e` auf Page → tempfile öffnet → editieren → save → Page
  ist im Browser aktualisiert.

### CF-10 — Page-Create (Write 2)

- `confluence:space`-Action `create-page` (Shift+A oder `a`),
  `confluence:page`-Action `create-child`.
- Template-Datei mit leerem `<p></p>` + Header-Block für Titel.
- `POST /content` mit `type=page`, `space.key`, ggf. `ancestors=[{id:...}]`.
- Smoke: Space → `a` → Titel + Body → save → neue Page erscheint.

### CF-11 — Page-Delete (Write 3)

- Action `D` (capital) auf Page → Confirm-Popup → `DELETE
/content/{id}` (status=current → in Trash).
- Optional: zweites `D` → `DELETE /content/{id}?status=trashed`
  (endgültig). Vorerst nur Trash, Purge dokumentiert aber nicht
  exponiert (Sicherheit).
- Smoke: `D` → confirm → Page weg aus Liste.

### CF-12 — Comments-CRUD (Write 4)

- `confluence:page`-Action `add-comment` (`c`) öffnet leeren Editor.
- `confluence:comment`-Actions `edit` (`e`), `delete` (`D`).
- `POST /content` mit `type=comment, container={page_id}`.
- `PUT /content/{commentId}` analog Edit-Page.
- Smoke: alle drei Aktionen je einmal.

### CF-13 — Attachment-Upload (Write 5)

- `confluence:page`-Action `upload-attachment` (`A`) öffnet FilePicker
  (Taiga-Pattern, bereits in `not-yet-done-tui/src/widgets/file_picker.rs`).
- `POST /content/{id}/child/attachment` multipart + `X-Atlassian-Token: nocheck`.
- Smoke: `A` → FilePicker → Datei wählen → erscheint in Attachment-Liste.

### CF-14 — Clone-Page

- Analog Jira/Taiga clone-action (Pattern: `y` zum Markieren,
  `p` zum Paste — oder Direkt-Clone-Action mit Titel-Prompt).
- `GET /content/{id}` → `POST /content` mit gleichem Body + neuem Titel.
- Smoke: clone in gleichem Space, clone in anderen Space.

### CF-15 — Docs + Final-Smoke + Install + Commit

- README-Sektion „Confluence Adapter".
- `docs/smoke-tests.md` ergänzen (Confluence-Block).
- View-YAML mit allen Sub-Tabs als Default mitliefern.
- `cargo build --release` + `cargo test --release` + install.
- Bundle-Commit oder per-Phase-Commits — je nach User-Präferenz.

## Open Questions (vor CF-1 zu klären)

1. **Cookie-Quelle:** **Geklärt (User 2026-06-02): gleicher Crowd-SSO,
   gleicher Cookie-Pool wie Jira.** Bestehendes Skript wiederverwenden,
   evtl. `--path` / `--service`-Parameter, der die Cookies für den
   richtigen Subpfad filtert.
2. **DB-Cache-Schema:** **Geklärt (User 2026-06-02):** CF-1 startet
   mit nur `auth_session` + `view_sort_state`. User-Cache analog
   `JiraCache` (`Arc<Mutex<...>>` für `confluence_user`-Tabelle) wird
   später nachgereicht, **wenn es sich anbietet** — frühestens bei
   CF-7 (Comments brauchen Author-Auflösung) oder CF-12 (Comments-CRUD
   mit Mention-Autocomplete). Labels bleiben page-lokal, kein
   eigener Cache geplant.
3. **Storage-Format-Editor:** **Vorgehen (User 2026-06-02): bei CF-9 ad
   hoc evaluieren** — Roundtrip-Verhalten gegen reale Page testen
   (gemischtes XHTML mit Atlassian-Namespaces wie `<ac:structured-macro>`,
   `<ri:user>` etc.). Wenn `xmllint --format` nichts kaputt macht,
   bleibt's roh; sonst Custom-Pretty-Printer schreiben.
4. **CQL-Beispiele:** **Geklärt: post-compact** — wenn wir bei CF-8
   landen, klären wir die Defaults am echten Workflow.

## Anhang: Refactor-Kandidaten aus dem Adapter-Survey

(Vollständige Analyse: `memory/adapter_survey_2026_06_02.md`.)

Quick wins (vor oder direkt nach Confluence):

1. `sort_serde` aus den beiden Adaptern in `not-yet-done-content` ziehen.
2. `view_sort_state` SeaORM-Entity als shared definieren statt 2× zu
   duplizieren.

Größere Refactors (besser nach Confluence, weil dann 3 Datenpunkte):

3. `not-yet-done-adapter-common` mit `HttpClientBuilder` (auth-header
   injection, timeout, http_log, retry-on-401).
4. `SlugTable<T>` + `SlugResolver` Trait (Jira: `ll-`/`uu-`; Taiga:
   `ss-`/`uu-`/`tt-`; Confluence wird Labels + User auch brauchen).
5. `TemplateRenderer` für das 3b-Format (Header/Body/Comments).
6. `diffy`-basiertes 3-way-merge als shared utility.
7. Composite-ID-Codec (`<parent>/<type>/<child>` Parser & Builder).

Adapter-Registrierung:

8. Statt hardcoded HashMap in `main.rs::build_adapter_factories()`
   ein `inventory::collect!`-basiertes Registry-Pattern — Adapter-
   Crates registrieren sich selbst, `main.rs` muss nicht angefasst
   werden wenn ein neuer dazu kommt. Klein, lohnt sich aber.
