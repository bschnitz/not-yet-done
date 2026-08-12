# Generic View Specification

## Vision

Every main tab in the TUI is defined by a declarative YAML file. The TUI no
longer knows any Jira-specific logic — it renders generic "content views"
that are fed with data by a ContentAdapter. Tasks and trackings stay native
tabs (own DB, own logic).

---

## View configuration

Directory: `~/.config/not_yet_done/views/`

Each `.yaml` file = one main tab. At startup the TUI loads all files and
creates one tab per file. The order comes from the `order` field, otherwise
alphabetical.

One tab = one connection. Several instances of the same adapter type
(e.g. two Jira servers) = several YAML files = several tabs.

### Complete example: `jira.yaml`

```yaml
tab:
  name: Jira
  order: 3
  icon: 🎫 # optional, for the tab bar

adapter:
  type: jira # registered AdapterFactory name
  # Opaque config string — the format is determined by the adapter.
  # It can be a file path (relative to views/ or absolute)...
  config: jira-globex.yaml
  # ...or inline as a string:
  # config_inline: |
  #   url: https://jira.example.com
  #   session_id: abc123

views:
  # Every entry is a subtab or a navigable level.
  - name: Tickets
    node_type: jira:issue
    default: true # this subtab is shown when switching to the tab

    # Which query is used on load.
    query:
      default: "assignee = currentUser() ORDER BY updated DESC"
      editable: true # the user can enter their own query via :
      menu_key: q # opens the q menu with all saved queries

      # Note: saved queries are *no longer* enumerated here. Their bodies
      # live on the adapter side as individual files under
      # `<XDG_DATA_HOME>/not_yet_done/<adapter>/<instance>/queries/`
      # (see SavedQueryStore / FsSavedQueryStore), keyboard shortcuts in
      # the DB table `query_shortcut(scope, name, shortcut)`. Management
      # happens at runtime: `:query new`, `:query edit`, `:query delete`,
      # plus Ctrl+f in the q menu to bind a shortcut.
      #
      # Shortcut validation: saved-query shortcuts take effect at the
      # view-claim level and would shadow every key dispatched after them
      # (e.g. j/k navigation). When binding, the key is therefore checked
      # against *all* bindings active in the tab (globals, common
      # navigation, window chords including the leader prefix, subtab
      # keys, menu_key, YAML `actions:`/`shortcuts:`, chord prefixes such
      # as `z` in front of `zg`, other saved-query shortcuts) and rejected
      # on collision, naming the conflict. When loading from the DB
      # (externally written rows, or rows stale after config changes) a
      # collision produces a warning in the notification bar; the shortcut
      # stays active until the user rebinds it.
      #
      # Default query: in the q menu, `ctrl+t` (keybinding
      # `query_menu.set_default`, configurable in tui.yaml) marks the
      # selected saved query as the default (★ in front of the name;
      # shortcuts appear as a dimmed `[key]` suffix). The default query is
      # applied automatically at app start — it beats this view's YAML
      # `query.default` (content tabs) resp. the restore of the last
      # active filter (native tasks/trackings). Why: the YAML `default` is
      # the shared, checked-in preset; the default query is the personal
      # choice, repointable at runtime without a config edit. `ctrl+t` on
      # the current default query clears the mark again. Persistence: one
      # settings row `default_query:{scope}` per scope (content: the tab's
      # `query_scope`; native: `task`/`tracking`); if the query disappears
      # from the store, the default is silently skipped at start. Limit:
      # queries with mandatory variables (`{var}`) are applied raw at
      # start, without the variable popup.
      #
      # `inherit_default: true` (default false) additionally stamps the
      # user default query (★) onto *this* subtab at start — the plain
      # start-apply only hits the tab's default view. Why opt-in: sibling
      # views usually show *different* data where the same query means
      # nothing (Postgres tables vs. scripts); views that are only another
      # projection of the same rows (trackings normal/condensed/tree), on
      # the other hand, want the default filter everywhere (analogous to
      # the native tab, which had ONE filter state across all subviews).

    # Columns of the table — which metadata keys are displayed.
    columns:
      - key: key
        label: Key
        style: accent # reference to a theme color
        sizing: max # max = as wide as the content, flex(N) = proportional
      - key: type
        label: Type
        style: text_med
        sizing: max
      - key: status
        label: Status
        style: success
        sizing: max
      - key: priority
        label: Priority
        style: warning
        sizing: max
      - key: summary
        label: Summary
        source: label # "label" = node.label(), otherwise a metadata key
        style: text_high
        sizing: flex(1)

    # Preview pane configuration
    preview:
      enabled: true
      source: content # "content" = node.content().read_text()
      split: horizontal # horizontal (left/right) or vertical (top/bottom)
      ratio: 50 # percent for the preview side
      keybinding: P

    # Actions on selected nodes.
    #
    # `key:` may be a single key (`e`) OR a multi-character chord (`al`,
    # `ay`). Chords work on content tabs without further wiring: the app
    # chord interceptor only knows the typed `keybindings.*` sections, but
    # additionally asks the view keymap via
    # `ContentView::yaml_action_chord_prefix` — the first character of a
    # chord is stashed as a prefix, the second one fires. (Node
    # `shortcuts:`, by contrast, are single-character by definition and
    # never chords.)
    actions:
      - name: Edit
        key: e
        type: edit # opens the external editor
        # What is edited? The adapter generates the template and parses
        # the output (editor_template / parse_editor_output). Here you
        # only state WHICH fields should be editable:
        edit:
          content: true # content body (description)
          metadata: # additional editable metadata fields in the template
            - summary

      - name: Refresh
        key: r
        type: reload # reload the list

      - name: Open in Browser
        key: o
        type: open_url # opens node.metadata("url") in the browser

    # Navigation to child nodes (e.g. the comments of a ticket)
    children:
      - name: Comments
        key: Enter
        node_type: jira:comment
        columns:
          - key: author
            label: Author
            sizing: max
          - key: created
            label: Date
            sizing: max
          - key: body
            source: label
            label: Comment
            sizing: flex(1)
        actions:
          - name: Add Comment
            key: a
            type: create
          - name: Edit Comment
            key: e
            type: edit
            edit:
              content: true

      - name: Attachments
        key: A
        node_type: jira:attachment
        columns:
          - key: filename
            label: File
            sizing: flex(1)
          - key: size
            label: Size
            sizing: max
          - key: author
            label: Author
            sizing: max
        actions:
          - name: Download
            key: d
            type: download # node.content().read() → save to file

  # Second subtab: projects
  - name: Projects
    node_type: jira:project
    query:
      default: null # all projects
      editable: false
    columns:
      - key: key
        label: Key
        sizing: max
        style: accent
      - key: name
        source: label
        label: Name
        sizing: flex(1)
    actions:
      - name: Open
        key: Enter
        type: navigate # switches to the ticket list of this project
        navigate_to: Tickets # name of the view
        query_template: "project = {key}" # sets the target view's query
```

### Multi-line rows: `row_layout`

By default a view renders its items as a **single-line** table: one logical
row = one terminal line, all `columns` next to each other. With the optional
field `row_layout` a logical row is instead rendered as a **stack of several
physical lines** (chat layout):

```yaml
columns:
  - { key: author, source: author, style: accent, sizing: max }
  - { key: time, source: time, style: text_dim, sizing: max }
  - { key: content, source: content, markdown: true, sizing: "flex(1)" }
row_layout:
  - [author, time] # line 1: meta (emphasized via the columns' `style:`)
  - [content] # line 2: message text (markdown, multi-line)
  - [] # line 3: empty line (spacer)
```

**Why:** a flat `Author | Time | Message` table is unreadable for chat-/feed-like
data as soon as the message gets long. The chat layout separates metadata and
content visually and gives the body the full width.

Rules and behaviour:

- **Every entry** in `row_layout` is one physical line and lists the `columns`
  keys rendered there (left to right). The keys must be declared in `columns`
  (otherwise a hard validation error).
- **Empty list `[]`** = empty line/spacer.
- **Emphasis** works purely through the per-column `style:` (a theme
  reference, hence overridable via `tui.yaml`). There is no separate color
  option — if you want to emphasize the meta line, give its columns a
  `style:`.
- **Header:** in multi-line mode the column header is suppressed.
- **Selection:** when a row is selected, all physical lines get the selection
  background — **except** the spacer and every line that explicitly opts out:
  `- { columns: [foo], highlight_on_select: false }`. (An empty line defaults
  to `highlight_on_select: false`, a non-empty one to `true`.)
- **Limitation:** multi-line only applies to flat drill-down lists, not to
  tree views. Column cursor / horizontal scroll and the jump mode (`f`) still
  operate on the first (primary) line only.

#### `markdown:` — multi-line, soft-wrapped body

A column with `markdown: true` renders its value as **Markdown**: hard line
breaks _and_ soft wrapping at the pane width, plus inline styling
(`**bold**`, `*italic*`, `` `code` ``), lists, headings and blockquotes.
Intended for chat/long-text columns (e.g. the Stoat message body).

```yaml
- { key: content, source: content, markdown: true, sizing: "flex(1)" }
```

**Why it exists:** without `markdown:` a column truncates its value to a
single (possibly newline-collapsed) line — unreadable for longer messages.
`markdown: true` instead expands the value into as many physical lines as the
wrapping needs; the row height grows accordingly.

Rules:

- **Alone on its line:** a `markdown` column must be the **only** column in
  its `row_layout` line (`- [content]`). Otherwise a hard validation error
  instead of silently dropping the neighbouring columns.
- **Source:** as a rule `source: content` (or another metadata field), so that
  the **raw** body is read — not the `label` collapsed onto one line.
- **Colors** come from the theme (via the markdown theme bridge); there are no
  hardcodes and nothing to set per column.
- **Current cuts:** the `/` search does **not** highlight hits in the rendered
  body (filtering/matching still run over `label`/body), and code blocks get
  no background. Syntax highlighting is optional and separate.
- **Preview pane:** the same markdown rendering is available in the preview
  pane via `preview.markdown: true` (see the `preview:` block) — e.g. to show
  the full message body nicely rendered with `p`.

#### `sizing:` — column width

Per column, default `max`. Determines how the table engine distributes the
column width against the width budget (= the **actual pane width at render
time**):

| `sizing:`       | Behaviour                                                        |
| --------------- | ---------------------------------------------------------------- |
| `max`           | as wide as the widest content (capped at the free remainder)     |
| `fixed(N)`      | exactly `N` columns wide                                         |
| `flex(N)`       | shares the **remainder** by weight `N` with other `flex` columns |
| `fit`           | `min(content width, free remainder)` — content-wide, no stretch  |
| `auto(min,max)` | content-wide between `min`/`max`; **ignores the budget** (below) |

`flex` columns fill the space left over after `max`/`fixed` **up to the pane
width** — they do not blow the table up beyond the area. A `flex` column may
therefore also sit in the **middle** of the column list (e.g. the
task/description column): the columns behind it stay visible.

`fit` is the **combination of `max` and `flex`** (equivalent to CSS
`fit-content`): the column becomes as wide as its content, but never wider
than the space remaining after all `fixed`/`max`/`auto` columns —
`min(content width, free remainder)`. Unlike `flex` it therefore does **not**
stretch to the full pane width (if the content is short, the table stays
narrower than the area), and unlike `max` it is laid out **deferred** (only
after all fixed-width columns) — a `fit` column in the middle therefore never
pushes the following columns off-screen. Intended e.g. for the task column of
the tasks view, which should only be as wide as the longest visible task
title. If several `fit` columns sit next to each other, the leftmost helps
itself first; `flex` fills whatever remains after that. (Historically the
engine laid out against a fixed budget of 300 instead of against the pane
width; a non-final `flex` column then pushed the following columns off-screen.
Fixed — the engine now fits to the real pane width and re-fits on resize /
preview toggle.)

`auto(min,max)` is the special case for tables with an **unknown number of
columns** (e.g. dynamic Postgres rows): such columns deliberately ignore the
pane budget and may make the table wider than the area — then the
**horizontal scroll** kicks in (column cursor, only active with
`column_cursor: true`). Views with a fixed column list that fits into the pane
have no horizontal scroll, because all columns are on-screen.

#### `kind:` — typed column values

With `kind:` a column declares the **semantic type** of its value. The adapter
still only delivers strings, but in a **canonical** form; the table engine
parses them and takes care of formatting, alignment and styling. The default
is `text` — every existing (remote) column therefore stays unchanged.

```yaml
- { key: elapsed, source: elapsed, kind: duration } # right-aligned, H:MM:SS
- { key: started, source: started, kind: datetime } # localized
- { key: count, source: count, kind: number } # right-aligned
- { key: taskpath, source: taskpath, kind: path, separator: " › " }
- { key: running, kind: elapsed, elapsed_from: started } # live now − started
```

| `kind:`    | Canonical adapter input              | Display                           | Alignment |
| ---------- | ------------------------------------ | --------------------------------- | --------- |
| `text`     | anything                             | unchanged                         | left      |
| `number`   | decimal number (`"42"`)              | unchanged                         | right     |
| `duration` | integer **seconds** (`"5400"`)       | `H:MM:SS` (via `format_duration`) | right     |
| `datetime` | RFC 3339 (`"2026-06-09T08:15:00Z"`)  | local time zone, `%Y-%m-%d %H:%M` | left      |
| `path`     | `/`-separated segments (`"/a/b/c"`)  | joined with `separator:`, styled  | left      |
| `elapsed`  | _no own value_ — reads another field | `now − field` as `H:MM:SS`, live  | right     |

Three optional companion fields:

- **`format:`** — only for `datetime`: a strftime pattern replacing the
  default `%Y-%m-%d %H:%M` (e.g. `format: "%H:%M"`).
- **`separator:`** — only for `path`: the display separator (default `/`). It
  is drawn in the theme style `taskpath_separator` (bold), and the path always
  leads with a separator (a root renders as a bare separator).
- **`elapsed_from:`** — only for `kind: elapsed`: the key of the `datetime`
  field (RFC 3339) to compute against; the default is the column's own `key`.
  The column has no value of its own, it renders `now − <elapsed_from>` as a
  duration and is **recomputed on every repaint tick** (no refetch) — this is
  how, for instance, the running time of an active tracking ticks live. An
  instant in the future (clock drift) is clamped to `00`, an empty field stays
  empty, and an unparsable value is shown unchanged.

**Why it exists:** without `kind:` every adapter would have to pre-format
duration/date/path for display itself — either as an unaligned raw string or
with adapter-specific layout logic that cannot be aggregated or sorted. With
typed columns the **machine-readable** value stays the source of truth
(seconds, RFC 3339, path segments), and alignment, localized formatting and
the separator styling of the taskpath column are a generic engine feature
instead of copy-and-paste per adapter. The type deliberately lives in the view
YAML and not on the adapters' `MetadataField`, so that remote adapters (Jira,
Taiga, Postgres, Confluence, Stoat) need not change a line. `elapsed` is
additionally the only **time-dependent** type: its value depends only on the
display time, not on the loaded data — driven by the `Repaint` signal of the
domain event bus, the engine re-renders the affected panes per tick without
reloading.

#### `smooth_scroll:` — continuous line-wise scrolling

Lives on `ViewDef` **and** `ChildDef` (same level as `row_layout`), default
`false`. With `smooth_scroll: true` the table no longer scrolls discretely
from entry to entry, but **one physical line per step** across the whole
content — the content "travels" continuously across the screen. Intended for
long, multi-line lists (e.g. the chat).

```yaml
- name: messages
  node_type: "stoat:message"
  smooth_scroll: true
  row_layout: [...]
```

**Why it exists:** with multi-line rows (chat: meta + body + spacer) the
discrete mode jumps whole message blocks in and out, which feels jerky in long
histories. Line-wise scrolling reads smoothly.

Behaviour:

- **Navigation:** ↑/↓ scroll one line each; `Ctrl+u`/`Ctrl+d` and PageUp/Down
  by half resp. a full pane height (in lines); `g`/`G` to start/end. Bottom
  clamp: you cannot scroll past the end.
- **Selection (the cursor rides the leading edge):** the highlight (and the
  target of `e`/`d`/`+`/`p`) is bound to _one_ row, not to a screen position.
  Scrolling only moves the viewport, but every step also hands the selection
  **one row onward in the direction of travel as soon as that row can be
  seen** — down → the next selectable row the moment any of its highlightable
  lines enters the viewport, up → the previous one. The row you are leaving
  does _not_ have to disappear first. Only while the neighbour is still
  off-screen (a row taller than what is left of the viewport) does the
  selection stay put and the step is pure scrolling.

  The trigger is the **neighbour**, not the current row, because the opposite
  rule made `j` feel dead in a chat: a long message kept the highlight for a
  dozen keypresses while the next message already sat fully on screen. One step
  hands over at most one row, so `j`/`k` walk the list entry by entry.

  A line that opts out of the highlight (`highlight_on_select: false`, e.g. the
  trailing spacer of a chat row) does not count as "can be seen" — otherwise
  the selection would move to a row whose highlight is nowhere on screen. A
  page-sized jump (`Ctrl+u`/`Ctrl+d`) can outrun the one-row handover; the
  selection then re-attaches to the first row still visible. `g`/`G` explicitly
  select the first/last row; a programmatic selection (reload, jump, search)
  scrolls the target _minimally_ into view.

- **Cursor step when nothing scrolls:** because the selection is driven _by
  scrolling_, `j`/`k` would do nothing as soon as there is nothing to scroll —
  the whole list fits on screen, or the viewport already sits at the edge. So
  that the virtual cursor still moves, the selection jumps to the next/previous
  selectable row in that case. This also keeps the very first/last message
  reachable once scrolling has hit the end.
- **Orthogonal to `markdown:`/`row_layout:`** — it also works for single-line
  tables (there = line-wise scrolling), but it pays off with multi-line rows.

#### `card:` — card mode instead of a table

Lives on `ViewDef` **and** `ChildDef` (same level as `row_layout`), optional.
It unlocks a **second presentation** for this level: a logical row is not
rendered as a table row but as a **framed card** whose fields sit in a grid of
`columns:` slots per line.

The **number of card lines is derived** — `fields ÷ columns`, rounded up. Six
fields with `columns: 3` therefore give a **2×3 card**; there is deliberately
no `rows:` field that could get out of step with the field list. Surplus slots
in the last line stay empty so that all cards have the same height.

```yaml
- name: Tickets
  node_type: "jira:issue"
  columns:
    - { key: key, source: key, label: Key, style: accent }
    - { key: summary, source: summary, label: Summary }
    - { key: status, source: status, label: Status, style: secondary }
    - { key: assignee, source: assignee, label: Assignee }
    - { key: creator, source: creator, label: Creator }
    - { key: updated, source: updated, label: Updated, kind: datetime }
  card:
    key: C # toggle key (opt-in, no default binding)
    columns: 3 # three fields next to each other …
    fields: [key, status, updated, assignee, creator, summary] # … × 6 fields = 2 lines
    weights: [1, 1, 2] # the third slot gets half the inner width
    labels: inline # Label: value
    border: rounded
    padding: 1
    gap: 1 # empty line between two cards
```

**Why it exists:** `row_layout` solves the same problem (table too wide for the
content), but requires you to write out every physical line with its columns by
hand — with six fields that means two lists, both of which you have to touch
when reordering. Card mode needs **one** field list plus **one** number and
derives the line grid from it; on top of that come borders, labels and a gap
between the blocks, which `row_layout` does not know. And it is
**toggleable**: the same level stays reachable as a table instead of
committing to one presentation when writing the config.

Fields:

| Field           | Default   | Meaning                                                                                                                                |
| --------------- | --------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `fields:`       | all       | Fields in reading order, filled into the grid line by line. **Omitted = all columns of the level** in their order.                     |
| `columns:`      | `1`       | Fields next to each other per card line.                                                                                               |
| `weights:`      | equal     | Width weights per grid column. Empty = equal shares, otherwise exactly `columns:` entries.                                             |
| `labels:`       | `inline`  | `none` (values only), `inline` (`Label: value`), `above` (labels on their own line → twice as many lines).                             |
| `border:`       | `rounded` | `none`, `plain` (square), `rounded`.                                                                                                   |
| `border_style:` | theme     | Theme color for the border glyphs; without it the slot `card_border`.                                                                  |
| `label_style:`  | theme     | Theme color for the labels; without it the slot `card_label`.                                                                          |
| `padding:`      | `1`       | Empty columns between border and content (left and right).                                                                             |
| `gap:`          | `0`       | Empty lines after each card. They never take the selection background.                                                                 |
| `separator:`    | `"  "`    | Filler between two grid slots **within** a card line.                                                                                  |
| `divider:`      | empty     | Separator line **between two cards**: the glyph repeated across the card width (`divider: "─"`). Empty = `gap:` only.                  |
| `key:`          | —         | Key that toggles the mode on this level. Same forms as any other binding: alternatives (`key: [C, ctrl+d]`) and chords (`key: 'v c'`). |
| `default:`      | `false`   | The level opens directly in card mode.                                                                                                 |

A `fields` entry is either the bare column key (`- key`) — the label then comes
from the column's `label:`, otherwise from its `key` — or a map with its own
label: `- { column: key, label: "Ticket" }`.

**Omitting `fields:` entirely** means "show the whole table": the card takes
the level's column list in its effective order, `markdown:` columns excluded.
That is the default, because the alternative — writing out every column key a
second time — has exactly one failure mode: a column added later shows up in
the table but is silently missing from the card. An explicit `fields:` list is
only needed when the card should show **fewer** or **differently ordered**
fields than the table — e.g. to put long values on the wide slot (`weights:`).

Behaviour:

- **Available, not active.** A `card:` block does not switch the level over, it
  makes the mode _reachable_. `key:` toggles at runtime, `default: true` starts
  in card mode.
- **Opt-in key.** The action `toggle_card_mode` has **no** default binding — on
  levels without `card:` every key stays free. The key comes from `card.key`
  (or, if you want it globally, from `keybindings.content.toggle_card_mode`).
- **Collisions surface at load time.** `card.key` is — like
  `preview.keybinding` — tracked statically as a claim (`views.<v>.card.key`).
  A collision with an `actions:` key of the same level is therefore a config
  error at startup, not a silently lost toggle. The key is also directly
  editable in the keybinding editor (Ctrl+Y). A chord (`key: 'v c'`) is the
  convenient way out when all single keys of the level are taken — its prefix
  has to be free there.
- **Survives a restart.** The choice is stored **per level** (same level
  identity as the column overrides: `view:<name>` / `child:<view>/<child>` /
  `tree:<view>/<chain>`), in the settings store under `card_mode:<tab>`.
  Switching back to the config default deletes the entry again instead of
  pinning it.
- **The status bar** shows the key with the target mode, i.e. `cards` while the
  table is up and `table` while cards are up.
- **Flush right edge.** The widths are distributed by weight, the rounding
  remainder goes to the last grid slot — every card line is exactly equally
  wide.
- **Hidden columns drop out.** Hiding a column via the column popup (`c`) also
  hides it from the card; the grid closes up.
- **Grouping pauses.** While cards are up, `group_by:` is not applied — the
  card occupies the whole line block.
- **`divider:` instead of a border.** `border: none` plus `divider: "─"` is the
  frameless variant: no box, but a continuous line between two cards. The line
  takes the place of the **last** `gap:` line — `gap: 1` plus `divider:` is
  therefore exactly one line instead of an empty line, `gap: 0` gets its line
  anyway, and no line is drawn after the **last** card. Like the empty lines it
  does not take the selection background and is drawn in the border color
  (`card_border` resp. `border_style:`). Not to be confused with `separator:` —
  that is the filler between two slots _within_ a line.

Limits (v1):

- **Flat levels only.** Tree levels and the `record_detail:` follower pane do
  not offer the mode.
- **No `markdown: true` in the grid** — a markdown column expands into
  arbitrarily many soft-wrapped lines and does not fit into a slot of fixed
  height (hard validation error).
- **`sizing:` does not apply inside the card** — the slot widths come from
  `weights:`.
- **Cards only stacked**, not side by side (no kanban grid).
- Column cursor / horizontal scroll and the jump mode (`f`) work, as in
  multi-line mode, only on the first (primary) line.

#### `record_detail:` — record detail view in a split (`o`)

Lives on `ViewDef` **and** `ChildDef` (same level as `column_cursor`), default
`false`. On a **flat table level** marked this way, the key `o` opens a coupled
split on the right showing the **currently selected record transposed**: one
line per field, column 1 = field name, column 2 = field value. When the cursor
moves in the source table, the detail view updates automatically (it follows
the selection frame by frame). `o` again closes the follower; `X` toggles value
wrapping in the follower (default off → values clipped to a single line; on →
long values/hard line breaks are wrapped onto continuation lines).

```yaml
- name: Rows
  node_type: "postgres:row"
  column_cursor: true
  record_detail: true
```

**Why it exists:** Postgres rows (and script results) often have very many,
wide columns — reading a single record across all those columns is tedious in
the row view (you scroll horizontally). The transposed detail view shows _one_
record completely stacked, without the table view losing its layout.

Behaviour / limits:

- **Flat levels only.** Tree levels are excluded — a tree expands records
  inline anyway, and the detail split targets wide _flat_ rows (Postgres rows,
  script results). A follower does not offer `o` again itself.
- **Read-only (v1).** The detail view shows values, it does not edit them.
- **No fetch of its own.** The follower is built purely synthetically from the
  already loaded source record; it triggers no additional query.
- **Cascades on close.** If the source pane is closed, its detail follower goes
  with it (own backlink, separate from coupled drilldowns).

#### `window_ops:` — window/split operations (`w` leader)

Lives on `ViewDef`, default `false`. It unlocks the window operations reachable
via the `w` leader for this view: `wv` (split right), `ws` (split down), `wq`
(close pane), `wh`/`wl` (focus parent/child pane) as well as `w<tag>` (focus
the pane with that tag letter). The chords are configurable via
`keybindings.window`.

Deliberately **opt-in**: on every view without `window_ops: true` the `w`
leader does not engage and `w` stays a free, ordinary key (subtab switch, node
shortcut, …). Enable it only where several panes really make sense — a view
with a coupled child `split:` (e.g. the Stoat chat) or one with a
`record_detail:` split (e.g. Postgres rows).

```yaml
- name: chats
  node_type: "stoat:server"
  window_ops: true # `w` leader (split / close / focus / pane tag) active here
```

The switch applies to the whole view (all drilldown levels of this subtab) — it
is not set per child.

If `window_ops` is active, the **status bar** lists the chords with their full
mnemonic (`wv split right`, `ws split down`, `wq close pane`, `wh focus
parent`, `wl focus child`) — the same treatment as the tree fold chords
`zm`/`zr`. What is _not_ in the bar is `w<tag>`: the tag letter is assigned by
the current split layout and is therefore not a fixed binding; it only appears
in the WINDOW mode display of the action bar while the `w` leader is held.

#### `group_by:` / `aggregates:` — grouping & totals (M3)

Live on `ViewDef` **and** `ChildDef` (same level as `row_layout` /
`smooth_scroll`), both optional. They switch on the **grouped render path** of
the single-line table: the filtered entries are partitioned by **one** key,
every partition gets a **group header line** with a subtotal, and below the
whole table sits a pinned **grand total** (footer).

Engine-side grouping is deliberately **single-level**. Finer "condensing"
layouts (e.g. trackings "condensed": one summed line per day _and_ task) are
**the adapter's business**, not the engine's — they belong to data storage and
interpretation and can, where a DB sits underneath, be done natively as
`GROUP BY`. The adapter condenses its rows up front itself and delivers a flat
list, which is then grouped single-level by day here (see
`grouping::condense_cells` as a generic, opt-in building block).

```yaml
- name: grouped
  node_type: "tracking"
  group_by: { column: started, bucket: day, order: desc } # by day, newest first
  aggregates:
    - { column: duration, op: sum, total_column: total } # per-day sum + grand total
```

**`group_by:`** — what to group by. The mandatory field `column:` is a column
`key` (or a raw metadata field name if no column displays it). The optional
`bucket:` collapses a **`kind: datetime`** value into a date bucket instead of
grouping by the exact timestamp:

| `bucket:` | Label format   | Example      |
| --------- | -------------- | ------------ |
| `day`     | `%Y-%m-%d`     | `2026-06-09` |
| `week`    | `%G-W%V` (ISO) | `2026-W23`   |
| `month`   | `%Y-%m`        | `2026-06`    |
| `year`    | `%Y`           | `2026`       |

Without `bucket:` the column value is used **verbatim** as the group key (e.g.
a status or category column). The labels are deliberately chosen to be
**ISO-sortable**, so that the lexicographic ordering of the groups is at the
same time the chronological one.

In the **header line** the ISO key is additionally rendered human-readable
(pure display — identity and sorting stay the ISO key): `day` renders as
`W24 2026-06-08 Mon` (ISO week + weekday), `week` as `W23 2026`; `month`/`year`
and verbatim keys stay unchanged.

The optional `order:` (`asc` default, `desc`) determines the **order of the
groups** — with date buckets `desc` shows the newest bucket first (the usual
layout of a time log). The rows _within_ a group keep the adapter order
regardless. `zg` (below) picks up the configured `order:` when cycling.

**`aggregates:`** — list of the columns that are summed per group and overall.
Every entry has `column:` (a column `key`) and `op:` (currently only `sum`, the
default). The sum is computed on the **canonical** value (for `kind: duration`
therefore on the seconds number); it is rendered by the same typed formatter as
the data cells, so a duration sum appears as `H:MM:SS` again. Without
`aggregates:` there are no subtotals and no footer — what remains is plain
grouping with header lines.

The optional `total_column:` (a column `key`) moves the group sum out of the
`──` header line into this **dedicated column**, written onto the **last data
line** of every outermost group (and onto the `Σ` footer). This is the classic
timesheet layout where a "total" column closes off each day — the header line
then stays a plain label. The target column is declared as an ordinary column
(typically `kind: duration`); as long as grouping is switched off (`zg` on
`None`) it is **hidden**, because a group sum without groups would have no
content.

**Runtime toggle (`cycle_grouping`, default `zg`):** the action
`cycle_grouping` (bindable in `keybindings.yaml`, default `zg`) cycles the
grouping of the active level: ungrouped → `day` → `week` → `month` → `year` →
ungrouped. It is only active if the level has a `group_by:` configured at all.
The toggle state is view state (not persisted) and overrides the configured
`group_by:` only for the running session.

**Direct-jump menu (`group_menu`, default `u`):** instead of cycling,
`group_menu` opens a small hotkey popup over the same five states — `n` no
grouping, `d` day, `w` week, `m` month, `y` year (arrows + Enter/Space work
too, Esc cancels). Looks like the native grouping menu: standard popup chrome
with a keybinding legend at the bottom, `●` marks the current state, the hotkey
letter is underlined in the label. Same condition and same semantics as `zg`
(it only rotates the outer level, view state, not persisted) — it is the parity
with the `u` menu of the native trackings tab. On levels without `group_by:`,
`u` stays free for YAML `shortcuts:`.

**Flip the group order (`toggle_group_order`, default `o`):** on a grouped flat
view, `o` flips exclusively the **order of the groups** (e.g. day buckets
newest-first ⟷ oldest-first) — granularity (`bucket:`) and the item order
_within_ the groups are untouched (the latter is controlled by `S`, see below).
Same gate condition as `zg` (the level must have a `group_by:`); view state,
not persisted. `o` is only claimed as long as the view does not offer a
`record_detail:` split — there `o` keeps its detail-split meaning (see above).
The status bar shows the current direction (`order ↓` = descending / `order ↑`
= ascending).

**Item sorting within the groups (`sort`, default `S`):** `S` opens a two-step
picker (column → direction) over the columns reported by the adapter via
`sortable_columns()` and sorts the **individual items**. The sorting is
**adapter-driven**: every sortable column declares a `SortKind` (`Text`
lexicographic / `Number` numeric / `DateTime` chronological), and the adapter
applies it through the generic helper `apply_sort` — _before_ any grouping,
whose bucket sorting is stable, so that the chosen item order is preserved
within every group. Unparsable cells of a typed column (empty `ended`, the
literal `running`) sort to the end. Which sorting was actually applied is
reported back by the adapter via `ListResult::applied_sort` (footer indicator).
Adapters that sort server-side (Jira via JQL `ORDER BY`, Taiga) ignore
`SortKind` and translate the `SortKey`s into their backend language.

> **Restrictions.** The grouped path only applies to **single-line** tables —
> `group_by:` together with `row_layout:` (multi-line/chat) is ignored.
> Engine-side grouping is furthermore a flat-list feature; in tree mode the
> **adapter** groups instead (see the next section) — without the corresponding
> adapter capability, `group_by:` has no effect in a tree.

The color of the group header and footer lines is configurable via the theme
(`group_header`, see `tui-theme.yaml` / theme reference).

#### Adapter-side tree grouping (`group_by_via_adapter`)

A **tree** cannot be grouped engine-side: the engine loads lazily and cannot
fold the subtree sums of an individual bucket itself — the adapter owns the
fold (see `tree_aggregate:` below). That is why responsibility is inverted in a
tree: the engine passes the pane's active `group_by:` through to the adapter in
the root `list()` call (`ListParams.group_by`), and the adapter answers with
**one bucket node per group** as the root level; every bucket expands into a
subtree whose values are folded from the entries of _that_ bucket only.

```yaml
- name: tree
  node_type: "tracking:tree-group" # root level = the adapter's bucket nodes
  tree_label: task
  group_by: { column: started, bucket: day, order: desc }
  children:
    - name: subtasks
      node_type: "tracking:tree-item" # recursive item level
      recursive: true
```

- **Capability gate.** The adapter declares `group_by_via_adapter` (see
  `AdapterCapabilities`). Only then are `zg`/`u` active in a tree; without the
  capability a `group_by:` on a tree root level has no effect (same double-gate
  logic as `tree_aggregate`).
- **Toggling = reload.** `zg` and the `u` menu work in a tree just like in a
  flat list, but every change is an **adapter reload** (the adapter has to
  re-bucket), not a local rebuild. The state stays view state (not persisted).
- **"No grouping" = one config, two shapes.** If you switch grouping off, the
  adapter answers the same root request with the unbucketed tree (items instead
  of buckets). The engine's chain resolution matches **by type**: with buckets
  the root `ViewDef` level applies (`tracking:tree-group`), without buckets the
  recursive item `ChildDef` matches from depth 0. So no second view is needed —
  but the root level's columns/`shortcuts:` only apply to bucket rows (buckets
  are read-only aggregates; row actions belong on the item level).
- **Bucket identity is part of the node id.** The same task can appear in
  several buckets; nodes under a bucket carry the bucket scope in their id so
  that `get_by_id` can compute the right (bucket-folded) values without query
  context. The pane's saved-query filter additionally arrives per `list()`
  (`propagates_query_to_subtree`) and narrows the bucket further.
- **Consistent labels.** Bucket keys and display labels come from the same
  module (`not_yet_done_content::grouping`) that engine-side flat grouping uses
  — a day is named exactly the same in a grouped tree as in a grouped flat
  list.

##### `group_headers:` — buckets as `── label` header lines

Without further config the bucket nodes are **ordinary tree rows**: selectable,
with the items one indentation level deeper. That reads differently from the
same grouping on a flat list (there, group heads are non-selectable `── label`
lines without extra indentation). `group_headers:` on the tree root level
switches the bucket rows to exactly that header rendering:

```yaml
- name: tree
  node_type: "tracking:tree-group"
  tree_label: task
  group_by: { column: started, bucket: day, order: desc }
  expand_depth: all # mandatory in practice, see below
  group_headers:
    total: # optional: group total in its own column
      key: total
      label: Total
      kind: duration
      style: accent
      sizing: max
      source: duration # metadata field of the BUCKET node holding the total
```

- **Rendering.** Bucket rows become `── label` lines in the group header style
  (same chrome as flat grouping), **not selectable**; the rows below them lose
  the bucket's indentation level — the forest starts at indentation 0 under
  every header.
- **`total:` (optional).** A full `ColumnDef` that appears as the last column
  only while grouping is active and shows the group total on the **last** line
  of every group (the classic timesheet layout — same semantics as
  `total_column` of flat grouping). `source:` names the metadata field of the
  bucket node carrying the total (fallback: `key`). With grouping switched off
  the column disappears.
- **`expand_depth` is practically mandatory:** headers are not selectable, so a
  collapsed bucket could never be opened by cursor. The validator requires
  `tree_label` + `group_by` on the same view.
- Applies only while grouping is actually in effect (capability + active
  `group_by`); with "no grouping" the tree renders normally.

#### `tree_aggregate:` — own vs. summed value in a tree (M4)

The counterpart to `group_by:` for **tree mode**: per node, a column can show
either its **own value** (the column's `key:` field) or the **subtree sum**
computed by the adapter (`cumulated_field:`) — toggleable at runtime.

```yaml
columns:
  - { key: name, source: label }
  - key: duration # own value of the node (canonical: seconds)
    kind: duration
    tree_aggregate:
      cumulated_field: duration_cumulated # the adapter delivers the subtree sum
      default: own # own (default) | cumulated
```

- **`cumulated_field:`** (mandatory) — the metadata field key under which the
  adapter delivers the **already summed** subtree value (canonical for the
  column's `kind:`, e.g. seconds with `kind: duration`). The own value still
  comes from the column's `key:`.
- **`default:`** — which value is shown before the first toggle: `own`
  (default) or `cumulated`.

**Why adapter-driven?** The tree is loaded **lazily** — collapsed branches are
not in memory at all. The TUI therefore cannot fold by itself; **only the
adapter** knows whether it has the full tree and can sum up a subtree. It
delivers both values as metadata fields and declares the capability
`supports_tree_aggregation` for it (see `AdapterCapabilities`). If it cannot
cumulate, it omits the field.

**Runtime toggle (`toggle_tree_aggregate`, default `zt`):** the action toggles
**all** `tree_aggregate` columns of the active level between own and summed
value. It is only active if **two** conditions hold: the level (in tree mode)
has a `tree_aggregate` column at all **and** the adapter reports
`supports_tree_aggregation`. If it does not report the capability (or no
adapter is bound at all), the key stays unbound and the toggle is a no-op — a
`tree_aggregate:` declaration alone is therefore not enough. The state is view
state (not persisted).

> **Own _and_ summed value side by side** needs no new mechanism — just put two
> ordinary columns on the two fields (e.g. `key: duration` and
> `key: duration_cumulated`, both `kind: duration`).

> **Restrictions.** `tree_aggregate:` only takes effect in **tree mode**; in
> flat lists it is ignored. Mirror image of `group_by:`, which engine-side only
> takes effect in flat lists (in a tree the adapter groups, see
> `group_by_via_adapter` above).

#### `collapsed_source:` — a different metadata field on a collapsed node

A column can render a **different metadata field** while its row is a
**collapsed** tree node (has children, is not expanded). On expanded nodes,
leaves and in flat lists it shows its `source:`/`key:` field unchanged.

```yaml
columns:
  - key: tracking
    label: Tr
    # Collapsed node → shows the adapter's roll-up field instead of the own
    # `tracking`; expanded/leaf/flat → `tracking` again.
    collapsed_source: tracking_rollup
```

**What for?** Markers that are a state _within the subtree_ and would otherwise
disappear on collapse. Example tasks tree: the `⏱` tracking marker hangs on the
running task; collapse its parent and the marker would sit in the hidden
branch. The adapter additionally delivers a roll-up field (`tracking_rollup` =
`⏱` if the node **or a descendant** is tracked), and `collapsed_source` shows it
exactly while the node is collapsed — the marker visibly "bubbles" up to the
collapsed parent.

- **Why not propagate upwards always?** The engine knows the **collapse state**
  (`tree.expanded`), the adapter does not; the adapter knows the **subtree**,
  the engine (lazily loaded) does not. `collapsed_source` splits the
  responsibility: the adapter folds the roll-up field, the engine decides based
  on the collapse state whether the own or the roll-up field is shown.
- **Purely additive & generic.** No capability gate, no dedicated action — if
  the roll-up field is missing from the metadata, the cell renders empty (like
  any missing field). Freely reusable for other subtree markers (notes, links …).
- **Tree only.** In flat lists there is no collapse state; there
  `collapsed_source` is inert and the column always shows its `source:`/`key:`
  field.

#### `hidden:` — column hidden by default, but configurable

```yaml
columns:
  - { key: description, source: label, sizing: fit }
  - { key: tag_names, label: Tags, hidden: true } # there, but not shown
```

A column with `hidden: true` is part of the level's column list but is **not
rendered in the default layout**. It shows up in the `c` column-config popup as
an available, **unchecked** row — the user can reveal it there. That is exactly
what the flag is for: occasionally useful columns that would clutter the
standard layout (e.g. `tag_names` in the tasks tree, showing the spelled-out
tag names next to the compact symbol column) should be retrievable without
permanently costing space.

- **Default vs. override.** `hidden:` only takes effect as long as **no** column
  override is set for the level. As soon as the user (de)selects something in
  the popup, only their selection counts (a revealed `hidden` field then stays
  visible). If they reselect exactly the default-visible set (hidden columns
  off), the override is **deleted** — a clean reset.
- **The `tree_label` column ignores `hidden:`** — it carries the tree and can
  never be hidden.
- **Purely additive.** Default `false`; existing views without `hidden:` are
  unchanged.

#### `tree_connector_style:` — color of the connector glyphs per tree

In tree mode the `tree_label` column paints a **connector run** in front of the
label: the box characters `├──`/`└──`/`│` and the expand arrows `▶`/`▼`. This
run is colored separately from the label — it should be readable as a quiet
structure _behind_ the labels, not compete with them.

```yaml
views:
  - name: tasks
    tree_label: description
    tree_connector_style: text_dim # optional; otherwise theme color `tree_connector`
```

- A theme color name (`text_dim`, `tree_connector`, `accent`, … — the same
  vocabulary as for a column `style:`). Without it, the global theme color
  `tree_connector` (`tui.yaml`) applies.
- Lives **on the root `ViewDef`** and applies to the **whole tree** (all
  depths) — deliberately _per tree_, not per level: a dense, deep task tree
  wants duller connectors than a shallow one; a tree on a colored surface wants
  a different tint than one on the base background. This way every view tunes
  the contrast independently instead of forcing one global connector color.
- Only takes effect in **tree mode**; without `tree_label` it has no effect.

#### `tree_lines:` / `tree_markers:` — lines and expand markers per tree

The two constituents of the connector run are configurable separately: the box
lines (`├──`/`└──`/`│`) via `tree_lines`, the expand markers (`▶`/`▼`) via
`tree_markers`. Both live — like `tree_connector_style` — on the **root
`ViewDef`** and apply to the whole tree.

```yaml
views:
  - name: databases
    tree_label: name
    tree_lines: false # default true; false = indentation only instead of lines
    tree_markers: # optional; omit = ▶/▼ as before
      enabled: true # false hides the markers completely
      collapsed: "+" # default ▶
      expanded: "-" # default ▼
```

- **`tree_lines: false`** replaces the lines with plain indentation (two spaces
  per depth). Why: the lines carry sibling/continuation structure — worth it on
  deep, irregular trees (tasks), but visual noise on shallow, regular drilldowns
  (database → schema → table). Markers and `leaf_glyph` are untouched.
- **`tree_markers.enabled: false`** hides the expand markers; the rows remain
  expandable with the usual keys, only the visual hint is gone.
  `collapsed`/`expanded` override the glyphs individually — e.g. `+`/`-` for a
  more compact look, or Nerd Font icons.
- Both only take effect in **tree mode**; without `tree_label` they have no
  effect. The leaf symbol is still configured by `leaf_glyph` (per level), the
  color of the whole run by `tree_connector_style`.

#### `icon:` — type symbol per level

`icon:` lives **per level** (on the root `ViewDef` or on a `ChildDef`) and draws
a glyph directly in front of the label — on **every** row of that level,
expandable or not.

```yaml
children:
  - name: channels # uncategorized channels …
    node_type: "stoat:channel"
    tree_label: name
    icon: "💬"
  - name: categories # … and categories share one depth
    node_type: "stoat:category"
    tree_label: name
    icon: "📁"
```

- Distinction from `leaf_glyph`: that one encodes the **expand state** ("nothing
  comes below me"), `icon` the **kind** of the row. Two different questions — a
  level of nothing but expandable rows never gets a `leaf_glyph`, but very much
  wants a type symbol.
- It is needed everywhere **two branches share the same depth**: in the Stoat
  tree, both uncategorized channels and categories hang under the server.
  Without a symbol a channel reads there exactly like a category — and expanding
  it surprises you with messages instead of channels.
- Resolution as with `leaf_glyph`: the glyph of the producing `ChildDef`,
  otherwise the one of the `ViewDef`, otherwise none. The order in the label
  cell is `connector · leaf_glyph · unread_marker · icon · label`.
- An emoji is **two cells wide**; the column width accounts for the rendered
  width. Only takes effect in **tree mode**.

#### `unread_style:` / `unread_marker:` — unread highlight (chat adapters)

Chat adapters (Stoat) mark unread entries: a channel/category with unread
messages gets a **marker glyph** in front of the label in the tree, and marker +
name in the unread color; the header line (author/time) of an unread message in
the message list is painted in the same color. The carrier is an `unread`
metadata field (`"true"`/empty) on the node — the adapter sets it, the view
paints. Without that field both options have no effect.

```yaml
views:
  - name: servers
    tree_label: name
    unread_style: unread # optional; otherwise theme color `unread`
    unread_marker: "💬" # optional; default 💬, "" = color only, no glyph
```

- **`unread_style`** — a theme color name (`unread`, `accent`, … — the same
  vocabulary as for a column `style:`). Without it, the global theme color
  `unread` (`tui.yaml`) applies. Why per view: the unread emphasis competes with
  the view's own accents (selection, fuzzy hit, group header); a dense server
  tree and a flat message list want it at different strengths.
- **`unread_marker`** — the leading glyph; default `💬` (speech bubble). An
  empty string suppresses the marker (color only). Note: an emoji marker is
  **two cells wide** — the tree indentation accounts for the rendered width. Why
  configurable: emoji vs. Nerd Font glyph render differently depending on
  terminal/font; some prefer a quiet ASCII dot. If a level carries an `icon:`
  (type symbol), the marker must be a **different** glyph — otherwise the same
  glyph stands twice next to each other, meaning "unread" once and "is a
  channel" the other time.
- The fuzzy hit highlight wins over the unread color: with an active search the
  matched substring stays in the hit color, the rest of the label in the unread
  color.

#### `tab.unread_marker:` / `tab.unread_style:` — unread in the tab bar

The same `unread` metadata field also surfaces one level up: while **any** row
in **any** of the view's panes is unread, the view's entry in the tab bar gets
a marker glyph in front of its icon and its label is emphasised. That is what
makes a **background** chat tab useful — the tree inside it is not on screen,
the tab label is.

```yaml
tab:
  name: Stoat
  icon: "💬"
  unread_marker: "🔔" # optional; default = view's `unread_marker`, then 🔔
  unread_style: [bold] # optional; default = bold
```

Rendered: `🔔 💬 9 Stoat` — marker, icon, switch key, name, the same order the
tree uses inside the view.

- **`tab.unread_marker`** — the leading glyph. Unset, it falls back to the
  view's own `unread_marker` (so tree and tab agree without configuring the
  glyph twice), and only then to the default `🔔`. Note that this default is a
  **bell**, not the `💬` the rows fall back to: a tab already carries its own
  `icon:`, and in a chat view that icon is often the very speech balloon that
  marks "this is a channel" — the two must not collide. An empty string
  suppresses the glyph and leaves `tab.unread_style` to carry the signal.
- **`tab.unread_style`** — how the label itself is emphasised. Three forms:

  | Form                                              | Effect                           |
  | ------------------------------------------------- | -------------------------------- |
  | `unread_style: unread`                            | theme color name, no font change |
  | `unread_style: [bold]`                            | font modifiers, no recolor       |
  | `unread_style: { fg: unread, modifiers: [bold] }` | both                             |

  Modifiers: `bold`, `dim`, `italic`, `underlined`, `reversed`, `crossed_out`
  (how much a terminal honours is up to the terminal). Whatever it resolves to
  is layered **on top of** the bar's normal active/inactive style, so an
  omitted part keeps the theme's value. Unset, the label renders **bold** —
  the one emphasis that reads on both the active tab (already bold and
  colored) and the inactive one without fighting the bar's palette.

  Why this is its own setting rather than reusing the view-level
  `unread_style`: that one recolors _rows_, where hue is free to vary; the tab
  bar paints active/inactive tabs from the theme, so an unread tab usually
  wants a font change instead of a color.

Only what a pane holds **right now** counts — the tree's loaded nodes in tree
mode, the current level's items otherwise. A level a pane has drilled away from
is a frozen snapshot that no invalidation refreshes, so counting it could keep
the tab lit after everything was read. For a chat view that is also the right
rule: the tree keeps its own pane through the coupled `split:`, and the server
rows there already carry the unread state of every channel below them.

### `tab.load_banner` — where this tab says that it is loading

While a tab fetches, a progress line reports what it is doing and for how long
(`Loading… 40 % (3s)`). Where that line appears is configurable, globally in
`tui.yaml` and per tab in the view file:

```yaml
tab:
  name: Postgres
  load_banner: global # optional; default = notifications.load_banner (`tab`)
```

| Value    | Where the line goes                                                    |
| -------- | ---------------------------------------------------------------------- |
| `tab`    | on the tab's own banner line — visible only while that tab is in front |
| `global` | on the bar shared by all tabs, prefixed with the tab's name            |
| `off`    | nowhere                                                                |

The default is `tab`, and deliberately so: a load resolves by itself, so from
another tab it is noise. It is the opposite of an MFA prompt, which is global
because nothing happens until the user answers it. The per-tab override exists
because the cost of a load is per-tab — a query over a slow tunnel is worth
watching from elsewhere, a local task list is finished before the line is read.

Two details of the `global` route: the text names its tab, because the shared
bar cannot say which tab is meant; and several tabs loading at once collapse
into one counter (`3 tabs loading… (4s)`) rather than one line each, so loads
never crowd out the messages that need answering. When
`notifications.alert_enabled` is off, the counter uses the bottom notification
bar instead of the top strip, exactly as a prominent `notify` action does.

Only the progress line is routed. Errors, retries and login prompts stay in the
tab they happened in — they name a place the user has to go.

#### The `deleted` metadata field — soft-deleted rows dimmed

An adapter that keeps deleted records around as context (instead of removing
them hard) can **dim** them: if a node carries a `deleted` metadata field with
the value `"true"`, the engine paints **every cell** of the row in the theme
color `text_dim` — the row reads as "there, but inactive". On segmented cells
(tree label, taskpath) this dims the text while the structural glyphs
(connector, separator) keep their own color.

This is a **pure styling signal without view configuration** (no key, no opt-in
flag): the adapter sets the field, the view dims. It takes effect in **all**
render paths — the ungrouped flat list, the **grouped** flat view (`── day ──`
headers) and the tree. Unlike `unread`, the color is **not** overridable per
view; deleted-vs-active should look the same everywhere.

A deleted row only becomes visible if the query **includes** it — the adapters
load the full including-deleted universe, and the query is the sole filter (see
"Query = the only filter"). With the default query (`[deleted, =, false]`)
everything deleted stays invisible; only a query that includes deleted rows
shows them dimmed. Tasks and trackings use the same signal.

#### `mark_read_on_reach_end:` — action when the cursor hits the last row

A generic engine hook on the drill level (`children:`): when the cursor reaches
the last (bottom) row of a flat list **for the first time** **and** that row is
still unread, the engine invokes the named action on the selected node exactly
**once**.

```yaml
children:
  - name: messages
    node_type: "stoat:message"
    mark_read_on_reach_end: mark-read # action id on the message node
```

- **Value** — the `id` of an `invoke_action` the adapter understands on the row
  node (with Stoat, `mark-read` acks the channel up to that message and
  remembers the read state locally, whereupon the unread marker disappears). If
  the field is missing, the hook is off.
- **Two gates to keep it honest:** _arrival_ — the cursor has to newly land on
  the last row (not already be there), so that merely opening the list or a
  keypress at the end of the list triggers nothing. _Unread_ — the row has to
  carry the `unread` metadata field, so that the hook does not fire again after
  the reload triggered by the ack (the row then counts as read). Together the
  two make it idempotent.
- **Why generic instead of adapter-specific:** "cursor reaches the end of the
  list" is a pure view event (the engine knows the selection and the row count),
  while acking is an adapter action (only the adapter knows the REST `ack`). The
  hook connects the two via an action `id` without the engine having to know the
  chat notion of "reading" — any adapter can use it for a "seen up to here"
  semantics.
- **Flat mode only** (lists). In tree views without a clearly defined "end" the
  field is ignored.

#### `cursor_on_open:` — where the cursor lands when a level is opened

The counterpart of `mark_read_on_reach_end` at the _other_ end of the list: it
decides which row the cursor sits on when a drill level is **opened**, once its
items arrive.

```yaml
children:
  - name: messages
    node_type: "stoat:message"
    cursor_on_open: first_unread # first | last | first_unread
```

| Value          | Cursor lands on                                                                                           |
| -------------- | --------------------------------------------------------------------------------------------------------- |
| _(unset)_      | The first row — what every level did before this option existed.                                          |
| `first`        | The same, spelled out.                                                                                    |
| `last`         | The last (bottom) row. On an oldest-first chat page that is the newest message.                           |
| `first_unread` | The first row whose `unread` metadata field is `"true"`, **anchored at the top edge**; `last` if none is. |

- **Why `first_unread` anchors at the top** rather than scrolling minimally into
  view: the point of the jump is everything that comes _after_ the target. Parked
  at the bottom edge the unread run would sit off-screen below the cursor; at the
  top edge it reads downward, and scrolling on eventually hits
  `mark_read_on_reach_end`.
- **Why it falls back to `last`** when nothing is unread: with no catching-up to
  do, the newest row is what the user opened the channel for.
- **Only the opening applies it.** A reload of an already-open level — `r`, a
  live invalidation, an incoming message, a page change — leaves the cursor where
  the user put it. The placement is armed by the drill and consumed by the first
  load that brings rows in; a still-empty first page keeps it armed, so a channel
  that receives its first message while open still gets the jump.
  A reload re-selects the row **by node id**, not by row index: a feed that
  renders its newest page moves every row up as soon as one message arrives, so
  the old index would land on the neighbour. A node that is gone by then falls
  back to the index.
- **Same generic hook idea as `mark_read_on_reach_end`:** it reads the `unread`
  metadata field the adapter already sets for the row highlight, so any
  chat-/feed-style adapter opts in by naming a value — no frontend code.
- **Flat drill levels only** (`children:`). Ignored in tree mode, where an
  expansion is not a fresh list.

#### `expand_depth:` — initial expand depth per tree

```yaml
views:
  - name: tasks
    tree_label: description
    expand_depth: 2 # depth 0 and 1 expand automatically after loading
  - name: tree
    tree_label: task
    expand_depth: all # always fully expanded (e.g. the trackings tree)
```

Lazily loaded trees start fully collapsed by default — right for expensive
remote adapters (Postgres, Confluence), wrong for cheap in-memory forests
(tasks), where the user wants to see their working set immediately.
`expand_depth` on the **root `ViewDef`** automatically expands all rows with
depth `< expand_depth` after (re)loading the root list — `2` therefore shows
three levels (roots, children, grandchildren) and mirrors the native
`tasks.tree.default_expand_depth`.

- **One-shot cascade:** every level loads through the normal expand path (the
  same requests as a manual Enter). As soon as there is nothing left to load,
  the cascade disarms itself — manual expanding/collapsing is never overridden
  afterwards. A new saved query starts the cascade again on the filtered tree.
- **Runtime chords `zm` / `zr`:** in a tree pane, `tree_collapse_all` (default
  `zm`) collapses back to exactly this configured initial depth — an expanded
  path only survives while its depth is `< expand_depth`; deeper manual
  expansions are dropped (`expand_depth: 0`/omitted → back to the roots, the
  previous behaviour). `tree_expand_all` (default `zr`) is the counterpart: it
  arms the same cascade with an unlimited target depth and expands the whole
  tree, lazily loading as with `expand_depth: all`. Loaded children stay in the
  cache in both cases, so expanding/collapsing again is cheap. Both chords are
  only registered on tree panes (root `ViewDef` with `tree_label`).
- **`expand_depth: all`:** no depth ceiling — the cascade runs until a round
  finds nothing left to expand. Intended for small in-memory trees that should
  always be fully open (e.g. the trackings tree, native parity); on remote
  adapters use a number instead.
- **Cost:** one round of adapter calls per level (fan-out per node). Keep it
  small on remote adapters; `0`/omitted = off (default, previous behaviour).
- **A reload refreshes expanded levels:** if a root reload (the `r` reload
  action, an `Invalidation::All` from the adapter, or the post-mutation reload
  of an action) lands in a tree pane, the children of **every expanded** node
  are additionally re-fetched — the same requests as a manual
  collapse/expand, with the old rows staying visible until the new ones arrive
  (no flicker). Without it, deeper levels would stay at their pre-reload state
  (e.g. a tracking just started, without the `⏱` marker on a nested task).
  Expanded paths hidden under a **collapsed** ancestor are not refreshed —
  they fetch fresh data on the next expand.

#### Eager subtree (`supports_eager_subtree`) — the whole tree in one call

The cascade described above is correct, but expensive: one adapter fan-out per
level, and one tree rebuild per landed response. With `expand_depth: all` over
a deep in-memory forest this becomes quadratic (O(N²) rebuilds), because every
single node requests its children separately and the tree is re-flattened after
every response. For adapters whose data is fully in memory anyway (tasks,
trackings) this is pure waste — they could deliver the whole subtree in one go.

For that there is the contract addition `list_subtree(params, depth)` on the
`Node` trait and the capability gate `supports_eager_subtree` on
`AdapterCapabilities`:

- **`list_subtree(params, depth)`** returns a recursive `Subtree`
  (`{ items: Vec<SubtreeNode>, page }`, where every `SubtreeNode` carries its
  `summary` plus its own `children: Subtree`). `depth` is the target level:
  `list_subtree(depth)` delivers `depth + 1` visible levels, i.e. exactly the
  depths `0..=depth` the cascade would reach. The **default implementation**
  recurses via `list()` + `get_child()` (one call per node, identical to the
  cascade, only bundled server-side) — every adapter inherits it for free.
  In-memory adapters **override** it with a pure projection walk over their
  snapshot (no I/O, one pass). Nodes with `has_children == Some(false)` are not
  descended into.
- **`supports_eager_subtree`** switches the TUI from the cascade to a single
  `list_subtree` call: as soon as the root list has landed, the engine — if
  `expand_depth` is non-zero — requests the whole expected subtree (`all` →
  unlimited, `Levels(n)` → depth `n`) in **one** call and puts it into the tree
  cache in **one** pass (`ingest_subtree_level`), followed by **one** rebuild.
  The path scheme is byte-for-byte the cascade's (`parent_path + [node.id]`), so
  that selection, collapse and re-expand stay indistinguishable.
- **Why a gate instead of always:** remote adapters (Jira, Taiga, Postgres,
  Confluence) report `supports_eager_subtree: false` and keep the progressive
  cascade — a single blocking call across many levels would freeze the UI, while
  the cascade loads level by level and shows visible progress. Eager only pays
  off if the adapter can deliver the tree without network I/O.
- **Fallback:** if the eager call fails, the engine automatically falls back to
  the cascade (`drive_tree_auto_expand`) — the tree then simply expands
  progressively. Pagination (`… N more`) and live row patches are untouched,
  because the same `PageInfo` is passed through per level and the ids are
  identical.

#### Tree column inheritance — `columns:` once at the root

All rows of a tree render into **one** shared column grid. A `ChildDef` that
continues the tree (`tree_label` set) and declares **no** `columns:` block of
its own therefore inherits the columns of the nearest level above it that has
any. A tree thus only has to declare its columns **once at the root** instead of
repeating them identically at every depth (which inevitably drifts apart).

The inheritance runs **once, directly after the parse**
(`inherit_tree_columns`), before the validator and any runtime column query read
the config — both therefore already see a fully populated set and need no
inheritance logic of their own. The scope is deliberately narrow:

- **Only tree-continuing levels inherit** (gate: `tree_label` set). A pure drill
  child without `tree_label` stays empty and keeps the auto fallback from the
  item metadata (e.g. the Postgres rows level, see below).
- **A level with its own `columns:` stays untouched** and itself becomes the
  inheritance source for continuing levels below it — if you deliberately want
  to deviate, declare your own columns.
- **Separate views do not inherit across the view boundary** (a flat list
  `ViewDef` next to the tree stays independent and declares its columns itself).

```yaml
views:
  - name: tasks
    tree_label: description
    columns: # declared here once …
      - { key: status, label: St }
      - { key: description, label: Task, source: label }
    children:
      - name: subtasks
        tree_label: description
        recursive: true
        # … no columns: — inherits St/Task from the root.
```

#### Tree action and shortcut inheritance — `inherit:` per entry

Analogous to the columns, `actions:` and `shortcuts:` entries can be inherited
**down** the tree-continuing levels, so that the recursive branch does not have
to repeat them verbatim. The inheritance is **fine-grained and opt-in per
entry**, not all-or-nothing:

- An `actions:` entry is inherited if it carries `inherit: true`.
- A `shortcuts:` entry is inherited if it uses the long form
  `{ action: <name>, inherit: true }` instead of the short form
  `<key>: <name>` (see [per-node actions](#per-node-actions-shortcuts)).

The inheritance runs **once, directly after the parse**
(`inherit_tree_actions`, next to `inherit_tree_columns`), before the validator
and the runtime read the config. The scope is deliberately narrow — the same
three rules as for the columns, plus an **override-per-field** rule:

- **Only tree-continuing levels inherit** (gate: `tree_label` set).
- **Override by key, per field:** if the child level declares the same key
  (action `key` resp. shortcut char) itself, the local entry wins — the
  inherited one is not copied for exactly that key. This makes it possible to
  inherit _one_ thing and override _another_ deliberately.
- **Inherited entries keep their inheritability** and cascade further down
  (relevant with more than one continuing level).
- **Separate views do not inherit across the view boundary** (a flat list
  `ViewDef` next to the tree binds its keys itself).
- **The single-level search family is never inherited:** `fuzzy_filter`,
  `search` and `tree_find` are excluded from inheritance (even if
  `inherit: true` were set), because the validator restricts them to the tree
  root anyway.

```yaml
views:
  - name: tasks
    tree_label: description
    actions:
      - { name: edit, key: e, type: edit, id: edit, inherit: true }
      - {
          name: add,
          key: a,
          type: create,
          id: add,
          under_selection: true,
          inherit: true,
        }
      - { name: fuzzy filter, key: f, type: fuzzy_filter } # not inheritable
    shortcuts:
      d: { action: delete, inherit: true } # inherited
      s: toggle-tracking # short form → NOT inherited
    children:
      - name: subtasks
        tree_label: description
        recursive: true
        # no actions:/shortcuts: — inherits edit/add + `d` from the root.
        # `s` (short form) and `f` (search family) are not inherited.
```

#### Column config popup (`c c`) — visibility & order at runtime

The column config popup (`common.column_config`, default `c c`) works on content
tabs exactly as on the native tasks/trackings tabs: show/hide columns (`Space`)
and reorder them (`Ctrl+D`/`Ctrl+F`), applied with `Enter`. It exists so that
users can adapt the column layout to their work **without editing the view
YAML** — the YAML stays the shared default definition, the popup is the personal
override on top of it.

- **Configurable per level:** every level has its own layout — the root view,
  every drilled child level, and in tree mode every `node_type_chain` (the
  cursor row decides which level is configured). Splits of the same level share
  the layout.
- **Tree levels with the same column set share one override.** Since all levels
  of a tree render into **one** grid, an override per depth would be absurd:
  `c c` on the root would not cover the children, and two depths could drift apart.
  The override key therefore collapses across tree levels that show the
  **identical** column set onto the column-declaring ancestor level — which is
  exactly the case produced by
  [tree column inheritance](#tree-column-inheritance--columns-once-at-the-root)
  (inherited level == root), and it also folds every recursion depth (all
  resolve to the same `ChildDef`) onto **one** key. `c c` at any depth therefore
  configures the whole tree. A level that deliberately declares **deviating**
  `columns:` keeps its own per-level key and stays independently configurable.
  (Consequence: old overrides of a uniform tree, stored per depth before this
  rule, no longer fit the new keys and are ignored — the tree then shows the
  YAML default again until it is reconfigured.)
- **Persistence:** one settings row per tab (`content_columns:<tab name>`, a
  JSON map level key → visible column keys in order), loaded at startup.
- **Reset semantics:** if the selection matches the YAML order exactly again,
  the override is removed (and with an empty map the settings row is deleted) —
  a reset layout leaves behind no state that could mask later YAML changes.
- **The `tree_label` column is fixed:** it carries the tree itself (connectors,
  indentation) and cannot be hidden.
- **Auto-fallback levels** (no `columns:` in the YAML, the schema derived from
  the item metadata — e.g. Postgres rows) are not configurable; `c c` reports
  that via a notification. There is no stable column identity there that an
  override could pin to across reloads.

#### Sort menu (`c s`) — the whole sort spec in one list

The sort menu (`common.sort_menu`, default `c s`) lists **every** sortable
column of the active level: the ones currently sorted first, in sort order and
with their direction, the unsorted ones below. It is the second UI path onto the
same state the sort-hint mode (`common.sort_mode`, default `S`) edits — both
write through one commit function, so a sort built with `S` reads back in the
menu and vice versa.

- **Keys:** the list navigation keys (`common.list_prev`/`list_next`, by default
  `k`/`j`) move the cursor, `Ctrl+K`/`Ctrl+J` move the selected entry within the
  sorted block, `a` sorts ascending, `d` descending, `0` takes the column out of
  the sort again. `Enter` applies, `Esc` discards.
- **Sorted entries stay a prefix of the list.** That is what makes the list
  readable as the sort spec (rank 1, 2, 3 …), and it is why `a`/`d` on an
  unsorted column appends it to the sorted block — the same "append" the
  hint mode performs — while `0` drops it back into its natural column position.
  An unsorted entry has no rank, so `Ctrl+K`/`Ctrl+J` do nothing on it.
- **Nothing is applied while the menu is open.** For adapter-side sorts every
  keystroke would otherwise cost a reload; the menu is a draft that `Enter`
  commits (and only then, if the spec actually changed, reloads and persists).
- Levels that expose no sortable columns say so via a notification instead of
  opening an empty popup — the same gate `S` uses.

### Second example: `confluence.yaml`

```yaml
tab:
  name: Wiki
  order: 4

adapter:
  type: confluence
  config: confluence-prod.yaml

views:
  - name: Spaces
    node_type: confluence:space
    default: true
    columns:
      - key: key
        label: Key
        sizing: max
        style: accent
      - key: name
        source: label
        label: Name
        sizing: flex(1)
    children:
      - name: Pages
        key: Enter
        node_type: confluence:page
        columns:
          - key: title
            source: label
            label: Title
            sizing: flex(1)
          - key: last_modified
            label: Modified
            sizing: max
          - key: author
            label: Author
            sizing: max
        preview:
          enabled: true
          source: content
        actions:
          - name: Edit
            key: e
            type: edit
            edit:
              content: true
          - name: Create Page
            key: a
            type: create
```

### Example: adapter config `jira-globex.yaml`

The adapter config is adapter-specific. For Jira, for example:

```yaml
url: https://jira.example.com
session_id: "JSESSIONID=abc123; atlassian.xsrf.token=..."

# Adapter-internal caching (optional, defaults live in the adapter)
cache:
  labels:
    enabled: true
    ttl: 3600
  users:
    enabled: true
    ttl: 3600
```

The caching is adapter-internal — the adapter needs it in order to provide e.g.
labels/users for autocomplete in the editor template. The generic view YAML
knows nothing about it.

---

## Adapter construction

### Current (Jira-specific)

```rust
JiraAdapter::from_connection(&JiraConnection) -> Result<JiraAdapter>
```

### Generic (via AdapterFactory)

```rust
pub trait AdapterFactory: Send + Sync {
    fn adapter_type(&self) -> &str;
    fn create(&self, config: &str) -> Result<Box<dyn ContentAdapter>>;
}
```

The TUI:

1. Reads `adapter.type` from the view YAML → e.g. `"jira"`
2. Finds the registered `AdapterFactory` for that type
3. Reads `adapter.config` (file content) or `adapter.config_inline`
4. Calls `factory.create(config_string)`
5. Receives a `Box<dyn ContentAdapter>`

The adapter parses the config string internally — it can be YAML, JSON, TOML, a
connection string or whatever else.

---

## Caching

### Responsibility: adapter-internal

Caching is **entirely adapter-internal**. The TUI knows nothing about it.

**Why?** The adapter needs cached data for its own logic:

- Editor templates with autocomplete hints (labels, users, status values)
- Schema discovery (which fields exist?)
- Avoiding redundant API calls on quick list/get_by_id sequences

### Interface towards the TUI

The TUI needs no explicit cache access. Instead the adapter delivers the cached
data implicitly through the existing trait methods:

- `editor_template()` already contains the autocomplete hints from the cache
- `schema()` delivers fields including `allowed_values` from the cache
- `list()` can cache internally, the TUI does not notice

The only TUI interaction: a generic "refresh" could call
`adapter.invalidate_all()` if the adapter offers it.

### Adapter-internal implementation

Every adapter decides on its own cache strategy. For example:

```rust
// Internal to JiraAdapter
struct JiraCache {
    labels: Option<(Vec<String>, Instant)>,   // (data, loaded_at)
    users: Option<(Vec<JiraUser>, Instant)>,
    ttl: Duration,
}

impl JiraCache {
    fn labels(&self) -> Option<&[String]> { /* checks the TTL */ }
    async fn ensure_labels(&mut self, client: &JiraClient) -> &[String] { /* lazy load */ }
}
```

Configurable through the adapter's own config (e.g. `cache.labels.ttl` in
`jira-globex.yaml`), not through the view YAML.

---

## Editor templates

### Problem

Currently the TUI assembles the editor template by hand
(`"# KEY: summary\n\ndescription"`). That is Jira-specific.

### Solution: the adapter supplies the templates

An extension of the `Node` trait (not `Content`, since metadata fields are
involved too):

```rust
#[async_trait]
pub trait Node: Send + Sync {
    // ... existing methods ...

    /// Generate the template for the external editor.
    /// Contains the editable fields + the content body in a format
    /// readable for the user.
    /// `editable_fields`: which metadata keys should be editable
    /// (from the view YAML actions.edit.metadata).
    async fn editor_template(&self, editable_fields: &[String]) -> Result<String>;

    /// Parse the editor output → (metadata_changes, new_content_body).
    /// Returns an error if the format is invalid.
    fn parse_editor_output(
        &self,
        text: &str,
    ) -> Result<(Vec<MetadataChange>, Option<Vec<u8>>)>;
}
```

### Example: Jira issue template

```
# PROJ-202
# Type: Bug | Status: In Progress | Priority: High
# ─────────────────────────────────────────────────
summary: Fix login timeout on mobile devices
labels: mobile, auth, bug       # available: mobile, auth, bug, feature, backend, ...
assignee: jane.doe           # available: jane.doe, john.smith, ...
# ─────────────────────────────────────────────────

Login timeout occurs after 30 seconds on iOS devices...
```

The adapter generates this template and also knows how to parse it back:

- Lines starting with `#` = comments (read-only info)
- `key: value` lines before the separator = editable metadata
- Everything after the separator = the content body

The `# available:` hints come from the adapter-internal cache. The TUI does not
have to care.

### Example: Confluence page template

```
# Space: DEV | Last modified: 2024-03-15
# ─────────────────────────────────────────────────
title: Architecture Decision Records
labels: architecture, adr
# ─────────────────────────────────────────────────

## ADR-001: Use PostgreSQL for persistence
...
```

### Generic editor flow in the TUI

```
1. The TUI calls node.editor_template(editable_fields)
   → the adapter generates the template (with autocomplete hints from its cache)
2. The TUI opens the external editor with the template
3. The user edits and saves
4. The TUI calls node.parse_editor_output(text)
   → the adapter parses it back: (metadata_changes, new_content)
5. The TUI calls node.update_metadata(changes, version) and/or
   node.content_mut().write(content, version)
```

The TUI does not know the template format — the adapter is responsible for it.

---

## Navigation: breadcrumbs & going back

When the user navigates into child nodes (ticket → comments), the TUI needs a
navigation stack:

```
Jira > Tickets > PROJ-202 > Comments
```

### Implementation

```rust
struct ContentView {
    adapter: Arc<dyn ContentAdapter>,
    view_config: ViewConfig,          // from the YAML
    nav_stack: Vec<NavFrame>,         // breadcrumb stack
}

struct NavFrame {
    node_id: String,                  // id of the parent node
    node_label: String,               // for the breadcrumb display
    view_def: ChildViewDef,           // column/action config
    scroll_position: usize,           // for restore when going back
    selected_index: usize,
}
```

- `Enter` / the configured key → push a frame, load the list of child nodes
- `Esc` / `Backspace` → pop the frame, back to the parent list
- The breadcrumb is shown in the tab bar or as a line of its own

### Jump mode (`jump_mode`, default `J`)

A Vimium-style direct jump across the visible rows — parity with the native
tasks tab (`p` there). The action `jump_mode` is bindable in
`keybindings.yaml` under `content:`; the default is `J` (capital J), so that
the adapter tab keeps `p` free for a `paste`/`paste-move` shortcut (the native
tab still uses `p` via `common.jump_mode`).

Flow:

1. `J` opens the jump overlay (phase 1).
2. Type any character → every visible row containing that character gets a
   label (phase 2). If there is only one hit, the cursor jumps there
   immediately; with no hit the overlay closes.
3. Type the label → the cursor jumps to the corresponding row. `Esc` cancels at
   any time.

The label alphabet comes from `navigation.jump_chars` (shared with the native
tab). The jump only affects the focused pane; in splits it applies to the
currently active pane.

### Link hop (`link_hop`, opt-in)

Vimium-style link selection: the configured key (usually `f`) labels every link
visible in the focused pane; typing the label opens the corresponding URL in the
browser. Useful above all in markdown-rendered panes (e.g. the Stoat chat).

**Opt-in per view/child** — there is _no_ built-in default. Link hop is only
claimed where a binding exists: either on a view resp. a child via
`keybindings: { link_hop: f }`, or globally via `keybindings.content.link_hop`
in `tui.yaml`. Without a binding the key stays free. This way link hop can be
offered exactly on the panes that actually carry links (e.g. the messages pane
of the Stoat chat):

```yaml
children:
  - name: messages
    node_type: "stoat:message"
    keybindings: { link_hop: f }
```

Two link forms are recognized in the row text:

- **bare URLs** — `https://example.com/x`
- **markdown links** — `[text](url)`; the displayed `text` is labelled, the
  `url` is opened (the markdown renderer only shows the text, the URL is
  reconstructed from the raw text).

Flow: `f` labels all visible links; type the label → the URL is opened with the
configured opener (`Esc` cancels; if there is no link, a hint appears). The
overlay shares its label alphabet (`navigation.jump_chars`) and its interaction
logic with the jump mode.

The opener is configurable via `navigation.link_opener` (default `xdg-open`);
the string is split at whitespace and the URL appended as the last argument — so
flags work too, e.g. `firefox --new-tab`. The process is started detached (own
process group, `/dev/null` stdio), so the TUI never blocks on the browser.

```yaml
navigation:
  jump_chars: "abcdefghijklmnopqrstuvwxyz"
  link_opener: "xdg-open"
```

---

## Per-node actions (`shortcuts:`)

Besides the `actions:` entries of a view (refresh, filter, search, …),
individual nodes can offer _their own_ actions — e.g. a TableNode offers
`edit_sql`, a DbScriptNode offers `execute`, `edit`, `delete`. They are
advertised by the adapter via
[`Node::actions()`](../not-yet-done-content/src/lib.rs) and bound to keys from
the YAML through the `shortcuts:` map.

```yaml
children:
  - name: DB Script
    node_type: "postgres:db_script"
    shortcuts:
      x: execute # → Node::invoke_action("execute", …)
      e: edit # → Node::invoke_action("edit", …)
      d: delete # → Node::invoke_action("delete", …)
```

A `shortcuts:` map exists both on the view level (`ViewDef.shortcuts`) and on
every `ChildDef`. The TUI resolver picks the deepest matching entry along the
`node_type` chain; if none applies, it falls back to the view level.

Action values can be prefixed with `parent:` — the resolver then fires against
the immediate parent node instead of the selected one. Example:

```yaml
- name: Rows
  node_type: "postgres:row"
  shortcuts:
    Q: "parent:edit_sql" # → acts on the underlying table node
```

A shortcut value has **two forms**: the short form `<key>: <action>` (above)
and the explicit map form `<key>: { action: <action>, inherit: <bool> }`. Both
bind the same action name; the map form additionally carries the `inherit` flag
(default `false`), which inherits the shortcut down the tree-continuing levels
(see
[tree action and shortcut inheritance](#tree-action-and-shortcut-inheritance--inherit-per-entry)).
The `parent:` prefix works in both forms.

```yaml
shortcuts:
  d: delete # short form — this level only
  s: { action: toggle-tracking, inherit: true } # inherits downwards
```

**`under_selection` on `type: create` actions:** by default a `create` action
creates the new child in the currently drilled container (root → top level,
drilled into a task → its child). With `under_selection: true` the **marked
row** becomes the target parent instead — in a tree the create thus nests under
the cursor without drilling in first. If nothing is selected (empty tree), the
engine resolves the parent to the adapter root, so that both cases become a
top-level create. This is how the tasks tab implements `a` (child of the
selection / top level via the adapter id `add`) and `A` (sibling via the adapter
id `add-sibling`).

**`on_container` on `type: custom` actions:** an action that acts on the
**whole list/level** (instead of on a row) is declared as an `actions:` entry
with `on_container: true` — not as a `parent:` shortcut. The difference is
visibility and reachability at the flat root level: a `parent:` shortcut
resolves its target from the nav stack, which at the not-yet-drilled-into root
is **empty** → the hint disappears and the key does nothing. An `on_container`
action, by contrast, builds its hint **statically** from the config (so it is
always visible) and dispatches against `adapter.root()` via the `invoke_action`
path (not the popup/`execute` track). This way the returned `ActionDispatch` —
e.g. a `Confirm` — is effective. Today only `type: custom` uses this flag;
example: the trackings tab's `A restore all` (restores the deleted trackings
**within the active query** — see set scoping below — asking for confirmation
via `(y/n)` first). The adapter action name comes from the `id:` field that the
root node handles in `invoke_action`.

```yaml
actions:
  - name: restore all
    key: A
    type: custom
    id: restore-all # Node::invoke_action("restore-all", …) on adapter.root()
    on_container: true
```

What `Node::invoke_action(name, ctx)` returns is described by the
[`ActionDispatch`](../not-yet-done-content/src/lib.rs) enum (`OpenEditor`,
`ExecuteQuery`, `CreateChild`, `DeleteSelf`, `Reload`, `Confirm`, `Noop`,
`Error`). The TUI translates that into the matching view flow — an editor
opens, a query lands in a paginated result pane, a delete spawns a confirm
popup. `Reload` refetches the pane at the level it currently shows — a drilled
pane re-lists its child level under the same parent, never the root view. `Confirm { prompt }` is the **generic** confirmation mechanism: the
adapter returns it on the first call (when `ActionContext.confirmed == false`),
the TUI shows the `(y/n)` prompt, and on "y" the same action is invoked again on
the same node with `confirmed: true` — then the adapter does the (often
irreversible) work. Unlike `DeleteSelf` (whose confirm/execute split lives in
the TUI's delete plumbing), **any** action can thus guard itself behind a
confirmation, and the adapter phrases the text, because only it knows what the
action does (e.g. how many follow-up intervals a restore purges).

#### Set scoping: set-wide actions follow the active query

**Contract:** every action that acts on **more than the calling node** — a
container/list-wide action (`restore-all`), a bulk delete, an aggregate
operation — MUST scope to the **visible set** of the pane, never to the
adapter's entire (including-deleted) loaded universe.

The channel for that is `ActionContext.query: Option<String>`: the TUI puts the
pane's **active query text** in there — exactly the same filter string it passes
to the adapter on every `list()` via `LoadParams.query` anyway. The adapter
re-resolves the visible set from it itself (e.g. via `find_filtered`), just like
on a list load. This is **not** feeding rendered content back (no id list, no
table rows) — only the _identity_ of the active filter, which the adapter knows
anyway. `None`/empty means "no filter" → the whole list is in scope (= what the
pane shows).

The query is therefore the **only** lever: a `restore-all` reaches deleted rows
only if the query makes them visible (the query is the sole filter, a
`deleted=false` is baked in nowhere — see "query = the only filter"). If the
user wants to restore all deleted rows, they include them in the query, instead
of an action touching the whole DB past the filter.

Single-node actions (deleting/restoring one row, a toggle) ignore `ctx.query` —
their target already is the calling node. A natural boundary: `task.undelete`
(tasks tab) restores the **most recently deleted** task — an undo step, not a
set operation; it deliberately does not scope over the query, because by
definition it concerns exactly one, the latest.

Validator (start time): empty action ids are rejected; so is a `shortcuts:` key
already claimed by an `actions:` entry of the same view.

### Structured input forms (`InputSpec::Form`, M6/E5)

Instead of an external editor (`InputSpec::Editor`), an action can request a
**generic form** in the terminal. The adapter declares the fields in the action
descriptor; the TUI renders them generically (text, select, toggle and datetime
field, via the spec-driven form driver from `not-yet-done-ratatui`), collects
the values and hands them back to `Node::execute` via
`ActionInput::Form(HashMap<String,String>)`. There is **no** YAML for this — the
field structure is adapter knowledge, not view configuration.

```rust
// in the adapter, in Node::actions():
NodeAction::new(
    "edit",
    "Edit",
    InputSpec::Form {
        fields: vec![
            FormFieldSpec::text("title", "Title"),
            FormFieldSpec::select("status", "Status",
                vec!["todo".into(), "in_progress".into(), "done".into()]),
            FormFieldSpec::toggle("urgent", "Urgent"),
        ],
    },
)
```

Field types (`FormFieldKind`):

| Kind     | Widget               | Value in `ActionInput::Form`      |
| -------- | -------------------- | --------------------------------- |
| `Text`   | single-line text box | free string                       |
| `Select` | horizontal choice    | the chosen `allowed_values` entry |
| `Toggle` | on/off               | `"true"` / `"false"`              |

Per field:

- **`required`** (default: `text`/`select` = true, `toggle` = false) — the TUI
  blocks submission while a mandatory field is empty and shows a hint in the
  popup footer.
- **`default`** — a static initial value. For an edit form the adapter overrides
  it per field via **`Node::form_prep(action_id)`** (returns a
  `HashMap<key → initial value>`; missing keys fall back to `default`). The
  default trait implementation returns an empty map — right for a pure create
  form.
- **`masked`** (default: false, `Text` only) — renders the value as bullets
  instead of its characters. For passwords, API tokens and anything else that
  must not stand readable on a shared screen; the adapter still receives the
  clear text in `ActionInput::Form`. Masking is deliberately a display-only
  decision, so no adapter has to treat a secret field differently on the way
  back.

Handling in the popup: `tab`/`↑`/`↓` switches the field, `←`/`→` moves the
cursor (text) resp. the choice (select), `space` picks the option under the
cursor resp. flips the toggle, `enter` submits, `esc` cancels.

Why a form instead of an editor template: for small, clearly typed inputs
(status choice, boolean flag, short title) a structured popup is faster and less
error-prone than a YAML buffer in `$EDITOR`. For long free text (issue body,
wiki page) `InputSpec::Editor` remains the right choice. Both ways are usable
uniformly across all adapters.

### Marking & moving (`mark-move` / `paste-move`, M7/E6)

Structural moves (re-parenting a task, dragging a page into another node) run
through a generic **move clipboard**. Two standard action names form the
vocabulary:

- **`mark-move`** — remembers the current node as the move source. Pure frontend
  session state; the adapter returns `ActionDispatch::Noop` (it only has to list
  the action in `Node::actions()` so that the keybinding/hint takes effect). The
  TUI shows the marked source as an indicator in the status bar
  (`move: <label>`) until paste or `esc`.
- **`paste-move`** — the TUI calls `Node::invoke_action("paste-move", ctx)` on
  the **target** node, with `ctx.marked` carrying the marked source
  (`ActionContext.marked: Option<MarkedNode>`). **The adapter performs the
  move** (reparent/relocate) and returns `ActionDispatch::Reload`; the TUI
  reloads the target pane and clears the clipboard.

```rust
// in the adapter, in Node::invoke_action():
async fn invoke_action(&self, name: &str, ctx: &ActionContext)
    -> Result<ActionDispatch> {
    match name {
        "mark-move" => Ok(ActionDispatch::Noop), // the clipboard is frontend state
        "paste-move" => match &ctx.marked {
            Some(src) => {
                // check src.node_id / src.node_type, then move …
                self.reparent(&src.node_id, self.id()).await?;
                Ok(ActionDispatch::Reload)
            }
            None => Ok(ActionDispatch::Error("nothing marked".into())),
        },
        _ => Ok(ActionDispatch::Noop),
    }
}
```

`MarkedNode` carries `node_id` (the adapter-local id, as accepted by
`get_by_id`), `node_type` (so that the target can reject incompatible types) and
`label` (for the indicator). The move semantics live entirely in the adapter —
it alone knows its hierarchy and restrictions; the TUI only holds the clipboard
and passes it through on paste.

Why a generic mechanism instead of bespoke cut/paste per view: the native tasks
tree, the link feature and the DB script folders each used to carry their own
mark/paste paths. With `ActionContext.marked` + `mark-move`/`paste-move` every
adapter (from A1, the TaskAdapter, onwards) profits from the same clipboard
without touching TUI code.

## Pagination modes (`pagination:`)

Every ChildDef can configure its pagination mode:

```yaml
- name: Rows
  node_type: "postgres:row"
  pagination:
    mode: server # or: cursor
    page_size: 100
```

- **`server`** — the adapter pulls a `PageRequest { offset, limit }` and handles
  the fetch via `LIMIT`/`OFFSET` (or comparable server-side pagination).
  Suitable when `ORDER BY` / stability across page boundaries is required.

- **`cursor`** — the adapter holds a server-side cursor (Postgres:
  `DECLARE … CURSOR FOR …` in an open TX) for the lifetime of the result pane.
  `>`/`<` call `FETCH FORWARD N` / re-open. Notes:
  - Multi-statement bodies (e.g. `CREATE TEMP TABLE … ; SELECT …`) are
    supported — all non-SELECTs run as a prelude in the same TX, the final
    SELECT becomes the cursor.
  - `ORDER BY` is **not** automatic — the order follows the cursor plan. If you
    need a stable order, write it into the statement.
  - Backward navigation (`<`) re-opens the cursor (NO SCROLL).
  - Closing the pane emits `CloseAdapterCursor` and the TX is ended. On a
    `query_timeout_secs` timeout the entire connection pool is torn down; active
    cursors die with it and the pane shows "cursor lost".

## Edit in place (`editor_in_place:`)

A ChildDef can opt to create editor temp files **in the target directory**
instead of in `$TMPDIR`:

```yaml
- name: DB Script
  node_type: "postgres:db_script"
  editor_in_place: true
  shortcuts:
    e: edit
```

**When it makes sense**: when an external editor / language server derives
configuration or project context from the path of the opened file — e.g.
`postgres-language-server.jsonc` next to the script, `.editorconfig`,
`.clang-format`, `pyrightconfig.json`. Such tools usually walk upwards from the
file and find nothing in `$TMPDIR`.

**How it works**: the TUI creates the temp file with the fixed prefix
`.nyd_tmp_` and a random component directly under
`<instance_data_dir>/db_scripts/<db>/…/` (i.e. in the same directory as the
persistent script). On `:w` the TUI reads the buffer, strips editor-only markers
(banner, completion line) and writes to the real target. Afterwards the temp
file is removed; that also happens when the editor exits with an error, because
the `NamedTempFile` drop logic takes care of it.

**Cleanup guarantee**: if the TUI crashes in the middle of an editor session,
`.nyd_tmp_*` files can be left behind. The prefix marks them clearly as TUI
artifacts; deleting them is safe.

**Default**: `false`. Other sessions (tasks/trackings/filter) still live in
`$TMPDIR` — the flag only affects the editor path of the ChildDef it is set on
(currently used by the Postgres adapter for DB scripts).

## Adapter child process environment

Adapters can pass additional environment variables when starting a child process
(editor _or_ script). There is no configuration for it — the feature is a
**trait extension of the adapter**:

```rust
fn child_process_env(&self, node: &NodeRef) -> HashMap<String, String>
```

**What for**: the `postgres-language-server` (the editor LSP for SQL) needs a
real database connection for any form of completion. A TUI user, however, has
the tunnel port + password _only in the TUI_ — hand-maintaining a
`postgres-language-server.jsonc` with a plaintext password would violate the "no
real customer data in/next to the repo" rule, and the dynamic tunnel port moves
with every reconnect.

The Postgres adapter answers `child_process_env` with:

| Variable     | Source                                                        |
| ------------ | ------------------------------------------------------------- |
| `PGHOST`     | `TransportConnection::host` (`127.0.0.1` with an SSH tunnel)  |
| `PGPORT`     | the dynamic tunnel port from the live connection              |
| `PGUSER`     | `postgres.user` from `postgres-adapter.yaml`                  |
| `PGPASSWORD` | the resolved password (e.g. from `pass`), in RAM at runtime   |
| `PGDATABASE` | the second segment of the NodeRef (fallback `admin_database`) |
| `PGSSLMODE`  | mirrors `postgres.sslmode`                                    |

As long as the adapter connection is not yet open, the function returns an empty
map (no forced connect from the sync path).

**TUI side**: the variables are passed on via
[`EditorSpawnContext`](#editor-spawn-context) resp. the script spawn paths in
`app/script.rs` through `Command::envs(map)`. The TUI does not know the contents
— it only copies them. That is the clean architectural boundary: **connection
details stay with the adapter**, the TUI is data-/credential-agnostic.

**For other adapters**: Jira/Taiga implement the default (an empty map). If a
future adapter wants to drive its own CLI tools via `:script`/editor, it can use
the same hook — e.g. a `git-jira` plugin with `JIRA_HOST`/`JIRA_TOKEN`.

**Lifecycle**: a snapshot at spawn time. Reconnects change the port; already
running editor children keep their old env (which goes wrong if the reconnect
happens _during_ an LSP session — rare enough in practice that no refresh
mechanism exists).

<a id="editor-spawn-context"></a>**EditorSpawnContext**: the TUI bundles the
editor spawn knobs (temp file path/prefix for edit-in-place + child env) in a
struct returned by `EditSession::spawn_context()`. New spawn-time knobs (e.g.
cwd, ulimit) can be added there without touching every session and every
dispatch call.

## Action types

The view YAML knows the following generic action types:

| Type           | Description                                       | Action bar |
| -------------- | ------------------------------------------------- | ---------- |
| `fuzzy_filter` | fuzzy search over configurable fields             | ✅ (modal) |
| `edit`         | editor template from the adapter, external editor | ✅ (modal) |
| `create`       | create a new child node (schema from the adapter) | ✅ (modal) |
| `query_edit`   | edit the query in the editor                      | ✅ (modal) |
| `reload`       | reload the list                                   | ❌         |
| `navigate`     | switch into the child node level                  | ❌         |
| `open_url`     | open a URL from the metadata in the browser       | ❌         |
| `download`     | `node.content().read()` → save to a file          | ❌         |
| `script`       | start an external script with node JSON on stdin  | ❌         |
| `tag`          | tag management menu for the selected task         | ✅ (modal) |
| `custom`       | adapter-specific action (via `custom_action`)     | ❌         |
| `delete`       | delete the node (with confirmation)               | ❌         |

### Action bar vs. status bar

Actions are shown in two bars:

- **Action bar** (top): actions with persistent/modal state. They take over the
  input (the fuzzy filter input field) or indicate that an editor is currently
  open. Types: `fuzzy_filter`, `edit`, `create`, `query_edit`. In the future
  also: custom scripts with a running process.

- **Status bar** (bottom): fire-and-forget actions that execute immediately and
  have no lasting state. Types: `reload`, `navigate`, `custom`, `open_url`,
  `download`.

Every action has an optional `hide_from_bar: true` flag to override the default
(e.g. to hide an edit action from the action bar).

### Editor profile per action (`editor:`)

`edit` and `create` actions can optionally choose a **named editor profile**:

```yaml
actions:
  - { name: new, key: n, type: create, id: send_message, editor: compose-below }
  - { name: edit, key: e, type: edit, id: edit_message, editor: compose-below }
```

`editor:` references a key under the top-level block `editors:` in `tui.yaml` (a
profile of `command`/`inline`/`pause_tui`/…). If the field is missing, the
profile `default` is used. An unknown profile name is a **hard validation
error** when loading the config.

What for: different actions want different editor geometries — e.g. a chat
compose in a narrow terminal split at the bottom instead of in the full vsplit.
With an **external** editor this is a foreign process (no PTY embedding into a
TUI pane), so the geometry is realized by the terminal via the profile's
`command`.

Alternatively a profile sets `builtin: true`: then editing happens in a pane of
the TUI itself (a modal Vim-like editor from the crate `vimrealm`), without a
child process and without a temp file; `height: "30%"` determines the pane
height. Both are interchangeable per action — switching one `editor:` field is
enough, and the real `$EDITOR` remains selectable at any time. See `editors:` in
the `tui.yaml` docs.

### Apply on every save (`commit_on_save`)

```yaml
actions:
  - {
      name: new,
      key: n,
      type: create,
      id: send_message,
      editor: compose-below,
      commit_on_save: true,
    }
```

By default an `edit`/`create` action is applied only when the editor **closes**.
With `commit_on_save: true` it takes effect on **every save (`:w`)** while the
editor stays open — built for chat compose:

- The first `:w` executes the action (e.g. sends the message).
- If it produces a new node (`ActionOutcome::Navigate`), the session switches to
  that node's edit action; every further `:w` **edits** that node in place (e.g.
  the message just sent).
- Saving without a change since the last apply — including several `:w` in a row
  and the final close — is a no-op. Nothing is ever sent twice.

Prerequisite: a **detached** editor profile (`inline: false`), so that
intermediate saves are observable at all (the launch/detached path watches the
mtime of the temp file). Default `false` — only set the flag where this
behaviour is wanted; on a Jira ticket edit it would push a half-finished body on
every `:w`.

### Fuzzy filter

```yaml
actions:
  - name: fuzzy filter
    key: f
    type: fuzzy_filter
    fuzzy_filter:
      fields:
        [key, summary] # optional — search only these fields
        # empty/absent = all fields + label
```

The fuzzy filter filters the current list live. An input field appears in the
action bar. `fields` allows restricting it to specific metadata keys. The
special value `label` searches the node label (usually the title/summary).

**In tree mode it filters by path pruning across _all_ levels:** a node stays
visible if it matches itself **or** has a matching descendant. Hits thus show up
together with their ancestor chain, while non-matching sibling subtrees
disappear. A deeply nested hit is therefore found and made visible through its
parents — even if the `fuzzy_filter` action is only declared on the root view
(where it merely serves as the switch that arms the filter).

**Eager trees load the whole subtree when the filter opens:** on adapters with
`supports_eager_subtree` (local tasks/trackings), opening the filter pulls the
complete tree once (`list_subtree(u32::MAX)`) and expands it. This way nodes in
collapsed or not-yet-paginated branches match too — the "the filter sees the
whole forest" behaviour of the native tab. The expand state before the filter is
stashed and restored when the filter is cleared (the tree collapses back into
exactly its previous shape). On remote trees without that capability, the search
stays limited to the currently loaded/expanded nodes: a hit in an unloaded
branch only becomes visible once that branch is loaded.

**The matching substring is highlighted** (parity with the native tasks tab): in
tree mode the matched runs in the **label** of the `tree_label` column are drawn
in the theme `accent` color (bold) — the box connector keeps its own
`tree_connector` color. In flat mode the searched columns (`fields`, resp. all
of them with an empty list) get their hit runs in `accent` as well. Every
whitespace-separated token is matched individually; the matched character
indices are unioned and merged into contiguous ranges. If a token does not match
in the label/column (the row survived the filter through another field), nothing
is marked there.

### Script actions (`type: script`)

```yaml
actions:
  - name: script
    key: x
    type: script # opens the script menu; scripts live under
    #   <data>/not_yet_done/scripts/<tab>/<view-node-type…>/
```

A `script` action collects the scripts from the directory conventional for tab +
view level and offers them as a selection menu. The chosen script is started as
a foreign process and gets a **JSON on stdin**. Mutating scripts
(non-interactive) trigger a pane reload afterwards.

**`scope:` — what the script gets on stdin.** The default is `node`:

```yaml
- { name: script, key: x, type: script } # scope: node (default)
```

| `scope`        | stdin JSON                                                                                                                                             |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `node`         | `{"node": {id, label, node_type, tab, fields:{…}}}` — the **one** selected node                                                                        |
| `filtered_set` | `{"tracking_ids": […], "filter_min_date": …, "filter_max_date": …}` — **all** currently filtered row ids + the date bounds of the active query         |
| `table`        | `{"rows": [{id, label, fields:{…}}, …], "query": …, "selected_index": …, "selected_field": …}` — the **whole displayed table** with the cursor context |

```yaml
- { name: script, key: x, type: script, scope: filtered_set }
```

`scope: filtered_set` is intended for **batch/aggregate scripts** that run over
the whole visible list (e.g. an hours report over the filtered period). The
engine collects:

- **`tracking_ids`** — all ids the user currently sees: with an active fuzzy
  filter exactly the hit set, otherwise the complete query-filtered list of the
  pane.
- **`filter_min_date` / `filter_max_date`** — the lower/upper date bound of the
  active saved query (relative specifications such as `last month` are resolved
  at run time, RFC 3339; `null` if there is no bound).

For backwards compatibility the stdin key is named `tracking_ids` — the engine
path itself is generic, so that the historical trackings scripts
(`daily_report.py`, `hours_report.py`, …) run unchanged through the adapter tab.

```yaml
- { name: script, key: x, type: script, scope: table, default_field: name }
```

`scope: table` passes on the **whole currently displayed table together with the
cursor context** — intended for scripts that operate on a row/cell while wanting
to see the neighbouring rows or the query. It works on every content table,
including the transposed record detail split (`o`), where every "row" is a
field/value pair of the record — so **one** scope covers both "complete record +
selected field" (detail) and "all rows + query + cursor" (list). The engine
collects:

- **`rows`** — every visible row as `{id, label, fields:{…}}` (the same shape as
  a single `node`), in display order; with an active fuzzy filter exactly the
  hit set.
- **`query`** — the pane's active query text (`null` if there is none, e.g. in
  the detail split).
- **`selected_index`** — the index of the cursor row in `rows`.
- **`selected_field`** — the column key under the column cursor; if the column
  cursor is off, the action's configured **`default_field`** applies (otherwise
  `null`).

**Script shortcuts (`ctrl+s` in the menu).** As in the query menu, a key can be
assigned to a script in the script menu via **`ctrl+s`**. The captured chord is
stored in the DB table `query_shortcut(scope, name, shortcut)` under the scope
`script:<tab>/<view-node-type…>` (the same derivation as the script directory)
for the file name. If the focus afterwards sits on a level offering a
`type: script` action, the chord starts the script directly — exactly as if the
menu had been opened and Enter pressed on the entry (same `scope:`/
`default_field` context). The chord is checked against all keys active in its
tab (including chord prefixes) and rejected on collision. Assigned shortcuts
appear in the menu as a `[chord]` suffix.

### Tag management (`type: tag`)

```yaml
actions:
  - name: tags
    key: T
    type: tag
```

A `tag` action opens the **global tag management menu** (`:tag`), attached to
the pane's currently selected node. It is the same menu as on the native tasks
tab — the action type wires it generically into any content/adapter tab:

- **Enter** on a tag: assigns it to the task / removes it (toggle). The current
  state is loaded freshly when opening, not from a cache.
- **Type a name + Enter**: creates a new tag and assigns it to the task.
- **ctrl+e**: opens the YAML form of a tag (symbol / name / color).
- **ctrl+d**: deletes the tag (from all tasks).

After every change the pane is reloaded, so that the `tag_symbols` / `tag_names`
columns show the new state.

Tags are a task concept: the selected node has to carry a task id (the `tasks`
adapter delivers the bare UUID as the node id). On a node without a task id the
menu answers with a note instead of opening.

Convention: shortcut **`T` (shift+t)**, because `t` on the tasks tab is taken by
the tree/list view switch. In a tree, declare it with `inherit: true` so that
the action takes effect on every subtask level.

> **Status:** `type: tag` hung on the host-side `tag_service` and was the legacy
> burden being dismantled with the DB split (C5). **Toggling _and_
> creating/renaming/deleting** are now fully migrated to the adapter-driven
> `type: option_menu` (see below, fields `toggle`/`create`/`rename`/`delete`) —
> the tasks tab uses `option_menu` exclusively. `type: tag` and the cmdline
> `:tag` menu are therefore only relevant for hosts without a migrated adapter.

### Option menu (`type: option_menu`)

```yaml
actions:
  - name: tags
    key: T
    type: option_menu
    option_menu:
      source: tags # key for `list_values` on the adapter
      marker: tag_ids # hidden node field holding the set values
      toggle: toggle-tag # adapter action that fires on Enter
      create: create-tag # optional: ctrl+n creates an entry (asks for text)
      rename: rename-tag # optional: ctrl+e renames the focused one
      delete: delete-tag # optional: ctrl+d deletes it (y/n confirm)
      title: Tags # popup title (optional; default = the action name)
```

A **host-side, adapter-agnostic** selection menu that toggles values on the
selected node (e.g. tags). **Why it exists:** an action coupled to a GUI form
(picker, form) forces the adapter to know the host surface. Instead the adapter
delivers a flat list of selectable values via `list_values(source)` and receives
the chosen value through an ordinary `invoke_action` (`ActionContext.value`) —
the menu itself is pure host logic and lives in the config. This way the adapter
knows nothing about the menu, and the same action type serves tags, status sets,
labels and the like without a new contract shape.

Flow:

- When opening, the host loads the options via `list_values(source)` and reads
  the currently set values from the node's `marker` metadata field (comma-
  separated stable ids). Set options are marked with **★**.
- **Enter** on an option: fires the `toggle` action with the chosen value in
  `ActionContext.value`. The adapter decides assign-vs-unassign itself based on
  the actual membership and returns an `ActionDispatch` (nonsense values come
  back as `ActionDispatch::Error`).
- The popup **stays open** (multi-toggle); the ★ marker flips live immediately
  while the pane reloads in the background.
- **Managing the value list** (optional, depending on which field is set):
  - **`create`** (default `ctrl+n`): opens an inline text prompt. Enter fires
    the `create` action with the entered name in `ActionContext.text` (no
    `value`); empty text cancels.
  - **`rename`** (default `ctrl+e`): opens the same prompt, pre-filled with the
    label of the focused option. Enter fires the `rename` action with the
    option's stable id in `ActionContext.value` **and** the new name in
    `ActionContext.text`.
  - **`delete`** (default `ctrl+d`): shows an inline `(y/n)` confirm. `y` fires
    the `delete` action with the focused option's id in `ActionContext.value`;
    `n`/Esc cancels.
  - These verbs are **pure data operations on the value list** (not on the
    node) — the selected node is only the dispatch vehicle. On success the menu
    reloads the option list (the prompt closes, the popup stays open); an
    `ActionDispatch::Error` return is shown as a hint without closing the menu.
- **Esc** closes.

Fields:

| Field    | Required | Meaning                                                               |
| -------- | -------- | --------------------------------------------------------------------- |
| `source` | yes      | Key passed to `list_values(source)`; maps to `Vec<ValueOption>`.      |
| `marker` | yes      | Hidden node field with the set values (e.g. `tag_ids`).               |
| `toggle` | yes      | Adapter action id called with the value on Enter.                     |
| `create` | no       | Adapter action id for "create" (ctrl+n; name → `ActionContext.text`). |
| `rename` | no       | Adapter action id for "rename" (ctrl+e; id → `value`, name → `text`). |
| `delete` | no       | Adapter action id for "delete" (ctrl+d, y/n confirm; id → `value`).   |
| `title`  | no       | Popup title; default = the action's name.                             |

The menu shares its key bindings with the tag menu (`tag_menu` section: toggle /
create / edit / delete / next / prev / close), because the menu shape is
identical. A `create`/`rename`/`delete` field without an action set leaves the
respective key inactive.

### Custom actions

Adapters register their own actions:

```rust
pub trait ContentAdapter: Send + Sync {
    // ...
    fn custom_actions(&self, node_type: &NodeType) -> Vec<CustomAction>;
    async fn execute_action(
        &self,
        node_id: &str,
        action_id: &str,
        input: Option<&str>,
    ) -> Result<()>;
}

pub struct CustomAction {
    pub id: String,
    pub label: String,
    pub needs_input: bool,            // e.g. a transition needs a target status
    pub allowed_values: Option<Vec<String>>,  // for a popup/dropdown
}
```

Example in the YAML:

```yaml
actions:
  - name: Transition
    key: t
    type: custom
    custom_action: transition # → popup with allowed_values
  - name: Assign
    key: a
    type: custom
    custom_action: assign # → popup with the user list
```

---

## Open questions & difficulties

### 1. Async & loading times

Problem: `list()` for labels/users can take seconds (Jira fan-out). The TUI must
not block.

**Proposal**:

- All adapter calls async on principle, through the existing LoadMsg channel
- The TUI shows a "Loading..." indicator
- The adapter-internal cache makes most follow-up calls instant

### 2. Schema discovery

When a user configures `type: create`, the TUI has to know which fields can be
given at creation time. Currently not in the ContentAdapter.

**Proposal**: the node type delivers schema info:

```rust
pub trait ContentAdapter: Send + Sync {
    // ...
    fn schema(&self, node_type: &NodeType) -> Option<Vec<FieldSchema>>;
}

pub struct FieldSchema {
    pub key: String,
    pub label: String,
    pub field_type: FieldType,       // Text, Select, MultiSelect, Date, User
    pub required: bool,
    pub default: Option<String>,
}

pub enum FieldType {
    Text,
    Select,         // dropdown, allowed_values come from the adapter/cache
    MultiSelect,    // e.g. labels
    Date,
    User,           // autocomplete from cached users
}
```

### 3. Dynamic vs. static columns

The YAML defines fixed columns. But some adapters have dynamic fields (Jira
custom fields, DB columns). Two options:

a) The YAML defines everything explicitly → the user has to know the custom
fields
b) `columns: auto` → the adapter delivers the columns based on the first result
set

**Proposal**: support both. `auto` as the default, an explicit configuration
overrides it.

### 4. Bulk actions

Selecting several nodes and editing them at once (e.g. transitioning 5 tickets).
Needs multi-select in the table.

**Proposal**: out of scope for now, but the data model should not prevent it.
`actions` could get a `bulk: true` flag.

### 5. Dependency tasks ↔ content views

Tasks/trackings are native tabs with their own DB logic. But there are cross
connections:

- Link a task to a Jira ticket
- Stop a tracking automatically when a Jira ticket is transitioned

**Proposal**: keep them independent for now. Later integration is possible via
an event system or hooks.

### 6. Hot reload of the view configuration

When the user changes a YAML, the tab should update immediately (without a
restart). Needs a file watch on the views/ directory.

**Proposal**: nice to have. For now load only at startup. Refresh via a
`:reload-views` command.

---

## Implementation plan

### Phase 1: extend the traits (`not-yet-done-content`) ✅

Extend the content trait crate with the missing abstractions. The app stays
runnable — only new trait methods with default impls.

- [x] `editor_template()` and `parse_editor_output()` on the `Node` trait
- [x] `custom_actions()` and `execute_action()` on `ContentAdapter`
- [x] `FieldSchema`, `FieldType`, `CustomAction`, `EditorOutput` types
- [x] `schema()` on `ContentAdapter`

### Phase 2: switch the JiraAdapter to a config string ✅

Generic construction via `AdapterFactory`. The JiraAdapter gets an internal
cache for labels/users (for templates/autocomplete).

- [x] Implement `JiraAdapterFactory` (`create(config_string)`)
- [x] Config parsing from a YAML string (url, auth, cache settings)
- [x] Internal cache with TTL (labels, users) — in `JiraRoot`, shared via
      `Arc<Mutex<JiraCache>>`
- [x] `from_connection()` stays as a convenience

### Phase 3: editor templates in the JiraAdapter ✅

The adapter generates and parses editor templates. The TUI code becomes generic.

- [x] `editor_template()` on `JiraIssueNode` — with autocomplete hints from the
      cache
- [x] `parse_editor_output()` on `JiraIssueNode`
- [x] TUI `editor.rs`: generic `ContentEdit` EditorAction +
      `process_content_edit()`
- [x] TUI `mod.rs`: `OpenJiraTicketEditor` uses `editor_template()` instead of a
      hardcoded template
- The old `JiraTicketEdit` path is still present as a legacy fallback

### Phase 4: ViewConfig YAML parser ✅

Load and parse the declarative view configuration.

- [x] Rust structs for the YAML structure (TabConfig, ViewDef, ColumnDef,
      ActionDef, ChildDef, PreviewConfig, QueryConfig, EditConfig, SavedQuery)
- [x] `load_views()`: load `~/.config/not_yet_done/views/*.yaml`
- [x] AdapterFactory registry (HashMap<String, Box<dyn AdapterFactory>>)
- [x] Create adapter instances from the config string (inline or file reference)

### Phase 5: ContentView component 🔧 (in progress)

The generic TUI component that replaces JiraView.

- [x] `ContentView` struct with `ViewFileConfig` + `Arc<dyn ContentAdapter>` —
      skeleton created
- [x] Build the table from `ColumnDef` + `NodeSummary` metadata
- [x] Preview pane from `PreviewConfig`
- [x] Keybindings from `ActionDef` (config-driven actions)
- [ ] **App integration: `Tab::Jira` → `Tab::Content(usize)`, `jira_view` →
      `content_views: Vec<ContentView>`**
      A large coordinated rebuild across ~6 files: mod.rs, editor.rs, render.rs,
      tab_bar.rs, tabs/mod.rs, jira_view.rs. Recommended approach: first a
      mechanical 1:1 migration, then the YAML loading.
- [ ] Generic action handler (edit, create, delete, reload, download, open_url,
      custom)
- [ ] Saved queries / favorites from `QueryConfig`
- [ ] App: dynamic tab creation from the loaded ViewConfigs
- [ ] Remove JiraView + the legacy JiraTicketEdit path

### Phase 6: NavStack & children ⬜

Breadcrumb navigation for nested node types.

- [ ] `NavFrame` struct (node_id, label, view_def, scroll_pos, selected_idx)
- [ ] Push/pop logic with scroll position restore
- [ ] Breadcrumb rendering (a line above the table or in the tab bar)
- [ ] `ChildDef` from the YAML → an automatic Enter keybinding for navigation
