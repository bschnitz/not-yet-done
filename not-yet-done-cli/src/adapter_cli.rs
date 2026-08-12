//! Generic, adapter-driven CLI front-end (Block D).
//!
//! Adapters are addressed under a single `adapter` command, by a **type path**
//! that names a *level* in the content tree — the instance id, then one local
//! child-type name per level of descent, joined by `:`:
//!
//! ```text
//! nyd adapter <inst>[:<child>[:<grandchild>…]] [ID] <command> [flags]
//! ```
//!
//! The **command is the last positional**. It is either a framework verb or an
//! adapter action id; `help`/`ls` need no id, an action on a concrete node
//! takes its (self-contained) node id in the slot before the command:
//!
//! ```text
//! nyd adapter jira help                              # document the root level
//! nyd adapter jira:issue:comment help                # document the comment level (by type, no fetch)
//! nyd adapter jira:issue <issue-id> ls [--type T]    # list a node's children
//! nyd adapter jira:issue:comment <comment-id> delete # run an action on a node
//! nyd adapter jira:issue <id> show                   # [-o table|json]
//! nyd adapter jira values <source>                   # list value options
//! ```
//!
//! `help` is special: with no id it documents the level **by type alone**
//! (walking [`not_yet_done_content::child_types_of_type`] down the path and
//! rendering [`not_yet_done_content::render_level_for_type`]), so no node is
//! fetched — the id-free navigation the `childs` single-source refactor enables.
//! With an id it renders the concrete node's level ([`render_level`]).
//!
//! Any id-taking command also accepts `--path /A/B/C` instead of an id: a label
//! walk that descends one child label per segment from the root (substring, or
//! regex with `re:`; `-i` folds case).
//!
//! Adapter actions look up the action's [`InputSpec`] to decide how to source
//! input from the command line:
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
//! the interactive-only dispatches (`OpenEditor`/`ExecuteQuery`)
//! report that they need a full frontend.
//!
//! Dispatch is intercepted before the legacy `tusks` command tree in `main.rs`:
//! `adapter …` routes here, `config …` to the config editor, the top level
//! (no args / `help`) prints the overview (with the configured instances), and
//! everything else is tried as a `cli.yaml` alias before falling through to the
//! remaining `tusks` built-ins (`tag`/`backup`).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::adapter_connect;
use crate::adapter_query;
use not_yet_done_content::{
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, ContentAdapter, ContentError,
    FormFieldSpec, GroupBucket, GroupSpec, InputSpec, ListParams, Node, NodeAction, NodeSummary,
    NodeType, SortDirection, SortKey, Subtree, ValueOption, children,
};

/// Output format for the read verbs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    Table,
    Json,
}

/// Parsed generic invocation: `nyd adapter <inst>[:child…] [ID] <command> [flags]`.
struct Invocation {
    /// First segment of the type path — the adapter instance to resolve.
    instance: String,
    /// The child-type names after the instance (`jira:issue:comment` →
    /// `["issue", "comment"]`). Selects the level for type-addressed `help`.
    child_path: Vec<String>,
    /// The framework verb the command resolved to (`ls`/`show`/`cat`/`actions`/
    /// `values`/`help`), or `"do"` when the command is an adapter action id.
    verb: String,
    /// Positional arguments. Their meaning is verb-specific: a node id/prefix
    /// or value source for the read verbs (at most one), and `ACTION [ID]` for
    /// `do` (the action id, then an optional target node id).
    positionals: Vec<String>,
    type_filter: Option<String>,
    /// `--query`: a query body typed on the command line, in the adapter's own
    /// query language.
    query: Option<String>,
    /// `--query-name`: a *stored* query, named. Which of the two stores holds
    /// the name decides whether the body is one native query or an extended
    /// document (see [`crate::adapter_query`]); mutually exclusive with
    /// `--query`, since a body and a reference to one are different things.
    query_name: Option<String>,
    /// `--var k=v` (repeatable): bindings for the query's variables. The CLI
    /// cannot prompt, so a variable without a binding *and* without a default
    /// is an error rather than a guess.
    vars: Vec<(String, String)>,
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
    /// `--full`: on `help`, print the full level reference (capabilities, child
    /// types, sort columns) instead of the default CLI-usage view.
    full: bool,
}

/// Route the top-level argv. Returns `Some(code)` when this module handled it
/// (`adapter …`, `config …`, the overview, or a `cli.yaml` alias that expands
/// to one of those), or `None` to let the legacy `tusks` path handle the two
/// remaining built-ins (`tag`/`backup`).
///
/// Adapter instances are **not** top-level commands anymore — they live under
/// `adapter <instance>[:child…]`. That frees the top level so an alias name can
/// never be shadowed by an instance, and lets the overview list the instances
/// (the plain `tusks` help never could).
pub fn try_dispatch(args: &[String]) -> Option<ExitCode> {
    match args.get(1).map(String::as_str) {
        // No args or an explicit top-level help request → our own overview,
        // which lists the configured adapter instances.
        None | Some("help") | Some("-h") | Some("--help") => {
            print_top_level_help();
            Some(ExitCode::SUCCESS)
        }
        Some("config") => Some(finish(crate::cli_config::run_config(args))),
        Some("adapter") => Some(finish(run_adapter(args))),
        // Legacy `tusks` built-ins keep their own dispatch + help.
        Some("tag") | Some("backup") => None,
        // Anything else: try a `cli.yaml` alias (which expands to a full
        // `adapter …`/`config …` argv), else fall through to `tusks`.
        Some(_) => match expand_alias(args) {
            Some(Ok(expanded)) => Some(dispatch_expanded(&expanded)),
            Some(Err(e)) => {
                eprintln!("nyd: {e:#}");
                Some(ExitCode::FAILURE)
            }
            None => None,
        },
    }
}

/// Route an alias-expanded argv. Aliases expand to a full `adapter …` (or
/// `config …`) invocation, so only those two targets are valid here — a
/// re-entry into [`try_dispatch`] could otherwise re-expand and loop.
fn dispatch_expanded(args: &[String]) -> ExitCode {
    match args.get(1).map(String::as_str) {
        Some("adapter") => finish(run_adapter(args)),
        Some("config") => finish(crate::cli_config::run_config(args)),
        other => {
            eprintln!(
                "nyd: alias expanded to unsupported command '{}' (aliases must expand to `adapter …`)",
                other.unwrap_or("")
            );
            ExitCode::FAILURE
        }
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
        Err(e) => return Some(Err(e.context(format!("expanding alias '{name}'")))),
    };
    let mut argv = Vec::with_capacity(expanded.len() + 1);
    argv.push(args.first().cloned().unwrap_or_else(|| "nyd".to_string()));
    argv.extend(expanded);
    Some(Ok(argv))
}

/// Handle an `adapter …` invocation: `args[1] == "adapter"`.
///
/// `adapter` on its own lists the configured instances; `adapter help` prints
/// the adapter-interface help; otherwise `args[2]` is the type path and the rest
/// is parsed + executed on a tokio runtime (adapter construction is sync, the
/// verbs async).
fn run_adapter(args: &[String]) -> Result<()> {
    let Some(path) = args.get(2) else {
        return list_instances();
    };
    if matches!(path.as_str(), "help" | "-h" | "--help") {
        print_adapter_help();
        return Ok(());
    }

    let inv = parse_adapter(args)?;

    let rt = tokio::runtime::Runtime::new().context("creating tokio runtime")?;
    rt.block_on(async move {
        let ctx = not_yet_done_host::host_context();
        let adapter: Arc<dyn ContentAdapter> =
            Arc::from(not_yet_done_host::resolve_adapter(&inv.instance, &ctx)?);
        // The adapter just connected (we built it): fire its `connected` hook.
        // No-op unless this instance's view file configures one; throttled via
        // the host state file, so e.g. an auto-backup runs at most once a day
        // however often the CLI is invoked. Best-effort — never blocks the verb.
        not_yet_done_host::fire_hook(adapter.as_ref(), &inv.instance, "connected").await;

        // `help` is answered from the adapter's static description — no
        // connection, no credentials, so it stays usable when the backend is
        // down or not configured yet.
        if inv.verb == "help" {
            return cmd_help(adapter.as_ref(), &inv).await;
        }

        // `queries` reads the two stores, which live next to the instance's
        // config — asking a locked or unreachable backend for credentials just
        // to print local file names would be a connection nobody asked for.
        if inv.verb == "queries" {
            return cmd_queries(adapter.as_ref(), &inv).await;
        }

        // Everything else talks to the backend. Watch the connection for the
        // rest of the command: report progress on stderr, ask for credentials
        // on the terminal, and give up loudly when neither is possible — see
        // `adapter_connect`.
        let mut sup = adapter_connect::Supervisor::start(Arc::clone(&adapter));
        // Addressing the root is what makes an adapter start connecting (the
        // TUI's `r` does the same thing). Cheap and side-effect free for the
        // adapters that are already connected.
        let a = Arc::clone(&adapter);
        sup.guard(async move { a.root().await.map_err(|e| anyhow!("{e}")) })
            .await
            .with_context(|| format!("connecting to '{}'", inv.instance))?;
        // Only then run the verb: an adapter that builds its connection in the
        // background would otherwise serve an empty snapshot, and an empty
        // list is indistinguishable from "there is nothing there".
        sup.guard(adapter_connect::wait_until_connected(adapter.as_ref()))
            .await
            .with_context(|| format!("connecting to '{}'", inv.instance))?;

        let verb = async {
            match inv.verb.as_str() {
                "ls" | "list" => cmd_ls(adapter.as_ref(), &inv).await,
                "show" | "get" => cmd_show(adapter.as_ref(), &inv).await,
                "cat" | "read" => cmd_cat(adapter.as_ref(), &inv).await,
                "actions" => cmd_actions(adapter.as_ref(), &inv).await,
                "values" => cmd_values(adapter.as_ref(), &inv).await,
                "do" => cmd_do(adapter.as_ref(), &inv).await,
                other => Err(anyhow!("unknown command '{other}'")),
            }
        };
        sup.guard(verb).await
    })
}

/// The concise top-level help, printed for no-args / `help`. Lists only the
/// top-level commands; the adapter interface (instances, level commands,
/// examples) is documented one level down under `adapter help`.
fn print_top_level_help() {
    const PROG: &str = "not-yet-done-cli";
    println!("not-yet-done — task & time tracking\n");
    println!("Usage:");
    println!("  {PROG} <command> …\n");
    println!("Commands:");
    println!("  adapter <…>   work with adapter instances (tasks, jira, trackings, …)");
    println!("  tag <…>       manage tags");
    println!("  backup <…>    create a backup");
    println!("  config <…>    edit config files, `config generate <inst>`,");
    println!("                `config build <type>` (interactive connection config),");
    println!("                `config template <type>` (static skeleton),");
    println!("                or `config auth <type>` (the type's auth mechanisms)");
    println!("  help          show this help");
    println!();
    println!("Run `{PROG} adapter help` for the adapter interface,");
    println!("or `{PROG} adapter` to list the configured instances.");
}

/// The adapter-interface help, printed for `adapter help`. Documents the type
/// path grammar, enumerates the configured instances, and lists the commands
/// available at any level — the entry point everything adapter-side hangs off.
fn print_adapter_help() {
    const PROG: &str = "not-yet-done-cli";
    println!("not-yet-done — adapter interface\n");
    println!("Usage:");
    println!("  {PROG} adapter <inst>[:child…] [ID] <command> [flags]");
    println!("  {PROG} adapter                    list configured instances");
    println!();
    println!("Adapter instances (address as `adapter <instance>[:child…]`):");
    print_instance_lines("  ");
    println!();
    println!("Commands at a level (the last positional):");
    println!("  help [--full]       how to drive this level (--full: capabilities, sort columns)");
    println!("  ls                  list children (of the root, or of a node given by id)");
    println!("  show | cat          print a node's row / its raw content");
    println!("  queries             list the stored queries (`ls --query-name NAME` runs one)");
    println!("  actions             list the actions available here");
    println!("  values <source>     list the option values for a source key");
    println!("  <action> [flags]    run an adapter action (e.g. delete, edit_markdown)");
    println!();
    println!("Examples:");
    println!("  {PROG} adapter jira help");
    println!("  {PROG} adapter jira:issue:comment help");
    println!("  {PROG} adapter tasks add -m 'buy milk'");
    println!("  {PROG} adapter jira:issue:comment <comment-id> delete --yes");
}

/// List the configured adapter instances — the target of bare `adapter`.
fn list_instances() -> Result<()> {
    let instances = not_yet_done_host::discover_instances();
    if instances.is_empty() {
        println!(
            "no adapter instances configured (add a views/*.yaml with `tab:` + `adapter:` keys)"
        );
    } else {
        print_instance_lines("");
    }
    Ok(())
}

/// Print one `<instance>  (type: <adapter_type>)` line per discovered instance,
/// id column aligned, each prefixed by `indent`.
fn print_instance_lines(indent: &str) {
    let instances = not_yet_done_host::discover_instances();
    if instances.is_empty() {
        println!("{indent}(none configured)");
        return;
    }
    let width = instances
        .iter()
        .map(|d| d.instance_id().len())
        .max()
        .unwrap_or(0);
    for d in &instances {
        println!(
            "{indent}{:width$}  (type: {})",
            d.instance_id(),
            d.adapter.adapter_type,
            width = width
        );
    }
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

/// Framework verbs — the command names handled by the content layer / generic
/// front-end rather than routed to the adapter as an action. Any command not in
/// this set is treated as an adapter action id.
fn is_framework_verb(cmd: &str) -> bool {
    matches!(
        cmd,
        "help" | "ls" | "list" | "show" | "get" | "cat" | "read" | "actions" | "values" | "queries"
    )
}

/// The local (unqualified) name of a type id — the part after the last `:`, so
/// `jira:comment` → `comment`, `tracking:tree-group` → `tree-group`. Type-path
/// segments match against this (or the full type id).
fn type_local_name(type_id: &str) -> &str {
    type_id.rsplit(':').next().unwrap_or(type_id)
}

/// Parse an `adapter <inst>[:child…] [ID] <command> [flags]` invocation.
/// `args[2]` is the type path; flags start at `args[3]`.
///
/// The command is the last bare positional; the one before it (if any) is the
/// node id / value source. More than one leading positional is an error — a
/// node is addressed by its (self-contained) id, not an id chain.
fn parse_adapter(args: &[String]) -> Result<Invocation> {
    let mut segments = args[2].split(':').map(str::to_string);
    let instance = segments
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("empty adapter path — expected <instance>[:child…]"))?;
    let child_path: Vec<String> = segments.collect();

    let mut positionals: Vec<String> = Vec::new();
    let mut type_filter = None;
    let mut query = None;
    let mut query_name = None;
    let mut vars: Vec<(String, String)> = Vec::new();
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
    let mut full = false;

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
            "--query-name" => query_name = Some(take_value(&mut i, "--query-name")?),
            "--var" => {
                let kv = take_value(&mut i, "--var")?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| anyhow!("--var expects name=value, got '{kv}'"))?;
                vars.push((k.to_string(), v.to_string()));
            }
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
            "--full" => full = true,
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

    // One query per listing. Silently preferring one of the two would run a
    // query the command line does not read as asking for.
    if query.is_some() && query_name.is_some() {
        return Err(anyhow!(
            "--query and --query-name are mutually exclusive — pass a body or the name of a stored one"
        ));
    }

    // The command is the last bare positional; an optional single positional
    // before it is the node id (or value source for `values`). A bare
    // `adapter <path>` with no command documents the level (`help`).
    let command = positionals.pop();
    let id = positionals.pop();
    if !positionals.is_empty() {
        return Err(anyhow!(
            "too many positionals — expected `[ID] <command>` (a node is addressed by one self-contained id)"
        ));
    }
    let command = command.unwrap_or_else(|| "help".to_string());

    // Map the command onto the verb + positional layout the handlers expect.
    // Framework verbs read a single leading id (or source); an adapter action
    // reuses the `do` handler, which wants `[action, id]`.
    let (verb, positionals) = if is_framework_verb(&command) {
        (command, id.into_iter().collect())
    } else {
        let mut p = vec![command];
        p.extend(id);
        ("do".to_string(), p)
    };

    Ok(Invocation {
        instance,
        child_path,
        verb,
        positionals,
        type_filter,
        query,
        query_name,
        vars,
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
        full,
    })
}

/// `help` — document a level. By default it prints the compact CLI-usage view
/// (`print_level_usage`): how to drive *this* level. `--full` swaps in the
/// content layer's complete Markdown reference (capabilities, child types, sort
/// columns). With an id (or `--path`) it documents the concrete node's level;
/// otherwise the level named by the type path alone, fetching nothing (the
/// id-free path the `childs` refactor enables).
async fn cmd_help(adapter: &dyn ContentAdapter, inv: &Invocation) -> Result<()> {
    if let Some(mut node) =
        resolve_target(adapter, inv, inv.positionals.first().map(String::as_str)).await?
    {
        node.hydrate().await;
        if inv.full {
            print_markdown(
                &not_yet_done_content::render_level(adapter, node.as_ref(), false).await,
            );
        } else {
            let actions = not_yet_done_content::level_actions(adapter, node.as_ref());
            let kids: Vec<String> = adapter
                .childs(node.as_ref())
                .iter()
                .map(|c| type_local_name(&c.node_type.type_id).to_string())
                .collect();
            print_level_usage(inv, &actions, &kids, false);
        }
    } else {
        let (nt, is_root) = resolve_type_level(adapter, &inv.child_path).await?;
        if inv.full {
            print_markdown(
                &not_yet_done_content::render_level_for_type(adapter, &nt, is_root).await,
            );
        } else {
            let actions = not_yet_done_content::level_actions_for_type(adapter, &nt);
            let kids: Vec<String> = not_yet_done_content::child_types_of_type(adapter, &nt)
                .iter()
                .map(|k| type_local_name(&k.type_id).to_string())
                .collect();
            print_level_usage(inv, &actions, &kids, is_root);
        }
    }
    Ok(())
}

/// The default `help` view: how to drive *this level* from the CLI — its
/// address, the read verbs, the concrete actions here (each with the flag that
/// feeds its input), and how to descend into child levels. Copy-pasteable full
/// commands; `--full` swaps in the content layer's complete reference.
fn print_level_usage(
    inv: &Invocation,
    actions: &[NodeAction],
    child_locals: &[String],
    is_root: bool,
) {
    const PROG: &str = "not-yet-done-cli";
    let addr = if inv.child_path.is_empty() {
        inv.instance.clone()
    } else {
        format!("{}:{}", inv.instance, inv.child_path.join(":"))
    };
    let base = format!("{PROG} adapter {addr}");

    println!("Usage — adapter `{addr}`\n");

    println!("Inspect (read-only):");
    println!("  {base} ls               list children of the root");
    println!("  {base} <id> ls          list a node's children");
    println!("  {base} <id> show        print a node's row");
    println!("  {base} <id> cat         print its raw content");
    println!("  {base} queries          list the stored queries of this instance");
    println!("  {base} actions          list the actions here");
    println!();
    println!("Filter a listing:");
    println!("  ls -q '<body>'          a query in this adapter's own language");
    println!("  ls --query-name NAME    a stored query (`queries`); may be an extended document");
    println!("  ls --var name=value     bind a query variable (repeatable)");
    println!("  {base} help --full      full reference (capabilities, sort columns)");
    println!();

    // Adapter actions — drop the built-in `help` (covered by the line above).
    let acts: Vec<&NodeAction> = actions.iter().filter(|a| a.id != "help").collect();
    if !acts.is_empty() {
        if is_root {
            println!("Actions here (the action is the last word):");
        } else {
            println!("Actions here (a node id comes before the action):");
        }
        let w = acts.iter().map(|a| a.id.len()).max().unwrap_or(0);
        let target = if is_root { "" } else { "<id> " };
        for a in &acts {
            println!(
                "  {base} {target}{id:<w$}   {label}  [{hint}]",
                id = a.id,
                label = a.label,
                hint = action_flag_hint(a),
                w = w,
            );
        }
        println!();
    }

    if child_locals.is_empty() {
        println!("Descend into: leaf level — nothing below.");
    } else {
        println!("Descend into child levels:");
        for c in child_locals {
            println!("  {base}:{c} help");
        }
    }
}

/// The CLI flag that feeds an action's input, keyed by its [`InputSpec`]. Shown
/// per action in [`print_level_usage`] so the user knows how to supply input.
fn action_flag_hint(a: &NodeAction) -> &'static str {
    use not_yet_done_content::InputSpec;
    match &a.input {
        InputSpec::None => "no input; --yes to skip a confirm",
        InputSpec::Editor => "editor: -m TEXT | --file FILE (else $EDITOR)",
        InputSpec::Picker => "picker: --value V (see `values`)",
        InputSpec::FilePicker { multi: true } => "files: --file PATH (repeatable)",
        InputSpec::FilePicker { multi: false } => "file: --file PATH",
        InputSpec::Form { .. } | InputSpec::ColumnForm => "form: --field K=V (repeatable)",
    }
}

/// Walk the child-type path from the adapter root, resolving each segment
/// against the child types of the current level (by local name or full type
/// id). Returns the resolved [`NodeType`] and whether it is the root level.
async fn resolve_type_level(
    adapter: &dyn ContentAdapter,
    child_path: &[String],
) -> Result<(NodeType, bool)> {
    let root = adapter.root().await?;
    let mut nt = root.node_type().clone();
    for seg in child_path {
        let kids = not_yet_done_content::child_types_of_type(adapter, &nt);
        match kids
            .iter()
            .find(|k| type_local_name(&k.type_id) == seg.as_str() || &k.type_id == seg)
        {
            Some(found) => nt = found.clone(),
            None => {
                let avail: Vec<&str> = kids.iter().map(|k| type_local_name(&k.type_id)).collect();
                return Err(anyhow!(
                    "no child '{seg}' at level '{}' (available: {})",
                    nt.type_id,
                    avail.join(", ")
                ));
            }
        }
    }
    Ok((nt, child_path.is_empty()))
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
                ));
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

    let child_types = children::child_types(adapter, parent.as_ref());
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

    // A query is either typed here or named in one of the stores; the store
    // decides its kind, the way the TUI's merged menu list does.
    let query = match (&inv.query_name, &inv.query) {
        (Some(name), _) => Some(adapter_query::load(adapter, name).await?),
        (None, Some(text)) => Some(adapter_query::StoredQuery {
            text: text.clone(),
            kind: not_yet_done_content::QueryKind::Saved,
        }),
        (None, None) => None,
    };
    let bindings = adapter_query::bindings(&inv.vars);

    if let Some(document) = query
        .as_ref()
        .filter(|q| q.kind == not_yet_done_content::QueryKind::Extended)
    {
        // A subtree is one adapter call for every level at once, and an
        // extended document is not one query — the TUI has the same limit and
        // loads such a tree level by level.
        if inv.tree {
            return Err(anyhow!(
                "--tree cannot run an extended query — list one level at a time"
            ));
        }
        let res = adapter_query::run_extended(
            adapter,
            parent.as_ref(),
            node_type.clone(),
            &document.text,
            &bindings,
            &inv.sort,
            inv.group_by.clone(),
        )
        .await?;
        output_list(&res.items, inv.output);
        return Ok(());
    }

    let params = ListParams {
        node_type: node_type.clone(),
        query: query
            .as_ref()
            .map(|q| adapter_query::render_native(adapter, &q.text, &bindings))
            .transpose()?,
        sort: inv.sort.clone(),
        page: None,
        download: false,
        group_by: inv.group_by.clone(),
    };

    if inv.tree {
        let depth = inv.depth.unwrap_or(u32::MAX);
        let sub = children::list_subtree(adapter, parent.as_ref(), params, depth).await?;
        output_tree(&sub, inv.output);
    } else {
        let res = children::list(adapter, parent.as_ref(), params).await?;
        output_list(&res.items, inv.output);
    }
    Ok(())
}

/// `queries` — the stored queries of this adapter instance, both kinds.
///
/// The discovery half of `--query-name`: without it the names live only on
/// disk (and in the TUI's menu), and a script has nothing to name.
async fn cmd_queries(adapter: &dyn ContentAdapter, inv: &Invocation) -> Result<()> {
    let entries = adapter_query::list(adapter).await?;
    if entries.is_empty() {
        if inv.output == Output::Json {
            println!("[]");
        } else {
            println!("(no stored queries)");
        }
        return Ok(());
    }
    match inv.output {
        Output::Json => {
            let rows: Vec<serde_json::Value> = entries
                .iter()
                .map(|(name, kind)| serde_json::json!({ "name": name, "kind": kind.as_str() }))
                .collect();
            println!("{}", serde_json::to_string_pretty(&rows)?);
        }
        Output::Table => {
            let w = entries.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
            for (name, kind) in &entries {
                println!("{name:<w$}  {kind}", w = w);
            }
        }
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
    // Route through the content layer's `level_actions*` seam so the built-in
    // `help` action shows up alongside the adapter's own actions on every
    // level, without the adapter declaring it.
    let actions: Vec<NodeAction> = if let Some(id) = inv.positionals.first() {
        let node = resolve_node(adapter, id).await?;
        not_yet_done_content::level_actions(adapter, node.as_ref())
    } else if let Some(t) = &inv.type_filter {
        let nt = find_node_type(adapter, t).await?;
        not_yet_done_content::level_actions_for_type(adapter, &nt)
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

    // Was a target id given, or do we fall back to the adapter root? The root
    // is where `help` documents the adapter-wide capabilities.
    let target_id = inv.positionals.get(1).map(String::as_str);
    let is_root = target_id.is_none();
    let mut node: Box<dyn Node> = match resolve_target(adapter, inv, target_id).await? {
        Some(node) => node,
        None => adapter.root().await?,
    };

    // Framework built-in actions (currently just `help`) are handled by the
    // content layer, not the adapter: `help` documents the current level as
    // Markdown, rendered to the terminal when stdout is a TTY (raw Markdown
    // otherwise, so `> file.md` / pipes stay clean).
    if not_yet_done_content::is_builtin(&action_id) {
        if let Some(ActionOutcome::Done { message }) =
            not_yet_done_content::run_builtin(&action_id, adapter, node.as_ref(), is_root).await
        {
            print_markdown(&message.unwrap_or_default());
        }
        return Ok(());
    }

    // Resolve the action so we know its input shape, from the adapter's
    // type-level set — the single source of truth the TUI's shortcut hints and
    // id-free help share.
    let action = find_action(node.as_ref(), adapter, &action_id).ok_or_else(|| {
        let avail: Vec<String> = adapter
            .actions_for_type(node.node_type())
            .into_iter()
            .map(|a| a.id)
            .collect();
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
        InputSpec::ColumnForm => {
            // Fields are the columns the adapter describes for this node's
            // type; map them the same way every front-end does, then reuse the
            // ordinary form path.
            let node_type = node.node_type().type_id.clone();
            let fields: Vec<not_yet_done_content::FormFieldSpec> = adapter
                .describe_columns(&node_type)
                .await
                .iter()
                .map(|c| c.to_form_field())
                .collect();
            do_form(node.as_mut(), &action_id, &fields, inv).await
        }
        InputSpec::Picker => do_picker(node.as_mut(), &action_id, inv).await,
        InputSpec::FilePicker { multi } => do_files(node.as_mut(), &action_id, multi, inv).await,
        InputSpec::None => do_dispatch(node.as_mut(), &action_id, inv).await,
    }
}

/// Find an action by id from the adapter's type-level
/// [`ContentAdapter::actions_for_type`] — the sole action-set source.
fn find_action(node: &dyn Node, adapter: &dyn ContentAdapter, id: &str) -> Option<NodeAction> {
    adapter
        .actions_for_type(node.node_type())
        .into_iter()
        .find(|a| a.id == id)
}

/// `InputSpec::Editor`: seed a buffer from [`Node::prepare`], let the user
/// fill it, then [`Node::execute`] with [`ActionInput::Edited`]. The template
/// is passed back as `original` so the adapter can diff/merge exactly as it
/// does for the TUI editor session.
///
/// The edited text is taken, in order: `-m -` reads stdin; `-m <text>` uses the
/// inline value; `--file <path>` reads that file; otherwise `$EDITOR` is
/// launched. The stdin/file paths make it practical to feed a whole document
/// (e.g. `jira do from_markdown KEY --file ticket.md`) without shell-quoting it.
async fn do_editor(node: &mut dyn Node, action_id: &str, inv: &Invocation) -> Result<()> {
    let prep = node
        .prepare(action_id)
        .await
        .with_context(|| format!("preparing editor for '{action_id}'"))?;
    let text = match (&inv.message, inv.files.first()) {
        (Some(m), _) if m == "-" => read_stdin_to_string()?,
        (Some(m), _) => m.clone(),
        (None, Some(path)) => std::fs::read_to_string(path)
            .with_context(|| format!("reading edited text from {}", path.display()))?,
        (None, None) => edit_in_editor(&prep.template, &prep.suffix)?,
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
        return Err(anyhow!(
            "action '{action_id}' needs at least one --file <path>"
        ));
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
        ActionDispatch::Confirm { prompt } => {
            Err(anyhow!("{prompt}\n  (re-run with --yes to confirm)"))
        }
        // The adapter wants the frontend's delete-confirm flow, which on
        // "yes" runs `execute(action, None)`. We mirror that: `--yes` →
        // perform the delete; otherwise surface the (adapter-authored) prompt.
        ActionDispatch::DeleteSelf { confirm } => {
            if !inv.yes {
                let prompt = confirm.unwrap_or_else(|| format!("Delete '{}'? (y/n)", node.label()));
                return Err(anyhow!("{prompt}\n  (re-run with --yes to confirm)"));
            }
            let outcome = node.execute(action_id, ActionInput::None).await?;
            report_outcome(outcome, action_id)
        }
        // Interactive-only dispatches: they drive a UI flow (editor
        // session, paginated result pane) the CLI can't stand in for.
        ActionDispatch::OpenEditor { session_kind, .. } => Err(anyhow!(
            "action '{action_id}' opens an interactive '{session_kind}' editor — use the TUI"
        )),
        ActionDispatch::ExecuteQuery { .. } => Err(anyhow!(
            "action '{action_id}' runs a query result pane — use the TUI"
        )),
    }
}

/// Print Markdown to stdout: rendered with termimad when stdout is a terminal
/// (headers, lists, inline code get styling/colour), raw otherwise so piping
/// or redirecting (`help > level.md`) keeps clean, parseable Markdown.
fn print_markdown(md: &str) {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        termimad::print_text(md);
    } else {
        print!("{md}");
        if !md.ends_with('\n') {
            println!();
        }
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
        ActionOutcome::Reopen { content, .. } => {
            Err(anyhow!("'{action_id}' rejected the input:\n{content}"))
        }
        ActionOutcome::OpenEditor { action_id: next } => Err(anyhow!(
            "'{action_id}' opens an interactive editor for '{next}' — use the TUI"
        )),
    }
}

/// Read all of stdin into a string — the `-m -` editor-input source.
fn read_stdin_to_string() -> Result<String> {
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("reading edited text from stdin")?;
    Ok(buf)
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
        (Some(_), Some(_)) => Err(anyhow!("give a node id or --path, not both")),
        (Some(p), None) => Ok(Some(resolve_path(adapter, p, inv.case_insensitive).await?)),
        (None, Some(id)) => Ok(Some(resolve_node(adapter, id).await?)),
        (None, None) => Ok(None),
    }
}

/// A `--path` segment matcher: a literal substring, or a compiled regex when
/// the segment was written `re:<pattern>`. Case folding (from `-i`) is baked
/// in at construction so matching is a plain predicate.
enum SegMatch {
    Substring {
        needle: String,
        case_insensitive: bool,
    },
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
                needle: if case_insensitive {
                    seg.to_lowercase()
                } else {
                    seg.to_string()
                },
                case_insensitive,
            })
        }
    }

    fn matches(&self, label: &str) -> bool {
        match self {
            SegMatch::Substring {
                needle,
                case_insensitive,
            } => {
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
        for nt in children::child_types(adapter, current.as_ref()) {
            let params = ListParams {
                node_type: nt,
                query: None,
                sort: Vec::new(),
                page: None,
                download: false,
                group_by: None,
            };
            for item in children::list(adapter, current.as_ref(), params)
                .await?
                .items
            {
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
                    .map_err(|e| anyhow!("{e}"));
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
    if let Some(nt) = children::child_types(adapter, root.as_ref())
        .into_iter()
        .find(|nt| nt.type_id == type_id)
    {
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
    for nt in children::child_types(adapter, root.as_ref()) {
        let params = ListParams {
            node_type: nt,
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        };
        let sub = children::list_subtree(adapter, root.as_ref(), params, depth).await?;
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
        out.push(format!(
            "{indent}{}  [{}]",
            node.summary.label,
            short(&node.summary.id)
        ));
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
                let cols = vec![
                    "field".to_string(),
                    "value".to_string(),
                    "editable".to_string(),
                ];
                let rows: Vec<Vec<String>> = node
                    .metadata()
                    .fields
                    .iter()
                    .map(|f| {
                        vec![
                            f.display_label.clone(),
                            f.value.clone(),
                            if f.editable {
                                "yes".into()
                            } else {
                                "no".into()
                            },
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
                    })
                })
                .collect();
            print_json(&serde_json::Value::Array(arr));
        }
        Output::Table => {
            let cols = vec!["id".to_string(), "label".to_string(), "input".to_string()];
            let rows: Vec<Vec<String>> = actions
                .iter()
                .map(|a| vec![a.id.clone(), a.label.clone(), input_kind(a)])
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
                .map(|v| {
                    let mut obj = serde_json::Map::new();
                    obj.insert("value".into(), v.value.clone().into());
                    obj.insert("label".into(), v.label.clone().into());
                    if !v.extra.is_empty() {
                        let extra: serde_json::Map<String, serde_json::Value> = v
                            .extra
                            .iter()
                            .map(|(k, val)| (k.clone(), val.clone().into()))
                            .collect();
                        obj.insert("extra".into(), serde_json::Value::Object(extra));
                    }
                    serde_json::Value::Object(obj)
                })
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
            if *multi {
                "files".into()
            } else {
                "file".into()
            }
        }
        InputSpec::Form { fields } => format!("form({})", fields.len()),
        InputSpec::ColumnForm => "form(columns)".into(),
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
                let pad = widths
                    .get(i)
                    .copied()
                    .unwrap_or(0)
                    .saturating_sub(c.chars().count());
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
        let args: Vec<String> = [
            "nyd",
            "adapter",
            "tasks",
            "abc123",
            "ls",
            "--type",
            "task:item",
            "--tree",
            "--depth",
            "2",
            "-o",
            "json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let inv = parse_adapter(&args).unwrap();
        assert_eq!(inv.instance, "tasks");
        assert_eq!(inv.verb, "ls");
        assert_eq!(inv.positionals, vec!["abc123".to_string()]);
        assert_eq!(inv.type_filter.as_deref(), Some("task:item"));
        assert!(inv.tree);
        assert_eq!(inv.depth, Some(2));
        assert!(matches!(inv.output, Output::Json));
    }

    #[test]
    fn parse_reads_a_named_query_with_its_variable_bindings() {
        let args: Vec<String> = [
            "nyd",
            "adapter",
            "jira",
            "ls",
            "--query-name",
            "my board",
            "--var",
            "who=me",
            "--var",
            "when=today",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let inv = parse_adapter(&args).unwrap();
        assert_eq!(inv.verb, "ls");
        assert_eq!(inv.query_name.as_deref(), Some("my board"));
        assert_eq!(inv.query, None);
        assert_eq!(
            inv.vars,
            vec![
                ("who".to_string(), "me".to_string()),
                ("when".to_string(), "today".to_string()),
            ]
        );
    }

    #[test]
    fn a_query_body_and_a_query_name_together_are_refused() {
        let args: Vec<String> = [
            "nyd",
            "adapter",
            "jira",
            "ls",
            "-q",
            "assignee = me",
            "--query-name",
            "mine",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let err = parse_adapter(&args)
            .err()
            .expect("a body and a name together must not parse")
            .to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn queries_is_a_framework_verb_not_an_adapter_action() {
        let args: Vec<String> = ["nyd", "adapter", "jira", "queries"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let inv = parse_adapter(&args).unwrap();
        assert_eq!(inv.verb, "queries");
        assert!(inv.positionals.is_empty());
    }

    #[test]
    fn parse_do_reads_action_node_and_input_flags() {
        let args: Vec<String> = [
            "nyd", "adapter", "tasks", "abc123", "edit", "-m", "new body", "--yes",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let inv = parse_adapter(&args).unwrap();
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
            "nyd",
            "adapter",
            "pg",
            "create",
            "--field",
            "name=report",
            "--field",
            "db=live",
            "--value",
            "v1",
            "--text",
            "hello",
            "--file",
            "/tmp/a.sql",
            "--file",
            "/tmp/b.sql",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let inv = parse_adapter(&args).unwrap();
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
        let args: Vec<String> = ["nyd", "adapter", "pg", "create", "--field", "noeq"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse_adapter(&args).is_err());
    }

    #[test]
    fn depth_without_tree_implies_tree() {
        let args: Vec<String> = ["nyd", "adapter", "tasks", "ls", "--depth", "1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let inv = parse_adapter(&args).unwrap();
        assert!(inv.tree);
        assert_eq!(inv.depth, Some(1));
    }

    #[test]
    fn unknown_flag_errors() {
        let args: Vec<String> = ["nyd", "adapter", "tasks", "ls", "--bogus"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse_adapter(&args).is_err());
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
            "nyd",
            "adapter",
            "trk",
            "ls",
            "--group-by",
            "started_at:day",
            "--path",
            "/Work/Report",
            "-i",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let inv = parse_adapter(&args).unwrap();
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
