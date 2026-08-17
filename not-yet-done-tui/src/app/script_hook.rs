//! Hook-triggered view scripts — the same scripts the `:script` menu
//! runs, started by an event instead of a key press.
//!
//! Three pieces:
//!   - [`ScriptHook`]: the vocabulary. Today only `reload`, stored per
//!     `(scope, name)` in the `script_hook` table next to the script's key
//!     chord — see [`crate::app::script::ScriptContext::shortcut_scope`]
//!     for the scope string both share.
//!   - The picker ([`ScriptHookPicker`]), opened with Ctrl+H on a selected
//!     entry in the script menu.
//!   - The firing seam, [`App::fire_reload_hooks`], called from the
//!     `LoadMsg::ContentItems` handler once a pane's rows have landed.
//!
//! **Loop protection.** A hook script may itself ask for a reload — that
//! is the point of the `commands` mode — so the trigger has to be able to
//! tell "the user reloaded" from "a script reloaded". The answer rides
//! with the load: [`App::load_hook_depth`] is the ambient depth every
//! newly spawned load is stamped with, and it is raised for exactly as
//! long as a hook run (plus every command it emits) is executing. Loads
//! arriving with a depth of [`HOOK_MAX_DEPTH`] or more do not fire hooks,
//! so a reload *caused by* a reload script runs no reload scripts. The
//! guard deliberately sits on the load and not on the command name: a
//! script emitting `:jump`, a query command or anything else that ends in
//! a fetch would otherwise slip past a `:reload`-only check.

use std::sync::Arc;

use crate::app::editor::parse_script_mode;
use crate::app::{App, EditorRequest};
use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::config::keybindings::ScriptMenuAction;
use crate::views::content_view::PaneId;

/// Events a view script can be bound to. Unknown values read from the
/// database parse to `None` and are ignored, so a binding written by a
/// newer version never breaks an older one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptHook {
    /// Run right after the pane's rows have (re)loaded — including the
    /// first load when the view opens, and a drill-down into a level.
    Reload,
}

impl ScriptHook {
    pub const ALL: &'static [ScriptHook] = &[ScriptHook::Reload];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reload => "reload",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reload" => Some(Self::Reload),
            _ => None,
        }
    }

    /// One-line explanation shown next to the entry in the picker.
    pub fn description(self) -> &'static str {
        match self {
            Self::Reload => "after the view's rows (re)loaded",
        }
    }
}

/// Hook depth at which hooks stop firing. `1` means: a load caused by a
/// hook script runs no further hooks.
pub const HOOK_MAX_DEPTH: u8 = 1;

/// Open picker over [`ScriptHook::ALL`], plus a "no hook" entry. Bound to
/// one `(scope, name)` — the same key the script's chord is stored under.
pub struct ScriptHookPicker {
    pub popup: SearchablePopup,
    scope: String,
    name: String,
}

impl App {
    /// All hook bindings registered for `scope`, as `(script name, hook)`.
    /// Empty on a DB error — a hook that cannot be read must not take the
    /// view down with it.
    fn hooks_for_scope(&self, scope: &str) -> Vec<(String, String)> {
        let repo = Arc::clone(&self.script_hook_repo);
        let scope = scope.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { repo.list_by_scope(&scope).await.unwrap_or_default() })
        })
        .into_iter()
        .map(|m| (m.name, m.hook))
        .collect()
    }

    /// Map of `script name → hook`, for decorating the script menu.
    pub(super) fn hook_labels_for_scope(
        &self,
        scope: &str,
    ) -> std::collections::HashMap<String, String> {
        self.hooks_for_scope(scope).into_iter().collect()
    }

    /// Persist (or clear, when `hook` is `None`) the hook of one script.
    fn store_script_hook(&mut self, scope: &str, name: &str, hook: Option<ScriptHook>) {
        let repo = Arc::clone(&self.script_hook_repo);
        let scope_owned = scope.to_string();
        let name_owned = name.to_string();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                match hook {
                    Some(h) => repo
                        .set(&scope_owned, &name_owned, h.as_str())
                        .await
                        .map(|_| ()),
                    None => repo.unset(&scope_owned, &name_owned).await,
                }
            })
        });
        match result {
            Ok(()) => match hook {
                Some(h) => self.notify(format!("Script '{name}' now runs {}", h.description())),
                None => self.notify(format!("Hook removed from script '{name}'")),
            },
            Err(e) => self.notify_error(format!("Failed to store hook for '{name}': {e}")),
        }
    }

    /// Drop a script's hook binding without a message — used when the
    /// script file itself is deleted, so no orphan row keeps pointing at
    /// a script that is gone.
    pub(super) fn clear_script_hook(&mut self, scope: &str, name: &str) {
        let repo = Arc::clone(&self.script_hook_repo);
        let scope = scope.to_string();
        let name = name.to_string();
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { repo.unset(&scope, &name).await })
        });
    }

    /// Ctrl+H in the script menu: offer the hooks this script can be bound
    /// to. Interactive and capture modes are rejected up front — a hook
    /// fires while a load lands, where there is nobody to hand a terminal
    /// or an editor to.
    pub(super) fn open_script_hook_picker(&mut self, scope: String, name: String, path: &str) {
        if let Some(reason) = hook_mode_rejection(path) {
            self.notify_error(format!("Cannot hook '{name}': {reason}"));
            return;
        }
        let current = self
            .hooks_for_scope(&scope)
            .into_iter()
            .find(|(n, _)| n == &name)
            .map(|(_, h)| h);
        let mut items = vec![PopupItem {
            label: "none".to_string(),
            value: String::new(),
            marked: current.is_none(),
            suffix: Some("run manually only".to_string()),
        }];
        items.extend(ScriptHook::ALL.iter().map(|h| PopupItem {
            label: h.as_str().to_string(),
            value: h.as_str().to_string(),
            marked: current.as_deref() == Some(h.as_str()),
            suffix: Some(h.description().to_string()),
        }));
        let popup = SearchablePopup::new(
            Arc::clone(&self.shared_theme),
            format!("Hook · {name}"),
            items,
        )
        .with_popup_kb(
            self.keybindings.popup.clone(),
            self.keybindings.key_icons.clone(),
        )
        .with_hints(vec![
            (
                self.keybindings.script_menu.label(&ScriptMenuAction::Run),
                "bind".into(),
            ),
            (
                self.keybindings.script_menu.label(&ScriptMenuAction::Close),
                "cancel".into(),
            ),
        ]);
        self.script_hook_picker = Some(ScriptHookPicker { popup, scope, name });
    }

    /// Dispatch one keypress while the hook picker is open.
    pub fn handle_script_hook_picker_key(&mut self, key: &str) {
        let kb = self.keybindings.script_menu.clone();
        if kb
            .get(&ScriptMenuAction::Close)
            .is_some_and(|b| b.matches(key))
        {
            self.script_hook_picker = None;
            return;
        }
        if kb
            .get(&ScriptMenuAction::Run)
            .is_some_and(|b| b.matches(key))
        {
            let Some(picker) = self.script_hook_picker.take() else {
                return;
            };
            let Some(item) = picker.popup.selected_item() else {
                return;
            };
            let hook = ScriptHook::parse(&item.value);
            if hook.is_none() && !item.value.is_empty() {
                return;
            }
            self.store_script_hook(&picker.scope, &picker.name, hook);
            return;
        }
        if let Some(picker) = &mut self.script_hook_picker {
            picker.popup.handle_key(key);
        }
    }

    /// Run every script bound to [`ScriptHook::Reload`] on the pane whose
    /// load just landed. `hook_depth` is the depth the finished load
    /// carried; see the module docs for how it stops a trigger loop.
    pub(super) fn fire_reload_hooks(&mut self, view_index: usize, pane_id: PaneId, hook_depth: u8) {
        if hook_depth >= HOOK_MAX_DEPTH {
            return;
        }
        // Re-entrancy guard. The synchronous runner cannot re-enter on its
        // own — it blocks the main loop — but a hook that ends in a load
        // for the *same* pane must not be able to stack up either.
        if self.hook_runs_in_flight.contains(&(view_index, pane_id)) {
            return;
        }
        let Some(scope) = self
            .content_view(view_index)
            .and_then(|cv| cv.pane_script_scope(pane_id))
        else {
            return;
        };
        let mut names: Vec<String> = self
            .hooks_for_scope(&scope)
            .into_iter()
            .filter(|(_, hook)| ScriptHook::parse(hook) == Some(ScriptHook::Reload))
            .map(|(name, _)| name)
            .collect();
        if names.is_empty() {
            return;
        }
        // Stable order: the menu lists scripts by name, so hooks run in the
        // order the user sees them.
        names.sort();

        self.hook_runs_in_flight.insert((view_index, pane_id));
        let previous_depth = self.load_hook_depth;
        // Everything spawned from here on — the script's own commands, the
        // reload `run_script` does after a batch/table run — is stamped one
        // level deeper and therefore fires no further hooks.
        self.load_hook_depth = hook_depth + 1;
        for name in names {
            self.run_hook_script(view_index, pane_id, &name);
        }
        self.load_hook_depth = previous_depth;
        self.hook_runs_in_flight.remove(&(view_index, pane_id));
    }

    /// One hook run: rebuild the payload context exactly as the manual run
    /// would ([`App::run_script_shortcut`] does the same for a chord) and
    /// execute the script.
    fn run_hook_script(&mut self, view_index: usize, pane_id: PaneId, name: &str) {
        use crate::config::view_config::ScriptScope;
        let Some((scope, default_field)) = self
            .content_view(view_index)
            .and_then(|cv| cv.pane_script_action(pane_id))
        else {
            return;
        };
        let ctx = match scope {
            ScriptScope::Node => self.build_content_node_ctx(view_index, pane_id),
            ScriptScope::FilteredSet => self.build_content_batch_ctx(view_index, pane_id),
            ScriptScope::Table => self.build_content_table_ctx(view_index, pane_id, default_field),
        };
        let Some(ctx) = ctx else {
            return;
        };
        let path = ctx.scripts_dir().join(name);
        // A hook whose script was deleted behind the app's back is not an
        // error the user needs on every single load — the menu shows the
        // binding, and re-binding clears it.
        if !path.exists() {
            return;
        }
        if let Some(reason) = hook_mode_rejection(&path.to_string_lossy()) {
            self.notify_error(format!("Hook script '{name}' not run: {reason}"));
            return;
        }
        // A hook fires with no cursor intent behind it, so the script's own
        // `# scope:` matters most here: a reload hook that maintains a column
        // wants the rows that just landed, not whatever row the cursor sits on.
        let ctx = self.apply_script_scope_header(ctx, &path);
        let request = self.run_script(&ctx, &path.to_string_lossy());
        // The modes a hook is allowed to use never open an editor; a
        // request here would mean the header changed after binding.
        if !matches!(request, EditorRequest::None) {
            self.notify_error(format!(
                "Hook script '{name}' asked for an editor — ignored (hooks run unattended)"
            ));
        }
    }
}

/// Why `path` cannot run as a hook, or `None` when it can. Hooks run
/// unattended while a load lands, so only the two modes that need neither
/// the terminal nor an editor are allowed: `background` and `commands`.
fn hook_mode_rejection(path: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mode = parse_script_mode(&content);
    if mode.is_interactive() {
        return Some("interactive scripts cannot run as a hook".to_string());
    }
    if mode.captures_output() {
        return Some("capture-mode scripts cannot run as a hook".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_names_round_trip() {
        for hook in ScriptHook::ALL {
            assert_eq!(ScriptHook::parse(hook.as_str()), Some(*hook));
        }
    }

    /// A row written by a newer version must be ignored, not crash the
    /// load path.
    #[test]
    fn unknown_hook_names_are_ignored() {
        assert_eq!(ScriptHook::parse("on_startup"), None);
        assert_eq!(ScriptHook::parse(""), None);
    }

    #[test]
    fn only_unattended_modes_may_hook() {
        let dir = std::env::temp_dir().join("nyd-hook-mode-test");
        std::fs::create_dir_all(&dir).unwrap();
        let cases = [
            ("bg.py", "# mode: background\n", false),
            ("cmd.py", "# mode: commands\n", false),
            ("plain.py", "print(1)\n", false), // defaults to background
            ("cap.py", "# mode: capture\n", true),
            ("int.py", "# mode: interactive\n", true),
            ("intcmd.py", "# mode: interactive+commands\n", true),
        ];
        for (file, body, rejected) in cases {
            let path = dir.join(file);
            std::fs::write(&path, body).unwrap();
            assert_eq!(
                hook_mode_rejection(&path.to_string_lossy()).is_some(),
                rejected,
                "{file}"
            );
        }
    }
}
