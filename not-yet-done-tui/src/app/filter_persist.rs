//! Filter / saved-query persistence and live application.
//!
//! Helpers for the three query-filter scopes (Tasks, Trackings, Content):
//! parsing the YAML buffer, applying it to the active view, persisting to
//! the `saved_query_repo`, and prompting for the favorite-shortcut on
//! new entries.
//!
//! Sits next to [`super::editor`] which dispatches the relevant
//! [`crate::edit_session::FollowUp`] variants here from
//! `handle_follow_up`.

use std::sync::Arc;

use uuid::Uuid;

use crate::query_filter;
use crate::tabs::LoadState;

use super::{App, LoadMsg};

impl App {
    // -----------------------------------------------------------------------
    // Content query — live + close
    // -----------------------------------------------------------------------

    /// Apply the query to the view live (no DB save, no prompt).
    /// Used during live-reload while the editor is still open.
    pub(super) fn apply_content_query_live(
        &mut self,
        content: &str,
        view_index: usize,
        save_name: Option<&str>,
    ) {
        let query = content.trim().to_string();
        if query.is_empty() {
            return;
        }
        let name = save_name.map(|s| s.to_string());
        let pane_id = if let Some(cv) = self.content_view_mut(view_index) {
            cv.set_query(query, name);
            cv.active_pane_id()
        } else {
            return;
        };
        self.spawn_content_load(view_index, pane_id);
    }

    /// Final processing on editor close: apply, save to DB, prompt for shortcut on new entries.
    pub(super) fn process_content_query_edit(
        &mut self,
        content: &str,
        view_index: usize,
        save_name: Option<&str>,
        is_new: bool,
    ) {
        let query = content.trim().to_string();
        if query.is_empty() {
            self.notify("Cancelled (empty query)".to_string());
            return;
        }
        let name = save_name.map(|s| s.to_string());
        let pane_id = if let Some(cv) = self.content_view_mut(view_index) {
            cv.set_query(query.clone(), name.clone());
            cv.active_pane_id()
        } else {
            return;
        };
        self.spawn_content_load(view_index, pane_id);

        if let Some(name) = name {
            let scope = self.content_view(view_index)
                .map(|cv| cv.query_scope.clone())
                .unwrap_or_default();
            let repo = Arc::clone(&self.saved_query_repo);
            let _ = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    repo.upsert(&scope, &name, &query, None).await
                })
            });
            self.reload_content_saved_queries(view_index);
            if is_new {
                self.modal_message = Some(format!(
                    "Query '{}' saved.\n\nPress a shortcut key or Esc to skip",
                    name
                ));
                self.awaiting_favorite_shortcut = Some((scope, name, query));
            } else {
                self.notify(format!("Query '{}' updated", name));
            }
        } else {
            self.notify("Query applied".to_string());
        }
    }

    // -----------------------------------------------------------------------
    // Task query filter — live apply, close, persistence, load-active
    // -----------------------------------------------------------------------

    pub fn apply_query_filter(&mut self, content: &str) {
        match query_filter::parse(content) {
            Ok(parsed) => {
                let name = if parsed.name.is_empty() { None } else { Some(parsed.name.clone()) };
                self.tasks_view.active_filter = Some(parsed.expr.clone());
                self.tasks_view.active_filter_options = parsed.options.clone();
                self.tasks_view.active_filter_json = Some(content.to_string());
                self.tasks_view.active_filter_name = name.clone();
                // Keep favorites in sync with the latest filter JSON.
                if let Some(ref filter_name) = name {
                    self.update_favorite_json("task", filter_name, content);
                }

                self.tasks_view.state.load_state = LoadState::Loading;
                let service = Arc::clone(&self.task_service);
                let tx = self.load_tx.clone();
                let expr = parsed.expr;
                let options = parsed.options;
                tokio::spawn(async move {
                    let msg = match service.list_filtered_with_options(&expr, &options).await {
                        Ok(tasks) => LoadMsg::Tasks(tasks),
                        Err(e) => LoadMsg::Error(e.to_string()),
                    };
                    let _ = tx.send(msg);
                });
                self.set_query_error(None);
                self.notify(format!(
                    "Filter applied{}",
                    if parsed.name.is_empty() { String::new() } else { format!(": {}", parsed.name) }
                ));
            }
            Err(e) => {
                self.set_query_error(Some(e.to_string()));
            }
        }
    }

    pub(super) async fn process_query_filter_close(&mut self, name: &str, is_new: bool) {
        self.save_active_filter(name).await;
        self.set_query_error(None);
        self.load_saved_queries();
        if is_new && !name.is_empty() {
            let query = self.tasks_view.active_filter_json.clone().unwrap_or_default();
            self.modal_message = Some(format!(
                "Query '{}' saved.\n\nPress a shortcut key or Esc to skip",
                name
            ));
            self.awaiting_favorite_shortcut = Some(("task".to_string(), name.to_string(), query));
        } else {
            self.notify("Filter saved".to_string());
        }
    }

    async fn save_active_filter(&self, name: &str) {
        let Some(json) = &self.tasks_view.active_filter_json else { return };
        let filter_name = if name.is_empty() { "last unnamed filter" } else { name };

        match self.saved_query_repo.upsert("task", filter_name, json, None).await {
            Ok(saved) => {
                let _ = self.settings_repo
                    .set("active_saved_filter_task", &saved.id.to_string())
                    .await;
            }
            Err(e) => {
                eprintln!("Failed to save filter: {e}");
            }
        }
    }

    pub async fn load_active_filter(&mut self) {
        // An explicitly marked default query (★ in the query menu)
        // beats the last-active filter restore.
        if let Some(saved) = self.load_default_query("task").await {
            if let Ok(parsed) = query_filter::parse(&saved.1) {
                self.tasks_view.active_filter = Some(parsed.expr);
                self.tasks_view.active_filter_options = parsed.options;
                self.tasks_view.active_filter_json = Some(saved.1);
                self.tasks_view.active_filter_name = Some(saved.0);
                self.spawn_load();
                return;
            }
        }

        let Some(filter_id_str) = self.settings_repo
            .get("active_saved_filter_task").await.ok().flatten()
        else { return };

        let Ok(filter_id) = filter_id_str.parse::<Uuid>() else { return };

        let Some(saved) = self.saved_query_repo
            .find_by_id(filter_id).await.ok().flatten()
        else { return };

        if let Ok(parsed) = query_filter::parse(&saved.query) {
            self.tasks_view.active_filter = Some(parsed.expr);
            self.tasks_view.active_filter_options = parsed.options;
            self.tasks_view.active_filter_json = Some(saved.query);
            self.tasks_view.active_filter_name = Some(saved.name);
            self.spawn_load();
        }
    }

    /// Resolve the `default_query:{scope}` setting to `(name, query)`.
    /// Self-contained (reads the setting itself) so it works regardless
    /// of whether `load_saved_queries` ran first; a stale name with no
    /// matching saved query yields `None` (callers fall back to the
    /// last-active restore).
    async fn load_default_query(&self, scope: &str) -> Option<(String, String)> {
        let name = self.settings_repo
            .get(&format!("default_query:{scope}")).await.ok().flatten()?;
        let models = self.saved_query_repo.list_by_scope(scope).await.ok()?;
        models.into_iter()
            .find(|m| m.name == name)
            .map(|m| (m.name, m.query))
    }

    // -----------------------------------------------------------------------
    // Tracking query filter — live apply, close, persistence, load-active
    // -----------------------------------------------------------------------

    pub fn apply_tracking_query_filter(&mut self, content: &str) {
        match query_filter::parse(content) {
            Ok(parsed) => {
                let name = if parsed.name.is_empty() { None } else { Some(parsed.name.clone()) };
                self.trackings_view.active_filter = Some(parsed.expr);
                self.trackings_view.active_filter_json = Some(content.to_string());
                self.trackings_view.active_filter_name = name.clone();
                if let Some(ref filter_name) = name {
                    self.update_favorite_json("tracking", filter_name, content);
                }
                self.set_query_error(None);
                self.spawn_load_trackings();
                self.notify(format!(
                    "Tracking filter applied{}",
                    if parsed.name.is_empty() { String::new() } else { format!(": {}", parsed.name) }
                ));
            }
            Err(e) => {
                self.set_query_error(Some(e.to_string()));
            }
        }
    }

    pub(super) async fn process_tracking_query_filter_close(&mut self, name: &str, is_new: bool) {
        self.save_active_tracking_filter(name).await;
        self.set_query_error(None);
        self.load_saved_queries();
        if is_new && !name.is_empty() {
            let query = self.trackings_view.active_filter_json.clone().unwrap_or_default();
            self.modal_message = Some(format!(
                "Query '{}' saved.\n\nPress a shortcut key or Esc to skip",
                name
            ));
            self.awaiting_favorite_shortcut = Some(("tracking".to_string(), name.to_string(), query));
        } else {
            self.notify("Tracking filter saved".to_string());
        }
    }

    async fn save_active_tracking_filter(&self, name: &str) {
        let Some(json) = &self.trackings_view.active_filter_json else { return };
        let filter_name = if name.is_empty() { "last unnamed filter" } else { name };
        match self.saved_query_repo.upsert("tracking", filter_name, json, None).await {
            Ok(saved) => {
                let _ = self.settings_repo
                    .set("active_saved_filter_tracking", &saved.id.to_string())
                    .await;
            }
            Err(e) => {
                eprintln!("Failed to save tracking filter: {e}");
            }
        }
    }

    pub async fn load_active_tracking_filter(&mut self) {
        // See `load_active_filter`: an explicit default query wins.
        if let Some(saved) = self.load_default_query("tracking").await {
            if let Ok(parsed) = query_filter::parse(&saved.1) {
                self.trackings_view.active_filter = Some(parsed.expr);
                self.trackings_view.active_filter_json = Some(saved.1);
                self.trackings_view.active_filter_name = Some(saved.0);
                self.spawn_load_trackings();
                return;
            }
        }

        let Some(filter_id_str) = self.settings_repo
            .get("active_saved_filter_tracking").await.ok().flatten()
        else { return };

        let Ok(filter_id) = filter_id_str.parse::<Uuid>() else { return };

        let Some(saved) = self.saved_query_repo
            .find_by_id(filter_id).await.ok().flatten()
        else { return };

        if let Ok(parsed) = query_filter::parse(&saved.query) {
            self.trackings_view.active_filter = Some(parsed.expr);
            self.trackings_view.active_filter_json = Some(saved.query);
            self.trackings_view.active_filter_name = Some(saved.name);
            self.spawn_load_trackings();
        }
    }
}
