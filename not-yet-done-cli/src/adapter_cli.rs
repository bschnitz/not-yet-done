//! Generic, adapter-driven CLI front-end (Block D).
//!
//! Every configured adapter instance (a `views/*.yaml` file) is addressable on
//! the command line as `nyd <instance> <verb> …`, where the verbs drive the
//! frontend-agnostic [`ContentAdapter`] protocol directly. Because nothing here
//! knows about tasks, Jira, Postgres, … specifically, the same verbs work for
//! *every* adapter:
//!
//! ```text
//! nyd <inst> ls   [ID] [--type T] [--query Q] [--tree [--depth N]] [--sort S] [--group-by G] [-o table|json]
//! nyd <inst> show  ID                                                                         [-o table|json]
//! nyd <inst> actions (ID | --type T)                                                          [-o table|json]
//! nyd <inst> values  SOURCE                                                                   [-o table|json]
//! nyd <inst> do    ACTION [ID] [input flags] [--yes]
//! ```
//!
//! Any verb that targets a node by id accepts `--path /A/B/C` instead: a
//! generic front-end walk that descends one label per segment from the root.
//! A segment matches a child label by substring, or by regex when prefixed
//! `re:`; `-i` makes the match case-insensitive. The walk uses only the
//! protocol's per-level `list`, so it works for *every* adapter — the same way
//! the TUI lets you drill in by name without knowing opaque ids.
//!
//! The **read** verbs (`ls`/`show`/`actions`/`values`) came in D2a; the
//! mutating **`do`** verb (D2b) drives the same protocol the TUI's
//! shortcut/menu paths use. It looks up the action's [`InputSpec`] to decide
//! how to source input from the command line:
//!
//! ```text
//! Editor      → -m <text>  (or $EDITOR on a seeded temp file)
//! Form        → --field k=v   (repeatable)
//! Picker      → --value <v>
//! FilePicker  → --file <path>  (repeatable)
//! None        → invoke_action; ctx fed from --value / --text / --query / --yes
//! ```
//!
//! For `InputSpec::None` actions the returned [`ActionDispatch`] is handled
//! here: `Reload`/`Noop` report success, `Error` fails, `Confirm` and
//! `DeleteSelf` require `--yes` (a confirmed `DeleteSelf` then runs
//! `execute(action, None)`, mirroring the TUI's confirm→delete plumbing), and
//! the interactive-only dispatches (`CreateChild`/`OpenEditor`/`ExecuteQuery`)
//! report that they need a full frontend.
//!
//! Dispatch is intercepted before the legacy `tusks` command tree in
//! `main.rs`: a first argument that names a configured adapter instance (and
//! is not a built-in subcommand) routes here; everything else falls through
//! unchanged.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use not_yet_done_content::{
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, ContentAdapter, ContentError,
    FormFieldSpec, GroupBucket, GroupSpec, InputSpec, ListParams, Node, NodeAction, NodeSummary,
    NodeType, SortDirection, SortKey, Subtree, ValueOption,
};

/// Built-in `tusks` subcommands. A first argument matching one of these is
/// never treated as an adapter instance, so an adapter accidentally named like
/// a built-in can't shadow it. Only `tag`/`backup` remain (plus `help`); the
/// former `task`/`project`/`track`/`query`/`db` names were freed when those
/// commands moved to the adapter protocol (D3b), so they are now usable as
/// adapter instance names or `cli.yaml` alias names.
const BUILTIN_COMMANDS: &[&str] = &["tag", "backup", "help"];

/// Output format for the read verbs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    Table,
    Json,
}

/// Parsed generic invocation: `nyd <instance> <verb> [positionals…] [flags]`.
struct Invocation {
    instance: String,
    verb: String,
    /// Positional arguments. Their meaning is verb-specific: a node id/prefix
    /// or value source for the read verbs (at most one), and `ACTION [ID]` for
    /// `do` (the action id, then an optional target node id).
    positionals: Vec<String>,
    type_filter: Option<String>,
    query: Option<String>,
    tree: bool,
    depth: Option<u32>,
    sort: Vec<SortKey>,
    /// `--group-by col[:bucket][:order]`: adapter-side grouping (only adapters
    /// with `group_by_via_adapter` honor it; others ignore it with a warning).
    group_by: Option<GroupSpec>,
    /// `--path /A/B`: name the target node by a label walk instead of an id.
    path: Option<String>,
    /// `-i`/`--case-insensitive`: fold case in `--path` segment matching.
    case_insensitive: bool,
    output: Output,
    // ---- `do` input mapping (D2b) ----
    /// `-m`/`--message`: editor text supplied inline (skips `$EDITOR`).
    message: Option<String>,
    /// `--field k=v` (repeatable): values for an `InputSpec::Form` action.
    fields: Vec<(String, String)>,
    /// `--value`: an `InputSpec::Picker` selection, or `ActionContext::value`
    /// for a value-accepting `InputSpec::None` action.
    value: Option<String>,
    /// `--text`: `ActionContext::text` (typed free text, e.g. a new tag name).
    text: Option<String>,
    /// `--file` (repeatable): paths for an `InputSpec::FilePicker` action.
    files: Vec<PathBuf>,
    /// `--yes`/`-y`: pre-confirm a `Confirm`/`DeleteSelf`-gated action.
    yes: bool,
}

/// Try to handle `args` as a generic adapter invocation. Returns `Some(code)`
/// when this module took over (the first argument names a configured adapter
/// instance, a `cli.yaml` alias, or the `config` subcommand), or `None` to let
/// the legacy `tusks` path handle it.
///
/// Discovery is cheap (it reads the view-config headers only) and side-effect
/// free, so probing here before the task-core path costs nothing for the
/// built-in commands.
pub fn try_dispatch(args: &[String]) -> Option<ExitCode> {
    let first = args.get(1)?;
    if first.starts_with('-') {
        return None;
    }

    // `nyd config edit …` — manage config files. Reserved word; a configured
    // adapter instance named "config" can't be addressed (rename it).
    if first == "config" {
        return Some(finish(crate::cli_config::run_config(args)));
    }

    if BUILTIN_COMMANDS.contains(&first.as_str()) {
        return None;
    }

    // A configured adapter instance takes precedence over an alias of the same
    // name, so a user alias can never shadow a real adapter.
    let instances = not_yet_done_host::discover_instances();
    if instances.iter().any(|d| d.instance_id() == first) {
        return Some(finish(run(args)));
    }

    // Otherwise: a `cli.yaml` alias? Expand it and re-enter the generic path.
    match expand_alias(args) {
        Some(Ok(expanded)) => Some(finish(run(&expanded))),
        Some(Err(e)) => {
            eprintln!("nyd: {e:#}");
            Some(ExitCode::FAILURE)
        }
        None => None,
    }
}

/// Map a verb result to a process exit code, reporting errors to stderr.
fn finish(result: Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nyd: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// If `args[1]` names a `cli.yaml` alias, expand it into a full argv (program
/// name + expanded tokens) ready for [`run`]. Returns `None` when it isn't an
/// alias (fall through to `tusks`), `Some(Err)` when it is but expansion fails
/// (e.g. a missing positional), `Some(Ok(argv))` on success.
fn expand_alias(args: &[String]) -> Option<Result<Vec<String>>> {
    let name = args.get(1)?;
    let cfg = match crate::cli_config::load() {
        Ok(c) => c,
        Err(e) => return Some(Err(e)),
    };
    let template = cfg.alias(name)?.to_vec();
    let expanded = match crate::cli_config::expand(&template, &args[2..]) {
        Ok(toks) => toks,
        Err(e) => {
            return Some(Err(e.context(format!("expanding alias '{name}'"))))
        }
    };
    let mut argv = Vec::with_capacity(expanded.len() + 1);
    argv.push(args.first().cloned().unwrap_or_else(|| "nyd".to_string()));
    argv.extend(expanded);
    Some(Ok(argv))
}

/// Parse + execute a generic invocation on its own tokio runtime (adapter
/// construction is sync, but the read verbs are async).
fn run(args: &[String]) -> Result<()> {
    let inv = parse(args)?;

    let rt = tokio::runtime::Runtime::new().context("creating tokio runtime")?;
    rt.block_on(async move {
        let ctx = not_yet_done_host::host_context();
        let adapter = not_yet_done_host::resolve_adapter(&inv.instance, &ctx)?;
        // The adapter just connected (we built it): fire its `connected` hook.
        // No-op unless this instance's view file configures one; throttled via
        // the host state file, so e.g. an auto-backup runs at most once a day
        // however often the CLI is invoked. Best-effort — never blocks the verb.
        not_yet_done_host::fire_hook(adapter.as_ref(), &inv.instance, "connected").await;
        match inv.verb.as_str() {
            "ls" | "list" => cmd_ls(adapter.as_ref(), &inv).await,
            "show" | "get" => cmd_show(adapter.as_ref(), &inv).await,
            "cat" | "read" => cmd_cat(adapter.as_ref(), &inv).await,
            "actions" => cmd_actions(adapter.as_ref(), &inv).await,
            "values" => cmd_values(adapter.as_ref(), &inv).await,
            "do" => cmd_do(adapter.as_ref(), &inv).await,
            other => Err(anyhow!(
                "unknown verb '{other}' (expected ls | show | cat | actions | values | do)"
            )),
        }
    })
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

fn parse(args: &[String]) -> Result<Invocation> {
    let instance = args[1].clone();
    let verb = args.get(2).cloned().ok_or_else(|| {
        anyhow!("missing verb for '{instance}' (try: ls | show | actions | values | do)")
    })?;

    let mut positionals: Vec<String> = Vec::new();
    let mut type_filter = None;
    let mut query = None;
    let mut tree = false;
    let mut depth = None;
    let mut sort = Vec::new();
    let mut group_by = None;
    let mut path = None;
    let mut case_insensitive = false;
    let mut output = Output::Table;
    let mut message = None;
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut value = None;
    let mut text = None;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut yes = false;

    let mut i = 3;
    while i < args.len() {
        let a = &args[i];
        let take_value = |i: &mut usize, name: &str| -> Result<String> {
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| anyhow!("flag {name} requires a value"))
        };
        match a.as_str() {
            "--type" | "-t" => type_filter = Some(take_value(&mut i, "--type")?),
            "--query" | "-q" => query = Some(take_value(&mut i, "--query")?),
            "--tree" => tree = true,
            "--depth" => {
                let v = take_value(&mut i, "--depth")?;
                depth = Some(
                    v.parse::<u32>()
                        .with_context(|| format!("--depth expects a number, got '{v}'"))?,
                );
            }
            "--sort" | "-s" => sort = parse_sort(&take_value(&mut i, "--sort")?),
            "--group-by" | "-g" => {
                group_by = Some(parse_group_by(&take_value(&mut i, "--group-by")?)?)
            }
            "--path" | "-p" => path = Some(take_value(&mut i, "--path")?),
            "--case-insensitive" | "-i" => case_insensitive = true,
            "--output" | "-o" => {
                let v = take_value(&mut i, "--output")?;
                output = match v.as_str() {
                    "table" => Output::Table,
                    "json" => Output::Json,
                    other => return Err(anyhow!("--output expects table|json, got '{other}'")),
                };
            }
            "--message" | "-m" => message = Some(take_value(&mut i, "--message")?),
            "--field" => {
                let kv = take_value(&mut i, "--field")?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| anyhow!("--field expects key=value, got '{kv}'"))?;
                fields.push((k.to_string(), v.to_string()));
            }
            "--value" => value = Some(take_value(&mut i, "--value")?),
            "--text" => text = Some(take_value(&mut i, "--text")?),
            "--file" => files.push(PathBuf::from(take_value(&mut i, "--file")?)),
            "--yes" | "-y" => yes = true,
            other if other.starts_with('-') => {
                return Err(anyhow!("unknown flag '{other}'"));
            }
            _ => positionals.push(a.clone()),
        }
        i += 1;
    }

    // `--tree` only makes sense with depth; default to fully expanded.
    if depth.is_some() && !tree {
        tree = true;
    }

    Ok(Invocation {
        instance,
        verb,
        positionals,
        type_filter,
        query,
        tree,
        depth,
        sort,
        group_by,
        path,
        case_insensitive,
        output,
        message,
        fields,
        value,
        text,
        files,
        yes,
    })
}

/// Parse a `--sort col[:asc|desc],col2[:dir]` spec into [`SortKey`]s. An
/// unspecified direction defaults to ascending.
fn parse_sort(spec: &str) -> Vec<SortKey> {
    spec.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (col, dir) = match s.split_once(':') {
                Some((c, d)) => (c, d),
                None => (s, "asc"),
            };
            SortKey {
                column: col.to_string(),
                direction: if dir.eq_ignore_ascii_case("desc") {
                    SortDirection::Desc
                } else {
                    SortDirection::Asc
                },
            }
        })
        .collect()
}

/// Parse a `--group-by col[:bucket][:order]` spec into a [`GroupSpec`]. The
/// first `:`-segment is the column; the rest are an optional date bucket
/// (`day|week|month|year`) and/or group order (`asc|desc`), in any order.
/// Group keys are ISO-formatted, so order is over the (chronological) keys.
/// Defaults: no bucket (verbatim values), ascending order.
fn parse_group_by(spec: &str) -> Result<GroupSpec> {
    let mut parts = spec.split(':').map(str::trim);
    let column = parts
        .next()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| anyhow!("--group-by needs a column, got '{spec}'"))?
        .to_string();
    let mut bucket = None;
    let mut order = SortDirection::Asc;
    for p in parts.filter(|p| !p.is_empty()) {
        match p.to_ascii_lowercase().as_str() {
            "day" => bucket = Some(GroupBucket::Day),
            "week" => bucket = Some(GroupBucket::Week),
            "month" => bucket = Some(GroupBucket::Month),
            "year" => bucket = Some(GroupBucket::Year),
            "asc" => order = SortDirection::Asc,
            "desc" => order = SortDirection::Desc,
            other => {
                return Err(anyhow!(
                    "--group-by: unknown qualifier '{other}' (expected day|week|month|year or asc|desc)"
                ))
            }
        }
    }
    Ok(GroupSpec {
        column,
        bucket,
        order,
    })
}

// ---------------------------------------------------------------------------
// Verbs
// ---------------------------------------------------------------------------

async fn cmd_ls(adapter: &dyn ContentAdapter, inv: &Invocation) -> Result<()> {
    let parent: Box<dyn Node> =
        match resolve_target(adapter, inv, inv.positionals.first().map(String::as_str)).await? {
            Some(node) => node,
            None => adapter.root().await?,
        };

    let child_types = parent.children_types();
    if child_types.is_empty() {
        return Err(anyhow!(
            "node '{}' has no listable child types",
            parent.id()
        ));
    }
    let node_type = pick_type(&child_types, inv.type_filter.as_deref())?;

    if inv.group_by.is_some() && !adapter.capabilities().group_by_via_adapter {
        eprintln!(
            "nyd: warning: '{}' does not support adapter-side grouping — --group-by ignored",
            inv.instance
        );
    }

    let params = ListParams {
        node_type: node_type.clone(),
        query: inv.query.clone(),
        sort: inv.sort.clone(),
        page: None,
        download: false,
        group_by: inv.group_by.clone(),
    };

    if inv.tree {
        let depth = inv.depth.unwrap_or(u32::MAX);
        let sub = parent.list_subtree(params, depth).await?;
        output_tree(&sub, inv.output);
    } else {
        let res = parent.list(params).await?;
        output_list(&res.items, inv.output);
    }
    Ok(())
}

async fn cmd_show(adapter: &dyn ContentAdapter, inv: &Invocation) -> Result<()> {
    let mut node = resolve_target(adapter, inv, inv.positionals.first().map(String::as_str))
        .await?
        .ok_or_else(|| anyhow!("show requires a node id or --path /A/B"))?;
    // Fill display fields a lazily-built stub leaves as placeholders.
    node.hydrate().await;
    output_node(node.as_ref(), inv.output);
    Ok(())
}

/// `nyd <inst> cat ID` — print a node's raw text content to stdout.
///
/// Generic across adapters: resolves the node, then writes
/// `node.content().read_text()` verbatim (no trailing newline added). Errors
/// when the node has no content body (e.g. a pure container or attachment).
async fn cmd_cat(adapter: &dyn ContentAdapter, inv: &Invocation) -> Result<()> {
    let node = resolve_target(adapter, inv, inv.positionals.first().map(String::as_str))
        .await?
        .ok_or_else(|| anyhow!("cat requires a node id or --path /A/B"))?;
    let content = node
        .content()
        .ok_or_else(|| anyhow!("node '{}' has no readable content", node.id()))?;
    let text = content.read_text().await?;
    print!("{text}");
    Ok(())
}

async fn cmd_actions(adapter: &dyn ContentAdapter, inv: &Invocation) -> Result<()> {
    let actions: Vec<NodeAction> = if let Some(id) = inv.positionals.first() {
        resolve_node(adapter, id).await?.actions()
    } else if let Some(t) = &inv.type_filter {
        let nt = find_node_type(adapter, t).await?;
        adapter.actions_for_type(&nt)
    } else {
        return Err(anyhow!("actions requires a node id or --type <type>"));
    };
    output_actions(&actions, inv.output);
    Ok(())
}

async fn cmd_values(adapter: &dyn ContentAdapter, inv: &Invocation) -> Result<()> {
    let source = inv
        .positionals
        .first()
        .ok_or_else(|| anyhow!("values requires a source key (e.g. `values tags`)"))?;
    let values = adapter.list_values(source).await?;
    output_values(&values, inv.output);
    Ok(())
}

// ---------------------------------------------------------------------------
// `do` — invoke a mutating action
// ---------------------------------------------------------------------------

/// `nyd <inst> do ACTION [ID] [input flags]` — run a node action.
///
/// The target node is the one named by the second positional, or the
/// adapter root when omitted (so container-level actions like `add` work
/// without an id). The action's [`InputSpec`] selects how input is sourced
/// and which protocol entry point fires (`execute` vs `invoke_action`).
async fn cmd_do(adapter: &dyn ContentAdapter, inv: &Invocation) -> Result<()> {
    let action_id = inv
        .positionals
        .first()
        .ok_or_else(|| anyhow!("do requires an action id (list them with `actions`)"))?
        .clone();

    let mut node: Box<dyn Node> =
        match resolve_target(adapter, inv, inv.positionals.get(1).map(String::as_str)).await? {
            Some(node) => node,
            None => adapter.root().await?,
        };

    // Resolve the action so we know its input shape. Prefer the node's own
    // list, fall back to the type-level list (the source the TUI's shortcut
    // hints use).
    let action = find_action(node.as_ref(), adapter, &action_id).ok_or_else(|| {
        let mut avail: Vec<String> = node.actions().into_iter().map(|a| a.id).collect();
        for a in adapter.actions_for_type(node.node_type()) {
            if !avail.contains(&a.id) {
                avail.push(a.id);
            }
        }
        if avail.is_empty() {
            anyhow!("node '{}' exposes no action '{action_id}'", node.id())
        } else {
            anyhow!(
                "no action '{action_id}' on '{}' (available: {})",
                node.id(),
                avail.join(", ")
            )
        }
    })?;

    match action.input {
        InputSpec::Editor => do_editor(node.as_mut(), &action_id, inv).await,
        InputSpec::Form { fields } => do_form(node.as_mut(), &action_id, &fields, inv).await,
        InputSpec::Picker => do_picker(node.as_mut(), &action_id, inv).await,
        InputSpec::FilePicker { multi } => do_files(node.as_mut(), &action_id, multi, inv).await,
        InputSpec::None => do_dispatch(node.as_mut(), &action_id, inv).await,
    }
}

/// Find an action by id: the node's own [`Node::actions`] first, then the
/// adapter's type-level [`ContentAdapter::actions_for_type`].
fn find_action(node: &dyn Node, adapter: &dyn ContentAdapter, id: &str) -> Option<NodeAction> {
    node.actions()
        .into_iter()
        .find(|a| a.id == id)
        .or_else(|| {
            adapter
                .actions_for_type(node.node_type())
                .into_iter()
                .find(|a| a.id == id)
        })
}

/// `InputSpec::Editor`: seed a buffer from [`Node::prepare`], let the user
/// fill it (inline via `-m`, else `$EDITOR`), then [`Node::execute`] with
/// [`ActionInput::Edited`]. The template is passed back as `original` so the
/// adapter can diff/merge exactly as it does for the TUI editor session.
async fn do_editor(node: &mut dyn Node, action_id: &str, inv: &Invocation) -> Result<()> {
    let prep = node
        .prepare(action_id)
        .await
        .with_context(|| format!("preparing editor for '{action_id}'"))?;
    let text = match &inv.message {
        Some(m) => m.clone(),
        None => edit_in_editor(&prep.template, &prep.suffix)?,
    };
    let input = ActionInput::Edited {
        text,
        original: prep.template,
        version: prep.version,
    };
    let outcome = node.execute(action_id, input).await?;
    report_outcome(outcome, action_id)
}

/// `InputSpec::Form`: start from [`Node::form_prep`] prefills + each field's
/// static default, then override with `--field k=v`. Required fields must end
/// up non-empty.
async fn do_form(
    node: &mut dyn Node,
    action_id: &str,
    specs: &[FormFieldSpec],
    inv: &Invocation,
) -> Result<()> {
    let mut values = node.form_prep(action_id).await.unwrap_or_default();
    for spec in specs {
        if !values.contains_key(&spec.key) {
            if let Some(d) = &spec.default {
                values.insert(spec.key.clone(), d.clone());
            }
        }
    }
    for (k, v) in &inv.fields {
        values.insert(k.clone(), v.clone());
    }
    for spec in specs {
        if spec.required && values.get(&spec.key).map(String::is_empty).unwrap_or(true) {
            return Err(anyhow!(
                "missing required field '{}' (pass --field {}=<value>)",
                spec.key,
                spec.key
            ));
        }
    }
    let outcome = node.execute(action_id, ActionInput::Form(values)).await?;
    report_outcome(outcome, action_id)
}

/// `InputSpec::Picker`: the chosen value comes from `--value` (enumerate the
/// options with `actions`/`values`).
async fn do_picker(node: &mut dyn Node, action_id: &str, inv: &Invocation) -> Result<()> {
    let value = inv.value.clone().ok_or_else(|| {
        anyhow!("action '{action_id}' needs a choice — pass --value <v> (see `values`)")
    })?;
    let outcome = node.execute(action_id, ActionInput::Picked(value)).await?;
    report_outcome(outcome, action_id)
}

/// `InputSpec::FilePicker`: paths come from one or more `--file` flags.
async fn do_files(
    node: &mut dyn Node,
    action_id: &str,
    multi: bool,
    inv: &Invocation,
) -> Result<()> {
    if inv.files.is_empty() {
        return Err(anyhow!("action '{action_id}' needs at least one --file <path>"));
    }
    if !multi && inv.files.len() > 1 {
        return Err(anyhow!("action '{action_id}' accepts only one --file"));
    }
    let outcome = node
        .execute(action_id, ActionInput::Files(inv.files.clone()))
        .await?;
    report_outcome(outcome, action_id)
}

/// `InputSpec::None`: the shortcut/dispatch path. Build an [`ActionContext`]
/// from the CLI flags, call [`Node::invoke_action`], and act on the returned
/// [`ActionDispatch`].
async fn do_dispatch(node: &mut dyn Node, action_id: &str, inv: &Invocation) -> Result<()> {
    let ctx = ActionContext {
        marked: None,
        confirmed: inv.yes,
        query: inv.query.clone(),
        value: inv.value.clone(),
        text: inv.text.clone(),
    };
    let dispatch = node.invoke_action(action_id, &ctx).await?;
    match dispatch {
        ActionDispatch::Reload => {
            println!("ok");
            Ok(())
        }
        // `invoke_action` reports no dispatch-style handling. Mirror the TUI's
        // primary `InputSpec::None` path (it calls `execute` directly): custom
        // None-actions such as `toggle_watch`, `open_in_browser` and
        // `export-bundle` do their work in `execute`, not `invoke_action`.
        // Fall back to it so those are CLI-invocable too; a genuinely
        // no-op action (no `execute` arm) still reports "ok (no change)".
        ActionDispatch::Noop => match node.execute(action_id, ActionInput::None).await {
            Ok(outcome) => report_outcome(outcome, action_id),
            Err(ContentError::NotSupported(_)) => {
                println!("ok (no change)");
                Ok(())
            }
            Err(e) => Err(e.into()),
        },
        ActionDispatch::Notify { message } => {
            println!("{message}");
            Ok(())
        }
        ActionDispatch::Error(msg) => Err(anyhow!("{msg}")),
        // Generic confirm gate. With `--yes` we already passed
        // `confirmed: true`, so the adapter does the work instead of asking;
        // a `Confirm` here means `--yes` was absent.
        ActionDispatch::Confirm { prompt } => Err(anyhow!(
            "{prompt}\n  (re-run with --yes to confirm)"
        )),
        // The adapter wants the frontend's delete-confirm flow, which on
        // "yes" runs `execute(action, None)`. We mirror that: `--yes` →
        // perform the delete; otherwise surface the (adapter-authored) prompt.
        ActionDispatch::DeleteSelf { confirm } => {
            if !inv.yes {
                let prompt =
                    confirm.unwrap_or_else(|| format!("Delete '{}'? (y/n)", node.label()));
                return Err(anyhow!("{prompt}\n  (re-run with --yes to confirm)"));
            }
            let outcome = node.execute(action_id, ActionInput::None).await?;
            report_outcome(outcome, action_id)
        }
        // Interactive-only dispatches: they drive a UI flow (name prompt,
        // editor session, paginated result pane) the CLI can't stand in for.
        ActionDispatch::CreateChild { hint } => Err(anyhow!(
            "action '{action_id}' creates a child interactively (hint '{hint}') — use the TUI"
        )),
        ActionDispatch::OpenEditor { session_kind, .. } => Err(anyhow!(
            "action '{action_id}' opens an interactive '{session_kind}' editor — use the TUI"
        )),
        ActionDispatch::ExecuteQuery { .. } => Err(anyhow!(
            "action '{action_id}' runs a query result pane — use the TUI"
        )),
    }
}

/// Report an [`ActionOutcome`] from [`Node::execute`]. A `Reopen` (validation
/// or conflict — the adapter re-rendered the buffer with error banners) can't
/// be re-edited non-interactively, so it surfaces as an error with the
/// rejected buffer attached.
fn report_outcome(outcome: ActionOutcome, action_id: &str) -> Result<()> {
    match outcome {
        ActionOutcome::Done { message } => {
            println!("{}", message.as_deref().unwrap_or("ok"));
            Ok(())
        }
        ActionOutcome::NoChanges => {
            println!("no changes");
            Ok(())
        }
        ActionOutcome::Navigate { node_id, .. } => {
            println!("ok → {node_id}");
            Ok(())
        }
        ActionOutcome::OpenExternal { target, message } => {
            // No viewer to launch non-interactively — report the message and
            // the path the frontend would have opened, so a script can act on
            // it (e.g. `xdg-open` the file itself).
            if let Some(msg) = message {
                println!("{msg}");
            }
            println!("{target}");
            Ok(())
        }
        ActionOutcome::Reopen { content, .. } => Err(anyhow!(
            "'{action_id}' rejected the input:\n{content}"
        )),
        ActionOutcome::OpenEditor { action_id: next } => Err(anyhow!(
            "'{action_id}' opens an interactive editor for '{next}' — use the TUI"
        )),
    }
}

/// Open `$EDITOR` (falling back to `$VISUAL`) on a temp file seeded with
/// `template`, using `suffix` for syntax highlighting, and return the saved
/// contents. Errors when no editor is configured — callers should suggest
/// `-m` for non-interactive use.
fn edit_in_editor(template: &str, suffix: &str) -> Result<String> {
    use std::io::Write;

    // Fail fast with the `-m` hint before seeding a temp file we'd discard.
    if std::env::var_os("EDITOR").is_none() && std::env::var_os("VISUAL").is_none() {
        return Err(anyhow!(
            "no $EDITOR set — pass -m <text> to supply the input non-interactively"
        ));
    }

    let mut tmp = tempfile::Builder::new()
        .suffix(suffix)
        .tempfile()
        .context("creating temp file for the editor")?;
    tmp.write_all(template.as_bytes())
        .context("writing editor template")?;
    tmp.flush().ok();
    let path = tmp.path().to_path_buf();

    crate::cli_config::launch_editor(&path)?;
    std::fs::read_to_string(&path).context("reading edited buffer")
}

// ---------------------------------------------------------------------------
// Node / type resolution
// ---------------------------------------------------------------------------

/// Pick the child type to list. With `--type`, match on `type_id` exactly;
/// without it, default to the first declared child type.
fn pick_type<'a>(child_types: &'a [NodeType], filter: Option<&str>) -> Result<&'a NodeType> {
    match filter {
        None => Ok(&child_types[0]),
        Some(t) => child_types
            .iter()
            .find(|nt| nt.type_id == t)
            .ok_or_else(|| {
                let avail: Vec<&str> = child_types.iter().map(|nt| nt.type_id.as_str()).collect();
                anyhow!("no child type '{t}' (available: {})", avail.join(", "))
            }),
    }
}

/// Resolve the single node a verb targets, preferring `--path` (a label walk)
/// over an explicit `id`. Returns `None` when neither is given, so the caller
/// supplies its own default (the adapter root for `ls`/`do`; an error for
/// `show`). Giving both at once is rejected as ambiguous.
async fn resolve_target(
    adapter: &dyn ContentAdapter,
    inv: &Invocation,
    id: Option<&str>,
) -> Result<Option<Box<dyn Node>>> {
    match (inv.path.as_deref(), id) {
        (Some(_), Some(_)) => Err(anyhow!(
            "give a node id or --path, not both"
        )),
        (Some(p), None) => Ok(Some(resolve_path(adapter, p, inv.case_insensitive).await?)),
        (None, Some(id)) => Ok(Some(resolve_node(adapter, id).await?)),
        (None, None) => Ok(None),
    }
}

/// A `--path` segment matcher: a literal substring, or a compiled regex when
/// the segment was written `re:<pattern>`. Case folding (from `-i`) is baked
/// in at construction so matching is a plain predicate.
enum SegMatch {
    Substring { needle: String, case_insensitive: bool },
    Regex(regex::Regex),
}

impl SegMatch {
    fn parse(seg: &str, case_insensitive: bool) -> Result<Self> {
        if let Some(pat) = seg.strip_prefix("re:") {
            let re = regex::RegexBuilder::new(pat)
                .case_insensitive(case_insensitive)
                .build()
                .with_context(|| format!("invalid regex in path segment 're:{pat}'"))?;
            Ok(SegMatch::Regex(re))
        } else {
            Ok(SegMatch::Substring {
                needle: if case_insensitive { seg.to_lowercase() } else { seg.to_string() },
                case_insensitive,
            })
        }
    }

    fn matches(&self, label: &str) -> bool {
        match self {
            SegMatch::Substring { needle, case_insensitive } => {
                if *case_insensitive {
                    label.to_lowercase().contains(needle)
                } else {
                    label.contains(needle)
                }
            }
            SegMatch::Regex(re) => re.is_match(label),
        }
    }
}

/// Walk a `/A/B/C` path from the adapter root, descending one node per
/// segment. Each segment matches a *direct child's* label (across all child
/// types of the current node) via [`SegMatch`]; a segment must resolve to
/// exactly one child or the walk errors (ambiguous / not found). Uses only the
/// per-level `list`, so it works for any adapter — the CLI analogue of drilling
/// in by name in the TUI.
async fn resolve_path(
    adapter: &dyn ContentAdapter,
    path: &str,
    case_insensitive: bool,
) -> Result<Box<dyn Node>> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut current = adapter.root().await?;
    let mut walked: Vec<String> = Vec::new();
    for seg in segments {
        let matcher = SegMatch::parse(seg, case_insensitive)?;
        let mut matches: Vec<NodeSummary> = Vec::new();
        for nt in current.children_types() {
            let params = ListParams {
                node_type: nt,
                query: None,
                sort: Vec::new(),
                page: None,
                download: false,
                group_by: None,
            };
            for item in current.list(params).await?.items {
                if matcher.matches(&item.label) {
                    matches.push(item);
                }
            }
        }
        // De-dupe by id (a node can surface under more than one child type).
        matches.sort_by(|a, b| a.id.cmp(&b.id));
        matches.dedup_by(|a, b| a.id == b.id);
        let chosen = match matches.len() {
            1 => matches.pop().unwrap(),
            0 => {
                let at = if walked.is_empty() {
                    "the root".to_string()
                } else {
                    format!("'{}'", walked.join("/"))
                };
                return Err(anyhow!("path segment '{seg}' matched no child under {at}"));
            }
            _ => {
                let preview: Vec<&str> =
                    matches.iter().take(10).map(|s| s.label.as_str()).collect();
                return Err(anyhow!(
                    "path segment '{seg}' is ambiguous — matches {} children:\n  {}",
                    matches.len(),
                    preview.join("\n  ")
                ));
            }
        };
        current = adapter
            .get_by_id(&chosen.id)
            .await
            .map_err(|e| anyhow!("resolving '{}': {e}", chosen.label))?;
        walked.push(chosen.label);
    }
    Ok(current)
}

/// Resolve a node id, with git-style prefix matching for adapters whose ids are
/// long and opaque (the local task/tracking UUID forests). For adapters that
/// keep their tree in memory ([`AdapterCapabilities::supports_eager_subtree`])
/// we survey the whole forest and accept a unique prefix; otherwise (remote
/// adapters, whose ids are short keys) we ask the adapter directly.
async fn resolve_node(adapter: &dyn ContentAdapter, input: &str) -> Result<Box<dyn Node>> {
    if adapter.capabilities().supports_eager_subtree {
        let summaries = survey(adapter).await?;
        let mut matches: Vec<String> = summaries
            .iter()
            .map(|s| s.id.clone())
            .filter(|id| id.starts_with(input))
            .collect();
        matches.sort();
        matches.dedup();
        match matches.len() {
            1 => {
                return adapter
                    .get_by_id(&matches[0])
                    .await
                    .map_err(|e| anyhow!("{e}"))
            }
            n if n > 1 => {
                let preview: Vec<&str> = matches.iter().take(10).map(String::as_str).collect();
                return Err(anyhow!(
                    "prefix '{input}' is ambiguous — matches {n} nodes:\n  {}",
                    preview.join("\n  ")
                ));
            }
            // 0 matches: fall through to an exact lookup (e.g. an id outside
            // the surveyed set).
            _ => {}
        }
    }
    adapter
        .get_by_id(input)
        .await
        .map_err(|e| anyhow!("no node '{input}': {e}"))
}

/// Find a [`NodeType`] by `type_id` by surveying the tree. Used by
/// `actions --type`.
async fn find_node_type(adapter: &dyn ContentAdapter, type_id: &str) -> Result<NodeType> {
    let root = adapter.root().await?;
    if let Some(nt) = root.children_types().into_iter().find(|nt| nt.type_id == type_id) {
        return Ok(nt);
    }
    let summaries = survey(adapter).await?;
    summaries
        .into_iter()
        .map(|s| s.node_type)
        .find(|nt| nt.type_id == type_id)
        .ok_or_else(|| anyhow!("no node type '{type_id}' found under this adapter"))
}

/// Flatten the adapter's tree into a list of summaries (unfiltered), used for
/// prefix resolution and type discovery. Eager adapters expand fully; others
/// only list the root level (their ids are exact short keys, so a single level
/// is enough and a deep walk would be N round-trips).
async fn survey(adapter: &dyn ContentAdapter) -> Result<Vec<NodeSummary>> {
    let depth = if adapter.capabilities().supports_eager_subtree {
        u32::MAX
    } else {
        0
    };
    let root = adapter.root().await?;
    let mut out = Vec::new();
    for nt in root.children_types() {
        let params = ListParams {
            node_type: nt,
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        };
        let sub = root.list_subtree(params, depth).await?;
        collect_summaries(&sub, &mut out);
    }
    Ok(out)
}

fn collect_summaries(sub: &Subtree, out: &mut Vec<NodeSummary>) {
    for node in &sub.items {
        out.push(node.summary.clone());
        collect_summaries(&node.children, out);
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn output_list(items: &[NodeSummary], output: Output) {
    match output {
        Output::Json => {
            let arr: Vec<serde_json::Value> = items.iter().map(summary_json).collect();
            print_json(&serde_json::Value::Array(arr));
        }
        Output::Table => {
            // Union of metadata keys (in first-seen order) becomes the columns,
            // prefixed by id + label.
            let mut cols: Vec<String> = vec!["id".into(), "label".into()];
            for it in items {
                for f in &it.metadata.fields {
                    if !cols.contains(&f.key) {
                        cols.push(f.key.clone());
                    }
                }
            }
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|it| {
                    cols.iter()
                        .map(|c| match c.as_str() {
                            "id" => it.id.clone(),
                            "label" => it.label.clone(),
                            key => it
                                .metadata
                                .fields
                                .iter()
                                .find(|f| f.key == key)
                                .map(|f| f.value.clone())
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .collect();
            print_table(&cols, &rows);
        }
    }
}

fn output_tree(sub: &Subtree, output: Output) {
    match output {
        Output::Json => print_json(&subtree_json(sub)),
        Output::Table => {
            let mut lines = Vec::new();
            tree_lines(sub, 0, &mut lines);
            for l in lines {
                println!("{l}");
            }
        }
    }
}

fn tree_lines(sub: &Subtree, depth: usize, out: &mut Vec<String>) {
    for node in &sub.items {
        let indent = "  ".repeat(depth);
        out.push(format!("{indent}{}  [{}]", node.summary.label, short(&node.summary.id)));
        tree_lines(&node.children, depth + 1, out);
    }
}

fn output_node(node: &dyn Node, output: Output) {
    match output {
        Output::Json => {
            let fields: Vec<serde_json::Value> = node
                .metadata()
                .fields
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "key": f.key,
                        "label": f.display_label,
                        "value": f.value,
                        "editable": f.editable,
                    })
                })
                .collect();
            print_json(&serde_json::json!({
                "id": node.id(),
                "label": node.label(),
                "type": node.node_type().type_id,
                "metadata": fields,
            }));
        }
        Output::Table => {
            println!("id:    {}", node.id());
            println!("label: {}", node.label());
            println!("type:  {}", node.node_type().type_id);
            if !node.metadata().fields.is_empty() {
                println!();
                let cols = vec!["field".to_string(), "value".to_string(), "editable".to_string()];
                let rows: Vec<Vec<String>> = node
                    .metadata()
                    .fields
                    .iter()
                    .map(|f| {
                        vec![
                            f.display_label.clone(),
                            f.value.clone(),
                            if f.editable { "yes".into() } else { "no".into() },
                        ]
                    })
                    .collect();
                print_table(&cols, &rows);
            }
        }
    }
}

fn output_actions(actions: &[NodeAction], output: Output) {
    match output {
        Output::Json => {
            let arr: Vec<serde_json::Value> = actions
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "id": a.id,
                        "label": a.label,
                        "input": input_kind(a),
                        "default_key": a.default_key.map(|c| c.to_string()),
                    })
                })
                .collect();
            print_json(&serde_json::Value::Array(arr));
        }
        Output::Table => {
            let cols = vec!["id".to_string(), "label".to_string(), "input".to_string(), "key".to_string()];
            let rows: Vec<Vec<String>> = actions
                .iter()
                .map(|a| {
                    vec![
                        a.id.clone(),
                        a.label.clone(),
                        input_kind(a),
                        a.default_key.map(|c| c.to_string()).unwrap_or_default(),
                    ]
                })
                .collect();
            print_table(&cols, &rows);
        }
    }
}

fn output_values(values: &[ValueOption], output: Output) {
    match output {
        Output::Json => {
            let arr: Vec<serde_json::Value> = values
                .iter()
                .map(|v| serde_json::json!({ "value": v.value, "label": v.label }))
                .collect();
            print_json(&serde_json::Value::Array(arr));
        }
        Output::Table => {
            let cols = vec!["value".to_string(), "label".to_string()];
            let rows: Vec<Vec<String>> = values
                .iter()
                .map(|v| vec![v.value.clone(), v.label.clone()])
                .collect();
            print_table(&cols, &rows);
        }
    }
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

/// Human-readable input-shape name for an action (matches the `InputSpec`
/// variants without leaking the enum).
fn input_kind(a: &NodeAction) -> String {
    use not_yet_done_content::InputSpec;
    match &a.input {
        InputSpec::None => "none".into(),
        InputSpec::Editor => "editor".into(),
        InputSpec::Picker => "picker".into(),
        InputSpec::FilePicker { multi } => {
            if *multi { "files".into() } else { "file".into() }
        }
        InputSpec::Form { fields } => format!("form({})", fields.len()),
    }
}

fn summary_json(s: &NodeSummary) -> serde_json::Value {
    let fields: Vec<serde_json::Value> = s
        .metadata
        .fields
        .iter()
        .map(|f| {
            serde_json::json!({
                "key": f.key,
                "label": f.display_label,
                "value": f.value,
            })
        })
        .collect();
    serde_json::json!({
        "id": s.id,
        "label": s.label,
        "type": s.node_type.type_id,
        "has_children": s.has_children,
        "metadata": fields,
    })
}

fn subtree_json(sub: &Subtree) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = sub
        .items
        .iter()
        .map(|node| {
            let mut v = summary_json(&node.summary);
            if !node.children.items.is_empty() {
                v["children"] = subtree_json(&node.children);
            }
            v
        })
        .collect();
    serde_json::Value::Array(arr)
}

fn print_json(v: &serde_json::Value) {
    match serde_json::to_string_pretty(v) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("nyd: JSON serialization failed: {e}"),
    }
}

/// First 8 chars of an opaque id, for compact tree display.
fn short(id: &str) -> String {
    if id.len() > 8 {
        id.chars().take(8).collect()
    } else {
        id.to_string()
    }
}

/// Render a simple left-aligned text table. Column widths use char counts —
/// good enough for the CLI; wide/zero-width glyphs in labels may misalign
/// slightly but never corrupt the data.
fn print_table(cols: &[String], rows: &[Vec<String>]) {
    if rows.is_empty() {
        println!("(no rows)");
        return;
    }
    let mut widths: Vec<usize> = cols.iter().map(|c| c.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let fmt_row = |cells: &[String]| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let pad = widths.get(i).copied().unwrap_or(0).saturating_sub(c.chars().count());
                format!("{c}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    println!("{}", fmt_row(cols));
    println!(
        "{}",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in rows {
        println!("{}", fmt_row(row));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sort_defaults_to_asc_and_reads_desc() {
        let keys = parse_sort("started:desc, label");
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].column, "started");
        assert_eq!(keys[0].direction, SortDirection::Desc);
        assert_eq!(keys[1].column, "label");
        assert_eq!(keys[1].direction, SortDirection::Asc);
    }

    #[test]
    fn parse_reads_verb_positional_and_flags() {
        let args: Vec<String> = ["nyd", "tasks", "ls", "abc123", "--type", "task:item", "--tree", "--depth", "2", "-o", "json"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let inv = parse(&args).unwrap();
        assert_eq!(inv.instance, "tasks");
        assert_eq!(inv.verb, "ls");
        assert_eq!(inv.positionals, vec!["abc123".to_string()]);
        assert_eq!(inv.type_filter.as_deref(), Some("task:item"));
        assert!(inv.tree);
        assert_eq!(inv.depth, Some(2));
        assert!(matches!(inv.output, Output::Json));
    }

    #[test]
    fn parse_do_reads_action_node_and_input_flags() {
        let args: Vec<String> = [
            "nyd", "tasks", "do", "edit", "abc123", "-m", "new body", "--yes",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let inv = parse(&args).unwrap();
        assert_eq!(inv.verb, "do");
        assert_eq!(
            inv.positionals,
            vec!["edit".to_string(), "abc123".to_string()]
        );
        assert_eq!(inv.message.as_deref(), Some("new body"));
        assert!(inv.yes);
    }

    #[test]
    fn parse_do_collects_fields_files_and_value() {
        let args: Vec<String> = [
            "nyd", "pg", "do", "create", "--field", "name=report", "--field", "db=live",
            "--value", "v1", "--text", "hello", "--file", "/tmp/a.sql", "--file", "/tmp/b.sql",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let inv = parse(&args).unwrap();
        assert_eq!(
            inv.fields,
            vec![
                ("name".to_string(), "report".to_string()),
                ("db".to_string(), "live".to_string()),
            ]
        );
        assert_eq!(inv.value.as_deref(), Some("v1"));
        assert_eq!(inv.text.as_deref(), Some("hello"));
        assert_eq!(inv.files.len(), 2);
    }

    #[test]
    fn parse_field_without_equals_errors() {
        let args: Vec<String> = ["nyd", "pg", "do", "create", "--field", "noeq"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse(&args).is_err());
    }

    #[test]
    fn depth_without_tree_implies_tree() {
        let args: Vec<String> = ["nyd", "tasks", "ls", "--depth", "1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let inv = parse(&args).unwrap();
        assert!(inv.tree);
        assert_eq!(inv.depth, Some(1));
    }

    #[test]
    fn unknown_flag_errors() {
        let args: Vec<String> = ["nyd", "tasks", "ls", "--bogus"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse(&args).is_err());
    }

    #[test]
    fn parse_group_by_column_only_defaults() {
        let g = parse_group_by("started_at").unwrap();
        assert_eq!(g.column, "started_at");
        assert_eq!(g.bucket, None);
        assert_eq!(g.order, SortDirection::Asc);
    }

    #[test]
    fn parse_group_by_bucket_and_order_any_order() {
        let g = parse_group_by("started_at:day:desc").unwrap();
        assert_eq!(g.column, "started_at");
        assert_eq!(g.bucket, Some(GroupBucket::Day));
        assert_eq!(g.order, SortDirection::Desc);
        // order before bucket parses identically
        let g2 = parse_group_by("started_at:desc:week").unwrap();
        assert_eq!(g2.bucket, Some(GroupBucket::Week));
        assert_eq!(g2.order, SortDirection::Desc);
    }

    #[test]
    fn parse_group_by_unknown_qualifier_errors() {
        assert!(parse_group_by("col:fortnight").is_err());
        assert!(parse_group_by("").is_err());
    }

    #[test]
    fn parse_reads_group_by_path_and_case_flag() {
        let args: Vec<String> = [
            "nyd", "trk", "ls", "--group-by", "started_at:day", "--path", "/Work/Report", "-i",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let inv = parse(&args).unwrap();
        assert_eq!(inv.group_by.unwrap().bucket, Some(GroupBucket::Day));
        assert_eq!(inv.path.as_deref(), Some("/Work/Report"));
        assert!(inv.case_insensitive);
    }

    #[test]
    fn seg_match_substring_and_case_fold() {
        let m = SegMatch::parse("Rep", false).unwrap();
        assert!(m.matches("Report"));
        assert!(!m.matches("REPORT"));
        let mi = SegMatch::parse("rep", true).unwrap();
        assert!(mi.matches("REPORT"));
        assert!(mi.matches("Report"));
    }

    #[test]
    fn seg_match_regex_opt_in() {
        let m = SegMatch::parse(r"re:^Rep", false).unwrap();
        assert!(m.matches("Report"));
        assert!(!m.matches("My Report"));
        assert!(SegMatch::parse("re:[unclosed", false).is_err());
    }

    #[test]
    fn table_renders_header_and_rows() {
        // Smoke: just ensure it doesn't panic on ragged rows.
        print_table(
            &["a".into(), "bb".into()],
            &[vec!["1".into(), "2".into()], vec!["xxx".into(), "y".into()]],
        );
    }
}
