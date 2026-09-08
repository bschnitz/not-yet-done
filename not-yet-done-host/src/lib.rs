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

use std::fmt;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

use not_yet_done_content::{
    AdapterFactory, AliasSpec, AliasTable, ContentAdapter, HostContext, InMemoryHostBus,
    action_event_adapter, aliasing_adapter,
};

pub use not_yet_done_content::AutoConnect;

pub mod hooks;
pub use hooks::{
    ActionFilter, HOOK_DEPTH_ENV, HookBinding, HookConfig, HookInputs, HookOutcome, HookReport,
    HookTarget, HookWhen, drain_event_hooks, event_hooks_suppressed, fire_connected_hooks, fire_hook,
    fire_hook_event, fire_hook_with, spawn_event_hook_runner,
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
        not_yet_done_content::typed(not_yet_done_jira_adapter::JiraAdapterFactory::new()),
    );
    factories.insert(
        "kimai".to_string(),
        not_yet_done_content::typed(not_yet_done_kimai_adapter::KimaiAdapterFactory::new()),
    );
    factories.insert(
        "taiga".to_string(),
        not_yet_done_content::typed(not_yet_done_taiga_adapter::TaigaAdapterFactory::new()),
    );
    factories.insert(
        "postgres".to_string(),
        not_yet_done_content::typed(not_yet_done_postgres_adapter::PostgresAdapterFactory::new()),
    );
    factories.insert(
        "sqlite".to_string(),
        not_yet_done_content::typed(not_yet_done_sqlite_adapter::SqliteAdapterFactory::new()),
    );
    factories.insert(
        "confluence".to_string(),
        not_yet_done_content::typed(
            not_yet_done_confluence_adapter::ConfluenceAdapterFactory::new(),
        ),
    );
    factories.insert(
        "stoat".to_string(),
        not_yet_done_content::typed(not_yet_done_stoat_adapter::StoatAdapterFactory::new()),
    );
    factories.insert(
        "mail".to_string(),
        not_yet_done_content::typed(not_yet_done_mail_adapter::MailAdapterFactory::new()),
    );
    factories.insert(
        "tasks".to_string(),
        not_yet_done_content::typed(not_yet_done_local_adapter::TaskAdapterFactory::new()),
    );
    factories.insert(
        "trackings".to_string(),
        not_yet_done_content::typed(not_yet_done_local_adapter::TrackingAdapterFactory::new()),
    );
    factories.insert(
        "projects".to_string(),
        not_yet_done_content::typed(not_yet_done_local_adapter::ProjectAdapterFactory::new()),
    );
    factories.insert(
        "calendar".to_string(),
        not_yet_done_content::typed(not_yet_done_calendar_adapter::CalendarAdapterFactory::new()),
    );
    factories.insert(
        "workflow".to_string(),
        not_yet_done_content::typed(not_yet_done_workflow::WorkflowAdapterFactory::new()),
    );
    // Wrap every factory in three decorators, one place, inherited by every
    // front-end:
    //   * custom-columns (innermost): injects the user's locally-stored extra
    //     columns onto every row and exposes the set-cell/clear-cell actions.
    //     Inert until cells exist, so it's free in normal use.
    //   * scripts (middle): exposes the user's view-scripts as addressable
    //     nodes and injects the list/create/edit/delete actions, so both the
    //     TUI and the CLI drive the same CRUD surface. Inert unless the user
    //     invokes a script action.
    //   * anonymizing (outermost): when the run requests anonymization
    //     ([`HostContext::anonymize`]), masks all user-visible output — off by
    //     default it's a transparent pass-through.
    // Order matters: anonymizing wraps the others, so injected custom values
    // and script names/bodies are scrubbed like any other free text in a
    // screenshot run and a user's local note can't leak past the mask. scripts
    // sits above custom-columns so it derives its scope from the real node
    // types, unaffected by the injected columns.
    factories
        .into_iter()
        .map(|(ty, factory)| {
            (
                ty,
                not_yet_done_content::anonymizing_factory(not_yet_done_scripts::scripts_factory(
                    not_yet_done_custom_columns::custom_columns_factory(factory),
                )),
            )
        })
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
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
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
    /// When this instance is allowed to connect on its own: `never` (the
    /// default), `on_open` (the first time its tab is opened) or `startup`
    /// (while the app comes up, unvisited). See [`AutoConnect`].
    ///
    /// Absent means "not stated here" rather than "never": the answer then
    /// comes from the older [`Self::manual_connect`] boolean. Read the
    /// resolved value through [`Self::connect_mode`], never this field.
    ///
    /// Only the TUI knows what opening a tab means, so only the TUI tells the
    /// three apart; the CLI and Waybar run one request and connect regardless.
    #[serde(default)]
    pub auto_connect: Option<AutoConnect>,
    /// The older boolean spelling of [`Self::auto_connect`]: `true` means
    /// [`AutoConnect::Never`], `false` means [`AutoConnect::Startup`]. Kept
    /// because every view file written before `auto_connect` existed says it.
    ///
    /// **Defaults to `true`.** Connecting is the side-effecting choice: it can
    /// open a tunnel, spend a VPN round-trip or put a credential dialog in
    /// front of the user before they have asked for anything. An instance that
    /// is cheap and local (the task DB, a local SQLite file) opts back in.
    ///
    /// An explicit `auto_connect:` wins when both keys are present — the
    /// newer key is the more specific statement, and it is the only one that
    /// can say `on_open` at all.
    #[serde(default = "manual_connect_default")]
    pub manual_connect: bool,
    /// Refresh this instance's tabs on a timer: `auto_reload: 10m` re-fetches
    /// every ten minutes. Written as a number plus a unit (`s`, `m`, `h`,
    /// `d`) — the same spelling as a hook's `throttle:`. Absent (the default)
    /// means never.
    ///
    /// The timer only ever *refreshes* — it never *connects*. It starts
    /// running once the instance has loaded at least once, so a
    /// `manual_connect` adapter still waits for the user's first `reload`
    /// and an adapter that is only configured, never opened, stays quiet.
    /// Each completed load re-arms it, so a manual `r` also resets the clock.
    ///
    /// Only the TUI honours it — the CLI and Waybar run one request and exit,
    /// so there is nothing to keep fresh.
    #[serde(default, deserialize_with = "deserialize_interval")]
    pub auto_reload: Option<Duration>,
    /// Action aliases declared on this instance: a new action name that stands
    /// for an existing adapter action with arguments filled in. An alias is a
    /// real action to every frontend — it is listed and bindable wherever its
    /// target is. See [`not_yet_done_content::AliasSpec`] for the block's
    /// shape and [`decorate_instance`] for where it takes effect.
    #[serde(default)]
    pub aliases: BTreeMap<String, AliasSpec>,
}

/// Serde default for [`AdapterInstance::manual_connect`] — see the field's
/// docs for why an unconfigured instance waits for `reload`.
fn manual_connect_default() -> bool {
    true
}

/// Parse an interval the way the config files spell one: an integer plus a
/// unit — `90s`, `10m`, `2h`, `7d`. The single spelling used across the
/// config surface; a hook's `when.throttle` parses through here too.
///
/// A unit is required on purpose. `24` is ambiguous — the user who writes it
/// under `throttle:` means hours at least as often as seconds — so it is an
/// error rather than a guess. Zero is rejected as well: an interval of
/// nothing is a busy loop, not a setting.
pub fn parse_interval(raw: &str) -> std::result::Result<Duration, String> {
    let text = raw.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| format!("interval `{raw}` has no unit (expected s/m/h/d)"))?;
    let (digits, unit) = text.split_at(split);
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("interval `{raw}` has no leading number"))?;
    let secs = match unit {
        "s" => value,
        "m" => value * 60,
        "h" => value * 3600,
        "d" => value * 86_400,
        other => {
            return Err(format!(
                "interval `{raw}`: unknown unit `{other}` (expected s/m/h/d)"
            ));
        }
    };
    if secs == 0 {
        return Err(format!("interval `{raw}` is zero — it must be positive"));
    }
    Ok(Duration::from_secs(secs))
}

/// Render an interval back in the spelling [`parse_interval`] accepts: the
/// largest unit that divides it evenly — `600s` comes back as `10m`, `5400s`
/// as `90m` (not `1.5h`, which is not a spelling we take). Used for the UI
/// hints that tell the user an interval is set, so what the screen shows can
/// be pasted straight back into the config.
pub fn format_interval(d: Duration) -> String {
    let secs = d.as_secs();
    for (unit, size) in [("d", 86_400u64), ("h", 3_600), ("m", 60)] {
        if secs >= size && secs % size == 0 {
            return format!("{}{unit}", secs / size);
        }
    }
    format!("{secs}s")
}

/// YAML scalar an interval may be written as. `10m` parses as a string, but a
/// unit-less `600` parses as a number — accepting both here lets
/// [`parse_interval`] answer it with "has no unit" instead of serde answering
/// with "invalid type: integer".
#[derive(Deserialize)]
#[serde(untagged)]
enum IntervalSpec {
    Number(u64),
    Text(String),
}

/// Serde bridge for [`AdapterInstance::auto_reload`]: an absent key is `None`,
/// anything else must parse as an interval or the whole view file fails to
/// load with the reason in the message.
fn deserialize_interval<'de, D>(de: D) -> std::result::Result<Option<Duration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = match Option::<IntervalSpec>::deserialize(de)? {
        None => return Ok(None),
        Some(IntervalSpec::Number(n)) => n.to_string(),
        Some(IntervalSpec::Text(text)) => text,
    };
    parse_interval(&raw)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

impl AdapterInstance {
    /// Effective instance id — explicit `id:` if given, else `adapter_type`.
    pub fn effective_instance_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.adapter_type)
    }

    /// When this instance may connect on its own — the resolved answer of the
    /// two keys that can state it.
    ///
    /// An explicit [`Self::auto_connect`] wins; otherwise the legacy
    /// [`Self::manual_connect`] boolean answers, and its default (`true`)
    /// makes an instance that states neither [`AutoConnect::Never`].
    pub fn connect_mode(&self) -> AutoConnect {
        match self.auto_connect {
            Some(mode) => mode,
            None if self.manual_connect => AutoConnect::Never,
            None => AutoConnect::Startup,
        }
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
    discover_instances_reporting().0
}

/// A view file that looked like an instance (`tab:` + `adapter:`) but whose
/// head did not parse, and why. Surfaced wherever a lookup fails, because a
/// silently dropped file reads as "instance not configured" — which sends the
/// reader to the wrong place (a stale binary that predates a new `hooks:` key
/// is the classic case).
#[derive(Debug, Clone)]
pub struct SkippedView {
    pub view_path: PathBuf,
    pub reason: String,
}

impl fmt::Display for SkippedView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.view_path.display(), self.reason)
    }
}

/// [`discover_instances`], plus the instance-shaped view files it had to skip.
pub fn discover_instances_reporting() -> (Vec<DiscoveredInstance>, Vec<SkippedView>) {
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
    let mut skipped = Vec::new();
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
        let head = match serde_yaml::from_str::<ViewFileHead>(&yaml) {
            Ok(head) => head,
            Err(err) => {
                skipped.push(SkippedView {
                    view_path: path,
                    reason: err.to_string(),
                });
                continue;
            }
        };
        out.push(DiscoveredInstance {
            adapter: head.adapter,
            hooks: head.hooks,
            view_path: path,
        });
    }
    (out, skipped)
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
            view_path.parent().unwrap_or(Path::new(".")).join(cfg_path)
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
pub fn resolve_adapter(instance_name: &str, ctx: &HostContext) -> Result<Box<dyn ContentAdapter>> {
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
    let (instances, skipped) = discover_instances_reporting();
    let found = instances
        .iter()
        .find(|d| d.instance_id() == instance_name)
        .ok_or_else(|| {
            let known: Vec<&str> = instances.iter().map(|d| d.instance_id()).collect();
            let mut msg = format!(
                "no adapter instance '{instance_name}' configured (known: {})",
                if known.is_empty() {
                    "<none>".to_string()
                } else {
                    known.join(", ")
                }
            );
            for view in &skipped {
                msg.push_str(&format!("\nview file skipped, it did not parse: {view}"));
            }
            anyhow!(msg)
        })?;

    let cfg = read_config_string(&found.adapter, &found.view_path)?;
    let factory = factories.get(&found.adapter.adapter_type).ok_or_else(|| {
        anyhow!(
            "no adapter factory registered for type '{}'",
            found.adapter.adapter_type
        )
    })?;
    let adapter = factory
        .create(found.instance_id(), &cfg, ctx)
        .map_err(|e| anyhow!("creating adapter '{instance_name}': {e}"))?;
    decorate_instance(adapter, &found.adapter, ctx)
}

/// Wrap a freshly created adapter in the per-*instance* decorators: the
/// action-event reporter (every instance, so `hooks: action_invoked:` works
/// everywhere) and the `aliases:` the instance's own block asks for. The
/// per-*type* decorators (scripts, custom columns, anonymization) live in
/// [`factories`] because they apply to every instance alike; these need the
/// instance, so every build site calls this right after
/// `AdapterFactory::create`. Order matters: aliasing wraps the reporter, so
/// reported actions carry the resolved id, never the alias name.
pub fn decorate_instance(
    adapter: Box<dyn ContentAdapter>,
    instance: &AdapterInstance,
    ctx: &HostContext,
) -> Result<Box<dyn ContentAdapter>> {
    let adapter = action_event_adapter(
        adapter,
        instance.effective_instance_id(),
        Arc::clone(&ctx.event_bus),
    );
    let table = AliasTable::new(instance.aliases.clone()).map_err(|e| {
        anyhow!(
            "adapter '{}': invalid aliases: {e}",
            instance.effective_instance_id()
        )
    })?;
    Ok(aliasing_adapter(adapter, table))
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
            auto_connect: None,
            manual_connect: false,
            auto_reload: None,
            aliases: Default::default(),
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
            auto_connect: None,
            manual_connect: false,
            auto_reload: None,
            aliases: Default::default(),
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
        assert!(
            head.adapter.manual_connect,
            "an adapter block that says nothing waits for an explicit reload"
        );
    }

    #[test]
    fn manual_connect_can_be_switched_off_per_instance() {
        let yaml = r#"
adapter:
  type: tasks
  config_inline: "database: sqlite::memory:"
  manual_connect: false
"#;
        let head: ViewFileHead = serde_yaml::from_str(yaml).unwrap();
        assert!(!head.adapter.manual_connect);
    }

    #[test]
    fn the_legacy_boolean_still_answers_when_it_is_the_only_key() {
        let manual: ViewFileHead = serde_yaml::from_str(
            r#"
adapter:
  type: tasks
  config_inline: "x"
  manual_connect: true
"#,
        )
        .unwrap();
        assert_eq!(manual.adapter.connect_mode(), AutoConnect::Never);

        let eager: ViewFileHead = serde_yaml::from_str(
            r#"
adapter:
  type: tasks
  config_inline: "x"
  manual_connect: false
"#,
        )
        .unwrap();
        assert_eq!(eager.adapter.connect_mode(), AutoConnect::Startup);
    }

    #[test]
    fn an_instance_that_states_nothing_waits_for_a_reload() {
        let head: ViewFileHead = serde_yaml::from_str(
            r#"
adapter:
  type: jira
  config_inline: "x"
"#,
        )
        .unwrap();
        assert_eq!(head.adapter.connect_mode(), AutoConnect::Never);
    }

    #[test]
    fn auto_connect_reads_all_three_answers() {
        for (written, expected) in [
            ("never", AutoConnect::Never),
            ("on_open", AutoConnect::OnOpen),
            ("startup", AutoConnect::Startup),
        ] {
            let head: ViewFileHead = serde_yaml::from_str(&format!(
                r#"
adapter:
  type: jira
  config_inline: "x"
  auto_connect: {written}
"#
            ))
            .unwrap();
            assert_eq!(
                head.adapter.connect_mode(),
                expected,
                "auto_connect: {written}"
            );
        }
    }

    #[test]
    fn auto_connect_wins_over_the_legacy_boolean() {
        // The old key cannot say `on_open` at all, so a file that has been
        // migrated must not be dragged back by a `manual_connect:` line its
        // author forgot to delete.
        let head: ViewFileHead = serde_yaml::from_str(
            r#"
adapter:
  type: jira
  config_inline: "x"
  manual_connect: true
  auto_connect: startup
"#,
        )
        .unwrap();
        assert_eq!(head.adapter.connect_mode(), AutoConnect::Startup);
    }

    #[test]
    fn a_typo_in_auto_connect_fails_the_view_file_instead_of_disabling_it() {
        let err = serde_yaml::from_str::<ViewFileHead>(
            r#"
adapter:
  type: jira
  config_inline: "x"
  auto_connect: on-open
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("on_open"),
            "the error should name the values it takes, got: {err}"
        );
    }

    #[test]
    fn auto_reload_is_off_unless_the_instance_asks_for_it() {
        let yaml = r#"
adapter:
  type: jira
  config_inline: "x"
"#;
        let head: ViewFileHead = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(head.adapter.auto_reload, None);
    }

    #[test]
    fn auto_reload_reads_a_human_interval() {
        let yaml = r#"
adapter:
  type: jira
  config_inline: "x"
  auto_reload: 10m
"#;
        let head: ViewFileHead = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(head.adapter.auto_reload, Some(Duration::from_secs(600)));
    }

    #[test]
    fn a_unit_less_auto_reload_is_told_what_it_is_missing() {
        let yaml = r#"
adapter:
  type: jira
  config_inline: "x"
  auto_reload: 90
"#;
        let err = serde_yaml::from_str::<ViewFileHead>(yaml).unwrap_err();
        assert!(
            err.to_string().contains("has no unit"),
            "a bare number should be told to add s/m/h/d, got: {err}"
        );
    }

    #[test]
    fn a_typo_in_auto_reload_fails_the_view_file_instead_of_disabling_it() {
        let yaml = r#"
adapter:
  type: jira
  config_inline: "x"
  auto_reload: 10x
"#;
        let err = serde_yaml::from_str::<ViewFileHead>(yaml).unwrap_err();
        assert!(
            err.to_string().contains("unknown unit"),
            "the reason should name the unit, got: {err}"
        );
    }

    #[test]
    fn parse_interval_understands_every_unit() {
        assert_eq!(parse_interval("45s").unwrap(), Duration::from_secs(45));
        assert_eq!(parse_interval("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_interval("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_interval("7d").unwrap(), Duration::from_secs(604_800));
    }

    #[test]
    fn parse_interval_rejects_zero_and_nonsense() {
        assert!(parse_interval("0m").is_err(), "zero is a busy loop");
        assert!(parse_interval("10").is_err(), "a unit is required");
        assert!(parse_interval("").is_err());
        assert!(parse_interval("soon").is_err());
    }

    #[test]
    fn format_interval_gives_back_a_spelling_we_accept() {
        for raw in ["45s", "10m", "90m", "2h", "7d"] {
            let parsed = parse_interval(raw).unwrap();
            assert_eq!(format_interval(parsed), raw, "round trip of {raw}");
        }
    }

    #[test]
    fn format_interval_picks_the_largest_whole_unit() {
        // 90 minutes is not a whole number of hours, so it stays minutes
        // rather than becoming a spelling `parse_interval` would reject.
        assert_eq!(format_interval(Duration::from_secs(5_400)), "90m");
        assert_eq!(format_interval(Duration::from_secs(3_600)), "1h");
        assert_eq!(format_interval(Duration::from_secs(30)), "30s");
        assert_eq!(format_interval(Duration::from_secs(0)), "0s");
    }

    #[test]
    fn read_config_string_prefers_inline() {
        let inst = AdapterInstance {
            adapter_type: "jira".into(),
            id: None,
            config: Some("does-not-exist.yaml".into()),
            config_inline: Some("inline-cfg".into()),
            auto_connect: None,
            manual_connect: false,
            auto_reload: None,
            aliases: Default::default(),
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
            auto_connect: None,
            manual_connect: false,
            auto_reload: None,
            aliases: Default::default(),
        };
        assert!(read_config_string(&inst, Path::new("/tmp/view.yaml")).is_err());
    }
}
