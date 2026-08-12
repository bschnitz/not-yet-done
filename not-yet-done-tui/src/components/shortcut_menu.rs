//! Shortcut menu — a searchable-at-a-glance list of every configured
//! keyboard shortcut, rendered as a [`LeaderList`] (name → keys).
//!
//! Opened by [`GlobalAction::ShortcutMenu`] (default `ctrl+y`). A three-tab
//! selector (rendered as the heading) chooses which rows are listed: the
//! shortcuts active in the current context, every tab's shortcuts, or only
//! the still-unbound actions (the "give me a key" view). The configured key
//! (default `Tab`) cycles between the three.
//! Enter is a reference no-op by default; when
//! [`ShortcutMenuConfig::execute_on_enter`] is on it closes the menu and
//! replays the selected row's key through normal dispatch (context scope
//! only, since keys from other tabs are contextless).
//!
//! The row list is a [`LeaderList`] with its fuzzy filter enabled: printable
//! keys type into a query matched against the left column (a leading `.`
//! switches the match to the keys column, e.g. `. ctrl+k`), `Ctrl-j`/`Ctrl-k`
//! (and the arrows) move the cursor, and the first `Esc` clears an active
//! filter before a second one closes the menu. Chrome is a borderless
//! floating panel in the form palette (matching the calendar "add event"
//! form): `Clear` + `form_panel_bg`, a `✦ {title}` heading and a compact
//! key-hint line.
//!
//! [`GlobalAction::ShortcutMenu`]: crate::config::keybindings::GlobalAction::ShortcutMenu
//! [`ShortcutMenuConfig::execute_on_enter`]: crate::config::ShortcutMenuConfig::execute_on_enter

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute};

use not_yet_done_ratatui::{LeaderList, LeaderListStyle, LeaderListStyleType};

use crate::config::ShortcutScope;
use crate::keymap::{KeySource, ShortcutRow};
use crate::ui::theme::Theme;

/// One existing binding that collides with a proposed new binding. Carries
/// everything the app needs to drop the colliding alternative, plus the
/// display metadata the prompt shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictItem {
    /// The conflicting shortcut's source (so the app can edit its file).
    pub source: KeySource,
    /// All current bindings of the conflicting shortcut (surface forms).
    pub current: Vec<String>,
    /// The specific alternative of `source` that collides.
    pub drop: String,
    /// Friendly name of the conflicting shortcut, for the prompt text.
    pub name: String,
    /// Whether this binding is editable (and thus removable). If any
    /// conflict is not removable the collision cannot be resolved and the
    /// new binding is refused.
    pub removable: bool,
}

/// Outcome of a key press while the menu is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutMenuMessage {
    /// Menu is not open — key not consumed here.
    Unhandled,
    /// Key consumed, menu stays open.
    Handled,
    /// Menu closed without running anything.
    Closed,
    /// Close and replay this key string through normal dispatch, so the
    /// selected action runs. Only emitted in execute mode + context scope.
    Execute(String),
    /// A binding was recorded for `row`. `binding` is the surface form — steps
    /// space-joined, e.g. `"ctrl+k l"`. The menu stays open; the app edits the
    /// config file and reloads. `overwrite` distinguishes the two record keys:
    /// Ctrl-N adds `binding` as an *additional* alternative (`false`), while
    /// Ctrl-U *replaces* the row's bindings with exactly `binding` (`true`) —
    /// the fix for turning a terminal key (e.g. `f`) into a chord (`f f`)
    /// without leaving the shadowing single key behind.
    AddBinding {
        row: ShortcutRow,
        binding: String,
        overwrite: bool,
    },
    /// Overwrite `row`'s bindings with exactly `values` — the delete path
    /// (Ctrl+D). Empty `values` disables the shortcut (`[]`); otherwise the
    /// surviving alternatives are written.
    SetBindings {
        row: ShortcutRow,
        values: Vec<String>,
    },
    /// Restore `row`'s built-in default (Ctrl+E) by dropping its override.
    RestoreDefault { row: ShortcutRow },
    /// Disable every tagged row (batch Ctrl+D). Each row's bindings are cleared
    /// (`[]`); deleting never conflicts, so the app applies it straight away.
    DeleteTagged { rows: Vec<ShortcutRow> },
    /// Restore every tagged row to its compiled default (batch Ctrl+E). The app
    /// checks whether any restored default collides with a still-set binding; a
    /// clean batch applies immediately, otherwise it raises an aggregated
    /// conflict prompt via [`ShortcutMenu::show_restore_conflicts`].
    RestoreTagged { rows: Vec<ShortcutRow> },
    /// Bind the recorded `binding` on every tagged row at once (batch Ctrl+N /
    /// Ctrl+U). `overwrite` carries the record mode (see [`Self::AddBinding`]).
    /// This is meant for the same action across different tabs (which never
    /// collide); the app aggregates any collision with a still-set binding and
    /// raises one prompt, and refuses outright if two tagged rows share a scope.
    BindTagged {
        rows: Vec<ShortcutRow>,
        binding: String,
        overwrite: bool,
    },
    /// The user confirmed (y) the aggregated batch-restore conflict prompt:
    /// drop every colliding binding in `items`, then restore every row in
    /// `rows` to its default. Only emitted when every item is removable.
    ResolveRestoreBatchApply {
        rows: Vec<ShortcutRow>,
        items: Vec<ConflictItem>,
    },
    /// The user confirmed (y) the aggregated batch-bind conflict prompt: drop
    /// every colliding binding in `items`, then bind `binding` on every row in
    /// `rows`. Only emitted when every item is removable.
    ResolveBindBatchApply {
        rows: Vec<ShortcutRow>,
        binding: String,
        items: Vec<ConflictItem>,
        overwrite: bool,
    },
    /// The user confirmed (y) a conflict prompt: drop every colliding
    /// alternative listed in `items` from its owning shortcut, then bind
    /// `binding` on `row`. Only emitted when every item is removable.
    /// `overwrite` carries the record mode through (see [`Self::AddBinding`]):
    /// `true` replaces the row's bindings, `false` appends.
    ResolveConflictApply {
        row: ShortcutRow,
        binding: String,
        items: Vec<ConflictItem>,
        overwrite: bool,
    },
}

/// In-progress key recording started by Ctrl-N. Steps accumulate until Return
/// (never itself a valid step); Esc cancels, Backspace drops the last step.
#[derive(Debug, Clone)]
struct Recorder {
    /// Index into the current scope's `rows()` of the row being (re)bound.
    row_index: usize,
    /// Recorded steps in canonical key-string form (each may carry modifiers).
    steps: Vec<String>,
    /// Ctrl-U (replace all bindings) vs Ctrl-N (add an alternative).
    overwrite: bool,
    /// Batch mode: apply the recorded binding to every tagged row at once
    /// (started by Ctrl-N/Ctrl-U while rows are tagged). `row_index` is unused
    /// then — the targets are the tagged set resolved at save time.
    batch: bool,
}

/// In-progress deletion picker started by Ctrl-D when a row has more than one
/// binding: pick which alternative to remove. A single binding deletes without
/// a picker; the write happens app-side.
#[derive(Debug, Clone)]
struct Deleter {
    row_index: usize,
    /// The row's current bindings, one per entry.
    bindings: Vec<String>,
    /// Which binding is highlighted for deletion.
    cursor: usize,
}

/// What a confirmed conflict prompt applies. Either a single recorded binding
/// (Ctrl-N/Ctrl-U) or a batch restore of every tagged row (Ctrl-E).
#[derive(Debug, Clone)]
enum PromptKind {
    /// Bind `binding` on `row`; `overwrite` is the record mode (replace vs add).
    Bind {
        row: ShortcutRow,
        binding: String,
        overwrite: bool,
    },
    /// Restore each of `rows` to its compiled default.
    RestoreBatch { rows: Vec<ShortcutRow> },
    /// Bind `binding` on each of `rows` at once; `overwrite` is the record mode.
    BindBatch {
        rows: Vec<ShortcutRow>,
        binding: String,
        overwrite: bool,
    },
}

/// A pending y/n prompt shown when a change collides with one or more existing
/// shortcuts. It lists every colliding binding. Confirming (y) drops each
/// colliding alternative and applies the pending change — but only when every
/// item is removable; if any is read-only the collision cannot be resolved and
/// the prompt only offers to dismiss. Declining leaves everything unchanged.
#[derive(Debug, Clone)]
struct ConflictPrompt {
    /// The change to apply once the collisions are cleared.
    kind: PromptKind,
    /// Every existing binding that collides with the pending change.
    items: Vec<ConflictItem>,
}

impl ConflictPrompt {
    /// The collision can be resolved only if every colliding binding can be
    /// removed (all owning shortcuts are editable).
    fn resolvable(&self) -> bool {
        self.items.iter().all(|i| i.removable)
    }

    /// The bound key the prompt is about (for the single-bind case), else the
    /// count of defaults being restored — used to phrase the heading.
    fn summary(&self) -> String {
        match &self.kind {
            PromptKind::Bind { binding, .. } => format!("'{binding}'"),
            PromptKind::RestoreBatch { rows } => match rows.len() {
                1 => "Restoring 1 default".to_string(),
                n => format!("Restoring {n} defaults"),
            },
            PromptKind::BindBatch { binding, rows, .. } => {
                format!("Binding '{binding}' on {} shortcut(s)", rows.len())
            }
        }
    }
}

/// The current bindings shown on a row (`"a / ctrl+k l"` → `["a", "ctrl+k l"]`).
fn row_bindings(row: &ShortcutRow) -> Vec<String> {
    row.keys
        .split(" / ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Render one recorded step in its YAML surface form (a literal space becomes
/// the word `space` so the step is legible and re-parseable).
fn step_to_surface(step: &str) -> String {
    if step == " " {
        "space".to_string()
    } else if let Some(mods) = step.strip_suffix("+ ") {
        format!("{mods}+space")
    } else {
        step.to_string()
    }
}

/// The full surface form of a recorded sequence: steps joined by spaces.
fn surface_form(steps: &[String]) -> String {
    steps
        .iter()
        .map(|s| step_to_surface(s))
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct ShortcutMenu {
    theme: Arc<Theme>,
    open: bool,
    /// Shortcuts active in the current tab + drilldown level.
    context: Vec<ShortcutRow>,
    /// Every configured shortcut across all tabs and levels.
    all: Vec<ShortcutRow>,
    /// The subset of `all` that currently has no binding — actions waiting
    /// for a key. Derived from `all` on open.
    unbound: Vec<ShortcutRow>,
    scope: ShortcutScope,
    execute_on_enter: bool,
    toggle_key: String,
    /// The list widget — owns cursor, scroll and the fuzzy-filter state. Its
    /// entries are rebuilt from `context`/`all` on open and on scope toggle.
    list: LeaderList,
    /// Active key recording (Ctrl-N), if any. While `Some`, all key presses
    /// feed the recorder instead of the list.
    recorder: Option<Recorder>,
    /// Active deletion picker (Ctrl-D on a multi-binding row), if any. While
    /// `Some`, keys navigate/confirm the picker instead of the list.
    deleter: Option<Deleter>,
    /// Pending conflict prompt (a recorded binding collided), if any. While
    /// `Some`, only y/n (Esc) are accepted.
    conflict: Option<ConflictPrompt>,
    /// Tagged rows, identified by their [`KeySource`] so a tag survives scope
    /// toggles (the same action shows up in Context and All). Ctrl-L toggles a
    /// tag; while any row is tagged the batch ops (Ctrl-N/Ctrl-U bind, Ctrl-D
    /// delete, Ctrl-E restore) act on the whole tagged set instead of the cursor
    /// row. Ephemeral: cleared each open.
    tagged: Vec<KeySource>,
}

impl ShortcutMenu {
    pub fn new(theme: Arc<Theme>, execute_on_enter: bool, toggle_key: String) -> Self {
        Self {
            theme,
            open: false,
            context: Vec::new(),
            all: Vec::new(),
            unbound: Vec::new(),
            scope: ShortcutScope::Context,
            execute_on_enter,
            toggle_key,
            list: LeaderList::default(),
            recorder: None,
            deleter: None,
            conflict: None,
            tagged: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Raise the conflict prompt after a recorded `binding` for `row` collided
    /// with one or more existing shortcuts (`items`). Called by the app when
    /// [`ShortcutMenuMessage::AddBinding`] detects a conflict instead of
    /// applying the binding. The prompt lists every colliding binding and asks
    /// whether to remove them; if any item is read-only it only offers to
    /// dismiss.
    pub fn show_conflicts(
        &mut self,
        row: ShortcutRow,
        binding: String,
        items: Vec<ConflictItem>,
        overwrite: bool,
    ) {
        self.conflict = Some(ConflictPrompt {
            kind: PromptKind::Bind {
                row,
                binding,
                overwrite,
            },
            items,
        });
    }

    /// Raise the aggregated conflict prompt for a batch restore: restoring the
    /// compiled defaults of `rows` would collide with the existing bindings in
    /// `items`. Confirming drops them all and restores every row; declining
    /// aborts the whole batch (no partial execution).
    pub fn show_restore_conflicts(&mut self, rows: Vec<ShortcutRow>, items: Vec<ConflictItem>) {
        self.conflict = Some(ConflictPrompt {
            kind: PromptKind::RestoreBatch { rows },
            items,
        });
    }

    /// Raise the aggregated conflict prompt for a batch bind: binding `binding`
    /// on `rows` would collide with the existing bindings in `items`. Confirming
    /// drops them all and binds every row; declining aborts the whole batch.
    pub fn show_bind_conflicts(
        &mut self,
        rows: Vec<ShortcutRow>,
        binding: String,
        overwrite: bool,
        items: Vec<ConflictItem>,
    ) {
        self.conflict = Some(ConflictPrompt {
            kind: PromptKind::BindBatch {
                rows,
                binding,
                overwrite,
            },
            items,
        });
    }

    /// Open the menu with the two row sets, starting in `scope`.
    pub fn open(&mut self, context: Vec<ShortcutRow>, all: Vec<ShortcutRow>, scope: ShortcutScope) {
        // "Unbound" is every all-tabs action that has no keys yet — the
        // give-me-a-key view.
        self.unbound = all
            .iter()
            .filter(|r| r.keys.trim().is_empty())
            .cloned()
            .collect();
        self.context = context;
        self.all = all;
        self.scope = scope;
        self.open = true;
        self.recorder = None;
        self.deleter = None;
        self.conflict = None;
        // Tags are ephemeral — each fresh open starts with nothing tagged.
        self.tagged.clear();
        self.rebuild_list();
    }

    /// Re-open the menu after an in-place mutation (a bind, delete or restore)
    /// while carrying the live fuzzy filter across the list rebuild. The scope
    /// stays put; only the query survives — tags are consumed by the op that
    /// triggered the refresh, matching a fresh `open`.
    pub fn refresh(&mut self, context: Vec<ShortcutRow>, all: Vec<ShortcutRow>) {
        let query = self.list.search_query().to_string();
        let scope = self.scope;
        self.open(context, all, scope);
        self.list.set_search_query(query);
    }

    /// Whether any row is currently tagged. When true the batch ops (Ctrl-D /
    /// Ctrl-E) act on the tagged set rather than the cursor row.
    pub fn has_tags(&self) -> bool {
        !self.tagged.is_empty()
    }

    /// Every currently-tagged row, resolved against the active scope's rows.
    /// A tag whose action is absent from the current scope is simply skipped
    /// (it reappears when the owning scope is shown again).
    fn tagged_rows(&self) -> Vec<ShortcutRow> {
        self.rows()
            .iter()
            .filter(|r| r.source.as_ref().is_some_and(|s| self.tagged.contains(s)))
            .cloned()
            .collect()
    }

    /// Toggles the tag on `src` (a row's source identity) and re-projects the
    /// marks onto the list.
    fn toggle_tag(&mut self, src: KeySource) {
        if let Some(pos) = self.tagged.iter().position(|s| s == &src) {
            self.tagged.remove(pos);
        } else {
            self.tagged.push(src);
        }
        self.reproject_marks();
    }

    /// Clears every tag (Ctrl-R) and the list marks.
    fn clear_tags(&mut self) {
        if self.tagged.is_empty() {
            return;
        }
        self.tagged.clear();
        self.list.clear_marked();
    }

    /// Re-computes which entry indices of the current scope are tagged and
    /// hands them to the list as its marked set. Called after every tag change
    /// and after a scope rebuild so marks track row identity across scopes.
    fn reproject_marks(&mut self) {
        let marked: Vec<usize> = self
            .rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| r.source.as_ref().is_some_and(|s| self.tagged.contains(s)))
            .map(|(i, _)| i)
            .collect();
        self.list.set_marked(marked);
    }

    fn rows(&self) -> &[ShortcutRow] {
        match self.scope {
            ShortcutScope::Context => &self.context,
            ShortcutScope::All => &self.all,
            ShortcutScope::Unbound => &self.unbound,
        }
    }

    /// The `(left, right)` display pairs for the current scope. In "all" scope
    /// the location (tab › level) is prefixed to the name on the left, so the
    /// right column stays the bare keys — e.g. `Jira › Attachments › Tab … 6`.
    fn entries(&self) -> Vec<(String, String)> {
        // The all-tabs and unbound views span tabs, so prefix each row with
        // its location (tab › level); the context view is already one tab.
        let prefixed = matches!(self.scope, ShortcutScope::All | ShortcutScope::Unbound);
        self.rows()
            .iter()
            .map(|r| {
                let left = if prefixed {
                    format!("{} › {}", r.scope, r.name)
                } else {
                    r.name.clone()
                };
                (left, r.keys.clone())
            })
            .collect()
    }

    /// The three-tab selector rendered as the menu heading in normal mode:
    /// `This view · All tabs · Unbound`, each with its row count. The active
    /// scope is highlighted (accent, bold, underlined); the others are dim.
    /// The configured toggle key (default `Tab`) cycles between them.
    fn tab_bar(&self, t: &Theme) -> Vec<Span<'static>> {
        let active = Style::default()
            .fg(t.form_accent())
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        let idle = Style::default().fg(t.form_hint());
        let tabs = [
            (ShortcutScope::Context, "This view", self.context.len()),
            (ShortcutScope::All, "All tabs", self.all.len()),
            (ShortcutScope::Unbound, "Unbound", self.unbound.len()),
        ];
        let mut spans: Vec<Span<'static>> = vec![Span::styled(
            "\u{2726} ",
            Style::default().fg(t.form_accent()),
        )];
        for (i, (sc, label, n)) in tabs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("   ", idle));
            }
            let style = if self.scope == *sc { active } else { idle };
            spans.push(Span::styled(format!("{label} ({n})"), style));
        }
        spans
    }

    /// Rebuilds the list widget for the current scope (fresh filter + cursor).
    fn rebuild_list(&mut self) {
        let entries = self.entries();
        let mut list = LeaderList::default()
            .with_entries(entries)
            .with_affixes(" ", ".", " ")
            .with_selectable(true)
            .with_status_line(true)
            .with_search(true)
            .with_search_placeholder("Type \". \" to search by keybinding")
            .with_style(self.style());
        list.attr(Attribute::Focus, AttrValue::Flag(true));
        self.list = list;
        // Re-pin marks for the (possibly new) scope, keyed by row identity.
        self.reproject_marks();
    }

    /// Dispatch a key string. Navigation, paging and fuzzy typing are handled
    /// by the [`LeaderList`]; only scope toggle, close and execute live here.
    pub fn handle_key(&mut self, key: &str) -> ShortcutMenuMessage {
        if !self.open {
            return ShortcutMenuMessage::Unhandled;
        }
        // A pending conflict prompt swallows everything but y (confirm) and
        // n / Esc (decline). Confirming resolves the conflict app-side — but
        // only when every colliding binding is removable; an unresolvable
        // collision (a read-only conflict) accepts only dismissal.
        if let Some(c) = self.conflict.as_ref() {
            let resolvable = c.resolvable();
            match key {
                "y" | "enter" if resolvable => {
                    let c = self.conflict.take().expect("conflict present");
                    return match c.kind {
                        PromptKind::Bind {
                            row,
                            binding,
                            overwrite,
                        } => ShortcutMenuMessage::ResolveConflictApply {
                            row,
                            binding,
                            items: c.items,
                            overwrite,
                        },
                        PromptKind::RestoreBatch { rows } => {
                            ShortcutMenuMessage::ResolveRestoreBatchApply {
                                rows,
                                items: c.items,
                            }
                        }
                        PromptKind::BindBatch {
                            rows,
                            binding,
                            overwrite,
                        } => ShortcutMenuMessage::ResolveBindBatchApply {
                            rows,
                            binding,
                            items: c.items,
                            overwrite,
                        },
                    };
                }
                "n" | "esc" => {
                    self.conflict = None;
                    return ShortcutMenuMessage::Handled;
                }
                _ => return ShortcutMenuMessage::Handled,
            }
        }
        // While recording (Ctrl-N), every key feeds the recorder — the list
        // and scope toggle are frozen until the recording ends.
        if self.recorder.is_some() {
            match key {
                "esc" => {
                    self.recorder = None;
                    return ShortcutMenuMessage::Handled;
                }
                "backspace" => {
                    if let Some(rec) = self.recorder.as_mut() {
                        rec.steps.pop();
                    }
                    return ShortcutMenuMessage::Handled;
                }
                // Return ends the recording; it is never itself a valid step.
                "enter" => {
                    let rec = self.recorder.take().expect("recorder present");
                    if rec.steps.is_empty() {
                        return ShortcutMenuMessage::Handled;
                    }
                    let binding = surface_form(&rec.steps);
                    // Batch mode: apply to every tagged row at once, then drop
                    // the tags (they are consumed by the op, like delete/restore).
                    if rec.batch {
                        let rows = self.tagged_rows();
                        self.clear_tags();
                        if rows.is_empty() {
                            return ShortcutMenuMessage::Handled;
                        }
                        return ShortcutMenuMessage::BindTagged {
                            rows,
                            binding,
                            overwrite: rec.overwrite,
                        };
                    }
                    // `take()` freed the borrow, so `rows()` is available now.
                    return match self.rows().get(rec.row_index).cloned() {
                        Some(row) => ShortcutMenuMessage::AddBinding {
                            row,
                            binding,
                            overwrite: rec.overwrite,
                        },
                        None => ShortcutMenuMessage::Handled,
                    };
                }
                other => {
                    if let Some(rec) = self.recorder.as_mut() {
                        rec.steps.push(other.to_string());
                    }
                    return ShortcutMenuMessage::Handled;
                }
            }
        }
        // While a deletion picker is open, keys navigate/confirm it.
        if self.deleter.is_some() {
            match key {
                "esc" => {
                    self.deleter = None;
                    return ShortcutMenuMessage::Handled;
                }
                "ctrl+k" | "up" => {
                    if let Some(d) = self.deleter.as_mut() {
                        d.cursor = d.cursor.saturating_sub(1);
                    }
                    return ShortcutMenuMessage::Handled;
                }
                "ctrl+j" | "down" => {
                    if let Some(d) = self.deleter.as_mut() {
                        d.cursor = (d.cursor + 1).min(d.bindings.len().saturating_sub(1));
                    }
                    return ShortcutMenuMessage::Handled;
                }
                "enter" => {
                    let d = self.deleter.take().expect("deleter present");
                    let mut remaining = d.bindings.clone();
                    if d.cursor < remaining.len() {
                        remaining.remove(d.cursor);
                    }
                    return match self.rows().get(d.row_index).cloned() {
                        Some(row) => ShortcutMenuMessage::SetBindings {
                            row,
                            values: remaining,
                        },
                        None => ShortcutMenuMessage::Handled,
                    };
                }
                _ => return ShortcutMenuMessage::Handled,
            }
        }
        // Ctrl-N (add an alternative) / Ctrl-U (overwrite all bindings) start
        // recording a binding for the selected row — but only if that row is
        // backed by an editable config entry (`source` set).
        if key == "ctrl+n" || key == "ctrl+u" {
            // With rows tagged, recording drives a batch bind over the whole
            // tagged set (all tagged rows are editable by construction), so the
            // cursor row is irrelevant. Otherwise record for the selected row.
            if self.has_tags() {
                self.recorder = Some(Recorder {
                    row_index: 0,
                    steps: Vec::new(),
                    overwrite: key == "ctrl+u",
                    batch: true,
                });
            } else if let Some(idx) = self.list.selected_index() {
                if self.rows().get(idx).is_some_and(|r| r.source.is_some()) {
                    self.recorder = Some(Recorder {
                        row_index: idx,
                        steps: Vec::new(),
                        overwrite: key == "ctrl+u",
                        batch: false,
                    });
                }
            }
            return ShortcutMenuMessage::Handled;
        }
        // Ctrl-D deletes bindings. With rows tagged it disables the whole tagged
        // set (batch); otherwise it acts on the selected (editable) row — one
        // binding disables straight away, several open a picker to choose which.
        if key == "ctrl+d" {
            if self.has_tags() {
                let rows = self.tagged_rows();
                self.clear_tags();
                return ShortcutMenuMessage::DeleteTagged { rows };
            }
            if let Some(idx) = self.list.selected_index() {
                if let Some(row) = self.rows().get(idx).filter(|r| r.source.is_some()).cloned() {
                    let bindings = row_bindings(&row);
                    return match bindings.len() {
                        0 | 1 => ShortcutMenuMessage::SetBindings {
                            row,
                            values: Vec::new(),
                        },
                        _ => {
                            self.deleter = Some(Deleter {
                                row_index: idx,
                                bindings,
                                cursor: 0,
                            });
                            ShortcutMenuMessage::Handled
                        }
                    };
                }
            }
            return ShortcutMenuMessage::Handled;
        }
        // Ctrl-L toggles the tag on the selected (taggable) row; Ctrl-A tags
        // every currently visible taggable row. Read-only rows (no source)
        // can't be tagged. Tags are keyed by row identity so they survive scope
        // toggles and drive the batch delete/restore ops.
        if key == "ctrl+l" {
            if let Some(idx) = self.list.selected_index() {
                if let Some(src) = self.rows().get(idx).and_then(|r| r.source.clone()) {
                    self.toggle_tag(src);
                }
            }
            return ShortcutMenuMessage::Handled;
        }
        // Tag every visible row. Ctrl-A is the canonical "select all" key and is
        // used because Ctrl-Shift-L is unreliable: most terminals can't tell it
        // apart from Ctrl-L (both send `0x0C`), so it would silently land on the
        // single-row toggle above. Ctrl-Shift-L is kept as an alias for
        // Kitty-protocol terminals that *do* disambiguate it (arriving as the
        // upper-case `ctrl+L`, see `key_event_to_string`).
        if key == "ctrl+a" || key == "ctrl+L" || key == "ctrl+shift+l" {
            let srcs: Vec<KeySource> = self
                .list
                .visible_indices()
                .into_iter()
                .filter_map(|i| self.rows().get(i).and_then(|r| r.source.clone()))
                .collect();
            for s in srcs {
                if !self.tagged.contains(&s) {
                    self.tagged.push(s);
                }
            }
            self.reproject_marks();
            return ShortcutMenuMessage::Handled;
        }
        // Ctrl-R clears every tag.
        if key == "ctrl+r" {
            self.clear_tags();
            return ShortcutMenuMessage::Handled;
        }
        // Ctrl-E restores a shortcut's compiled default (drops the override).
        // With rows tagged it restores the whole tagged set (batch, the app
        // aggregates any conflicts); otherwise it acts on the selected row.
        // Offered for built-ins and tab-switch keys (whose default is the
        // autonumber digit); a no-op for sources with no default.
        if key == "ctrl+e" {
            if self.has_tags() {
                let rows = self.tagged_rows();
                self.clear_tags();
                return ShortcutMenuMessage::RestoreTagged { rows };
            }
            if let Some(idx) = self.list.selected_index() {
                if let Some(row) = self
                    .rows()
                    .get(idx)
                    .filter(|r| r.source.as_ref().is_some_and(|s| s.has_compiled_default()))
                    .cloned()
                {
                    return ShortcutMenuMessage::RestoreDefault { row };
                }
            }
            return ShortcutMenuMessage::Handled;
        }
        if key == self.toggle_key {
            self.scope = match self.scope {
                ShortcutScope::Context => ShortcutScope::All,
                ShortcutScope::All => ShortcutScope::Unbound,
                ShortcutScope::Unbound => ShortcutScope::Context,
            };
            self.rebuild_list();
            return ShortcutMenuMessage::Handled;
        }
        match key {
            // First Esc clears an active filter; a second one closes.
            "esc" => {
                if self.list.search_active() {
                    self.list.clear_search();
                    ShortcutMenuMessage::Handled
                } else {
                    self.open = false;
                    ShortcutMenuMessage::Closed
                }
            }
            "enter" => {
                let replay = self.execute_on_enter && self.scope == ShortcutScope::Context;
                if replay {
                    if let Some(row) = self.list.selected_index().and_then(|i| self.rows().get(i)) {
                        // The row's `keys` may list alternatives ("j / down");
                        // replay the first — any one triggers the action.
                        let k = row
                            .keys
                            .split(" / ")
                            .next()
                            .unwrap_or(&row.keys)
                            .to_string();
                        self.open = false;
                        // A keyless action has nothing to replay: just close.
                        return if k.is_empty() {
                            ShortcutMenuMessage::Closed
                        } else {
                            ShortcutMenuMessage::Execute(k)
                        };
                    }
                }
                self.open = false;
                ShortcutMenuMessage::Closed
            }
            // Everything else (arrows, Ctrl-j/k, PgUp/Dn, printable filter
            // keys, Backspace) is forwarded to the list, then swallowed so it
            // can't leak to the tabs behind.
            _ => {
                if let Some(ev) = crate::events::key_string_to_tuirealm(key) {
                    let _ = self.list.on(&Event::<NoUserEvent>::Keyboard(ev));
                }
                ShortcutMenuMessage::Handled
            }
        }
    }

    fn style(&self) -> LeaderListStyle {
        let t = &self.theme;
        let cursor_bg = t.form_field_bg().unwrap_or_else(|| t.surface_2());
        LeaderListStyle::new()
            .set_style(
                LeaderListStyleType::Left,
                Style::default().fg(t.form_text()),
            )
            .set_style(
                LeaderListStyleType::Filler,
                Style::default().fg(t.form_hint()),
            )
            .set_style(
                LeaderListStyleType::Right,
                Style::default()
                    .fg(t.form_accent())
                    .add_modifier(Modifier::BOLD),
            )
            .set_style(LeaderListStyleType::Cursor, Style::default().bg(cursor_bg))
            .set_style(
                LeaderListStyleType::Status,
                Style::default()
                    .fg(t.form_hint())
                    .add_modifier(Modifier::ITALIC),
            )
            .set_style(
                LeaderListStyleType::Search,
                Style::default()
                    .fg(t.form_accent())
                    .add_modifier(Modifier::BOLD),
            )
            // Tagged rows glow in the warning colour (amber) and bold, so a
            // batch selection reads at a glance as "staged for an action".
            .set_style(
                LeaderListStyleType::Marked,
                Style::default()
                    .fg(t.warning())
                    .add_modifier(Modifier::BOLD),
            )
    }

    /// Borderless floating panel matching the calendar "add event" form:
    /// `Clear` + `form_panel_bg` fill (no border), a `✦ {title}` heading,
    /// the `LeaderList` body and a compact key-hint line — all in the form
    /// palette.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open {
            return;
        }
        let t = Arc::clone(&self.theme);
        let count = self.rows().len();

        // Content width: fit the widest label→keys line — the widget already
        // knows this via `min_width` (a + post + pre + b over all entries).
        let content_w = self.list.min_width() as usize;
        // The heading is a three-tab selector (This view / All tabs / Unbound);
        // measure its rendered width so the panel is wide enough for it.
        let tab_spans = self.tab_bar(&t);
        let tab_bar_w: usize = tab_spans.iter().map(|s| s.content.chars().count()).sum();

        // Compact help line (key, description) pairs, like the event form.
        // Navigation is on Ctrl-j/Ctrl-k (plain keys type into the filter).
        let recording = self.recorder.is_some();
        let deleting = self.deleter.is_some();
        let prompting = self.conflict.is_some();
        let hints: Vec<(&str, &str)> = if prompting {
            vec![("y", "apply"), ("n/Esc", "cancel")]
        } else if recording {
            vec![("↵", "save"), ("⌫", "del"), ("Esc", "cancel")]
        } else if deleting {
            vec![("↑↓", "pick"), ("↵", "delete"), ("Esc", "cancel")]
        } else if self.has_tags() {
            // With rows tagged, the op keys read as batch actions over the set.
            let mut h = vec![
                ("↑↓/C-jk", "nav"),
                ("C-l", "tag"),
                ("C-a", "tag all"),
                ("C-r", "untag all"),
                ("C-n", "bind tagged"),
                ("C-u", "replace tagged"),
                ("C-d", "del tagged"),
                ("C-e", "default tagged"),
            ];
            if self.execute_on_enter && self.scope == ShortcutScope::Context {
                h.push(("↵", "run"));
            }
            h.push(("Esc", "close"));
            h
        } else {
            let mut h = vec![
                ("↑↓/C-jk", "nav"),
                ("type", "filter"),
                (".", "by key"),
                ("Tab", "scope"),
                ("C-n", "bind"),
                ("C-u", "replace"),
                ("C-d", "del"),
                ("C-e", "default"),
                ("C-l", "tag"),
                ("C-a", "tag all"),
            ];
            if self.execute_on_enter && self.scope == ShortcutScope::Context {
                h.push(("↵", "run"));
            }
            h.push(("Esc", "close"));
            h
        };
        let help_w: usize = hints
            .iter()
            .map(|(k, d)| k.chars().count() + 1 + d.chars().count() + 2)
            .sum();

        // Panel geometry: pad(2) + heading + search + keys line, content-sized
        // and centred within `area` (already bounded by the caller so the
        // panel never touches the top/bottom chrome). Width fits the widest of
        // content / title / help.
        let inner_w_needed = content_w.max(tab_bar_w).max(help_w);
        let panel_w = ((inner_w_needed as u16) + 4).max(36).min(area.width);
        // heading(1) + gap(1) + search(1) + list(count+status) + gap(1)
        // + help(1) + pad(2).
        let wanted_h = count as u16 + 1 + 7;
        let panel_h = wanted_h.min(area.height).max(8);

        let px = area.x + area.width.saturating_sub(panel_w) / 2;
        let py = area.y + area.height.saturating_sub(panel_h) / 2;
        let panel = Rect::new(px, py, panel_w, panel_h);

        // Floating panel: clear behind, then fill (no border).
        frame.render_widget(Clear, panel);
        if let Some(bg) = t.form_panel_bg() {
            frame.render_widget(Block::default().style(Style::default().bg(bg)), panel);
        }

        let inner = Rect::new(
            panel.x + 2,
            panel.y + 1,
            panel.width.saturating_sub(4),
            panel.height.saturating_sub(2),
        );
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // Heading: `✦ {title}`, or the live recording prompt while recording,
        // or a "delete which?" prompt while picking a binding to remove, or a
        // conflict warning while a resolve prompt is pending.
        let heading = if self.conflict.is_some() {
            Line::from(vec![Span::styled(
                "\u{26a0} binding conflict",
                Style::default()
                    .fg(t.form_accent())
                    .add_modifier(Modifier::BOLD),
            )])
        } else if let Some(rec) = self.recorder.as_ref() {
            let so_far = surface_form(&rec.steps);
            let label = if rec.overwrite {
                "\u{25cf} rec (replace) "
            } else {
                "\u{25cf} rec "
            };
            Line::from(vec![
                Span::styled(
                    label,
                    Style::default()
                        .fg(t.form_accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{so_far}\u{258f}"),
                    Style::default()
                        .fg(t.form_text())
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        } else if deleting {
            Line::from(vec![Span::styled(
                "\u{2717} delete which binding?",
                Style::default()
                    .fg(t.form_accent())
                    .add_modifier(Modifier::BOLD),
            )])
        } else {
            Line::from(tab_spans)
        };
        frame.render_widget(Paragraph::new(heading), Rect { height: 1, ..inner });

        // Help line on the last inner row; the list (which draws its own
        // search prompt on its first row) fills the space between the heading
        // (with a one-row gap) and a one-row gap above the help.
        let help_y = inner.bottom().saturating_sub(1);
        let list_y = inner.y + 2;
        let list_h = help_y.saturating_sub(1).saturating_sub(list_y);

        if list_h > 0 {
            let list_area = Rect {
                x: inner.x,
                y: list_y,
                width: inner.width,
                height: list_h,
            };
            if let Some(c) = self.conflict.as_ref() {
                // Explain the collision(s) and what confirming will do. Each
                // colliding binding gets its own line; read-only ones are
                // tagged since they cannot be removed.
                let text = Style::default().fg(t.form_text());
                let accent = Style::default()
                    .fg(t.form_accent())
                    .add_modifier(Modifier::BOLD);
                let dim = Style::default().fg(t.form_hint());
                let resolvable = c.resolvable();
                let mut lines: Vec<Line> = Vec::new();
                lines.push(Line::from(vec![
                    Span::styled(c.summary(), accent),
                    Span::styled(" conflicts with:", text),
                ]));
                for item in &c.items {
                    let mut spans = vec![
                        Span::styled("  • ", dim),
                        Span::styled(item.drop.clone(), accent),
                        Span::styled(" — ", dim),
                        Span::styled(item.name.clone(), text),
                    ];
                    if !item.removable {
                        spans.push(Span::styled("  (read-only)", dim));
                    }
                    lines.push(Line::from(spans));
                }
                let apply_prompt = match &c.kind {
                    PromptKind::Bind { .. } => "Remove and bind here? (y/n)",
                    PromptKind::RestoreBatch { .. } => "Remove and restore defaults? (y/n)",
                    PromptKind::BindBatch { .. } => "Remove and bind all tagged? (y/n)",
                };
                lines.push(Line::from(Span::styled(
                    if resolvable {
                        apply_prompt.to_string()
                    } else {
                        "Read-only bindings can't be removed — press n/Esc.".to_string()
                    },
                    text,
                )));
                frame.render_widget(Paragraph::new(lines), list_area);
            } else if let Some(d) = self.deleter.as_ref() {
                // Vertical list of the row's bindings; the cursor row is
                // highlighted on the form field background.
                let cursor_bg = t.form_field_bg().unwrap_or_else(|| t.surface_2());
                let lines: Vec<Line> = d
                    .bindings
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let selected = i == d.cursor;
                        let mut style = Style::default().fg(t.form_text());
                        if selected {
                            style = style.bg(cursor_bg).add_modifier(Modifier::BOLD);
                        }
                        let marker = if selected { "› " } else { "  " };
                        Line::from(Span::styled(format!("{marker}{b}"), style))
                    })
                    .collect();
                frame.render_widget(Paragraph::new(lines), list_area);
            } else {
                self.list.view(frame, list_area);
            }
        }

        // Compact help line, form palette.
        let key = Style::default()
            .fg(t.form_accent())
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(t.form_hint());
        let mut spans: Vec<Span> = Vec::new();
        for (k, d) in &hints {
            spans.push(Span::styled(*k, key));
            spans.push(Span::styled(format!(" {d}  "), dim));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(inner.x, help_y, inner.width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(crate::config::ThemeConfig::default()))
    }

    fn row(name: &str, keys: &str, scope: &str) -> ShortcutRow {
        ShortcutRow {
            name: name.into(),
            keys: keys.into(),
            scope: scope.into(),
            source: None,
            key_scope: None,
        }
    }

    /// A row backed by an editable built-in source, so Ctrl-N can bind it.
    fn editable_row(name: &str, keys: &str) -> ShortcutRow {
        use crate::config::keybindings::GlobalAction;
        use crate::keymap::KeySource;
        ShortcutRow {
            name: name.into(),
            keys: keys.into(),
            scope: "Global".into(),
            source: Some(KeySource::Global(GlobalAction::Quit)),
            key_scope: None,
        }
    }

    /// A row backed by a view action (not a built-in) — Ctrl-R has no default
    /// to restore for these.
    fn view_action_row(name: &str, keys: &str) -> ShortcutRow {
        use crate::keymap::KeySource;
        ShortcutRow {
            name: name.into(),
            keys: keys.into(),
            scope: "Jira".into(),
            source: Some(KeySource::YamlAction {
                view: "tickets".into(),
                child_path: vec![],
                name: name.into(),
            }),
            key_scope: None,
        }
    }

    fn ctx() -> Vec<ShortcutRow> {
        vec![
            row("Quit", "ctrl+c", "Jira"),
            row("Delete", "d", "Jira"),
            row("Open", "enter / l", "Jira"),
        ]
    }

    fn all() -> Vec<ShortcutRow> {
        vec![row("Quit", "ctrl+c", "Global"), row("Delete", "d", "Taiga")]
    }

    fn menu(execute: bool) -> ShortcutMenu {
        let mut m = ShortcutMenu::new(theme(), execute, "tab".into());
        m.open(ctx(), all(), ShortcutScope::Context);
        m
    }

    #[test]
    fn unbound_scope_lists_only_keyless_all_rows() {
        let all = vec![
            row("Quit", "ctrl+c", "Global"),
            row("Rename", "", "Jira"),   // no binding → unbound
            row("Move", "   ", "Taiga"), // whitespace-only → unbound
        ];
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(ctx(), all, ShortcutScope::Unbound);
        let names: Vec<&str> = m.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Rename", "Move"]);
    }

    #[test]
    fn unhandled_when_closed() {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        assert_eq!(m.handle_key("ctrl+j"), ShortcutMenuMessage::Unhandled);
    }

    #[test]
    fn navigation_moves_and_clamps() {
        // Navigation is on Ctrl-j/Ctrl-k now; plain j/k type into the filter.
        let mut m = menu(false);
        assert_eq!(m.list.selected(), 0);
        m.handle_key("ctrl+k"); // already at top
        assert_eq!(m.list.selected(), 0);
        m.handle_key("ctrl+j");
        m.handle_key("ctrl+j");
        assert_eq!(m.list.selected(), 2);
        m.handle_key("ctrl+j"); // clamped to last
        assert_eq!(m.list.selected(), 2);
    }

    #[test]
    fn toggle_switches_scope_and_resets_cursor() {
        let mut m = menu(false);
        m.handle_key("ctrl+j");
        m.handle_key("ctrl+j"); // cursor 2
        assert_eq!(m.handle_key("tab"), ShortcutMenuMessage::Handled);
        assert_eq!(m.scope, ShortcutScope::All);
        // The list is rebuilt for the new scope, so the cursor starts at top.
        assert_eq!(m.list.selected(), 0);
        // Tab cycles through all three scopes: this view → all → unbound → back.
        m.handle_key("tab");
        assert_eq!(m.scope, ShortcutScope::Unbound);
        m.handle_key("tab");
        assert_eq!(m.scope, ShortcutScope::Context);
    }

    #[test]
    fn enter_reference_mode_just_closes() {
        let mut m = menu(false);
        assert_eq!(m.handle_key("enter"), ShortcutMenuMessage::Closed);
        assert!(!m.is_open());
    }

    #[test]
    fn enter_execute_mode_replays_first_key() {
        let mut m = menu(true);
        m.handle_key("ctrl+j");
        m.handle_key("ctrl+j"); // "Open" → "enter / l"
        assert_eq!(
            m.handle_key("enter"),
            ShortcutMenuMessage::Execute("enter".into())
        );
        assert!(!m.is_open());
    }

    #[test]
    fn enter_execute_mode_disabled_in_all_scope() {
        let mut m = menu(true);
        m.handle_key("tab"); // -> all scope
        assert_eq!(m.handle_key("enter"), ShortcutMenuMessage::Closed);
    }

    #[test]
    fn typing_filters_and_selects_match() {
        // Type "de" → only "Delete" matches; the cursor sits on it.
        let mut m = menu(false);
        m.handle_key("d");
        m.handle_key("e");
        assert_eq!(m.list.search_query(), "de");
        let idx = m.list.selected_index().expect("a match");
        assert_eq!(m.rows()[idx].name, "Delete");
    }

    #[test]
    fn esc_clears_filter_then_closes() {
        let mut m = menu(false);
        m.handle_key("d"); // active filter
        assert!(m.list.search_active());
        assert_eq!(m.handle_key("esc"), ShortcutMenuMessage::Handled);
        assert!(!m.list.search_active());
        assert!(m.is_open(), "first Esc only clears the filter");
        assert_eq!(m.handle_key("esc"), ShortcutMenuMessage::Closed);
        assert!(!m.is_open());
    }

    #[test]
    fn plain_key_is_swallowed_as_filter_input() {
        let mut m = menu(false);
        assert_eq!(m.handle_key("z"), ShortcutMenuMessage::Handled);
        assert!(m.is_open());
        assert_eq!(m.list.search_query(), "z");
    }

    #[test]
    fn refresh_carries_the_live_filter_across_the_rebuild() {
        // A batch op reopens the menu via `refresh`; the fuzzy filter the user
        // had typed must survive the list rebuild (a plain `open` would wipe it).
        let mut m = menu(false);
        m.handle_key("d");
        m.handle_key("e");
        assert_eq!(m.list.search_query(), "de");
        m.refresh(ctx(), all());
        assert_eq!(m.list.search_query(), "de", "refresh preserves the query");
        assert!(m.is_open());
    }

    /// A menu whose selected (first) row is editable, so Ctrl-N can record.
    fn rec_menu() -> ShortcutMenu {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![editable_row("Quit", "ctrl+c"), row("Delete", "d", "Jira")],
            all(),
            ShortcutScope::Context,
        );
        m
    }

    #[test]
    fn ctrl_n_starts_recording_only_for_an_editable_row() {
        // Row 0 is editable → recording starts.
        let mut m = rec_menu();
        assert_eq!(m.handle_key("ctrl+n"), ShortcutMenuMessage::Handled);
        assert!(m.recorder.is_some());
        // Row 1 has no source → Ctrl-N is a no-op recording-wise.
        let mut m2 = rec_menu();
        m2.handle_key("ctrl+j"); // select row 1 ("Delete", source None)
        m2.handle_key("ctrl+n");
        assert!(m2.recorder.is_none());
    }

    #[test]
    fn recording_accumulates_backspaces_and_saves_a_sequence() {
        let mut m = rec_menu();
        m.handle_key("ctrl+n");
        m.handle_key("ctrl+k");
        m.handle_key("x"); // oops
        m.handle_key("backspace"); // drop "x"
        m.handle_key("l");
        let msg = m.handle_key("enter");
        assert_eq!(
            msg,
            ShortcutMenuMessage::AddBinding {
                row: editable_row("Quit", "ctrl+c"),
                binding: "ctrl+k l".into(),
                overwrite: false,
            }
        );
        assert!(m.recorder.is_none(), "recording ends on save");
        assert!(m.is_open(), "menu stays open after adding a binding");
    }

    #[test]
    fn recording_a_single_key_saves_a_scalar_binding() {
        let mut m = rec_menu();
        m.handle_key("ctrl+n");
        m.handle_key("ctrl+shift+a");
        let msg = m.handle_key("enter");
        assert_eq!(
            msg,
            ShortcutMenuMessage::AddBinding {
                row: editable_row("Quit", "ctrl+c"),
                binding: "ctrl+shift+a".into(),
                overwrite: false,
            }
        );
    }

    #[test]
    fn ctrl_u_records_in_overwrite_mode() {
        let mut m = rec_menu();
        m.handle_key("ctrl+u");
        assert!(m.recorder.as_ref().is_some_and(|r| r.overwrite));
        m.handle_key("f");
        m.handle_key("f");
        assert_eq!(
            m.handle_key("enter"),
            ShortcutMenuMessage::AddBinding {
                row: editable_row("Quit", "ctrl+c"),
                binding: "f f".into(),
                overwrite: true,
            }
        );
    }

    #[test]
    fn space_step_records_as_the_word_space() {
        let mut m = rec_menu();
        m.handle_key("ctrl+n");
        m.handle_key(" ");
        assert_eq!(
            m.handle_key("enter"),
            ShortcutMenuMessage::AddBinding {
                row: editable_row("Quit", "ctrl+c"),
                binding: "space".into(),
                overwrite: false,
            }
        );
    }

    #[test]
    fn esc_cancels_recording_without_binding() {
        let mut m = rec_menu();
        m.handle_key("ctrl+n");
        m.handle_key("g");
        assert_eq!(m.handle_key("esc"), ShortcutMenuMessage::Handled);
        assert!(m.recorder.is_none());
        assert!(m.is_open(), "cancelling recording does not close the menu");
    }

    #[test]
    fn enter_on_empty_recording_just_cancels() {
        let mut m = rec_menu();
        m.handle_key("ctrl+n");
        assert_eq!(m.handle_key("enter"), ShortcutMenuMessage::Handled);
        assert!(m.recorder.is_none());
    }

    #[test]
    fn keys_during_recording_do_not_toggle_scope_or_filter() {
        let mut m = rec_menu();
        m.handle_key("ctrl+n");
        m.handle_key("tab"); // recorded as a step, not a scope toggle
        assert_eq!(m.scope, ShortcutScope::Context);
        assert!(!m.list.search_active());
    }

    #[test]
    fn step_surface_forms() {
        assert_eq!(step_to_surface("a"), "a");
        assert_eq!(step_to_surface("ctrl+k"), "ctrl+k");
        assert_eq!(step_to_surface(" "), "space");
        assert_eq!(step_to_surface("ctrl+ "), "ctrl+space");
        assert_eq!(
            surface_form(&["ctrl+k".into(), "l".into()]),
            "ctrl+k l".to_string()
        );
    }

    #[test]
    fn ctrl_d_on_a_single_binding_disables_it_directly() {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![editable_row("Quit", "ctrl+c")],
            all(),
            ShortcutScope::Context,
        );
        assert_eq!(
            m.handle_key("ctrl+d"),
            ShortcutMenuMessage::SetBindings {
                row: editable_row("Quit", "ctrl+c"),
                values: vec![],
            }
        );
        assert!(m.deleter.is_none(), "one binding needs no picker");
    }

    #[test]
    fn ctrl_d_on_multiple_bindings_opens_a_picker_and_removes_the_chosen_one() {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![editable_row("Quit", "ctrl+c / q / ctrl+x")],
            all(),
            ShortcutScope::Context,
        );
        assert_eq!(m.handle_key("ctrl+d"), ShortcutMenuMessage::Handled);
        assert!(m.deleter.is_some());
        m.handle_key("ctrl+j"); // cursor → "q"
        let msg = m.handle_key("enter");
        assert_eq!(
            msg,
            ShortcutMenuMessage::SetBindings {
                row: editable_row("Quit", "ctrl+c / q / ctrl+x"),
                values: vec!["ctrl+c".into(), "ctrl+x".into()],
            }
        );
        assert!(m.deleter.is_none(), "picker closes on confirm");
    }

    #[test]
    fn delete_picker_esc_cancels_without_deleting() {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![editable_row("Quit", "ctrl+c / q")],
            all(),
            ShortcutScope::Context,
        );
        m.handle_key("ctrl+d");
        assert!(m.deleter.is_some());
        assert_eq!(m.handle_key("esc"), ShortcutMenuMessage::Handled);
        assert!(m.deleter.is_none());
        assert!(m.is_open(), "cancelling the picker keeps the menu open");
    }

    #[test]
    fn ctrl_d_is_a_noop_on_a_read_only_row() {
        // The default `ctx()` rows have `source: None`.
        let mut m = menu(false);
        assert_eq!(m.handle_key("ctrl+d"), ShortcutMenuMessage::Handled);
        assert!(m.deleter.is_none());
    }

    #[test]
    fn ctrl_e_restores_a_builtin_default() {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![editable_row("Quit", "ctrl+c")],
            all(),
            ShortcutScope::Context,
        );
        assert_eq!(
            m.handle_key("ctrl+e"),
            ShortcutMenuMessage::RestoreDefault {
                row: editable_row("Quit", "ctrl+c"),
            }
        );
    }

    #[test]
    fn ctrl_e_is_a_noop_for_a_view_action() {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![view_action_row("Edit", "e")],
            all(),
            ShortcutScope::Context,
        );
        assert_eq!(m.handle_key("ctrl+e"), ShortcutMenuMessage::Handled);
    }

    // --- tagging (Ctrl-L / Ctrl-A / Ctrl-R) ---

    /// A menu whose first row is editable (taggable) and second read-only.
    fn tag_menu() -> ShortcutMenu {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![editable_row("Quit", "ctrl+c"), row("Delete", "d", "Jira")],
            all(),
            ShortcutScope::Context,
        );
        m
    }

    #[test]
    fn ctrl_l_toggles_a_tag_and_marks_the_list() {
        let mut m = tag_menu();
        assert!(!m.has_tags());
        m.handle_key("ctrl+l");
        assert!(m.has_tags());
        assert_eq!(m.list.marked(), vec![0], "the selected row is marked");
        // Toggling again untags it.
        m.handle_key("ctrl+l");
        assert!(!m.has_tags());
        assert!(m.list.marked().is_empty());
    }

    #[test]
    fn ctrl_l_is_a_noop_on_a_read_only_row() {
        let mut m = tag_menu();
        m.handle_key("ctrl+j"); // select row 1 (read-only, source None)
        m.handle_key("ctrl+l");
        assert!(!m.has_tags());
        assert!(m.list.marked().is_empty());
    }

    #[test]
    fn ctrl_a_tags_every_visible_taggable_row() {
        let mut m = tag_menu();
        m.handle_key("ctrl+a");
        // Only the editable row (index 0) is taggable; the read-only one is skipped.
        assert_eq!(m.list.marked(), vec![0]);
    }

    #[test]
    fn ctrl_shift_l_alias_still_tags_all_on_kitty_terminals() {
        // Kitty-protocol terminals disambiguate Ctrl+Shift+L, delivering it as
        // the canonical `ctrl+L`; the alias must keep working there.
        let mut m = tag_menu();
        m.handle_key("ctrl+L");
        assert_eq!(m.list.marked(), vec![0]);
    }

    #[test]
    fn ctrl_n_in_tag_mode_records_a_batch_bind_over_every_tagged_row() {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![
                editable_row("Quit", "ctrl+c"),
                editable_row("Save", "ctrl+s"),
            ],
            all(),
            ShortcutScope::Context,
        );
        m.handle_key("ctrl+a"); // tag every visible row
        m.handle_key("ctrl+n"); // start recording — batch mode, cursor irrelevant
        assert!(m.recorder.as_ref().is_some_and(|r| r.batch));
        m.handle_key("ctrl+g");
        match m.handle_key("enter") {
            ShortcutMenuMessage::BindTagged {
                rows,
                binding,
                overwrite,
            } => {
                assert_eq!(binding, "ctrl+g");
                assert!(!overwrite);
                assert_eq!(
                    rows.len(),
                    2,
                    "the recorded key applies to both tagged rows"
                );
            }
            other => panic!("expected BindTagged, got {other:?}"),
        }
        assert!(!m.has_tags(), "tags are consumed by the batch bind");
    }

    #[test]
    fn ctrl_u_in_tag_mode_records_an_overwriting_batch_bind() {
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![
                editable_row("Quit", "ctrl+c"),
                editable_row("Save", "ctrl+s"),
            ],
            all(),
            ShortcutScope::Context,
        );
        m.handle_key("ctrl+a");
        m.handle_key("ctrl+u");
        assert!(m.recorder.as_ref().is_some_and(|r| r.batch && r.overwrite));
        m.handle_key("x");
        match m.handle_key("enter") {
            ShortcutMenuMessage::BindTagged {
                binding, overwrite, ..
            } => {
                assert_eq!(binding, "x");
                assert!(overwrite);
            }
            other => panic!("expected BindTagged, got {other:?}"),
        }
    }

    #[test]
    fn empty_recording_in_tag_mode_binds_nothing() {
        let mut m = tag_menu();
        m.handle_key("ctrl+a");
        m.handle_key("ctrl+n");
        // Enter with no recorded steps just cancels — no batch message.
        assert_eq!(m.handle_key("enter"), ShortcutMenuMessage::Handled);
        assert!(m.recorder.is_none());
    }

    #[test]
    fn ctrl_r_clears_all_tags() {
        let mut m = tag_menu();
        m.handle_key("ctrl+l");
        assert!(m.has_tags());
        assert_eq!(m.handle_key("ctrl+r"), ShortcutMenuMessage::Handled);
        assert!(!m.has_tags());
        assert!(m.list.marked().is_empty());
    }

    #[test]
    fn a_tag_survives_a_scope_toggle() {
        // Context and All both carry the Quit action (same source identity),
        // so a tag set in one scope re-marks the matching row in the other.
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![editable_row("Quit", "ctrl+c")],
            vec![row("Other", "x", "Taiga"), editable_row("Quit", "ctrl+c")],
            ShortcutScope::Context,
        );
        m.handle_key("ctrl+l");
        assert_eq!(m.list.marked(), vec![0]);
        m.handle_key("tab"); // -> All scope; Quit is now at index 1
        assert_eq!(m.scope, ShortcutScope::All);
        assert_eq!(
            m.list.marked(),
            vec![1],
            "the tag re-projects onto Quit's new index in the All scope"
        );
    }

    #[test]
    fn tags_are_cleared_on_reopen() {
        let mut m = tag_menu();
        m.handle_key("ctrl+l");
        assert!(m.has_tags());
        m.open(ctx(), all(), ShortcutScope::Context);
        assert!(!m.has_tags(), "a fresh open starts untagged");
    }

    // --- batch tag ops (Ctrl-D / Ctrl-E on the tagged set) ---

    #[test]
    fn ctrl_d_with_tags_batch_disables_the_tagged_set() {
        let mut m = tag_menu();
        m.handle_key("ctrl+l"); // tag the editable Quit row
        assert!(m.has_tags());
        assert_eq!(
            m.handle_key("ctrl+d"),
            ShortcutMenuMessage::DeleteTagged {
                rows: vec![editable_row("Quit", "ctrl+c")],
            }
        );
        assert!(!m.has_tags(), "the batch op clears the tags");
    }

    #[test]
    fn ctrl_e_with_tags_batch_restores_the_tagged_set() {
        let mut m = tag_menu();
        m.handle_key("ctrl+l"); // tag the editable Quit row
        assert_eq!(
            m.handle_key("ctrl+e"),
            ShortcutMenuMessage::RestoreTagged {
                rows: vec![editable_row("Quit", "ctrl+c")],
            }
        );
        assert!(!m.has_tags(), "the batch op clears the tags");
    }

    #[test]
    fn batch_ops_carry_every_tagged_row() {
        // Two editable rows, both tagged via Ctrl-A, batch-delete carries both.
        let mut m = ShortcutMenu::new(theme(), false, "tab".into());
        m.open(
            vec![
                editable_row("Quit", "ctrl+c"),
                editable_row("Save", "ctrl+s"),
            ],
            all(),
            ShortcutScope::Context,
        );
        m.handle_key("ctrl+a");
        assert_eq!(
            m.handle_key("ctrl+d"),
            ShortcutMenuMessage::DeleteTagged {
                rows: vec![
                    editable_row("Quit", "ctrl+c"),
                    editable_row("Save", "ctrl+s"),
                ],
            }
        );
    }

    #[test]
    fn y_on_a_restore_batch_prompt_resolves_it() {
        let mut m = tag_menu();
        let rows = vec![editable_row("Quit", "ctrl+c")];
        let items = vec![conflict_item(true)];
        m.show_restore_conflicts(rows.clone(), items.clone());
        assert_eq!(
            m.handle_key("y"),
            ShortcutMenuMessage::ResolveRestoreBatchApply { rows, items }
        );
    }

    #[test]
    fn n_on_a_restore_batch_prompt_aborts_it() {
        let mut m = tag_menu();
        m.show_restore_conflicts(
            vec![editable_row("Quit", "ctrl+c")],
            vec![conflict_item(true)],
        );
        assert_eq!(m.handle_key("n"), ShortcutMenuMessage::Handled);
        // Prompt dismissed, nothing applied.
        assert!(m.conflict.is_none());
    }

    fn conflict_item(removable: bool) -> ConflictItem {
        use crate::keymap::KeySource;
        ConflictItem {
            source: KeySource::YamlAction {
                view: "tickets".into(),
                child_path: vec![],
                name: "Open".into(),
            },
            current: vec!["d".into(), "x".into()],
            drop: "d".into(),
            name: "Open".into(),
            removable,
        }
    }

    fn conflicted_menu() -> ShortcutMenu {
        let mut m = menu(false);
        m.show_conflicts(
            editable_row("Delete", "ctrl+c"),
            "d".into(),
            vec![conflict_item(true)],
            false,
        );
        m
    }

    #[test]
    fn conflict_prompt_confirm_emits_resolve() {
        let mut m = conflicted_menu();
        assert_eq!(
            m.handle_key("y"),
            ShortcutMenuMessage::ResolveConflictApply {
                row: editable_row("Delete", "ctrl+c"),
                binding: "d".into(),
                items: vec![conflict_item(true)],
                overwrite: false,
            }
        );
        assert!(m.conflict.is_none(), "prompt clears on confirm");
    }

    #[test]
    fn read_only_conflict_cannot_be_confirmed() {
        let mut m = menu(false);
        m.show_conflicts(
            editable_row("Delete", "ctrl+c"),
            "d".into(),
            vec![conflict_item(true), conflict_item(false)],
            false,
        );
        // `y` is inert while any conflict is read-only; only n/Esc dismiss.
        assert_eq!(m.handle_key("y"), ShortcutMenuMessage::Handled);
        assert!(m.conflict.is_some(), "unresolvable prompt stays up on y");
        assert_eq!(m.handle_key("esc"), ShortcutMenuMessage::Handled);
        assert!(m.conflict.is_none());
    }

    #[test]
    fn conflict_prompt_decline_leaves_everything_unchanged() {
        let mut m = conflicted_menu();
        assert_eq!(m.handle_key("n"), ShortcutMenuMessage::Handled);
        assert!(m.conflict.is_none());
        assert!(m.is_open());
        // Esc declines too.
        let mut m2 = conflicted_menu();
        assert_eq!(m2.handle_key("esc"), ShortcutMenuMessage::Handled);
        assert!(m2.conflict.is_none());
    }

    #[test]
    fn conflict_prompt_swallows_unrelated_keys() {
        let mut m = conflicted_menu();
        assert_eq!(m.handle_key("j"), ShortcutMenuMessage::Handled);
        assert!(m.conflict.is_some(), "still waiting for y/n");
        assert!(!m.list.search_active(), "keys do not leak to the filter");
    }
}
