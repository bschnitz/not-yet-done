//! Filter / saved-query persistence and live application.
//!
//! Helpers for the three query-filter scopes (Tasks, Trackings, Content):
//! parsing the YAML buffer, applying it to the active view, persisting to
//! the adapter's `SavedQueryStore`, and prompting for the favorite-shortcut
//! on new entries.
//!
//! Sits next to [`super::editor`] which dispatches the relevant
//! [`crate::edit_session::FollowUp`] variants here from
//! `handle_follow_up`.

use not_yet_done_content::QueryKind;

use super::App;

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
        kind: QueryKind,
    ) {
        let query = content.trim().to_string();
        if query.is_empty() {
            return;
        }
        let name = save_name.map(|s| s.to_string());
        let pane_id = if let Some(cv) = self.content_view_mut(view_index) {
            // No bindings: a live buffer is applied on every `:w`, and
            // stopping to prompt for variables mid-edit would take the
            // editor's place. Whatever the document declares stays
            // unrendered until it is applied from the menu.
            cv.set_query_of_kind(query, name, kind);
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
        kind: QueryKind,
    ) {
        let query = content.trim().to_string();
        if query.is_empty() {
            self.notify("Cancelled (empty query)".to_string());
            return;
        }
        let name = save_name.map(|s| s.to_string());
        let pane_id = if let Some(cv) = self.content_view_mut(view_index) {
            cv.set_query_of_kind(query.clone(), name.clone(), kind);
            cv.active_pane_id()
        } else {
            return;
        };
        self.spawn_content_load(view_index, pane_id);

        if let Some(name) = name {
            let scope = self
                .content_view(view_index)
                .map(|cv| cv.query_scope.clone())
                .unwrap_or_default();
            // Persist to the adapter's filesystem store for this kind — the
            // same two stores `reload_content_saved_queries` reads from and
            // the `:query` save path writes to. They are the *only* body
            // stores; the shortcut overlay in `query_shortcut` holds no
            // query text.
            self.save_content_query_body(view_index, &name, &query, kind);
            self.reload_content_saved_queries(view_index);
            if is_new {
                self.modal_message = Some(format!(
                    "Query '{}' saved.\n\nPress a shortcut key or Esc to skip",
                    name
                ));
                self.awaiting_favorite_shortcut = Some(super::PendingFavorite {
                    scope,
                    name,
                    query,
                    kind,
                });
            } else {
                self.notify(format!("Query '{}' updated", name));
            }
        } else {
            self.notify("Query applied".to_string());
        }
    }
}
