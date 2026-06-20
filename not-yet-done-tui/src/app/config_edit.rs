//! In-app editing of YAML config files under `~/.config/not_yet_done/`.
//!
//! Three pieces:
//!
//! * [`discover_config_files`] — recursive walk that yields every `*.yaml`
//!   under the config root.
//! * [`App::open_config_picker`] — opens a [`SearchablePopup`] of those
//!   files (or jumps straight in when the prefilter matches exactly one).
//! * [`App::handle_config_picker_key`] — modal key dispatch while the
//!   picker is open. Mirrors the link-popup pattern.
//!
//! The actual edit + reload happens via [`crate::edit_session::FileEditSession`]
//! (constructed when the picker activates a row) and
//! [`App::reload_config`] (called from the session's `FollowUp`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::config::TuiConfigService;
use crate::ui::theme::Theme;

use super::App;

/// Root used for config discovery and the prefix that's stripped when
/// building popup labels. `~/.config/not_yet_done/` in the common case.
pub fn config_root() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("not_yet_done"))
}

/// Recursively collect every `*.yaml` under `root`. Returns paths sorted
/// by their string form so the picker order is stable. Silently skips
/// unreadable entries — discovery should never panic on permission
/// errors.
pub fn discover_config_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_yaml(root, &mut out);
    out.sort();
    out
}

fn walk_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            walk_yaml(&path, out);
        } else if ft.is_file() && path.extension().and_then(|s| s.to_str()) == Some("yaml") {
            out.push(path);
        }
    }
}

/// Label shown in the picker — path relative to the config root, or the
/// absolute path when the file lives outside the root (shouldn't happen
/// in practice but the function stays total).
pub fn config_label(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

impl App {
    /// Open the `:config` picker. With `prefilter` set, the popup is
    /// pre-filtered to those substring matches; when exactly one file
    /// matches *and* a prefilter was given, the picker is skipped and
    /// the editor opens directly — `:config jira` → straight to jira.yaml.
    ///
    /// No-ops with an error notification when:
    /// * the config dir cannot be located (no `XDG_CONFIG_HOME`/`HOME`),
    /// * the dir is missing or contains no `*.yaml` files.
    pub fn open_config_picker(&mut self, prefilter: Option<&str>) {
        let Some(root) = config_root() else {
            self.notify_error("Cannot locate config dir".to_string());
            return;
        };
        if !root.exists() {
            self.notify_error(format!("Config dir not found: {}", root.display()));
            return;
        }
        let files = discover_config_files(&root);
        if files.is_empty() {
            self.notify_error(format!("No YAML configs under {}", root.display()));
            return;
        }

        if let Some(filter) = prefilter {
            let matches: Vec<&PathBuf> = files
                .iter()
                .filter(|p| config_label(p, &root).contains(filter))
                .collect();
            if matches.len() == 1 {
                let path = matches[0].clone();
                self.open_config_edit_session(path);
                return;
            }
        }

        let items: Vec<PopupItem> = files
            .iter()
            .map(|p| PopupItem {
                label: config_label(p, &root),
                value: p.display().to_string(),
                ..Default::default()
            })
            .collect();
        let mut popup = SearchablePopup::new(
            std::sync::Arc::clone(&self.shared_theme),
            "Edit config",
            items,
        )
        .with_popup_kb(
            self.keybindings.popup.clone(),
            self.keybindings.key_icons.clone(),
        );
        if let Some(filter) = prefilter {
            for c in filter.chars() {
                popup.insert_char(c);
            }
        }
        self.config_picker_popup = Some(popup);
    }

    /// Modal key dispatch for the config picker. Returns `true` when the
    /// key was consumed. Mirrors `handle_link_popup_key`.
    pub fn handle_config_picker_key(&mut self, key: &str) -> bool {
        if self.config_picker_popup.is_none() {
            return false;
        }
        match key {
            "esc" => {
                self.config_picker_popup = None;
                true
            }
            "enter" => {
                self.config_picker_activate_selected();
                true
            }
            // Navigation + text input — delegated to the popup's intrinsic
            // PopupAction bindings.
            other => {
                if let Some(p) = self.config_picker_popup.as_mut() {
                    let _ = p.handle_key(other);
                }
                true
            }
        }
    }

    fn config_picker_activate_selected(&mut self) {
        let Some(popup) = self.config_picker_popup.as_ref() else {
            return;
        };
        let Some(item) = popup.selected_item() else {
            return;
        };
        let path = PathBuf::from(&item.value);
        self.config_picker_popup = None;
        self.open_config_edit_session(path);
    }

    /// Open the external editor on `path` via a
    /// [`crate::edit_session::FileEditSession`]. Surfaces I/O errors
    /// (file missing, permission denied) as notifications instead of
    /// panicking — the picker only offers paths it found on disk, so
    /// failure here usually means a TOCTOU race with `rm`.
    pub fn open_config_edit_session(&mut self, path: PathBuf) {
        match crate::edit_session::FileEditSession::open(path.clone()) {
            Ok(session) => {
                let _ = self.open_session(Box::new(session));
            }
            Err(e) => {
                self.notify_error(format!("Cannot open {}: {e}", path.display()));
            }
        }
    }

    /// Re-apply a freshly-saved config file in-process. Granular when
    /// possible (single view yaml → rebuild only that
    /// [`crate::app::ContentSlot`]), full otherwise (tui.yaml or
    /// adapter yaml → rebuild theme + keybindings + every content view).
    ///
    /// Returns the message to surface in the notification bar on
    /// success, or an error string to surface as `notify_error`. On
    /// `Err` the in-memory config is left untouched — the old config
    /// keeps running until the user fixes the file.
    pub fn reload_config(&mut self, path: &Path) -> Result<String, String> {
        let canon = std::fs::canonicalize(path)
            .map_err(|e| format!("canonicalize {}: {e}", path.display()))?;
        let root = config_root().ok_or("config dir missing")?;
        let root_canon = std::fs::canonicalize(&root)
            .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;

        if !canon.starts_with(&root_canon) {
            return Err(format!(
                "{} is outside {}; refusing to reload",
                canon.display(),
                root_canon.display()
            ));
        }
        let rel = canon
            .strip_prefix(&root_canon)
            .expect("checked above")
            .to_path_buf();

        // tui.yaml at the config root → full theme/keybindings reload.
        if rel == Path::new("tui.yaml") {
            return self.reload_tui_config();
        }

        // View yaml under views/ that matches a Working slot → granular.
        let slot_idx = self.content_views.iter().position(|s| match s {
            crate::app::ContentSlot::Working(cv) => cv
                .source_path
                .as_ref()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .map(|p| p == canon)
                .unwrap_or(false),
            crate::app::ContentSlot::Broken { path: p, .. } => std::fs::canonicalize(p)
                .map(|c| c == canon)
                .unwrap_or(false),
        });
        if let Some(idx) = slot_idx {
            return self.reload_single_view(idx, &canon);
        }

        // Anything else under the config dir (adapter configs, snippets):
        // rebuild every content view so adapter clients pick up the change.
        self.reload_all_content_views()
            .map(|_| format!("All views reloaded after {} change", rel.display()))
    }

    /// Re-read `tui.yaml` and rebuild the theme + keybindings-bound
    /// components. Tasks/Trackings views are recreated, so their loaded
    /// data is wiped — `spawn_load` re-fills them right after.
    fn reload_tui_config(&mut self) -> Result<String, String> {
        let new_config = TuiConfigService::load().map_err(|e| e.to_string())?;
        let new_keybindings = new_config.keybindings.clone();
        let shared_theme = Arc::new(Theme::new(new_config.theme.clone()));
        let new_theme = crate::ui::theme::Theme::new(new_config.theme.clone());

        let content_tab_infos: Vec<crate::components::tab_bar::ContentTabInfo> = self
            .content_views
            .iter()
            .map(|slot| crate::components::tab_bar::ContentTabInfo {
                name: slot.tab_name().to_string(),
                icon: slot.tab_icon().unwrap_or_default().to_string(),
            })
            .collect();
        let tab_bar = crate::components::tab_bar::TabBarComponent::new(
            Arc::clone(&shared_theme),
            &new_keybindings,
            &content_tab_infos,
        );
        let status_bar = crate::components::status_bar::StatusBarComponent::new(
            Arc::clone(&shared_theme),
            &new_keybindings,
        );
        let mut notification_bar =
            crate::components::notification_bar::NotificationBarComponent::new(Arc::clone(
                &shared_theme,
            ));
        notification_bar.set_max_lines(new_config.notifications.max_lines);

        // Rebuild content views too — they hold keybinding/theme refs.
        let factories = (self.adapter_factory_builder)();
        let new_content_views = super::load_content_views(
            &shared_theme,
            &new_keybindings,
            &new_config.editors,
            factories,
        );

        self.config = new_config;
        self.keybindings = new_keybindings;
        self.shared_theme = shared_theme;
        self.theme = new_theme;
        self.tab_bar = tab_bar;
        self.status_bar = status_bar;
        self.notification_bar = notification_bar;
        self.content_views = new_content_views;
        // The `tabs:` section and/or the view set may have changed.
        self.rebuild_tab_layout();

        // Refill data the rebuild dropped.
        self.refresh_tracked_ids();
        self.reload_link_refs();

        Ok("tui.yaml reloaded".to_string())
    }

    /// Replace a single view slot by re-parsing its YAML. Preserves the
    /// rest of the App state — other tabs untouched.
    fn reload_single_view(&mut self, slot_idx: usize, path: &Path) -> Result<String, String> {
        use crate::config::view_config::ViewFileConfig;

        let yaml =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let raw: serde_yaml::Value =
            serde_yaml::from_str(&yaml).map_err(|e| format!("YAML parse: {e}"))?;
        if raw.get("tab").is_none() || raw.get("adapter").is_none() {
            return Err(format!(
                "{} is missing `tab:` and/or `adapter:` — not a view config",
                path.display()
            ));
        }
        let mut config: ViewFileConfig =
            serde_yaml::from_str(&yaml).map_err(|e| format!("view-config parse: {e}"))?;
        // Mirror the startup loader: inherit tree-continuation columns
        // before validating, so an in-app config edit is judged the same way.
        config.inherit_tree_columns();
        config.inherit_tree_actions();
        if let Err(errors) = config.validate(&self.keybindings, &self.config.editors) {
            return Err(format!("view-config validation:\n{}", errors.join("\n")));
        }

        let factories = (self.adapter_factory_builder)();
        let factory = factories.get(&config.adapter.adapter_type).ok_or_else(|| {
            format!(
                "no adapter factory registered for type '{}'",
                config.adapter.adapter_type
            )
        })?;

        let adapter_config = config
            .adapter
            .config_inline
            .as_ref()
            .cloned()
            .or_else(|| {
                config.adapter.config.as_ref().and_then(|cfg_path| {
                    let resolved = if Path::new(cfg_path).is_absolute() {
                        PathBuf::from(cfg_path)
                    } else {
                        path.parent().unwrap_or(Path::new(".")).join(cfg_path)
                    };
                    std::fs::read_to_string(&resolved).ok()
                })
            })
            .ok_or_else(|| {
                "adapter config missing (neither `config_inline` nor a readable `config:` path)"
                    .to_string()
            })?;

        let adapter = factory
            .create(config.adapter.effective_instance_id(), &adapter_config)
            .map_err(|e| format!("adapter init: {e}"))?;
        let adapter: std::sync::Arc<dyn not_yet_done_content::ContentAdapter> =
            std::sync::Arc::from(adapter);

        let mut view = crate::views::content_view::ContentView::new(
            std::sync::Arc::clone(&self.shared_theme),
            &config,
            Some(adapter),
            &self.keybindings,
        );
        view.source_path = Some(path.to_path_buf());
        view.view_index = slot_idx;

        self.content_views[slot_idx] = crate::app::ContentSlot::Working(view);

        // Rebuild the tab-bar names — `tab.name` may have changed.
        let content_tab_infos: Vec<crate::components::tab_bar::ContentTabInfo> = self
            .content_views
            .iter()
            .map(|slot| crate::components::tab_bar::ContentTabInfo {
                name: slot.tab_name().to_string(),
                icon: slot.tab_icon().unwrap_or_default().to_string(),
            })
            .collect();
        self.tab_bar = crate::components::tab_bar::TabBarComponent::new(
            Arc::clone(&self.shared_theme),
            &self.keybindings,
            &content_tab_infos,
        );
        // `tab.name` may have changed → re-resolve the constellation.
        self.rebuild_tab_layout();

        Ok(format!(
            "Reloaded view {}",
            path.file_name().and_then(|s| s.to_str()).unwrap_or("?")
        ))
    }

    /// Rebuild every content view from scratch — used when an adapter
    /// config under `views/` changed (and we can't tell which slots
    /// referenced it from the path alone). Preserves tui.yaml-derived
    /// theme + keybindings + trackings_view.
    fn reload_all_content_views(&mut self) -> Result<String, String> {
        let factories = (self.adapter_factory_builder)();
        let new_content_views = super::load_content_views(
            &self.shared_theme,
            &self.keybindings,
            &self.config.editors,
            factories,
        );
        self.content_views = new_content_views;

        let content_tab_infos: Vec<crate::components::tab_bar::ContentTabInfo> = self
            .content_views
            .iter()
            .map(|slot| crate::components::tab_bar::ContentTabInfo {
                name: slot.tab_name().to_string(),
                icon: slot.tab_icon().unwrap_or_default().to_string(),
            })
            .collect();
        self.tab_bar = crate::components::tab_bar::TabBarComponent::new(
            Arc::clone(&self.shared_theme),
            &self.keybindings,
            &content_tab_infos,
        );
        // View set / names may have changed → re-resolve the constellation.
        self.rebuild_tab_layout();

        Ok("All content views reloaded".to_string())
    }

    /// Re-open the editor on `path` after a reload failure. Reads the
    /// current on-disk content (the user's last save) and prepends an
    /// error banner with `error`. Used by [`FollowUp::ReloadConfig`]'s
    /// failure arm.
    pub fn reopen_config_with_error(&mut self, path: PathBuf, error: String) {
        match crate::edit_session::FileEditSession::with_error(path.clone(), error) {
            Ok(session) => {
                let _ = self.open_session(Box::new(session));
            }
            Err(e) => {
                self.notify_error(format!("Cannot reopen {}: {e}", path.display()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "").unwrap();
    }

    #[test]
    fn discover_finds_nested_yaml_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("tui.yaml"));
        touch(&root.join("views/jira.yaml"));
        touch(&root.join("views/sub/deep.yaml"));
        touch(&root.join("views/jira.yaml.bak"));
        touch(&root.join("README.md"));

        let found = discover_config_files(root);
        let labels: Vec<String> = found.iter().map(|p| config_label(p, root)).collect();
        assert_eq!(
            labels,
            vec!["tui.yaml", "views/jira.yaml", "views/sub/deep.yaml"]
        );
    }

    #[test]
    fn discover_returns_empty_when_root_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let found = discover_config_files(&missing);
        assert!(found.is_empty());
    }

    #[test]
    fn config_label_relative_to_root() {
        let root = Path::new("/home/u/.config/not_yet_done");
        let p = Path::new("/home/u/.config/not_yet_done/views/jira.yaml");
        assert_eq!(config_label(p, root), "views/jira.yaml");
    }

    #[test]
    fn config_label_falls_back_to_absolute_when_outside_root() {
        let root = Path::new("/home/u/.config/not_yet_done");
        let p = Path::new("/tmp/other.yaml");
        assert_eq!(config_label(p, root), "/tmp/other.yaml");
    }
}
