//! Shared host wiring for every not-yet-done front-end.
//!
//! A "front-end" is any binary that drives content adapters: the TUI, the CLI,
//! and the Waybar module. Until Block D each wired adapters up on its own — the
//! TUI built the factory map and the host event bus inline in `main.rs`, the
//! CLI and Waybar bypassed adapters entirely and talked to the old single
//! database directly. This crate is the one place that knows
//!
//!   * which adapter types exist and how to construct their factories
//!     ([`factories`]),
//!   * how to build the cross-adapter [`HostContext`] ([`host_context`]), and
//!   * how to turn a configured *instance* (a `views/*.yaml` file) into a live
//!     [`ContentAdapter`] ([`resolve_adapter`]).
//!
//! With this seam in place the CLI and Waybar become thin, fully generic
//! front-ends over the [`ContentAdapter`] protocol: they call
//! [`resolve_adapter`] and then drive whatever node tree / actions the adapter
//! exposes, so they work for *every* adapter (tasks, trackings, jira, taiga,
//! postgres, confluence, stoat), not a hard-coded subset.
//!
//! The crate deliberately does **not** depend on the TUI: the dependency goes
//! TUI → host (and CLI → host, Waybar → host), never the other way. The
//! adapter-instance descriptor ([`AdapterInstance`]) lives here and is
//! re-exported by the TUI's view-config module so there is a single source of
//! truth for the `adapter:` block schema.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use not_yet_done_content::{AdapterFactory, ContentAdapter, HostContext, InMemoryHostBus};

pub mod hooks;
pub use hooks::{
    fire_connected_hooks, fire_hook, fire_hook_with, HookBinding, HookConfig, HookInputs,
    HookOutcome, HookReport, HookTarget, HookWhen,
};

// ---------------------------------------------------------------------------
// Factory registry
// ---------------------------------------------------------------------------

/// Build the full set of adapter factories this build knows about, keyed by
/// adapter type name (the `adapter.type:` field in a view config).
///
/// Factories are stateless: each `create` receives its config string plus the
/// host's [`HostContext`], and the local factories open their own database
/// from that config (Phase C4). So this is a bare constructor, cheap to call
/// repeatedly — the TUI re-invokes it on every `reload_config` to rebuild the
/// set, and the CLI/Waybar call it once per run.
///
/// Adding an adapter to the product is a one-line change here (plus the
/// `Cargo.toml` dependency); every front-end inherits it automatically.
pub fn factories() -> HashMap<String, Box<dyn AdapterFactory>> {
    let mut factories: HashMap<String, Box<dyn AdapterFactory>> = HashMap::new();
    factories.insert(
        "jira".to_string(),
        Box::new(not_yet_done_jira_adapter::JiraAdapterFactory::new()),
    );
    factories.insert(
        "kimai".to_string(),
        Box::new(not_yet_done_kimai_adapter::KimaiAdapterFactory::new()),
    );
    factories.insert(
        "taiga".to_string(),
        Box::new(not_yet_done_taiga_adapter::TaigaAdapterFactory::new()),
    );
    factories.insert(
        "postgres".to_string(),
        Box::new(not_yet_done_postgres_adapter::PostgresAdapterFactory::new()),
    );
    factories.insert(
        "confluence".to_string(),
        Box::new(not_yet_done_confluence_adapter::ConfluenceAdapterFactory::new()),
    );
    factories.insert(
        "stoat".to_string(),
        Box::new(not_yet_done_stoat_adapter::StoatAdapterFactory::new()),
    );
    factories.insert(
        "tasks".to_string(),
        Box::new(not_yet_done_local_adapter::TaskAdapterFactory::new()),
    );
    factories.insert(
        "trackings".to_string(),
        Box::new(not_yet_done_local_adapter::TrackingAdapterFactory::new()),
    );
    factories.insert(
        "projects".to_string(),
        Box::new(not_yet_done_local_adapter::ProjectAdapterFactory::new()),
    );
    // Wrap every factory so that, when the run requests anonymization
    // ([`HostContext::anonymize`]), each adapter it produces is masked — one
    // place, inherited by every front-end. Off by default the wrapper is a
    // transparent pass-through, so this is free in normal use.
    factories
        .into_iter()
        .map(|(ty, factory)| (ty, not_yet_done_content::anonymizing_factory(factory)))
        .collect()
}

// ---------------------------------------------------------------------------
// Host context
// ---------------------------------------------------------------------------

/// Build a fresh [`HostContext`] — the capabilities the host injects into every
/// adapter at construction. Currently the cross-adapter [`HostEventBus`] (an
/// in-process [`InMemoryHostBus`]); adapters backed by the same data source
/// coordinate over it (keyed by their DSN) while unrelated adapters stay
/// silent.
///
/// Each front-end owns one context for its lifetime: the TUI keeps it so a
/// `reload_config` rebuild reuses the same bus; the CLI/Waybar build one per
/// run. Capacity (256) mirrors the historical domain bus.
///
/// [`HostEventBus`]: not_yet_done_content::HostEventBus
pub fn host_context() -> HostContext {
    HostContext {
        event_bus: std::sync::Arc::new(InMemoryHostBus::new(256)),
        anonymize: anonymize_requested(),
    }
}

/// Whether anonymization (fake data for screenshots/screencasts) is requested,
/// read from the `NYD_ANON` environment variable. Truthy values: `1`, `true`,
/// `yes`, `on` (case-insensitive). Anything else — including unset — is off.
///
/// An env switch (rather than a config-file key) is deliberate: it is per-run
/// and set at launch, so a normal session is never accidentally left in
/// anonymized mode, and a screencast is started by `NYD_ANON=1 not-yet-done`.
fn anonymize_requested() -> bool {
    std::env::var("NYD_ANON")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Adapter-instance descriptor (shared with the TUI view config)
// ---------------------------------------------------------------------------

/// The `adapter:` block of a `views/*.yaml` file — the declarative descriptor
/// of one configured adapter instance.
///
/// This is the single source of truth for the block's schema: the TUI's
/// `view_config` module re-exports it (as `AdapterConfig`) rather than
/// re-declaring the fields, so the TUI's full view parser and the host's
/// lightweight instance resolver can never drift apart.
#[derive(Debug, Clone, Deserialize)]
pub struct AdapterInstance {
    #[serde(rename = "type")]
    pub adapter_type: String,
    /// Stable per-instance identifier — used for the on-disk data directory
    /// (`<data>/not_yet_done/<adapter_type>/<id>/`) and for scoping things like
    /// saved queries. Default = `adapter_type`, so a single configured adapter
    /// of a given type just works. Multiple instances of the same type must
    /// each set an explicit `id:` — the loader errors on collision.
    #[serde(default)]
    pub id: Option<String>,
    /// Path to a separate config file holding the adapter's verbatim config
    /// string (resolved relative to the view file). Mutually informative with
    /// [`Self::config_inline`]; at least one must be present.
    #[serde(default)]
    pub config: Option<String>,
    /// The adapter's config string given inline in the view file. Takes
    /// precedence over [`Self::config`] when both are present.
    #[serde(default)]
    pub config_inline: Option<String>,
    /// When `true`, no load is spawned automatically for this instance —
    /// the user must trigger a `reload` action to make the adapter connect.
    /// Used for adapters whose connection is expensive or unreliable
    /// (Postgres-over-SSH-tunnel, slow VPN-gated APIs). Front-ends that always
    /// connect (the CLI, Waybar) ignore it; only the TUI defers the load.
    #[serde(default)]
    pub manual_connect: bool,
}

impl AdapterInstance {
    /// Effective instance id — explicit `id:` if given, else `adapter_type`.
    pub fn effective_instance_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.adapter_type)
    }
}

/// Minimal view-file head: just enough to find and build the adapter, plus the
/// optional `hooks:` block. The TUI parses the *whole* `ViewFileConfig` (tabs,
/// views, columns, …); the host only needs these two, so it parses this and
/// ignores the rest.
#[derive(Debug, Clone, Deserialize)]
struct ViewFileHead {
    adapter: AdapterInstance,
    /// Lifecycle-hook bindings for this instance (see [`hooks`]). Optional —
    /// most view files declare none.
    #[serde(default)]
    hooks: Option<hooks::HookConfig>,
}

// ---------------------------------------------------------------------------
// Instance discovery + resolution
// ---------------------------------------------------------------------------

/// The not-yet-done config root: `~/.config/not_yet_done/`.
///
/// Single source of truth for where front-ends look for configuration —
/// `views/`, the TUI's `tui.yaml`, and the CLI's `cli.yaml` all live under it.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("not_yet_done")
}

/// The directory holding view configs: `~/.config/not_yet_done/views/`.
pub fn views_dir() -> PathBuf {
    config_dir().join("views")
}

/// One adapter instance discovered in the views directory, plus the path of the
/// view file that declared it (needed to resolve a relative `config:` path).
#[derive(Debug, Clone)]
pub struct DiscoveredInstance {
    pub adapter: AdapterInstance,
    pub view_path: PathBuf,
    /// The instance's `hooks:` config, if any (see [`hooks`]).
    pub hooks: Option<hooks::HookConfig>,
}

impl DiscoveredInstance {
    /// The effective instance id this is addressed by.
    pub fn instance_id(&self) -> &str {
        self.adapter.effective_instance_id()
    }
}

/// Discover every adapter instance declared under [`views_dir`].
///
/// Mirrors the TUI's view-file detection: a file is a view config iff it has
/// both top-level `tab` and `adapter` keys (adapter-credential files like
/// `jira-adapter.yaml` have neither and are skipped). Unreadable or malformed
/// files are skipped silently — the caller surfaces "instance not found" with
/// the list of the ones that *did* parse. Results are sorted by file path so
/// the order is stable.
pub fn discover_instances() -> Vec<DiscoveredInstance> {
    let dir = views_dir();
    let mut yaml_files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext == "yaml" || ext == "yml")
        })
        .collect();
    yaml_files.sort();

    let mut out = Vec::new();
    for path in yaml_files {
        let Ok(yaml) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Same heuristic as the TUI loader: top-level `tab` AND `adapter`.
        let Ok(raw) = serde_yaml::from_str::<serde_yaml::Value>(&yaml) else {
            continue;
        };
        if raw.get("tab").is_none() || raw.get("adapter").is_none() {
            continue;
        }
        let Ok(head) = serde_yaml::from_str::<ViewFileHead>(&yaml) else {
            continue;
        };
        out.push(DiscoveredInstance {
            adapter: head.adapter,
            hooks: head.hooks,
            view_path: path,
        });
    }
    out
}

/// The `hooks:` config for one instance, parsed from its view file. `None` if
/// the instance is unknown or declares no hooks. Used by [`hooks::fire_hook`]
/// when the caller already holds a built adapter but not the parsed config.
pub fn load_hook_config(instance_name: &str) -> Option<hooks::HookConfig> {
    discover_instances()
        .into_iter()
        .find(|d| d.instance_id() == instance_name)
        .and_then(|d| d.hooks)
}

/// Resolve the verbatim config string for an instance: `config_inline` if
/// present, otherwise the contents of the `config:` file (relative to the view
/// file). Errors if neither is available — matching the TUI's behaviour.
fn read_config_string(inst: &AdapterInstance, view_path: &Path) -> Result<String> {
    if let Some(inline) = &inst.config_inline {
        return Ok(inline.clone());
    }
    if let Some(cfg_path) = &inst.config {
        let resolved = if Path::new(cfg_path).is_absolute() {
            PathBuf::from(cfg_path)
        } else {
            view_path
                .parent()
                .unwrap_or(Path::new("."))
                .join(cfg_path)
        };
        return std::fs::read_to_string(&resolved)
            .with_context(|| format!("reading adapter config {}", resolved.display()));
    }
    Err(anyhow!(
        "adapter '{}' has neither `config_inline` nor a `config:` path",
        inst.effective_instance_id()
    ))
}

/// Build a live [`ContentAdapter`] for the named instance, using the standard
/// [`factories`] registry. `instance_name` is the effective instance id (the
/// `adapter.id:`, or the `adapter.type:` when no id is set).
///
/// This is the entry point the CLI and Waybar use: discover the instance from
/// the view configs, read its config, look up the factory for its type, and
/// construct the adapter with the given [`HostContext`].
pub fn resolve_adapter(
    instance_name: &str,
    ctx: &HostContext,
) -> Result<Box<dyn ContentAdapter>> {
    let factories = factories();
    resolve_adapter_with(instance_name, ctx, &factories)
}

/// [`resolve_adapter`] against a caller-supplied factory map. Useful when the
/// caller already holds a registry (e.g. a plugin host with extra adapters) or
/// wants to resolve several instances without rebuilding the map each time.
pub fn resolve_adapter_with(
    instance_name: &str,
    ctx: &HostContext,
    factories: &HashMap<String, Box<dyn AdapterFactory>>,
) -> Result<Box<dyn ContentAdapter>> {
    let instances = discover_instances();
    let found = instances
        .iter()
        .find(|d| d.instance_id() == instance_name)
        .ok_or_else(|| {
            let known: Vec<&str> = instances.iter().map(|d| d.instance_id()).collect();
            anyhow!(
                "no adapter instance '{instance_name}' configured (known: {})",
                if known.is_empty() {
                    "<none>".to_string()
                } else {
                    known.join(", ")
                }
            )
        })?;

    let cfg = read_config_string(&found.adapter, &found.view_path)?;
    let factory = factories.get(&found.adapter.adapter_type).ok_or_else(|| {
        anyhow!(
            "no adapter factory registered for type '{}'",
            found.adapter.adapter_type
        )
    })?;
    factory
        .create(found.instance_id(), &cfg, ctx)
        .map_err(|e| anyhow!("creating adapter '{instance_name}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_instance_id_defaults_to_type() {
        let inst = AdapterInstance {
            adapter_type: "tasks".into(),
            id: None,
            config: None,
            config_inline: None,
            manual_connect: false,
        };
        assert_eq!(inst.effective_instance_id(), "tasks");
    }

    #[test]
    fn effective_instance_id_prefers_explicit_id() {
        let inst = AdapterInstance {
            adapter_type: "postgres".into(),
            id: Some("analytics".into()),
            config: None,
            config_inline: None,
            manual_connect: false,
        };
        assert_eq!(inst.effective_instance_id(), "analytics");
    }

    #[test]
    fn view_file_head_parses_adapter_block_and_ignores_the_rest() {
        let yaml = r#"
tab:
  name: Tasks
adapter:
  type: tasks
  config_inline: "database: sqlite::memory:"
views:
  - name: list
    node_type: task
"#;
        let head: ViewFileHead = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(head.adapter.adapter_type, "tasks");
        assert_eq!(head.adapter.effective_instance_id(), "tasks");
        assert_eq!(
            head.adapter.config_inline.as_deref(),
            Some("database: sqlite::memory:")
        );
        assert!(!head.adapter.manual_connect);
    }

    #[test]
    fn read_config_string_prefers_inline() {
        let inst = AdapterInstance {
            adapter_type: "jira".into(),
            id: None,
            config: Some("does-not-exist.yaml".into()),
            config_inline: Some("inline-cfg".into()),
            manual_connect: false,
        };
        let got = read_config_string(&inst, Path::new("/tmp/view.yaml")).unwrap();
        assert_eq!(got, "inline-cfg");
    }

    #[test]
    fn read_config_string_errors_when_nothing_provided() {
        let inst = AdapterInstance {
            adapter_type: "jira".into(),
            id: None,
            config: None,
            config_inline: None,
            manual_connect: false,
        };
        assert!(read_config_string(&inst, Path::new("/tmp/view.yaml")).is_err());
    }
}
