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
//!       with: { }              # ActionContext inputs: value / text
//!       when: { throttle: 24h } # fire at most once per window
//! ```
//!
//! This generalises the hard-coded daily backup: binding `backup` to the local
//! adapter's `connected` hook (fired on every program start) with a 24h throttle
//! backs the database up once a day on first use — but the same machinery works
//! for any adapter, any action, any cadence, with no front-end code change.
//!
//! **Where it runs.** The host owns hook firing so *both* the TUI and the CLI
//! (and any future front-end) inherit it from the one crate that builds
//! adapters. Two entry points:
//!
//!   * [`fire_hook`] — fire a named hook against an **already-built** adapter
//!     (the CLI calls this right after [`crate::resolve_adapter`], reusing the
//!     adapter it built for the command).
//!   * [`fire_connected_hooks`] — the startup helper the TUI calls: it checks
//!     the throttle *before* building anything, so within the throttle window no
//!     adapter is constructed at all (it would otherwise pay an idle DB open on
//!     every launch). Only instances with a *due* `connected` binding are
//!     resolved and fired.
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
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use not_yet_done_content::{ActionContext, ActionDispatch, ContentAdapter};

// ---------------------------------------------------------------------------
// Config schema (the `hooks:` block of a view file)
// ---------------------------------------------------------------------------

/// The `hooks:` block: hook id → its ordered list of bindings.
pub type HookConfig = HashMap<String, Vec<HookBinding>>;

/// One declarative action invocation bound to a hook.
#[derive(Debug, Clone, Deserialize)]
pub struct HookBinding {
    /// The adapter action id to invoke when the hook fires.
    pub run: String,
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
}

/// When a binding is allowed to fire.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookWhen {
    /// Minimum gap between fires, e.g. `24h`, `30m`, `90s`, `7d`. Absent → fire
    /// every time the hook fires (no throttle, no state stamp).
    #[serde(default)]
    pub throttle: Option<String>,
}

/// Parse a throttle duration: an integer followed by a unit `s`/`m`/`h`/`d`.
fn parse_throttle(spec: &str) -> Result<Duration, String> {
    let spec = spec.trim();
    let (num, unit) = spec.split_at(
        spec.find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| format!("throttle '{spec}' has no unit (expected s/m/h/d)"))?,
    );
    let n: i64 = num
        .parse()
        .map_err(|_| format!("throttle '{spec}' has no leading number"))?;
    match unit {
        "s" => Ok(Duration::seconds(n)),
        "m" => Ok(Duration::minutes(n)),
        "h" => Ok(Duration::hours(n)),
        "d" => Ok(Duration::days(n)),
        other => Err(format!(
            "throttle '{spec}': unknown unit '{other}' (expected s/m/h/d)"
        )),
    }
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
    match state.get(&state_key(instance, hook, &binding.run)) {
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

/// Process one binding against a built adapter: throttle-gate, resolve the
/// target node, invoke the action, stamp on success.
async fn process_binding(
    adapter: &dyn ContentAdapter,
    instance: &str,
    hook: &str,
    binding: &HookBinding,
    now: DateTime<Utc>,
) -> HookOutcome {
    if is_throttled(instance, hook, binding, now) {
        return HookOutcome::Throttled;
    }

    let node = match &binding.on.id {
        Some(id) => adapter.get_by_id(id).await,
        None => adapter.root().await,
    };
    let node = match node {
        Ok(n) => n,
        Err(e) => return HookOutcome::Failed(format!("resolving target node: {e}")),
    };

    let ctx = context_for(binding);
    let outcome = match node.invoke_action(&binding.run, &ctx).await {
        Ok(dispatch) => outcome_for(dispatch),
        Err(e) => HookOutcome::Failed(format!("invoking '{}': {e}", binding.run)),
    };

    // Stamp only on a real fire (so a failure retries next time) and only when a
    // throttle is configured (a throttle-less binding keeps no state).
    if matches!(outcome, HookOutcome::Fired(_)) && binding.when.throttle.is_some() {
        stamp(instance, hook, &binding.run, now);
    }
    outcome
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
        eprintln!(
            "nyd-hooks: instance '{instance}' configures hook '{hook}', but its adapter declares none such ({:?}) — skipping",
            adapter.hooks()
        );
        return bindings
            .iter()
            .map(|b| {
                report(
                    &b.run,
                    HookOutcome::Skipped("hook not declared by adapter".into()),
                )
            })
            .collect();
    }

    let now = Utc::now();
    let mut reports = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let outcome = process_binding(adapter, instance, hook, binding, now).await;
        if let HookOutcome::Failed(e) = &outcome {
            eprintln!(
                "nyd-hooks: {instance}:{hook}: '{}' failed: {e}",
                binding.run
            );
        }
        reports.push(report(&binding.run, outcome));
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
"#;
        let cfg: HookConfig = serde_yaml::from_str(yaml).unwrap();
        let connected = &cfg["connected"];
        assert_eq!(connected.len(), 2);
        assert_eq!(connected[0].run, "backup");
        assert_eq!(connected[0].when.throttle.as_deref(), Some("24h"));
        assert!(connected[0].on.id.is_none());
        assert_eq!(connected[1].on.id.as_deref(), Some("abc123"));
        assert_eq!(connected[1].with.value.as_deref(), Some("hi"));
        assert_eq!(connected[1].with.text.as_deref(), Some("there"));
    }

    #[test]
    fn untrottled_binding_never_throttles() {
        let b = HookBinding {
            run: "backup".into(),
            on: HookTarget::default(),
            with: HookInputs::default(),
            when: HookWhen { throttle: None },
        };
        assert!(!is_throttled("x", "connected", &b, Utc::now()));
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
