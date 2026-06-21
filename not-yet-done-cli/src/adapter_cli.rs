//! Generic, adapter-driven CLI front-end (Block D).
//!
//! Every configured adapter instance (a `views/*.yaml` file) is addressable on
//! the command line as `nyd <instance> <verb> …`, where the verbs drive the
//! frontend-agnostic [`ContentAdapter`] protocol directly. Because nothing here
//! knows about tasks, Jira, Postgres, … specifically, the same verbs work for
//! *every* adapter:
//!
//! ```text
//! nyd <inst> ls   [ID] [--type T] [--query Q] [--tree [--depth N]] [--sort S] [-o table|json]
//! nyd <inst> show  ID                                                          [-o table|json]
//! nyd <inst> actions (ID | --type T)                                           [-o table|json]
//! nyd <inst> values  SOURCE                                                    [-o table|json]
//! nyd <inst> do    ACTION [ID] [input flags] [--yes]
//! ```
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
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, ContentAdapter, FormFieldSpec,
    InputSpec, ListParams, Node, NodeAction, NodeSummary, NodeType, SortDirection, SortKey,
    Subtree, ValueOption,
};

/// Built-in `tusks` subcommands. A first argument matching one of these is
/// never treated as an adapter instance, so an adapter accidentally named like
/// a built-in can't shadow it.
const BUILTIN_COMMANDS: &[&str] = &[
    "task", "project", "tag", "db", "backup", "track", "query", "help",
];

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
/// when the first argument names a configured adapter instance (so this module
/// took over), or `None` to let the legacy `tusks` path handle it.
///
/// Discovery is cheap (it reads the view-config headers only) and side-effect
/// free, so probing here before the task-core path costs nothing for the
/// built-in commands.
pub fn try_dispatch(args: &[String]) -> Option<ExitCode> {
    let instance = args.get(1)?;
    if BUILTIN_COMMANDS.contains(&instance.as_str()) || instance.starts_with('-') {
        return None;
    }
    let instances = not_yet_done_host::discover_instances();
    if !instances.iter().any(|d| d.instance_id() == instance) {
        return None;
    }

    Some(match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nyd: {e:#}");
            ExitCode::FAILURE
        }
    })
}

/// Parse + execute a generic invocation on its own tokio runtime (adapter
/// construction is sync, but the read verbs are async).
fn run(args: &[String]) -> Result<()> {
    let inv = parse(args)?;

    let rt = tokio::runtime::Runtime::new().context("creating tokio runtime")?;
    rt.block_on(async move {
        let ctx = not_yet_done_host::host_context();
        let adapter = not_yet_done_host::resolve_adapter(&inv.instance, &ctx)?;
        match inv.verb.as_str() {
            "ls" | "list" => cmd_ls(adapter.as_ref(), &inv).await,
            "show" | "get" => cmd_show(adapter.as_ref(), &inv).await,
            "actions" => cmd_actions(adapter.as_ref(), &inv).await,
            "values" => cmd_values(adapter.as_ref(), &inv).await,
            "do" => cmd_do(adapter.as_ref(), &inv).await,
            other => Err(anyhow!(
                "unknown verb '{other}' (expected ls | show | actions | values | do)"
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

// ---------------------------------------------------------------------------
// Verbs
// ---------------------------------------------------------------------------

async fn cmd_ls(adapter: &dyn ContentAdapter, inv: &Invocation) -> Result<()> {
    let parent: Box<dyn Node> = match inv.positionals.first() {
        Some(id) => resolve_node(adapter, id).await?,
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

    let params = ListParams {
        node_type: node_type.clone(),
        query: inv.query.clone(),
        sort: inv.sort.clone(),
        page: None,
        download: false,
        group_by: None,
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
    let id = inv
        .positionals
        .first()
        .ok_or_else(|| anyhow!("show requires a node id"))?;
    let mut node = resolve_node(adapter, id).await?;
    // Fill display fields a lazily-built stub leaves as placeholders.
    node.hydrate().await;
    output_node(node.as_ref(), inv.output);
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

    let mut node: Box<dyn Node> = match inv.positionals.get(1) {
        Some(id) => resolve_node(adapter, id).await?,
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
        ActionDispatch::Noop => {
            println!("ok (no change)");
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
        ActionOutcome::Reopen { content, .. } => Err(anyhow!(
            "'{action_id}' rejected the input:\n{content}"
        )),
    }
}

/// Open `$EDITOR` (falling back to `$VISUAL`) on a temp file seeded with
/// `template`, using `suffix` for syntax highlighting, and return the saved
/// contents. Errors when no editor is configured — callers should suggest
/// `-m` for non-interactive use.
fn edit_in_editor(template: &str, suffix: &str) -> Result<String> {
    use std::io::Write;

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map_err(|_| {
            anyhow!("no $EDITOR set — pass -m <text> to supply the input non-interactively")
        })?;

    let mut tmp = tempfile::Builder::new()
        .suffix(suffix)
        .tempfile()
        .context("creating temp file for the editor")?;
    tmp.write_all(template.as_bytes())
        .context("writing editor template")?;
    tmp.flush().ok();
    let path = tmp.path().to_path_buf();

    // `$EDITOR` may carry arguments (e.g. "code --wait"); split on whitespace.
    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| anyhow!("$EDITOR is empty"))?;
    let status = std::process::Command::new(program)
        .args(parts)
        .arg(&path)
        .status()
        .with_context(|| format!("launching editor '{editor}'"))?;
    if !status.success() {
        return Err(anyhow!("editor '{editor}' exited with {status}"));
    }
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
    fn table_renders_header_and_rows() {
        // Smoke: just ensure it doesn't panic on ragged rows.
        print_table(
            &["a".into(), "bb".into()],
            &[vec!["1".into(), "2".into()], vec!["xxx".into(), "y".into()]],
        );
    }
}
