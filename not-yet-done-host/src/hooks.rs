//! Hook subsystem — declarative, throttled action invocation on adapter
//! lifecycle events (D5).
//!
//! A *hook* is a named point in an adapter's lifetime ([`ContentAdapter::hooks`]
//! declares which names it fires). A front-end turns a hook into work purely
//! through configuration: the instance's view file carries a `hooks:` block that
//! binds each hook id to one or more *action invocations*:
//!
//! ```yaml
//! hooks:
//!   connected:
//!     - run: backup            # adapter action id to invoke
//!       on: { }                # target: root (default) | { id: <node-id> } | { query: <q> }
//!       with: { }              # ActionContext inputs: value / text / args: { k: v }
//!       when: { throttle: 24h } # fire at most once per window
//!   tracking_started:
//!     - script: autotrack.py   # a view-script instead of an action
//! ```
//!
//! A binding names **either** an adapter action (`run:`) **or** a script
//! (`script:`). A script binding runs an executable file from the instance's
//! view-script directory (`<data_dir>/not_yet_done/scripts/<adapter type>/`; a
//! name containing `/` is taken relative to the scripts root, an absolute path
//! as is) and hands it the hook's payload as a JSON file in `argv[1]` — see
//! [`resolve_script_path`] and [`run_script`].
//!
//! This generalises the hard-coded daily backup: binding `backup` to the local
//! adapter's `connected` hook (fired on every program start) with a 24h throttle
//! backs the database up once a day on first use — but the same machinery works
//! for any adapter, any action, any cadence, with no front-end code change.
//!
//! **Where it runs.** The host owns hook firing so *both* the TUI and the CLI
//! (and any future front-end) inherit it from the one crate that builds
//! adapters. Entry points:
//!
//!   * [`fire_hook`] — fire a named hook against an **already-built** adapter
//!     (the CLI calls this right after [`crate::resolve_adapter`], reusing the
//!     adapter it built for the command).
//!   * [`fire_connected_hooks`] — the startup helper the TUI calls: it checks
//!     the throttle *before* building anything, so within the throttle window no
//!     adapter is constructed at all (it would otherwise pay an idle DB open on
//!     every launch). Only instances with a *due* `connected` binding are
//!     resolved and fired.
//!   * **Event hooks** — hooks an adapter fires *itself* while it runs (e.g.
//!     the local adapter's `tracking_started` / `tracking_stopped`). The
//!     adapter publishes a [`BusEvent`] whose topic is the hook name and whose
//!     source is its instance id; the host turns every such event into a hook
//!     firing with the event's payload. A long-lived front-end (the TUI) starts
//!     one [`spawn_event_hook_runner`] per built adapter; a one-shot front-end
//!     (the CLI) subscribes before its command and [`drain_event_hooks`] after
//!     it. Bindings run **sequentially and are awaited** (each script gets
//!     [`SCRIPT_TIMEOUT`]), so a stop fired before a start is delivered in that
//!     order. A hook script that drives the CLI would fire the same hooks
//!     again in the child process; the [`HOOK_DEPTH_ENV`] variable cuts that
//!     recursion — a process started by a hook script runs no event hooks.
//!
//! **Throttle state** is a single host-level JSON file
//! `~/.local/state/not_yet_done/hooks.json` (XDG state dir), adapter-independent
//! and shared across front-ends, mapping `"<instance>:<hook>:<action>"` to the
//! last-fire timestamp. A binding with no `throttle:` fires every time and is
//! never stamped.
//!
//! The subsystem is strictly best-effort: a malformed config, an unknown hook
//! name, a failing action, or an unwritable state file never aborts the caller —
//! failures go to stderr (prefix `nyd-hooks:`) and are surfaced in the returned
//! [`HookReport`]s for tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Weak;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use not_yet_done_content::{
    ActionArgs, ActionContext, ActionDispatch, BusEvent, ContentAdapter, HostEvent, HostEventBus,
};

// ---------------------------------------------------------------------------
// Config schema (the `hooks:` block of a view file)
// ---------------------------------------------------------------------------

/// The `hooks:` block: hook id → its ordered list of bindings.
pub type HookConfig = HashMap<String, Vec<HookBinding>>;

/// One declarative invocation bound to a hook: an adapter action (`run:`) or
/// a script (`script:`) — exactly one of the two.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookBinding {
    /// The adapter action id to invoke when the hook fires.
    #[serde(default)]
    pub run: Option<String>,
    /// The script to execute when the hook fires, instead of an action. A bare
    /// file name lives in the instance's view-script directory; see
    /// [`resolve_script_path`]. The script receives the hook payload as a JSON
    /// file in `argv[1]` (see [`run_script`]).
    #[serde(default)]
    pub script: Option<String>,
    /// Which node to invoke it on. Default: the adapter root.
    #[serde(default)]
    pub on: HookTarget,
    /// Inputs threaded into the action's [`ActionContext`].
    #[serde(default)]
    pub with: HookInputs,
    /// Firing conditions (currently just an optional throttle).
    #[serde(default)]
    pub when: HookWhen,
}

/// What a binding invokes, once the YAML has been checked for exactly one of
/// `run:` / `script:`.
enum BindingKind<'a> {
    Action(&'a str),
    Script(&'a str),
}

impl HookBinding {
    /// The name this binding is reported and throttle-stamped under: the
    /// action id, or `script:<name>`.
    pub fn label(&self) -> String {
        match (&self.run, &self.script) {
            (Some(run), _) => run.clone(),
            (None, Some(script)) => format!("script:{script}"),
            (None, None) => "<unbound>".into(),
        }
    }

    fn kind(&self) -> Result<BindingKind<'_>, String> {
        match (&self.run, &self.script) {
            (Some(_), Some(_)) => {
                Err("a binding names either `run:` or `script:`, not both".into())
            }
            (None, None) => Err("a binding needs `run: <action>` or `script: <file>`".into()),
            (Some(run), None) => Ok(BindingKind::Action(run)),
            (None, Some(script)) => Ok(BindingKind::Script(script)),
        }
    }
}

/// The target node for a hook's action.
///
/// Modelled as two optional fields rather than an enum so the YAML is forgiving:
/// `on:` may be omitted entirely (→ root), `on: { id: <node-id> }`, or
/// `on: { query: <q> }` (root node, with the query set in the [`ActionContext`]
/// so a set-scoped action operates on the matching set).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookTarget {
    /// Invoke on this specific node id (via `get_by_id`) instead of the root.
    #[serde(default)]
    pub id: Option<String>,
    /// Active-query string handed to the action (for set-scoped actions).
    #[serde(default)]
    pub query: Option<String>,
}

/// Action inputs carried into [`ActionContext`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookInputs {
    /// Selected-value input (e.g. an option id for a toggle action).
    #[serde(default)]
    pub value: Option<String>,
    /// Free-text input (e.g. a new name for a create/rename action).
    #[serde(default)]
    pub text: Option<String>,
    /// Named arguments (`args: { key: value, … }`), the general channel; a
    /// hook has no row, so no `{placeholder}` is expanded here. Checked
    /// against the action's declared parameters before it runs.
    #[serde(default)]
    pub args: ActionArgs,
}

/// When a binding is allowed to fire.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookWhen {
    /// Minimum gap between fires, e.g. `24h`, `30m`, `90s`, `7d`. Absent → fire
    /// every time the hook fires (no throttle, no state stamp).
    #[serde(default)]
    pub throttle: Option<String>,
}

/// Parse a throttle duration through the crate's one interval spelling
/// ([`crate::parse_interval`]): an integer followed by `s`/`m`/`h`/`d`. Only
/// the type differs — hook state is timestamped with chrono, so the std
/// duration is converted here.
fn parse_throttle(spec: &str) -> Result<Duration, String> {
    let std = crate::parse_interval(spec)?;
    Duration::from_std(std).map_err(|_| format!("throttle '{spec}' is out of range"))
}

// ---------------------------------------------------------------------------
// Throttle state file
// ---------------------------------------------------------------------------

/// The host-level hook state file: `~/.local/state/not_yet_done/hooks.json`.
/// Falls back to the data-local dir, then the temp dir, on platforms without a
/// state dir — the file is pure cache (a missing or unreadable file just means
/// "never fired", so a hook fires once and re-stamps).
fn state_path() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("not_yet_done")
        .join("hooks.json")
}

fn state_key(instance: &str, hook: &str, action: &str) -> String {
    format!("{instance}:{hook}:{action}")
}

fn load_state() -> HashMap<String, DateTime<Utc>> {
    let path = state_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_state(state: &HashMap<String, DateTime<Utc>>) -> Result<(), String> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Is this binding throttled right now (last fire too recent)? A binding with
/// no throttle is never throttled. Reads the shared state file each call so
/// front-ends see each other's stamps.
fn is_throttled(instance: &str, hook: &str, binding: &HookBinding, now: DateTime<Utc>) -> bool {
    let Some(spec) = &binding.when.throttle else {
        return false;
    };
    let window = match parse_throttle(spec) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("nyd-hooks: {instance}:{hook}: {e} — firing anyway");
            return false;
        }
    };
    let state = load_state();
    match state.get(&state_key(instance, hook, &binding.label())) {
        Some(last) => now.signed_duration_since(*last) < window,
        None => false,
    }
}

fn stamp(instance: &str, hook: &str, action: &str, now: DateTime<Utc>) {
    let mut state = load_state();
    state.insert(state_key(instance, hook, action), now);
    if let Err(e) = save_state(&state) {
        eprintln!("nyd-hooks: could not persist throttle state: {e}");
    }
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

/// The outcome of processing one hook binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// The action ran and succeeded; the optional message is its notification.
    Fired(Option<String>),
    /// Skipped because the throttle window has not elapsed.
    Throttled,
    /// Skipped before invocation (unknown hook, unsupported by the adapter, …).
    Skipped(String),
    /// The action was invoked but failed (or returned an unusable dispatch).
    Failed(String),
}

/// A per-binding report, returned for observability and tests.
#[derive(Debug, Clone)]
pub struct HookReport {
    pub instance: String,
    pub hook: String,
    pub action: String,
    pub outcome: HookOutcome,
}

// ---------------------------------------------------------------------------
// Firing
// ---------------------------------------------------------------------------

/// Build the [`ActionContext`] a binding invokes its action with. Hooks have no
/// interactive prompt, so `confirmed` is always `true` — a hook author opting in
/// to a confirm-gated action (e.g. a destructive cleanup) has already consented
/// by configuring it.
fn context_for(binding: &HookBinding) -> ActionContext {
    ActionContext {
        marked: None,
        confirmed: true,
        query: binding.on.query.clone(),
        value: binding.with.value.clone(),
        text: binding.with.text.clone(),
        args: binding.with.args.clone(),
    }
}

/// Interpret an action's dispatch as a hook outcome. The actions hooks invoke
/// are the fire-and-forget kind (`backup`, `restore-all`, a toggle): a `Notify`,
/// `Reload`, `Noop`, or `Confirm` (already auto-confirmed) means success. The
/// interactive dispatches (open an editor, run a query, create a child) cannot
/// be driven head-less, so they count as a configuration error.
fn outcome_for(dispatch: ActionDispatch) -> HookOutcome {
    match dispatch {
        ActionDispatch::Notify { message } => HookOutcome::Fired(Some(message)),
        ActionDispatch::Reload | ActionDispatch::Noop | ActionDispatch::Confirm { .. } => {
            HookOutcome::Fired(None)
        }
        ActionDispatch::Error(e) => HookOutcome::Failed(e),
        other => HookOutcome::Failed(format!(
            "action returned {other:?}, which a hook cannot drive head-less"
        )),
    }
}

/// Process one binding against a built adapter: throttle-gate, invoke the
/// action or run the script, stamp on success.
async fn process_binding(
    adapter: &dyn ContentAdapter,
    instance: &str,
    hook: &str,
    binding: &HookBinding,
    payload: &serde_json::Value,
    now: DateTime<Utc>,
) -> HookOutcome {
    let kind = match binding.kind() {
        Ok(k) => k,
        Err(e) => return HookOutcome::Failed(e),
    };
    if is_throttled(instance, hook, binding, now) {
        return HookOutcome::Throttled;
    }

    let outcome = match kind {
        BindingKind::Action(run) => invoke_action(adapter, run, binding).await,
        BindingKind::Script(name) => {
            run_script(adapter.adapter_type(), instance, hook, name, payload).await
        }
    };

    // Stamp only on a real fire (so a failure retries next time) and only when a
    // throttle is configured (a throttle-less binding keeps no state).
    if matches!(outcome, HookOutcome::Fired(_)) && binding.when.throttle.is_some() {
        stamp(instance, hook, &binding.label(), now);
    }
    outcome
}

/// The action half of [`process_binding`]: resolve the target node, check the
/// configured arguments, invoke.
async fn invoke_action(
    adapter: &dyn ContentAdapter,
    run: &str,
    binding: &HookBinding,
) -> HookOutcome {
    let node = match &binding.on.id {
        Some(id) => adapter.get_by_id(id).await,
        None => adapter.root().await,
    };
    let node = match node {
        Ok(n) => n,
        Err(e) => return HookOutcome::Failed(format!("resolving target node: {e}")),
    };

    let mut ctx = context_for(binding);
    // Check the configured arguments against what the action declares (an
    // action that declares nothing takes them as they come).
    let declared = adapter
        .actions_for_type(node.node_type())
        .into_iter()
        .find(|a| a.id == run)
        .map(|a| a.params)
        .unwrap_or_default();
    ctx.args = match not_yet_done_content::resolve_args(&declared, &ctx.args) {
        Ok(args) => args,
        Err(problems) => {
            return HookOutcome::Failed(format!(
                "arguments for '{run}': {}",
                not_yet_done_content::describe_problems(&problems)
            ));
        }
    };
    match node.invoke_action(run, &ctx).await {
        Ok(dispatch) => outcome_for(dispatch),
        Err(e) => HookOutcome::Failed(format!("invoking '{run}': {e}")),
    }
}

// ---------------------------------------------------------------------------
// Script bindings
// ---------------------------------------------------------------------------

/// How long a hook script may run before it is killed. Hook scripts are
/// awaited in order (a `tracking_stopped` script must finish before the
/// `tracking_started` one starts), so a hung script would otherwise stall
/// every later hook of the process.
pub const SCRIPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long the payload and log files of a script run are kept.
const RUN_FILE_RETENTION: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Environment variable naming the hook a script is running for.
pub const SCRIPT_HOOK_ENV: &str = "NYD_SCRIPT_HOOK";
/// Environment variable naming the adapter instance whose hook is running.
pub const HOOK_INSTANCE_ENV: &str = "NYD_HOOK_INSTANCE";
/// Environment variable carrying the hook nesting depth: a hook script is
/// started with its parent's depth plus one, and any process at depth ≥ 1 runs
/// no event hooks (see [`event_hooks_suppressed`]).
pub const HOOK_DEPTH_ENV: &str = "NYD_HOOK_DEPTH";

fn depth_from(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// This process's hook nesting depth (0 unless started by a hook script).
pub fn hook_depth() -> u32 {
    depth_from(std::env::var(HOOK_DEPTH_ENV).ok().as_deref())
}

/// Whether this process must not run event hooks because it was itself
/// started by a hook script. Cuts the recursion `hook → script → CLI → same
/// hook → …`: the child sees the mutation it performs, but stays silent.
pub fn event_hooks_suppressed() -> bool {
    hook_depth() >= 1
}

/// Where script runs leave their payload and log files:
/// `~/.local/state/not_yet_done/hooks/` (next to the throttle state).
fn run_dir() -> PathBuf {
    state_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .join("hooks")
}

/// Best-effort: drop run files older than [`RUN_FILE_RETENTION`].
fn sweep_run_dir(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if now
            .duration_since(modified)
            .is_ok_and(|age| age > RUN_FILE_RETENTION)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Resolve a binding's `script:` name to a file. A bare name (no `/`) lives in
/// the instance's top-level view-script directory — the same
/// `<root>/<adapter type>/` the TUI's script menu writes to, so a hook script
/// can be created and edited there. A relative path with `/` is taken from the
/// scripts root (e.g. `trackings/autotrack.py` from a tasks instance), an
/// absolute path as is.
pub fn resolve_script_path(root: &Path, adapter_type: &str, name: &str) -> PathBuf {
    let p = Path::new(name);
    if p.is_absolute() {
        p.to_path_buf()
    } else if name.contains('/') {
        root.join(name)
    } else {
        not_yet_done_scripts::ScriptScope::new(adapter_type, Vec::new())
            .dir(root)
            .join(name)
    }
}

/// The JSON document a hook script receives: the hook name and the instance,
/// plus the event payload's keys (an object is merged in, anything else lands
/// under `payload`). `hook` and `instance` win over payload keys of the same
/// name.
pub fn script_payload(
    hook: &str,
    instance: &str,
    payload: &serde_json::Value,
) -> serde_json::Value {
    let mut doc = serde_json::Map::new();
    match payload {
        serde_json::Value::Object(map) => {
            doc.extend(map.iter().map(|(k, v)| (k.clone(), v.clone())))
        }
        serde_json::Value::Null => {}
        other => {
            doc.insert("payload".into(), other.clone());
        }
    }
    doc.insert("hook".into(), serde_json::Value::String(hook.into()));
    doc.insert(
        "instance".into(),
        serde_json::Value::String(instance.into()),
    );
    serde_json::Value::Object(doc)
}

fn file_safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Run one script binding to completion: write the payload file, start the
/// script with stdout/stderr going to a log file next to it, await it with
/// [`SCRIPT_TIMEOUT`]. A non-zero exit or a timeout is a
/// [`HookOutcome::Failed`] naming the log file.
pub async fn run_script(
    adapter_type: &str,
    instance: &str,
    hook: &str,
    name: &str,
    payload: &serde_json::Value,
) -> HookOutcome {
    run_script_at(
        &not_yet_done_scripts::default_root(),
        &run_dir(),
        adapter_type,
        instance,
        hook,
        name,
        payload,
    )
    .await
}

/// [`run_script`] with the scripts root and the run-file directory injected
/// (for tests).
async fn run_script_at(
    scripts_root: &Path,
    run_dir: &Path,
    adapter_type: &str,
    instance: &str,
    hook: &str,
    name: &str,
    payload: &serde_json::Value,
) -> HookOutcome {
    static NEXT_RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let path = resolve_script_path(scripts_root, adapter_type, name);
    if !path.is_file() {
        return HookOutcome::Failed(format!("script {} not found", path.display()));
    }

    let dir = run_dir.to_path_buf();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return HookOutcome::Failed(format!("creating {}: {e}", dir.display()));
    }
    sweep_run_dir(&dir);

    let stem = format!(
        "{}-{}-{}-{}",
        file_safe(instance),
        file_safe(hook),
        std::process::id(),
        NEXT_RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let json_path = dir.join(format!("{stem}.json"));
    let log_path = dir.join(format!("{stem}.log"));

    let doc = script_payload(hook, instance, payload);
    if let Err(e) = std::fs::write(&json_path, doc.to_string()) {
        return HookOutcome::Failed(format!("writing {}: {e}", json_path.display()));
    }
    let (log, log_err) =
        match std::fs::File::create(&log_path).and_then(|f| Ok((f.try_clone()?, f))) {
            Ok(pair) => pair,
            Err(e) => return HookOutcome::Failed(format!("creating {}: {e}", log_path.display())),
        };

    let mut child = match tokio::process::Command::new(&path)
        .arg(&json_path)
        .current_dir(path.parent().unwrap_or(Path::new(".")))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(log_err))
        .env(SCRIPT_HOOK_ENV, hook)
        .env(HOOK_INSTANCE_ENV, instance)
        .env(HOOK_DEPTH_ENV, (hook_depth() + 1).to_string())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return HookOutcome::Failed(format!("starting {}: {e}", path.display())),
    };

    match tokio::time::timeout(SCRIPT_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => HookOutcome::Fired(None),
        Ok(Ok(status)) => HookOutcome::Failed(format!(
            "{name} exited with {status} (log: {})",
            log_path.display()
        )),
        Ok(Err(e)) => HookOutcome::Failed(format!("waiting for {name}: {e}")),
        Err(_) => {
            let _ = child.kill().await;
            HookOutcome::Failed(format!(
                "{name} timed out after {} s and was killed (log: {})",
                SCRIPT_TIMEOUT.as_secs(),
                log_path.display()
            ))
        }
    }
}

/// Where a failure is reported. The CLI wants stderr; the TUI owns the
/// terminal, so its background runner appends to `hooks/runner.log` in the
/// state directory instead.
#[derive(Clone, Copy)]
enum FailureSink {
    Stderr,
    LogFile,
}

fn report_failure(sink: FailureSink, msg: &str) {
    match sink {
        FailureSink::Stderr => eprintln!("nyd-hooks: {msg}"),
        FailureSink::LogFile => {
            use std::io::Write;
            let dir = run_dir();
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("runner.log"))
            {
                let _ = writeln!(f, "{} nyd-hooks: {msg}", Utc::now().to_rfc3339());
            }
        }
    }
}

/// Fire `hook` for `instance` against an already-built `adapter`.
///
/// Looks up the instance's configured bindings for `hook`, validates the hook id
/// against the adapter's declared [`ContentAdapter::hooks`], then processes each
/// binding (throttle → invoke → stamp). Best-effort: never panics, logs failures
/// to stderr, and returns one [`HookReport`] per binding.
pub async fn fire_hook(
    adapter: &dyn ContentAdapter,
    instance: &str,
    hook: &str,
) -> Vec<HookReport> {
    let Some(cfg) = crate::load_hook_config(instance) else {
        return Vec::new();
    };
    fire_hook_with(adapter, instance, hook, &cfg).await
}

/// [`fire_hook`] against a caller-supplied config (so callers that already read
/// the view file — e.g. [`fire_connected_hooks`] — need not re-discover it).
pub async fn fire_hook_with(
    adapter: &dyn ContentAdapter,
    instance: &str,
    hook: &str,
    cfg: &HookConfig,
) -> Vec<HookReport> {
    fire_bindings(
        adapter,
        instance,
        hook,
        cfg,
        &serde_json::Value::Null,
        FailureSink::Stderr,
    )
    .await
}

/// The one firing loop behind every entry point: validate the hook against the
/// adapter's declaration, then process each binding in order with `payload`
/// (what a script binding receives; action bindings ignore it).
async fn fire_bindings(
    adapter: &dyn ContentAdapter,
    instance: &str,
    hook: &str,
    cfg: &HookConfig,
    payload: &serde_json::Value,
    sink: FailureSink,
) -> Vec<HookReport> {
    let Some(bindings) = cfg.get(hook) else {
        return Vec::new();
    };
    if bindings.is_empty() {
        return Vec::new();
    }

    let report = |action: &str, outcome: HookOutcome| HookReport {
        instance: instance.to_string(),
        hook: hook.to_string(),
        action: action.to_string(),
        outcome,
    };

    // A configured hook the adapter does not declare is almost always a typo;
    // surface it rather than silently never firing.
    if !adapter.hooks().contains(&hook) {
        report_failure(
            sink,
            &format!(
                "instance '{instance}' configures hook '{hook}', but its adapter declares none such ({:?}) — skipping",
                adapter.hooks()
            ),
        );
        return bindings
            .iter()
            .map(|b| {
                report(
                    &b.label(),
                    HookOutcome::Skipped("hook not declared by adapter".into()),
                )
            })
            .collect();
    }

    let now = Utc::now();
    let mut reports = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let outcome = process_binding(adapter, instance, hook, binding, payload, now).await;
        if let HookOutcome::Failed(e) = &outcome {
            report_failure(
                sink,
                &format!("{instance}:{hook}: '{}' failed: {e}", binding.label()),
            );
        }
        reports.push(report(&binding.label(), outcome));
    }
    reports
}

// ---------------------------------------------------------------------------
// Event hooks (hooks an adapter fires itself, as bus events)
// ---------------------------------------------------------------------------

/// Fire the hook an adapter-published event stands for: `event.topic` is the
/// hook name, `event.source` the instance that fired it. Events from other
/// instances, and topics this instance binds nothing to, are ignored (an empty
/// report). The event payload is what script bindings receive.
pub async fn fire_hook_event(
    adapter: &dyn ContentAdapter,
    instance: &str,
    cfg: &HookConfig,
    event: &BusEvent,
) -> Vec<HookReport> {
    fire_hook_event_to(adapter, instance, cfg, event, FailureSink::Stderr).await
}

async fn fire_hook_event_to(
    adapter: &dyn ContentAdapter,
    instance: &str,
    cfg: &HookConfig,
    event: &BusEvent,
    sink: FailureSink,
) -> Vec<HookReport> {
    if event.source != instance || !cfg.contains_key(&event.topic) {
        return Vec::new();
    }
    fire_bindings(adapter, instance, &event.topic, cfg, &event.payload, sink).await
}

/// The instance's hook config if it binds anything an *event* could fire —
/// `connected` alone is fired by the front-end directly and does not warrant a
/// bus subscription.
fn event_hook_config(instance: &str) -> Option<HookConfig> {
    let cfg = crate::load_hook_config(instance)?;
    cfg.keys().any(|k| k != "connected").then_some(cfg)
}

/// Start the background runner that turns this instance's bus events into
/// hook firings, for a long-lived front-end. Returns whether a runner was
/// started: nothing runs when the process is itself a hook child (see
/// [`event_hooks_suppressed`]), when the instance binds no event hook, or when
/// no tokio runtime is current.
///
/// The runner holds the adapter only weakly: when the front-end drops the
/// adapter (a config reload builds a fresh one, which starts its own runner),
/// the old runner exits at its next event instead of firing twice. Bindings run
/// sequentially inside the runner, so the order of events is the order of
/// hook runs. Failures go to `hooks/runner.log` in the state directory, never
/// to the terminal the front-end owns.
pub fn spawn_event_hook_runner(
    adapter: Weak<dyn ContentAdapter>,
    instance: &str,
    bus: &dyn HostEventBus,
) -> bool {
    if event_hooks_suppressed() {
        return false;
    }
    let Some(cfg) = event_hook_config(instance) else {
        return false;
    };
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        report_failure(
            FailureSink::LogFile,
            &format!("no async runtime to run '{instance}' event hooks on"),
        );
        return false;
    };
    let mut rx = not_yet_done_content::subscribe_events(bus);
    let instance = instance.to_string();
    rt.spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match rx.recv().await {
                Ok(raw) => {
                    let Some(event) = BusEvent::from_host_event(&raw) else {
                        continue;
                    };
                    if event.source != instance {
                        continue;
                    }
                    let Some(adapter) = adapter.upgrade() else {
                        break;
                    };
                    fire_hook_event_to(
                        adapter.as_ref(),
                        &instance,
                        &cfg,
                        &event,
                        FailureSink::LogFile,
                    )
                    .await;
                }
                Err(RecvError::Lagged(n)) => {
                    report_failure(
                        FailureSink::LogFile,
                        &format!("'{instance}' event hooks lagged, {n} events dropped"),
                    );
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
    true
}

/// Fire the hooks for every event `rx` has buffered so far, for a one-shot
/// front-end: subscribe with [`not_yet_done_content::subscribe_events`]
/// *before* the command runs, drain after it. Same suppression rules as
/// [`spawn_event_hook_runner`]; failures go to stderr.
pub async fn drain_event_hooks(
    adapter: &dyn ContentAdapter,
    instance: &str,
    rx: &mut tokio::sync::broadcast::Receiver<HostEvent>,
) -> Vec<HookReport> {
    use tokio::sync::broadcast::error::TryRecvError;
    if event_hooks_suppressed() {
        return Vec::new();
    }
    let Some(cfg) = event_hook_config(instance) else {
        return Vec::new();
    };
    let mut reports = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(raw) => {
                let Some(event) = BusEvent::from_host_event(&raw) else {
                    continue;
                };
                reports.extend(
                    fire_hook_event_to(adapter, instance, &cfg, &event, FailureSink::Stderr).await,
                );
            }
            Err(TryRecvError::Lagged(_)) => continue,
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
        }
    }
    reports
}

/// Fire the `connected` hook for every configured instance that declares one —
/// the startup entry point for front-ends that build their adapters lazily (the
/// TUI). Crucially, the throttle is checked *before* the adapter is built, so a
/// within-window launch constructs nothing: only an instance with a *due*
/// binding is resolved and fired. Returns the collected reports.
pub async fn fire_connected_hooks() -> Vec<HookReport> {
    let now = Utc::now();
    let ctx = crate::host_context();
    let mut reports = Vec::new();

    for inst in crate::discover_instances() {
        let instance = inst.instance_id().to_string();
        let Some(cfg) = inst.hooks.clone() else {
            continue;
        };
        let Some(bindings) = cfg.get("connected") else {
            continue;
        };
        // Pre-gate: skip the (potentially expensive) adapter build entirely if
        // every binding is still throttled.
        if bindings
            .iter()
            .all(|b| is_throttled(&instance, "connected", b, now))
        {
            continue;
        }
        let adapter = match crate::resolve_adapter(&instance, &ctx) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("nyd-hooks: connected: could not build '{instance}': {e}");
                continue;
            }
        };
        reports.extend(fire_hook_with(adapter.as_ref(), &instance, "connected", &cfg).await);
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_throttle_units() {
        assert_eq!(parse_throttle("30s").unwrap(), Duration::seconds(30));
        assert_eq!(parse_throttle("15m").unwrap(), Duration::minutes(15));
        assert_eq!(parse_throttle("24h").unwrap(), Duration::hours(24));
        assert_eq!(parse_throttle("7d").unwrap(), Duration::days(7));
    }

    #[test]
    fn parse_throttle_rejects_garbage() {
        assert!(parse_throttle("24").is_err()); // no unit
        assert!(parse_throttle("h").is_err()); // no number
        assert!(parse_throttle("24y").is_err()); // unknown unit
    }

    #[test]
    fn hook_config_deserializes_minimal_and_full() {
        let yaml = r#"
connected:
  - run: backup
    when: { throttle: 24h }
  - run: notify
    on: { id: abc123 }
    with: { value: hi, text: there }
  - run: prune
    with: { args: { limit: 20, dry_run: true, folders: [a, b] } }
"#;
        let cfg: HookConfig = serde_yaml::from_str(yaml).unwrap();
        let connected = &cfg["connected"];
        assert_eq!(connected.len(), 3);
        assert_eq!(connected[0].run.as_deref(), Some("backup"));
        assert_eq!(connected[0].label(), "backup");
        assert_eq!(connected[0].when.throttle.as_deref(), Some("24h"));
        assert!(connected[0].on.id.is_none());
        assert_eq!(connected[1].on.id.as_deref(), Some("abc123"));
        assert_eq!(connected[1].with.value.as_deref(), Some("hi"));
        assert_eq!(connected[1].with.text.as_deref(), Some("there"));
        assert!(connected[1].with.args.is_empty());
        // `args:` keeps the YAML types: an int stays an int, a bool a bool.
        let args = &connected[2].with.args;
        assert_eq!(args.int("limit"), Some(20));
        assert_eq!(args.bool("dry_run"), Some(true));
        assert_eq!(args.list("folders"), Some(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn untrottled_binding_never_throttles() {
        let b = HookBinding {
            run: Some("backup".into()),
            ..HookBinding::default()
        };
        assert!(!is_throttled("x", "connected", &b, Utc::now()));
    }

    #[test]
    fn script_binding_parses_and_is_labelled() {
        let yaml = r#"
tracking_started:
  - script: autotrack.py
  - script: trackings/autotrack.py
    when: { throttle: 5s }
"#;
        let cfg: HookConfig = serde_yaml::from_str(yaml).unwrap();
        let b = &cfg["tracking_started"];
        assert!(b[0].run.is_none());
        assert_eq!(b[0].script.as_deref(), Some("autotrack.py"));
        assert_eq!(b[0].label(), "script:autotrack.py");
        assert!(matches!(
            b[0].kind(),
            Ok(BindingKind::Script("autotrack.py"))
        ));
        assert_eq!(b[1].when.throttle.as_deref(), Some("5s"));
    }

    #[test]
    fn binding_must_name_exactly_one_of_run_and_script() {
        let both = HookBinding {
            run: Some("backup".into()),
            script: Some("x.py".into()),
            ..HookBinding::default()
        };
        assert!(both.kind().is_err());
        let neither = HookBinding::default();
        assert!(neither.kind().is_err());
        assert_eq!(neither.label(), "<unbound>");
    }

    #[test]
    fn depth_guard_reads_the_env_value() {
        assert_eq!(depth_from(None), 0);
        assert_eq!(depth_from(Some("")), 0);
        assert_eq!(depth_from(Some("garbage")), 0);
        assert_eq!(depth_from(Some("1")), 1);
        assert_eq!(depth_from(Some(" 2 ")), 2);
    }

    #[test]
    fn script_path_resolves_bare_relative_and_absolute_names() {
        let root = Path::new("/data/scripts");
        assert_eq!(
            resolve_script_path(root, "tasks", "autotrack.py"),
            PathBuf::from("/data/scripts/tasks/autotrack.py")
        );
        assert_eq!(
            resolve_script_path(root, "trackings", "tasks/autotrack.py"),
            PathBuf::from("/data/scripts/tasks/autotrack.py")
        );
        assert_eq!(
            resolve_script_path(root, "tasks", "/opt/hooks/x.sh"),
            PathBuf::from("/opt/hooks/x.sh")
        );
    }

    #[test]
    fn script_payload_merges_the_event_and_pins_hook_and_instance() {
        let payload = serde_json::json!({"task_path": "/a/b", "hook": "spoofed"});
        let doc = script_payload("tracking_started", "tasks", &payload);
        assert_eq!(doc["hook"], "tracking_started");
        assert_eq!(doc["instance"], "tasks");
        assert_eq!(doc["task_path"], "/a/b");

        let doc = script_payload("connected", "tasks", &serde_json::Value::Null);
        assert_eq!(doc.as_object().unwrap().len(), 2);

        let doc = script_payload("x", "i", &serde_json::json!([1, 2]));
        assert_eq!(doc["payload"], serde_json::json!([1, 2]));
    }

    #[tokio::test]
    async fn run_script_hands_over_the_payload_and_reports_the_exit_status() {
        let tmp = std::env::temp_dir().join(format!("nyd-hook-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("tasks")).unwrap();
        let script = tmp.join("tasks/ok.sh");
        std::fs::write(&script, "#!/bin/sh\ngrep -q '\"hook\":\"tracking_started\"' \"$1\" && [ \"$NYD_HOOK_DEPTH\" = \"1\" ]\n").unwrap();
        let failing = tmp.join("tasks/fail.sh");
        std::fs::write(&failing, "#!/bin/sh\nexit 3\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&script, &failing] {
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }

        let ok = run_script_at(
            &tmp,
            &tmp.join("runs"),
            "tasks",
            "tasks",
            "tracking_started",
            "ok.sh",
            &serde_json::json!({}),
        )
        .await;
        assert_eq!(ok, HookOutcome::Fired(None));
        let failed = run_script_at(
            &tmp,
            &tmp.join("runs"),
            "tasks",
            "tasks",
            "tracking_started",
            "fail.sh",
            &serde_json::json!({}),
        )
        .await;
        assert!(
            matches!(&failed, HookOutcome::Failed(m) if m.contains("exit status: 3")),
            "{failed:?}"
        );
        let missing = run_script_at(
            &tmp,
            &tmp.join("runs"),
            "tasks",
            "tasks",
            "tracking_started",
            "nope.sh",
            &serde_json::json!({}),
        )
        .await;
        assert!(matches!(&missing, HookOutcome::Failed(m) if m.contains("not found")));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn outcome_mapping() {
        assert_eq!(
            outcome_for(ActionDispatch::Notify {
                message: "ok".into()
            }),
            HookOutcome::Fired(Some("ok".into()))
        );
        assert_eq!(outcome_for(ActionDispatch::Noop), HookOutcome::Fired(None));
        assert_eq!(
            outcome_for(ActionDispatch::Error("boom".into())),
            HookOutcome::Failed("boom".into())
        );
        assert!(matches!(
            outcome_for(ActionDispatch::ExecuteQuery {
                database: "db".into(),
                sql: "select 1".into(),
                paged: false,
            }),
            HookOutcome::Failed(_)
        ));
    }
}
