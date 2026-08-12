//! Routing layer for the interactive keybinding editor: maps a
//! [`KeySource`] to the config file *and* the node path that owns its key
//! binding, then applies add / rebind / disable / remove edits through the
//! comment-preserving [`yaml_edit`](crate::config::yaml_edit) primitive.
//!
//! Only the sources the editor is allowed to touch are routable:
//! * the built-in sections (`global` / `common` / `content` / `window`) in
//!   `tui.yaml`, and
//! * per-view `actions:` entries in a `views/*.yaml` file.
//!
//! Every other origin — DB-stored saved-query / script shortcuts, user
//! `action_chains`, pane search-jump keys and the more specialised YAML keys
//! (`menu_key`, `preview.keybinding`, subtab `key`, child `keybindings`) —
//! is presented read-only by the menu and returns `None` from
//! [`locate_binding`].
//!
//! The routing (pure, no I/O) lives here so it can be unit-tested against
//! fixture YAML; the App layer resolves [`EditTarget::ViewFile`] to a concrete
//! `source_path`, reads/writes the file and triggers the reload.

use crate::config::yaml_edit::{self, PathStep};
use crate::keymap::KeySource;

/// Which config file owns a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditTarget {
    /// `tui.yaml` at the config root.
    TuiYaml,
    /// A view YAML file, identified by the top-level view name it declares.
    /// The App resolves this to the file's `source_path`.
    ViewFile { view: String },
    /// A view YAML file, identified by its **tab** display name (`tab.name`).
    /// Used for the tab-switch key, which lives in the file's `tab:` block
    /// rather than under a `views[*]` entry. The App resolves this to the
    /// file's `source_path`.
    TabFile { tab: String },
}

/// A fully-resolved location of one binding entry: which file, the node path
/// to the owning mapping, and the entry key to set/remove within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingLocation {
    pub target: EditTarget,
    pub path: Vec<PathStep>,
    pub entry: String,
}

/// If `source` is a built-in section binding, return its `tui.yaml` section
/// name and the YAML map key of the action (its `to_string()` form, which is
/// exactly how the section serialises it).
fn builtin_section(source: &KeySource) -> Option<(&'static str, String)> {
    match source {
        KeySource::Global(a) => Some(("global", a.to_string())),
        KeySource::Common(a) => Some(("common", a.to_string())),
        KeySource::Content(a) => Some(("content", a.to_string())),
        KeySource::Window(a) => Some(("window", a.to_string())),
        _ => None,
    }
}

/// Resolve the config location that owns `source`'s binding, or `None` if the
/// source is not editable through this menu (see the module docs).
pub fn locate_binding(source: &KeySource) -> Option<BindingLocation> {
    if let Some((section, action_key)) = builtin_section(source) {
        return Some(BindingLocation {
            target: EditTarget::TuiYaml,
            path: vec![PathStep::key("keybindings"), PathStep::key(section)],
            entry: action_key,
        });
    }

    match source {
        KeySource::YamlAction {
            view,
            child_path,
            name,
        } => {
            let mut path = vec![PathStep::key("views"), PathStep::find("name", view.clone())];
            for child in child_path {
                path.push(PathStep::key("children"));
                path.push(PathStep::find("name", child.clone()));
            }
            path.push(PathStep::key("actions"));
            path.push(PathStep::find("name", name.clone()));
            Some(BindingLocation {
                target: EditTarget::ViewFile { view: view.clone() },
                path,
                entry: "key".to_string(),
            })
        }
        KeySource::TabSwitch { tab } => Some(BindingLocation {
            target: EditTarget::TabFile { tab: tab.clone() },
            path: vec![PathStep::key("tab")],
            entry: "key".to_string(),
        }),
        // `views[*].key` — the subtab-switch key. Lives directly on the view
        // node; removing the entry drops the quick-switch key.
        KeySource::YamlSubtab { view } => Some(BindingLocation {
            target: EditTarget::ViewFile { view: view.clone() },
            path: vec![PathStep::key("views"), PathStep::find("name", view.clone())],
            entry: "key".to_string(),
        }),
        // `views[*].query.menu_key` — the saved-query menu opener.
        KeySource::YamlMenuKey { view } => Some(BindingLocation {
            target: EditTarget::ViewFile { view: view.clone() },
            path: vec![
                PathStep::key("views"),
                PathStep::find("name", view.clone()),
                PathStep::key("query"),
            ],
            entry: "menu_key".to_string(),
        }),
        // `views[*]….preview.keybinding` — the preview toggle, at the view
        // root or on a drill-down child.
        KeySource::YamlPreviewKey { view, child_path } => {
            let mut path = vec![PathStep::key("views"), PathStep::find("name", view.clone())];
            for child in child_path {
                path.push(PathStep::key("children"));
                path.push(PathStep::find("name", child.clone()));
            }
            path.push(PathStep::key("preview"));
            Some(BindingLocation {
                target: EditTarget::ViewFile { view: view.clone() },
                path,
                entry: "keybinding".to_string(),
            })
        }
        // `views[*]….card.key` — the card-mode toggle, at the view root or on
        // a drill-down child. Same shape as the preview key above.
        KeySource::YamlCardKey { view, child_path } => {
            let mut path = vec![PathStep::key("views"), PathStep::find("name", view.clone())];
            for child in child_path {
                path.push(PathStep::key("children"));
                path.push(PathStep::find("name", child.clone()));
            }
            path.push(PathStep::key("card"));
            Some(BindingLocation {
                target: EditTarget::ViewFile { view: view.clone() },
                path,
                entry: "key".to_string(),
            })
        }
        // `views[*].children[…].keybindings.<action>` — a per-child override
        // of a built-in content action's key. The map key is the action verb.
        KeySource::YamlChildKeybinding {
            view,
            child_path,
            action,
        } => {
            let mut path = vec![PathStep::key("views"), PathStep::find("name", view.clone())];
            for child in child_path {
                path.push(PathStep::key("children"));
                path.push(PathStep::find("name", child.clone()));
            }
            path.push(PathStep::key("keybindings"));
            Some(BindingLocation {
                target: EditTarget::ViewFile { view: view.clone() },
                path,
                entry: action.clone(),
            })
        }
        // A per-node `shortcuts:` entry: the map key *is* the binding, so
        // `entry` is that key char and the only supported edit is removing
        // the whole line (there is no `key:` value to rewrite).
        KeySource::NodeShortcut {
            view,
            child_path,
            key,
            ..
        } => {
            let mut path = vec![PathStep::key("views"), PathStep::find("name", view.clone())];
            for child in child_path {
                path.push(PathStep::key("children"));
                path.push(PathStep::find("name", child.clone()));
            }
            path.push(PathStep::key("shortcuts"));
            Some(BindingLocation {
                target: EditTarget::ViewFile { view: view.clone() },
                path,
                entry: key.clone(),
            })
        }
        // An `action_chains:` entry. `scope_path` is empty for the global
        // `keybindings.action_chains` in tui.yaml, or `[view, child…]` for a
        // view/child-scoped map. The map key *is* the binding.
        KeySource::AppActionChain { scope_path, key } => {
            if scope_path.is_empty() {
                return Some(BindingLocation {
                    target: EditTarget::TuiYaml,
                    path: vec![PathStep::key("keybindings"), PathStep::key("action_chains")],
                    entry: key.clone(),
                });
            }
            let view = scope_path[0].clone();
            let mut path = vec![PathStep::key("views"), PathStep::find("name", view.clone())];
            for child in &scope_path[1..] {
                path.push(PathStep::key("children"));
                path.push(PathStep::find("name", child.clone()));
            }
            path.push(PathStep::key("action_chains"));
            Some(BindingLocation {
                target: EditTarget::ViewFile { view },
                path,
                entry: key.clone(),
            })
        }
        // A search action's `search.next_key` / `search.prev_key`. Routable
        // only when the owning action's identity is known (the static keymap
        // builder fills it; a bare runtime claim leaves it empty → read-only).
        KeySource::PaneSearchJump {
            view,
            child_path,
            action,
            direction,
        } => {
            if view.is_empty() || action.is_empty() {
                return None;
            }
            let mut path = vec![PathStep::key("views"), PathStep::find("name", view.clone())];
            for child in child_path {
                path.push(PathStep::key("children"));
                path.push(PathStep::find("name", child.clone()));
            }
            path.push(PathStep::key("actions"));
            path.push(PathStep::find("name", action.clone()));
            path.push(PathStep::key("search"));
            let entry = match direction {
                crate::keymap::SearchJump::Next => "next_key",
                crate::keymap::SearchJump::Prev => "prev_key",
            };
            Some(BindingLocation {
                target: EditTarget::ViewFile { view: view.clone() },
                path,
                entry: entry.to_string(),
            })
        }
        _ => None,
    }
}

/// Set (add or rebind) the located binding within `source` YAML text to
/// `values`, returning the rewritten text. An empty `values` slice writes the
/// disable form `[]`; one value writes a scalar; several write a flow list.
/// A missing entry is inserted; an existing one is replaced in place with its
/// inline comment preserved.
pub fn set_binding(
    location: &BindingLocation,
    source: &str,
    values: &[String],
) -> Result<String, String> {
    yaml_edit::set_entry(source, &location.path, &location.entry, values)
}

/// Like [`set_binding`], but the located `path` names a mapping (its last
/// step) that may be present-but-empty or entirely absent — the shape of a
/// per-node `shortcuts:` block. Here `location.entry` is the **key chord** and
/// `values` its single **action verb**, the reverse of a `key:`-valued
/// binding. Creates the `shortcuts:` map / block as needed.
pub fn set_binding_in_optional_map(
    location: &BindingLocation,
    source: &str,
    values: &[String],
) -> Result<String, String> {
    yaml_edit::set_entry_in_optional_map(source, &location.path, &location.entry, values)
}

/// Remove the located binding *entry line* from `source` YAML text entirely.
///
/// Note the semantic difference from writing `[]` via [`set_binding`]:
/// * For a **view action** this drops the `key:` line, leaving the action
///   keyless — the intended "delete this shortcut" behaviour.
/// * For a **built-in section** removing the entry reverts the action to its
///   compiled-in default rather than disabling it. To *disable* a built-in,
///   call [`set_binding`] with an empty slice (`[]`), not this.
pub fn remove_binding(location: &BindingLocation, source: &str) -> Result<String, String> {
    yaml_edit::remove_entry(source, &location.path, &location.entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::keybindings::{CommonAction, GlobalAction};

    fn yaml_action(view: &str, child_path: &[&str], name: &str) -> KeySource {
        KeySource::YamlAction {
            view: view.to_string(),
            child_path: child_path.iter().map(|s| s.to_string()).collect(),
            name: name.to_string(),
        }
    }

    #[test]
    fn builtin_sources_route_to_tui_yaml_section() {
        let loc = locate_binding(&KeySource::Global(GlobalAction::Quit)).unwrap();
        assert_eq!(loc.target, EditTarget::TuiYaml);
        assert_eq!(
            loc.path,
            vec![PathStep::key("keybindings"), PathStep::key("global")]
        );
        assert_eq!(loc.entry, "quit");

        let common = locate_binding(&KeySource::Common(CommonAction::ListNext)).unwrap();
        assert_eq!(
            common.path,
            vec![PathStep::key("keybindings"), PathStep::key("common")]
        );
        assert_eq!(common.entry, "list_next");
    }

    #[test]
    fn top_level_yaml_action_routes_to_view_file() {
        let loc = locate_binding(&yaml_action("tickets", &[], "Edit")).unwrap();
        assert_eq!(
            loc.target,
            EditTarget::ViewFile {
                view: "tickets".to_string()
            }
        );
        assert_eq!(
            loc.path,
            vec![
                PathStep::key("views"),
                PathStep::find("name", "tickets"),
                PathStep::key("actions"),
                PathStep::find("name", "Edit"),
            ]
        );
        assert_eq!(loc.entry, "key");
    }

    #[test]
    fn nested_child_action_threads_the_child_path() {
        let loc = locate_binding(&yaml_action("tickets", &["Comments"], "Delete")).unwrap();
        assert_eq!(
            loc.path,
            vec![
                PathStep::key("views"),
                PathStep::find("name", "tickets"),
                PathStep::key("children"),
                PathStep::find("name", "Comments"),
                PathStep::key("actions"),
                PathStep::find("name", "Delete"),
            ]
        );
    }

    #[test]
    fn tab_switch_routes_to_tab_block_in_view_file() {
        let loc = locate_binding(&KeySource::TabSwitch {
            tab: "Tasks".to_string(),
        })
        .unwrap();
        assert_eq!(
            loc.target,
            EditTarget::TabFile {
                tab: "Tasks".to_string()
            }
        );
        assert_eq!(loc.path, vec![PathStep::key("tab")]);
        assert_eq!(loc.entry, "key");
    }

    #[test]
    fn set_tab_switch_key_inserts_into_tab_block() {
        let src = "\
tab:
  name: Tasks
  icon: ✅
adapter:
  type: local
";
        let loc = locate_binding(&KeySource::TabSwitch {
            tab: "Tasks".to_string(),
        })
        .unwrap();
        let out = set_binding(&loc, src, &["ctrl+1".to_string(), "j".to_string()]).unwrap();
        assert!(out.contains("key: [ctrl+1, j]"), "got: {out}");
        assert!(out.contains("name: Tasks"));
        assert!(out.contains("type: local"));
    }

    #[test]
    fn restore_tab_switch_removes_the_override_line() {
        let src = "\
tab:
  name: Tasks
  key: ctrl+1
adapter:
  type: local
";
        let loc = locate_binding(&KeySource::TabSwitch {
            tab: "Tasks".to_string(),
        })
        .unwrap();
        let out = remove_binding(&loc, src).unwrap();
        assert!(!out.contains("key:"), "override line should be gone: {out}");
        assert!(out.contains("name: Tasks"));
    }

    #[test]
    fn db_stored_sources_are_not_routable_via_yaml() {
        // Saved-query / script shortcuts live in the `query_shortcut` DB
        // table, not YAML — the App edits them through the repository, so
        // the YAML router declines them.
        assert!(
            locate_binding(&KeySource::SavedQueryShortcut {
                view: "tickets".into(),
                name: "mine".into(),
            })
            .is_none()
        );
        assert!(
            locate_binding(&KeySource::ScriptShortcut {
                scope: "trackings/entry".into(),
                name: "report".into(),
            })
            .is_none()
        );
    }

    #[test]
    fn global_action_chain_routes_to_tui_yaml() {
        let loc = locate_binding(&KeySource::AppActionChain {
            scope_path: vec![],
            key: "ctrl+a".into(),
        })
        .unwrap();
        assert_eq!(loc.target, EditTarget::TuiYaml);
        assert_eq!(
            loc.path,
            vec![PathStep::key("keybindings"), PathStep::key("action_chains"),]
        );
        assert_eq!(loc.entry, "ctrl+a");
    }

    #[test]
    fn view_action_chain_routes_to_view_file() {
        let loc = locate_binding(&KeySource::AppActionChain {
            scope_path: vec!["tickets".into(), "Comments".into()],
            key: "g d".into(),
        })
        .unwrap();
        assert_eq!(
            loc.target,
            EditTarget::ViewFile {
                view: "tickets".into()
            }
        );
        assert_eq!(
            loc.path,
            vec![
                PathStep::key("views"),
                PathStep::find("name", "tickets"),
                PathStep::key("children"),
                PathStep::find("name", "Comments"),
                PathStep::key("action_chains"),
            ]
        );
        assert_eq!(loc.entry, "g d");
    }

    #[test]
    fn search_jump_routes_into_action_search_block() {
        let loc = locate_binding(&KeySource::PaneSearchJump {
            view: "tickets".into(),
            child_path: vec![],
            action: "Search".into(),
            direction: crate::keymap::SearchJump::Prev,
        })
        .unwrap();
        assert_eq!(
            loc.path,
            vec![
                PathStep::key("views"),
                PathStep::find("name", "tickets"),
                PathStep::key("actions"),
                PathStep::find("name", "Search"),
                PathStep::key("search"),
            ]
        );
        assert_eq!(loc.entry, "prev_key");
    }

    #[test]
    fn search_jump_without_identity_is_read_only() {
        assert!(
            locate_binding(&KeySource::PaneSearchJump {
                view: String::new(),
                child_path: vec![],
                action: String::new(),
                direction: crate::keymap::SearchJump::Next,
            })
            .is_none()
        );
    }

    #[test]
    fn subtab_key_routes_to_view_key() {
        let loc = locate_binding(&KeySource::YamlSubtab {
            view: "search".into(),
        })
        .unwrap();
        assert_eq!(
            loc.target,
            EditTarget::ViewFile {
                view: "search".into()
            }
        );
        assert_eq!(
            loc.path,
            vec![PathStep::key("views"), PathStep::find("name", "search")]
        );
        assert_eq!(loc.entry, "key");
    }

    #[test]
    fn menu_key_routes_into_query_block() {
        let loc = locate_binding(&KeySource::YamlMenuKey {
            view: "tickets".into(),
        })
        .unwrap();
        assert_eq!(
            loc.path,
            vec![
                PathStep::key("views"),
                PathStep::find("name", "tickets"),
                PathStep::key("query"),
            ]
        );
        assert_eq!(loc.entry, "menu_key");
    }

    #[test]
    fn preview_key_routes_into_preview_block_with_child_path() {
        let loc = locate_binding(&KeySource::YamlPreviewKey {
            view: "tickets".into(),
            child_path: vec!["Comments".into()],
        })
        .unwrap();
        assert_eq!(
            loc.path,
            vec![
                PathStep::key("views"),
                PathStep::find("name", "tickets"),
                PathStep::key("children"),
                PathStep::find("name", "Comments"),
                PathStep::key("preview"),
            ]
        );
        assert_eq!(loc.entry, "keybinding");
    }

    #[test]
    fn child_keybinding_routes_to_keybindings_map_by_action() {
        let loc = locate_binding(&KeySource::YamlChildKeybinding {
            view: "tickets".into(),
            child_path: vec!["Comments".into()],
            action: "back".into(),
        })
        .unwrap();
        assert_eq!(
            loc.path,
            vec![
                PathStep::key("views"),
                PathStep::find("name", "tickets"),
                PathStep::key("children"),
                PathStep::find("name", "Comments"),
                PathStep::key("keybindings"),
            ]
        );
        assert_eq!(loc.entry, "back");
    }

    #[test]
    fn remove_subtab_key_drops_the_line() {
        let src = "\
views:
  - name: search
    key: s
    columns: []
";
        let loc = locate_binding(&KeySource::YamlSubtab {
            view: "search".into(),
        })
        .unwrap();
        let out = remove_binding(&loc, src).unwrap();
        assert!(!out.contains("key: s"), "subtab key should be gone: {out}");
        assert!(out.contains("- name: search"));
        assert!(out.contains("columns: []"));
    }

    #[test]
    fn disable_a_builtin_writes_empty_list_and_keeps_comments() {
        let src = "keybindings:\n  global:\n    quit: ctrl+c  # exit\n    tab_next: tab\n";
        let loc = locate_binding(&KeySource::Global(GlobalAction::Quit)).unwrap();
        let out = set_binding(&loc, src, &[]).unwrap();
        assert_eq!(
            out,
            "keybindings:\n  global:\n    quit: []  # exit\n    tab_next: tab\n"
        );
    }

    #[test]
    fn rebind_a_view_action_replaces_in_place() {
        let src = "\
views:
  - name: tickets
    actions:
      - name: Edit
        key: e
        type: adapter
";
        let loc = locate_binding(&yaml_action("tickets", &[], "Edit")).unwrap();
        let out = set_binding(&loc, src, &["ctrl+e".to_string(), "E".to_string()]).unwrap();
        assert!(out.contains("key: [ctrl+e, E]"), "got: {out}");
        // Every other line survives verbatim.
        assert!(out.contains("- name: Edit"));
        assert!(out.contains("type: adapter"));
    }

    #[test]
    fn remove_a_view_action_key_drops_the_line() {
        let src = "\
views:
  - name: tickets
    actions:
      - name: Edit
        key: e
        type: adapter
";
        let loc = locate_binding(&yaml_action("tickets", &[], "Edit")).unwrap();
        let out = remove_binding(&loc, src).unwrap();
        assert!(!out.contains("key: e"), "key line should be gone: {out}");
        assert!(out.contains("- name: Edit"));
        assert!(out.contains("type: adapter"));
    }
}
