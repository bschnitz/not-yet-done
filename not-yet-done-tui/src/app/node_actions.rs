//! Per-node-action shortcut dispatch (Phase CP-1c).
//!
//! The adapter declares actions per node-type via [`Node::actions`];
//! the YAML view-config binds keys to action names via `shortcuts:` on
//! [`ViewDef`] / [`ChildDef`]. This module is the TUI-side glue:
//!
//! * [`resolve_shortcut`] walks the YAML tree using the selected
//!   entry's `node_type_chain` (MT-1 chain-aware semantics) and finds
//!   the action name bound to a single-char key, if any.
//! * [`dispatch_to_view_request`] translates an [`ActionDispatch`]
//!   returned by [`Node::invoke_action`] into the [`ViewRequest`] the
//!   App should handle next.
//!
//! The dispatcher is plumbed through [`ContentPane::handle_key`] →
//! [`ViewRequest::InvokeNodeAction`] → an async task on App that calls
//! `adapter.get_by_id(node_id).await?.invoke_action(name, ctx).await`
//! → [`LoadMsg::NodeActionDispatched`] → [`dispatch_to_view_request`]
//! → next ViewRequest.
//!
//! Phase CP-1c lays the rails; CP-1d adds the first migration
//! (`TableNode::edit_sql`). The path stays opt-in via YAML — adapters
//! that don't override `actions()` keep the legacy `LevelAction`
//! behaviour.
//!
//! **YAML shortcut value form (CP-1d):**
//! * `q: edit_sql` — invoke `edit_sql` on the **selected** node.
//! * `q: parent:edit_sql` — invoke `edit_sql` on the **immediate
//!   parent** of the selected node. Use this when the shortcut sits at
//!   a row-list level but the action lives on the parent (e.g. open the
//!   SQL editor for the table whose rows are being viewed).

use not_yet_done_content::ActionDispatch;

use crate::config::view_config::{ChildDef, ViewDef};
use crate::views::content_tree::child_def_for_type_chain;
use crate::views::{ViewRequest, content_view::PaneId};

/// Which node a YAML shortcut targets when its action fires. Decoded
/// from the action-name prefix: bare action names default to
/// [`Self::Selected`]; values prefixed with `parent:` are
/// [`Self::Parent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutTarget {
    /// The node under the cursor — usual case.
    Selected,
    /// The immediate parent of the selected node. In flat mode this is
    /// `ContentPane::parent_node_id`; in tree mode it's the last entry
    /// of the selected row's `parent_path`. Fires a notification when
    /// the user is at root level (no parent).
    Parent,
}

/// Parse a shortcut value. `"parent:foo"` → ([`ShortcutTarget::Parent`],
/// `"foo"`); anything else → ([`ShortcutTarget::Selected`], full input).
/// Empty bodies after the prefix are returned as-is — the validator
/// rejects them.
pub fn parse_shortcut_value(raw: &str) -> (ShortcutTarget, &str) {
    match raw.strip_prefix("parent:") {
        Some(rest) => (ShortcutTarget::Parent, rest),
        None => (ShortcutTarget::Selected, raw),
    }
}

/// Outcome of [`resolve_shortcut`]: the adapter-side action name plus
/// the target node ([`ShortcutTarget`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedShortcut<'a> {
    pub action_name: &'a str,
    pub target: ShortcutTarget,
}

/// Parse a Postgres-adapter DB-script node id of the shape
/// `<database>/db_scripts/<script>` into `(database, script)`. Returns
/// `None` for any other shape so the dispatcher can fall through.
///
/// Kept separate so CP-9's `DeleteSelf` arm has a single place to keep
/// the adapter-id format coupling, and so future migrations can switch
/// to a typed channel without touching the dispatcher.
/// Build the `NodeRef`-style scope string for a Postgres table-level
/// shortcut. Mirrors the path the adapter's internal node ids would
/// produce when joined with the app-wide `<adapter>/<instance>` prefix:
/// `postgres/<instance>/<db>/schemas/<schema>/tables/<table>`. This is
/// the form stored in `query_shortcut.scope` for table-scoped Postgres
/// script bindings (SQ-8).
pub(crate) fn postgres_table_scope(
    instance_id: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> String {
    format!("postgres/{instance_id}/{database}/schemas/{schema}/tables/{table}")
}

/// Parse a Postgres DB-script / DB-script-dir node id of the form
/// `<db>/db_scripts/<seg₁>/…/<segₙ>` (N ≥ 1) into its database and
/// rel-path segments. DSF-4: the segment count is unbounded — the
/// adapter's filesystem layer disambiguates dir-vs-script per segment.
/// Both the leaf node (script or dir) and the storage layer expect a
/// rel-path joined with `/`, so callers typically reassemble the
/// segments with [`db_script_rel_path_str`].
pub(crate) fn parse_db_script_node_id(node_id: &str) -> Option<(String, Vec<String>)> {
    let mut parts = node_id.split('/');
    let database = parts.next()?;
    if parts.next()? != "db_scripts" {
        return None;
    }
    let segments: Vec<String> = parts
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if database.is_empty() || segments.is_empty() {
        return None;
    }
    Some((database.to_string(), segments))
}

/// Join rel-path segments with `/`. Empty segments yield an empty
/// string (root). Mirrors the on-disk separator the adapter's storage
/// layer expects in `rel_path` arguments.
pub(crate) fn db_script_rel_path_str(segments: &[String]) -> String {
    segments.join("/")
}

/// DSF-4: Split a `CreateChild` hint body of the form `<db>[:<parent_rel>]`
/// into `(db, parent_rel)`. The hint comes from the adapter's
/// `invoke_action`; the dir/script discriminator is the hint's
/// *prefix* (already stripped by the caller). Empty `parent_rel`
/// means root.
fn split_create_hint(rest: &str) -> (&str, &str) {
    match rest.split_once(':') {
        Some((db, parent_rel)) => (db, parent_rel),
        None => (rest, ""),
    }
}

/// DSF-4: TUI-owned action names → ViewRequest. These actions
/// (`rename`, `mark-move`, `paste-move`) are exposed by the Postgres
/// adapter purely so the shortcut/hint chain works; the adapter
/// returns `Noop` for them and the App does the actual work. Returns
/// `Some(req)` only for db_script* nodes — other adapters with the
/// same action name (e.g. a future Jira rename) are left alone.
fn tui_owned_db_script_action(
    action_name: &str,
    view_index: usize,
    pane_id: PaneId,
    node_id: &str,
) -> Option<ViewRequest> {
    let (database, segments) = parse_db_script_node_id(node_id)?;
    let rel_path = db_script_rel_path_str(&segments);
    match action_name {
        "rename" => {
            // Dir-vs-script disambiguation can't be done from the id
            // alone (filesystem probe lives in the adapter). The App
            // resolves it before opening the prompt; default to
            // `is_dir: false` here and let the App re-probe.
            Some(ViewRequest::OpenDbScriptRenamePrompt {
                view_index,
                pane_id,
                database,
                rel_path,
                is_dir: false,
            })
        }
        "mark-move" => Some(ViewRequest::MarkDbScriptForMove {
            node_id: node_id.to_string(),
        }),
        "paste-move" => Some(ViewRequest::PasteDbScriptMove {
            target_node_id: node_id.to_string(),
        }),
        _ => None,
    }
}

/// M7/E6: what the generic mark/paste-move vocabulary should do for an
/// action firing on a content node. Returned by
/// [`generic_mark_move_effect`] so the App can mutate its clipboard state
/// while the pure decision stays unit-testable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkMoveEffect {
    /// `mark-move`: record the invoking node as the move source.
    Mark,
    /// `paste-move`: the adapter performed the move; clear the mark once
    /// the dispatch confirms success ([`ActionDispatch::Reload`]).
    ClearOnPasteSuccess,
    /// Not a generic mark/paste-move action (or a node that owns a
    /// bespoke move path) — leave the generic clipboard untouched.
    Ignore,
}

/// Decide how a content-node action interacts with the generic move
/// clipboard ([`crate::app::App::content_marked_node`]).
///
/// DB-script nodes keep their own bespoke mark/paste path (DSF-4, routed
/// through [`tui_owned_db_script_action`]); they are deliberately excluded
/// here so the two clipboards stay disjoint until the consolidation
/// follow-up migrates db-script onto this generic mechanism. Every other
/// adapter (TaskAdapter from A1 onward) drives the generic clipboard.
pub fn generic_mark_move_effect(action_name: &str, node_id: &str) -> MarkMoveEffect {
    if parse_db_script_node_id(node_id).is_some() {
        return MarkMoveEffect::Ignore;
    }
    match action_name {
        "mark-move" => MarkMoveEffect::Mark,
        "paste-move" => MarkMoveEffect::ClearOnPasteSuccess,
        _ => MarkMoveEffect::Ignore,
    }
}

/// Resolve `key` to an adapter action name using the YAML `shortcuts:`
/// maps along `type_chain`. Most-specific wins:
///
/// 1. The [`ChildDef`] whose chain prefix matches `type_chain` (the
///    deepest reachable level).
/// 2. Each ancestor `ChildDef` walking up to the root.
/// 3. The [`ViewDef`]'s view-level `shortcuts:` map.
///
/// An empty `type_chain` only consults the view-level map; chains that
/// don't match any ChildDef path fall through to the view-level map.
///
/// Returns a borrowed reference to the action-name string (the adapter
/// looks it up in [`Node::actions`] / [`Node::invoke_action`]).
pub fn resolve_shortcut<'a>(
    view_def: &'a ViewDef,
    type_chain: &[String],
    key: char,
) -> Option<ResolvedShortcut<'a>> {
    let raw = lookup_shortcut_raw(view_def, type_chain, key)?;
    let (target, action_name) = parse_shortcut_value(raw);
    Some(ResolvedShortcut {
        action_name,
        target,
    })
}

/// Walk a [`ViewDef`]'s ChildDef tree and report whether any ChildDef
/// matching one of the node-type segments encoded in `node_id` carries
/// `editor_in_place: true`.
///
/// `node_id` is the adapter's row id, expected to be slash-separated.
/// We extract the *terminal* type for matching by checking each ChildDef
/// against the last path segment as a fallback for adapters whose id
/// shape doesn't otherwise reveal node-type. Since EIP currently only
/// targets `postgres:db_script` and that ChildDef sits below
/// `db_script_dir` which is recursive, this is good enough: a single
/// flag at any matching level applies.
///
/// Falls back to `false` if no ChildDef has the flag set.
pub fn editor_in_place_for_node_id(view_def: &ViewDef, _node_id: &str) -> bool {
    fn walk(children: &[ChildDef]) -> bool {
        for c in children {
            if c.editor_in_place {
                return true;
            }
            if walk(&c.children) {
                return true;
            }
        }
        false
    }
    walk(&view_def.children)
}

fn lookup_shortcut_raw<'a>(
    view_def: &'a ViewDef,
    type_chain: &[String],
    key: char,
) -> Option<&'a str> {
    for end in (1..=type_chain.len()).rev() {
        if let Some(child) = child_def_for_type_chain(view_def, &type_chain[..end]) {
            if let Some(sc) = child.shortcuts.get(&key) {
                return Some(sc.action());
            }
        }
    }
    view_def.shortcuts.get(&key).map(|sc| sc.action())
}

/// Translate an [`ActionDispatch`] into the [`ViewRequest`] the App
/// should fire next. `view_index` / `pane_id` are the originating
/// pane's coordinates; `node_id` is the node the action was invoked
/// on (already known by the App when this is called).
///
/// `editor_in_place` mirrors the ChildDef flag of the row the action
/// fired on. Only [`ActionDispatch::OpenEditor`] honors it (forwards
/// it to [`ViewRequest::OpenAdapterDbScriptEditor`]); other dispatch
/// kinds ignore it.
///
/// Returns `None` when the dispatch has no observable follow-up
/// ([`ActionDispatch::Noop`]). [`ActionDispatch::Error`] surfaces as
/// [`ViewRequest::Notify`].
pub fn dispatch_to_view_request(
    dispatch: ActionDispatch,
    view_index: usize,
    pane_id: PaneId,
    node_id: String,
    action_name: String,
    editor_in_place: bool,
) -> Option<ViewRequest> {
    // DSF-4: certain action names are TUI-owned — the adapter exposes
    // them so the keybinding/hint chain works, but the actual work
    // (mark/paste state, rename prompt) happens in the App. The
    // adapter returns `Noop`, which would short-circuit below; we
    // intercept by name first so the right ViewRequest still fires.
    if let Some(req) =
        tui_owned_db_script_action(action_name.as_str(), view_index, pane_id, &node_id)
    {
        return Some(req);
    }
    match dispatch {
        ActionDispatch::OpenEditor {
            session_kind,
            params,
        } => match session_kind.as_str() {
            // Generic query editor — edits and (re)runs a query against a
            // backend. Reuses the `OpenAdapterQueryEditor` request; the
            // adapter has already validated `node_id` addresses the right
            // node (for Postgres: a TableNode path). The session_kind is
            // role-generic, not adapter-named.
            "query_editor" => Some(ViewRequest::OpenAdapterQueryEditor {
                view_index,
                pane_id,
                parent_node_id: node_id,
            }),
            // Generic named-script editor (CP-8): edits a persisted script
            // the user re-executes separately. The adapter packs
            // `(database, script)` into `params`; we re-derive `database`
            // from the `node_id` first segment as a fallback so the
            // dispatch survives an adapter that omits the params map.
            "script_editor" => {
                let database = params
                    .get("database")
                    .cloned()
                    .or_else(|| node_id.split('/').next().map(str::to_string))
                    .unwrap_or_default();
                let script = params
                    .get("script")
                    .cloned()
                    .or_else(|| node_id.rsplit('/').next().map(str::to_string))
                    .unwrap_or_default();
                Some(ViewRequest::OpenAdapterDbScriptEditor {
                    view_index,
                    pane_id,
                    database,
                    script,
                    in_place: editor_in_place,
                })
            }
            other => Some(ViewRequest::Notify(format!(
                "node-action '{action_name}': unknown session_kind '{other}'"
            ))),
        },
        ActionDispatch::ExecuteQuery {
            database,
            sql,
            paged,
        } => {
            // Non-paged execute would still be useful (DDL fire-and-forget)
            // but CP-8 only wires the paged path. Treat `paged: false`
            // as not-yet-implemented so the user gets a clear signal.
            if !paged {
                return Some(ViewRequest::Notify(format!(
                    "node-action '{action_name}' → unpaged ExecuteQuery not implemented yet"
                )));
            }
            // Derive a label from the last id segment so the result
            // pane's NavFrame shows the script name in the breadcrumb.
            let source_label = node_id
                .rsplit('/')
                .next()
                .unwrap_or(node_id.as_str())
                .to_string();
            Some(ViewRequest::RunAdapterDbScript {
                view_index,
                pane_id,
                source_node_id: node_id,
                source_label,
                database,
                sql,
            })
        }
        // CP-9 / DSF-4: `add` on the DB Scripts group or a Dir emits
        // `CreateChild { hint: "db_script:<db>[:<parent_rel>]" }`
        // (script) or `db_script_dir:<db>[:<parent_rel>]` (dir). The
        // `:<parent_rel>` suffix encodes the dir under which the new
        // entry lives (empty for root). Other hints fall through to a
        // Notify.
        ActionDispatch::CreateChild { hint } => {
            if let Some(rest) = hint.strip_prefix("db_script_dir:") {
                let (db, parent_rel) = split_create_hint(rest);
                if db.is_empty() {
                    Some(ViewRequest::Notify(format!(
                        "node-action '{action_name}' → CreateChild hint '{hint}' missing database"
                    )))
                } else {
                    Some(ViewRequest::OpenDbScriptDirNewPrompt {
                        view_index,
                        pane_id,
                        database: db.to_string(),
                        parent_rel: parent_rel.to_string(),
                    })
                }
            } else if let Some(rest) = hint.strip_prefix("db_script:") {
                let (db, parent_rel) = split_create_hint(rest);
                if db.is_empty() {
                    Some(ViewRequest::Notify(format!(
                        "node-action '{action_name}' → CreateChild hint '{hint}' missing database"
                    )))
                } else {
                    Some(ViewRequest::OpenDbScriptNewPrompt {
                        view_index,
                        pane_id,
                        database: db.to_string(),
                        parent_rel: parent_rel.to_string(),
                    })
                }
            } else {
                Some(ViewRequest::Notify(format!(
                    "node-action '{action_name}' → CreateChild hint '{hint}' not handled"
                )))
            }
        }
        // CP-9 / DSF-4: `delete` (script) or `delete-dir` (dir) on a
        // DB-script node. The Postgres adapter returns DeleteSelf for
        // both; we branch on `action_name` to pick the right confirm
        // flow. The script-confirm now carries the full rel-path so
        // nested scripts under directories work uniformly.
        //
        // CF-11: any other node-id shape falls through to the generic
        // `ConfirmDeleteContentNode` path, which after confirm calls
        // `Node::execute("delete", ActionInput::None)` on the adapter —
        // no adapter-specific App handler is needed. Confluence pages
        // use this; future Jira/Taiga deletes can opt in by returning
        // `DeleteSelf` from `invoke_action` without TUI changes.
        ActionDispatch::DeleteSelf { confirm } => match parse_db_script_node_id(&node_id) {
            Some((database, segments)) => {
                let rel_path = db_script_rel_path_str(&segments);
                if action_name == "delete-dir" {
                    Some(ViewRequest::ConfirmDeleteAdapterDbScriptDir {
                        view_index,
                        pane_id,
                        database,
                        rel_path,
                    })
                } else {
                    Some(ViewRequest::ConfirmDeleteAdapterDbScript {
                        view_index,
                        pane_id,
                        database,
                        script: rel_path,
                    })
                }
            }
            None => Some(ViewRequest::ConfirmDeleteContentNode {
                view_index,
                pane_id,
                node_id,
                action_name,
                confirm,
            }),
        },
        ActionDispatch::Reload => Some(ViewRequest::SpawnContentLoad {
            view_index,
            pane_id,
        }),
        // Generic confirm: the adapter wants a `(y/n)` prompt before doing
        // the work. On "y" the App re-invokes the *same* action on the
        // *same* node with `confirmed: true` (see
        // `PendingConfirmation::InvokeNodeAction`), so the adapter then
        // performs the work instead of asking again.
        ActionDispatch::Confirm { prompt } => Some(ViewRequest::ConfirmInvokeNodeAction {
            view_index,
            pane_id,
            node_id,
            action_name,
            prompt,
        }),
        ActionDispatch::Noop => None,
        // Success-with-a-message (e.g. `backup` reporting its file path): no
        // data changed, so surface the text in the status bar without a reload.
        ActionDispatch::Notify { message } => Some(ViewRequest::Notify(message)),
        ActionDispatch::Error(msg) => Some(ViewRequest::Notify(msg)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::ActionChains;
    use crate::config::view_config::{ChildDef, ShortcutDef, ViewDef};
    use std::collections::HashMap;

    fn view(node_type: &str, shortcuts: &[(char, &str)]) -> ViewDef {
        let mut sc = HashMap::new();
        for (k, name) in shortcuts {
            sc.insert(*k, ShortcutDef::Action((*name).to_string()));
        }
        ViewDef {
            row_layout: None,
            smooth_scroll: false,
            name: "v".into(),
            node_type: node_type.into(),
            default: false,
            window_ops: false,
            key: None,
            query: None,
            columns: Vec::new(),
            preview: None,
            actions: Vec::new(),
            children: Vec::new(),
            pagination: None,
            action_chains: ActionChains::default(),
            column_cursor: false,
            record_detail: false,
            tree_label: None,
            retries: 0,
            script_template: None,
            shortcuts: sc,
            leaf_glyph: None,
            group_by: None,
            aggregates: Vec::new(),
            tree_connector_style: None,
            unread_style: None,
            unread_marker: None,
            tree_lines: None,
            tree_markers: None,
            expand_depth: None,
            group_headers: None,
        }
    }

    fn child(node_type: &str, shortcuts: &[(char, &str)]) -> ChildDef {
        let mut sc = HashMap::new();
        for (k, name) in shortcuts {
            sc.insert(*k, ShortcutDef::Action((*name).to_string()));
        }
        ChildDef {
            row_layout: None,
            smooth_scroll: false,
            name: node_type.into(),
            node_type: node_type.into(),
            columns: Vec::new(),
            preview: None,
            actions: Vec::new(),
            children: Vec::new(),
            split: None,
            pagination: None,
            keybindings: HashMap::new(),
            action_chains: ActionChains::default(),
            column_cursor: false,
            record_detail: false,
            tree_label: None,
            shortcuts: sc,
            enter_action: None,
            recursive: false,
            editor_in_place: false,
            leaf_glyph: None,
            group_by: None,
            aggregates: Vec::new(),
            mark_read_on_reach_end: None,
        }
    }

    fn selected(name: &str) -> ResolvedShortcut<'_> {
        ResolvedShortcut {
            action_name: name,
            target: ShortcutTarget::Selected,
        }
    }

    fn parent(name: &str) -> ResolvedShortcut<'_> {
        ResolvedShortcut {
            action_name: name,
            target: ShortcutTarget::Parent,
        }
    }

    #[test]
    fn resolves_view_level_shortcut_for_empty_chain() {
        let vd = view("mock:root", &[('x', "execute")]);
        assert_eq!(resolve_shortcut(&vd, &[], 'x'), Some(selected("execute")));
    }

    #[test]
    fn resolves_child_level_shortcut_when_chain_matches() {
        let mut vd = view("mock:root", &[]);
        vd.children.push(child("mock:row", &[('e', "edit")]));
        let chain = vec!["mock:root".to_string(), "mock:row".to_string()];
        assert_eq!(resolve_shortcut(&vd, &chain, 'e'), Some(selected("edit")));
    }

    #[test]
    fn child_level_shadows_view_level_for_same_key() {
        let mut vd = view("mock:root", &[('x', "view-level")]);
        vd.children.push(child("mock:row", &[('x', "child-level")]));
        let chain = vec!["mock:root".to_string(), "mock:row".to_string()];
        assert_eq!(
            resolve_shortcut(&vd, &chain, 'x'),
            Some(selected("child-level"))
        );
    }

    #[test]
    fn falls_back_to_view_level_when_child_lacks_key() {
        let mut vd = view("mock:root", &[('q', "query")]);
        vd.children.push(child("mock:row", &[('e', "edit")]));
        let chain = vec!["mock:root".to_string(), "mock:row".to_string()];
        assert_eq!(resolve_shortcut(&vd, &chain, 'q'), Some(selected("query")));
    }

    #[test]
    fn walks_up_chain_to_intermediate_ancestor() {
        // root → mid (has 'm') → leaf (no shortcuts)
        let mut vd = view("mock:root", &[('r', "root-action")]);
        let mut mid = child("mock:mid", &[('m', "mid-action")]);
        mid.children.push(child("mock:leaf", &[]));
        vd.children.push(mid);

        let chain = vec![
            "mock:root".to_string(),
            "mock:mid".to_string(),
            "mock:leaf".to_string(),
        ];
        // Leaf has no shortcut; mid's 'm' wins (closer than root).
        assert_eq!(
            resolve_shortcut(&vd, &chain, 'm'),
            Some(selected("mid-action"))
        );
        // Root still reachable for 'r' (no closer ancestor binds it).
        assert_eq!(
            resolve_shortcut(&vd, &chain, 'r'),
            Some(selected("root-action"))
        );
    }

    #[test]
    fn returns_none_when_key_unbound() {
        let vd = view("mock:root", &[('x', "execute")]);
        assert_eq!(resolve_shortcut(&vd, &[], 'q'), None);
    }

    #[test]
    fn unknown_chain_falls_through_to_view_level() {
        let mut vd = view("mock:root", &[('x', "execute")]);
        vd.children.push(child("mock:row", &[('e', "edit")]));
        // Chain references a type that doesn't exist under root.
        let chain = vec!["mock:root".to_string(), "mock:nope".to_string()];
        // 'x' from view-level still resolves.
        assert_eq!(
            resolve_shortcut(&vd, &chain, 'x'),
            Some(selected("execute"))
        );
        // Child-only key doesn't.
        assert_eq!(resolve_shortcut(&vd, &chain, 'e'), None);
    }

    #[test]
    fn parent_prefix_yields_parent_target() {
        let mut vd = view("mock:root", &[]);
        vd.children
            .push(child("mock:row", &[('q', "parent:edit_sql")]));
        let chain = vec!["mock:root".to_string(), "mock:row".to_string()];
        assert_eq!(resolve_shortcut(&vd, &chain, 'q'), Some(parent("edit_sql")));
    }

    #[test]
    fn parent_prefix_on_view_level_works_too() {
        let vd = view("mock:root", &[('q', "parent:edit_sql")]);
        assert_eq!(resolve_shortcut(&vd, &[], 'q'), Some(parent("edit_sql")));
    }

    #[test]
    fn parse_shortcut_value_strips_only_parent_prefix() {
        assert_eq!(
            parse_shortcut_value("foo"),
            (ShortcutTarget::Selected, "foo")
        );
        assert_eq!(
            parse_shortcut_value("parent:foo"),
            (ShortcutTarget::Parent, "foo")
        );
        // Only the leading `parent:` is stripped — colons elsewhere stay.
        assert_eq!(
            parse_shortcut_value("foo:bar"),
            (ShortcutTarget::Selected, "foo:bar")
        );
        // Other prefixes are not recognised.
        assert_eq!(
            parse_shortcut_value("self:foo"),
            (ShortcutTarget::Selected, "self:foo")
        );
    }

    #[test]
    fn dispatch_noop_yields_no_request() {
        let req = dispatch_to_view_request(
            ActionDispatch::Noop,
            0,
            1,
            "node-1".into(),
            "execute".into(),
            false,
        );
        assert!(req.is_none());
    }

    #[test]
    fn dispatch_reload_yields_spawn_content_load() {
        let req = dispatch_to_view_request(
            ActionDispatch::Reload,
            7,
            3,
            "node-1".into(),
            "reload".into(),
            false,
        );
        match req {
            Some(ViewRequest::SpawnContentLoad {
                view_index,
                pane_id,
            }) => {
                assert_eq!(view_index, 7);
                assert_eq!(pane_id, 3);
            }
            other => panic!("expected SpawnContentLoad, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_execute_query_paged_yields_run_adapter_db_script() {
        // CP-8: ExecuteQuery{paged:true} maps to RunAdapterDbScript with
        // the node_id's last segment as `source_label`.
        let req = dispatch_to_view_request(
            ActionDispatch::ExecuteQuery {
                database: "live".into(),
                sql: "SELECT 1".into(),
                paged: true,
            },
            3,
            7,
            "live/db_scripts/report".into(),
            "execute".into(),
            false,
        );
        match req {
            Some(ViewRequest::RunAdapterDbScript {
                view_index,
                pane_id,
                source_node_id,
                source_label,
                database,
                sql,
            }) => {
                assert_eq!(view_index, 3);
                assert_eq!(pane_id, 7);
                assert_eq!(source_node_id, "live/db_scripts/report");
                assert_eq!(source_label, "report");
                assert_eq!(database, "live");
                assert_eq!(sql, "SELECT 1");
            }
            other => panic!("expected RunAdapterDbScript, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_execute_query_unpaged_yields_notify() {
        let req = dispatch_to_view_request(
            ActionDispatch::ExecuteQuery {
                database: "live".into(),
                sql: "VACUUM".into(),
                paged: false,
            },
            0,
            1,
            "live/db_scripts/foo".into(),
            "execute".into(),
            false,
        );
        match req {
            Some(ViewRequest::Notify(msg)) => assert!(msg.contains("unpaged")),
            other => panic!("expected Notify for unpaged ExecuteQuery, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_open_editor_postgres_db_script_uses_params() {
        let mut params = HashMap::new();
        params.insert("database".to_string(), "live".to_string());
        params.insert("script".to_string(), "report".to_string());
        let req = dispatch_to_view_request(
            ActionDispatch::OpenEditor {
                session_kind: "script_editor".into(),
                params,
            },
            2,
            4,
            "live/db_scripts/report".into(),
            "edit".into(),
            false,
        );
        match req {
            Some(ViewRequest::OpenAdapterDbScriptEditor {
                view_index,
                pane_id,
                database,
                script,
                in_place,
            }) => {
                assert_eq!(view_index, 2);
                assert_eq!(pane_id, 4);
                assert_eq!(database, "live");
                assert_eq!(script, "report");
                assert!(!in_place);
            }
            other => panic!("expected OpenAdapterDbScriptEditor, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_open_editor_postgres_db_script_falls_back_to_node_id() {
        // Adapter omits the params map — dispatcher recovers
        // (database, script) from the node_id segments.
        let req = dispatch_to_view_request(
            ActionDispatch::OpenEditor {
                session_kind: "script_editor".into(),
                params: HashMap::new(),
            },
            0,
            1,
            "live/db_scripts/report".into(),
            "edit".into(),
            false,
        );
        match req {
            Some(ViewRequest::OpenAdapterDbScriptEditor {
                database, script, ..
            }) => {
                assert_eq!(database, "live");
                assert_eq!(script, "report");
            }
            other => panic!("expected OpenAdapterDbScriptEditor, got {other:?}"),
        }
    }

    #[test]
    fn parse_db_script_node_id_accepts_three_segments() {
        assert_eq!(
            parse_db_script_node_id("live/db_scripts/report"),
            Some(("live".to_string(), vec!["report".to_string()]))
        );
    }

    /// DSF-4: N-segment ids (nested under directories) are valid.
    #[test]
    fn parse_db_script_node_id_accepts_nested_segments() {
        assert_eq!(
            parse_db_script_node_id("live/db_scripts/maint/vacuum/full"),
            Some((
                "live".to_string(),
                vec![
                    "maint".to_string(),
                    "vacuum".to_string(),
                    "full".to_string()
                ],
            ))
        );
    }

    #[test]
    fn parse_db_script_node_id_rejects_wrong_marker() {
        assert_eq!(parse_db_script_node_id("live/schemas/public"), None);
    }

    #[test]
    fn parse_db_script_node_id_rejects_short_id() {
        assert_eq!(parse_db_script_node_id("live/db_scripts"), None);
        assert_eq!(parse_db_script_node_id("live"), None);
    }

    #[test]
    fn db_script_rel_path_str_joins_with_slash() {
        let segs = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(db_script_rel_path_str(&segs), "a/b/c");
        assert_eq!(db_script_rel_path_str(&[]), "");
    }

    #[test]
    fn dispatch_create_child_db_script_yields_prompt() {
        // CP-9: `add` on the group node — dispatcher decodes the
        // `db_script:<db>` hint and routes to the cmdline prompt.
        let req = dispatch_to_view_request(
            ActionDispatch::CreateChild {
                hint: "db_script:live".into(),
            },
            5,
            9,
            "live/db_scripts".into(),
            "add".into(),
            false,
        );
        match req {
            Some(ViewRequest::OpenDbScriptNewPrompt {
                view_index,
                pane_id,
                database,
                parent_rel,
            }) => {
                assert_eq!(view_index, 5);
                assert_eq!(pane_id, 9);
                assert_eq!(database, "live");
                assert_eq!(parent_rel, "");
            }
            other => panic!("expected OpenDbScriptNewPrompt, got {other:?}"),
        }
    }

    /// DSF-4: `add-script` from a dir node passes the parent rel-path
    /// via the hint suffix — verifying the split/parse round-trip.
    #[test]
    fn dispatch_create_child_db_script_with_parent_rel_threads_through() {
        let req = dispatch_to_view_request(
            ActionDispatch::CreateChild {
                hint: "db_script:live:maint/vacuum".into(),
            },
            1,
            2,
            "live/db_scripts/maint/vacuum".into(),
            "add-script".into(),
            false,
        );
        match req {
            Some(ViewRequest::OpenDbScriptNewPrompt {
                database,
                parent_rel,
                ..
            }) => {
                assert_eq!(database, "live");
                assert_eq!(parent_rel, "maint/vacuum");
            }
            other => panic!("expected OpenDbScriptNewPrompt, got {other:?}"),
        }
    }

    /// DSF-4: `add-dir` hint routes to the dir-new prompt.
    #[test]
    fn dispatch_create_child_db_script_dir_yields_dir_prompt() {
        let req = dispatch_to_view_request(
            ActionDispatch::CreateChild {
                hint: "db_script_dir:live:maint".into(),
            },
            1,
            2,
            "live/db_scripts/maint".into(),
            "add-dir".into(),
            false,
        );
        match req {
            Some(ViewRequest::OpenDbScriptDirNewPrompt {
                database,
                parent_rel,
                ..
            }) => {
                assert_eq!(database, "live");
                assert_eq!(parent_rel, "maint");
            }
            other => panic!("expected OpenDbScriptDirNewPrompt, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_create_child_unknown_hint_yields_notify() {
        let req = dispatch_to_view_request(
            ActionDispatch::CreateChild {
                hint: "table:rows".into(),
            },
            0,
            1,
            "n".into(),
            "add".into(),
            false,
        );
        match req {
            Some(ViewRequest::Notify(msg)) => {
                assert!(msg.contains("table:rows"), "got: {msg}");
            }
            other => panic!("expected Notify, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_delete_self_db_script_yields_confirm() {
        // CP-9: `delete` on a db_script node — dispatcher decodes
        // (database, script) from the node-id shape and routes to the
        // confirm-then-unlink flow.
        let req = dispatch_to_view_request(
            ActionDispatch::DeleteSelf { confirm: None },
            4,
            8,
            "live/db_scripts/report".into(),
            "delete".into(),
            false,
        );
        match req {
            Some(ViewRequest::ConfirmDeleteAdapterDbScript {
                view_index,
                pane_id,
                database,
                script,
            }) => {
                assert_eq!(view_index, 4);
                assert_eq!(pane_id, 8);
                assert_eq!(database, "live");
                assert_eq!(script, "report");
            }
            other => panic!("expected ConfirmDeleteAdapterDbScript, got {other:?}"),
        }
    }

    /// DSF-4: nested script under directories — rel-path is preserved
    /// in `script` so the App's unlink can find the right file.
    #[test]
    fn dispatch_delete_self_db_script_preserves_nested_rel_path() {
        let req = dispatch_to_view_request(
            ActionDispatch::DeleteSelf { confirm: None },
            0,
            0,
            "live/db_scripts/maint/vacuum/full".into(),
            "delete".into(),
            false,
        );
        match req {
            Some(ViewRequest::ConfirmDeleteAdapterDbScript { script, .. }) => {
                assert_eq!(script, "maint/vacuum/full");
            }
            other => panic!("expected ConfirmDeleteAdapterDbScript, got {other:?}"),
        }
    }

    /// DSF-4: `delete-dir` action-name routes to the dir-confirm flow.
    #[test]
    fn dispatch_delete_self_db_script_dir_yields_dir_confirm() {
        let req = dispatch_to_view_request(
            ActionDispatch::DeleteSelf { confirm: None },
            0,
            0,
            "live/db_scripts/maint/vacuum".into(),
            "delete-dir".into(),
            false,
        );
        match req {
            Some(ViewRequest::ConfirmDeleteAdapterDbScriptDir {
                database, rel_path, ..
            }) => {
                assert_eq!(database, "live");
                assert_eq!(rel_path, "maint/vacuum");
            }
            other => panic!("expected ConfirmDeleteAdapterDbScriptDir, got {other:?}"),
        }
    }

    /// DSF-4: TUI-owned action interception — `mark-move` on a
    /// db_script* node returns `MarkDbScriptForMove`, not whatever the
    /// adapter dispatched (typically `Noop`).
    #[test]
    fn dispatch_mark_move_intercepts_for_db_script_nodes() {
        let req = dispatch_to_view_request(
            ActionDispatch::Noop,
            0,
            0,
            "live/db_scripts/report".into(),
            "mark-move".into(),
            false,
        );
        match req {
            Some(ViewRequest::MarkDbScriptForMove { node_id }) => {
                assert_eq!(node_id, "live/db_scripts/report");
            }
            other => panic!("expected MarkDbScriptForMove, got {other:?}"),
        }
    }

    /// DSF-4: `paste-move` intercept.
    #[test]
    fn dispatch_paste_move_intercepts_for_db_script_nodes() {
        let req = dispatch_to_view_request(
            ActionDispatch::Noop,
            0,
            0,
            "live/db_scripts/maint".into(),
            "paste-move".into(),
            false,
        );
        match req {
            Some(ViewRequest::PasteDbScriptMove { target_node_id }) => {
                assert_eq!(target_node_id, "live/db_scripts/maint");
            }
            other => panic!("expected PasteDbScriptMove, got {other:?}"),
        }
    }

    /// DSF-4: `rename` intercept — adapter `Noop` plus the
    /// `rename` action name yield an `OpenDbScriptRenamePrompt` with
    /// the rel-path reassembled.
    #[test]
    fn dispatch_rename_intercepts_for_db_script_nodes() {
        let req = dispatch_to_view_request(
            ActionDispatch::Noop,
            7,
            3,
            "live/db_scripts/maint/vacuum".into(),
            "rename".into(),
            false,
        );
        match req {
            Some(ViewRequest::OpenDbScriptRenamePrompt {
                view_index,
                pane_id,
                database,
                rel_path,
                is_dir,
            }) => {
                assert_eq!(view_index, 7);
                assert_eq!(pane_id, 3);
                assert_eq!(database, "live");
                assert_eq!(rel_path, "maint/vacuum");
                // is_dir default: App re-probes the filesystem.
                assert!(!is_dir);
            }
            other => panic!("expected OpenDbScriptRenamePrompt, got {other:?}"),
        }
    }

    /// DSF-4: TUI-owned action interception is scoped to db_script*
    /// nodes — `mark-move` on a non-db-script node falls through.
    #[test]
    fn dispatch_mark_move_does_not_intercept_non_db_script_nodes() {
        let req = dispatch_to_view_request(
            ActionDispatch::Noop,
            0,
            0,
            "TICKET-1".into(),
            "mark-move".into(),
            false,
        );
        // Noop → None, plus no db_script interception → None.
        assert!(req.is_none(), "got: {req:?}");
    }

    /// CF-11: non-db_script node ids on `DeleteSelf` route through the
    /// generic content-delete confirm flow. The dispatcher no longer
    /// errors out — adapters that haven't migrated still hit `Noop`
    /// from `invoke_action`, so `DeleteSelf` only fires for adapters
    /// that have explicitly opted in.
    #[test]
    fn dispatch_delete_self_non_db_script_yields_generic_confirm() {
        let req = dispatch_to_view_request(
            ActionDispatch::DeleteSelf { confirm: None },
            0,
            1,
            "live/schemas/public/tables/users".into(),
            "delete".into(),
            false,
        );
        match req {
            Some(ViewRequest::ConfirmDeleteContentNode {
                view_index,
                pane_id,
                node_id,
                action_name,
                confirm,
            }) => {
                assert_eq!(view_index, 0);
                assert_eq!(pane_id, 1);
                assert_eq!(node_id, "live/schemas/public/tables/users");
                assert_eq!(action_name, "delete");
                assert_eq!(confirm, None);
            }
            other => panic!("expected ConfirmDeleteContentNode, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_error_yields_notify_with_message() {
        let req = dispatch_to_view_request(
            ActionDispatch::Error("boom".into()),
            0,
            1,
            "node-1".into(),
            "execute".into(),
            false,
        );
        match req {
            Some(ViewRequest::Notify(msg)) => assert_eq!(msg, "boom"),
            other => panic!("expected Notify, got {other:?}"),
        }
    }

    // ── M7/E6: generic mark/paste-move clipboard effect ──────────────

    #[test]
    fn generic_mark_move_marks_on_mark_move() {
        assert_eq!(
            generic_mark_move_effect("mark-move", "task-42"),
            MarkMoveEffect::Mark
        );
    }

    #[test]
    fn generic_mark_move_clears_on_paste_move() {
        assert_eq!(
            generic_mark_move_effect("paste-move", "task-42"),
            MarkMoveEffect::ClearOnPasteSuccess
        );
    }

    #[test]
    fn generic_mark_move_ignores_other_actions() {
        assert_eq!(
            generic_mark_move_effect("edit", "task-42"),
            MarkMoveEffect::Ignore
        );
    }

    #[test]
    fn generic_mark_move_ignores_db_script_nodes() {
        // DB-script keeps its bespoke path; the generic clipboard must
        // not claim these even for the shared action names.
        assert_eq!(
            generic_mark_move_effect("mark-move", "live/db_scripts/report"),
            MarkMoveEffect::Ignore
        );
        assert_eq!(
            generic_mark_move_effect("paste-move", "live/db_scripts/folder"),
            MarkMoveEffect::Ignore
        );
    }
}
