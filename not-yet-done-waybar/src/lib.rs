use std::cell::RefCell;
use std::rc::Rc;

use chrono::Duration;
use serde::Deserialize;
use waybar_cffi::{
    gtk::{
        glib,
        prelude::{ContainerExt, LabelExt, StyleContextExt, WidgetExt},
        Label, Orientation, Box as GtkBox,
    },
    waybar_module, InitInfo, Module,
};

use not_yet_done_content::{ContentAdapter, ListParams, NodeType};

/// The view instance the module reads from — the same `trackings` adapter the
/// TUI's Trackings tab and `nyd ls trackings` resolve. Discovered from
/// `~/.config/not_yet_done/views/trackings.yaml`.
const TRACKINGS_INSTANCE: &str = "trackings";

// ---------------------------------------------------------------------------
// Config (from waybar JSON)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Config {
    /// Icon shown before the task description.
    #[serde(default = "default_icon")]
    icon: String,
    /// Maximum characters for the task description before truncation.
    #[serde(default = "default_max_chars")]
    max_chars: usize,
    /// Update interval in milliseconds.
    #[serde(default = "default_interval_ms")]
    interval_ms: u32,
}

fn default_icon() -> String { "⏱".to_string() }
fn default_max_chars() -> usize { 20 }
fn default_interval_ms() -> u32 { 5000 }

// ---------------------------------------------------------------------------
// Duration formatting: 30s, 22min, 1.5h
// ---------------------------------------------------------------------------

fn format_duration_short(d: Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let mins = secs / 60;
        format!("{mins}min")
    } else {
        let hours = secs as f64 / 3600.0;
        if hours < 10.0 {
            // One decimal place: 1.5h
            let rounded = (hours * 10.0).round() / 10.0;
            // Drop ".0" for whole numbers
            if rounded.fract() == 0.0 {
                format!("{}h", rounded as u64)
            } else {
                format!("{rounded:.1}h")
            }
        } else {
            format!("{}h", hours.round() as u64)
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}\u{2026}") // U+2026 HORIZONTAL ELLIPSIS
    }
}

// ---------------------------------------------------------------------------
// Adapter access
// ---------------------------------------------------------------------------
//
// The module is a thin protocol frontend (D6): it talks to the *same*
// in-process `trackings` ContentAdapter the TUI and `nyd` use, instead of
// opening the database itself. This drops the direct `not-yet-done-core` /
// `not-yet-done-task-core` coupling and — crucially — makes the module read
// the adapter's configured database (the split-out `tasks.db`) rather than the
// legacy core `nyd.db`, which no longer holds trackings after the DB split.

/// The flat-list child type. `TrackingRootNode::list` dispatches the *tree* and
/// *condensed* views by their own type ids and treats every other type id —
/// this one included — as the flat entry list, which is what we want. Only
/// `type_id` is read; the rest are inert here.
fn tracking_entry_type() -> NodeType {
    NodeType {
        type_id: "tracking:entry".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Tracking".to_string(),
    }
}

/// Resolve the `trackings` adapter via the host. Returns `None` (and the module
/// shows nothing) if no such view is configured or the adapter fails to build.
fn resolve_trackings_adapter(rt: &tokio::runtime::Runtime) -> Option<Box<dyn ContentAdapter>> {
    rt.block_on(async {
        let ctx = not_yet_done_host::host_context();
        not_yet_done_host::resolve_adapter(TRACKINGS_INSTANCE, &ctx)
    })
    .map_err(|e| eprintln!("nyd-waybar: could not resolve trackings adapter: {e}"))
    .ok()
}

/// Query the active tracking via the adapter and return (description, elapsed).
///
/// `root()` reloads the snapshot from the DB on every call, so a tracking
/// started or stopped after module init is picked up on the next tick. The
/// flat entry list marks the running tracking with a `⏱` glyph in its `marker`
/// field and carries the elapsed time (computed at `now`) in `duration`
/// (integer seconds).
fn get_active_tracking(
    rt: &tokio::runtime::Runtime,
    adapter: &dyn ContentAdapter,
) -> Option<(String, Duration)> {
    rt.block_on(async {
        let root = adapter.root().await.ok()?;
        let result = root
            .list(ListParams {
                node_type: tracking_entry_type(),
                query: None,
                sort: Vec::new(),
                page: None,
                download: false,
                group_by: None,
            })
            .await
            .ok()?;

        let field = |row: &not_yet_done_content::NodeSummary, key: &str| {
            row.metadata.fields.iter().find(|f| f.key == key).map(|f| f.value.clone())
        };

        let active = result
            .items
            .iter()
            .find(|row| field(row, "marker").as_deref() == Some("⏱"))?;

        let desc = field(active, "task").unwrap_or_else(|| active.label.clone());
        let secs: i64 = field(active, "duration").and_then(|v| v.parse().ok()).unwrap_or(0);
        Some((desc, Duration::seconds(secs)))
    })
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

fn update_label(
    label: &Label,
    inner: &GtkBox,
    rt: &tokio::runtime::Runtime,
    adapter: &Option<Box<dyn ContentAdapter>>,
    icon: &str,
    max_chars: usize,
) {
    if let Some(adapter) = adapter {
        if let Some((desc, elapsed)) = get_active_tracking(rt, adapter.as_ref()) {
            let dur = format_duration_short(elapsed);
            let short_desc = truncate(&desc, max_chars);
            label.set_text(&format!("{icon} {short_desc} {dur}"));
            label.set_tooltip_text(Some(&format!("{desc} \u{2014} {dur}")));
            inner.style_context().add_class("active");
            inner.show_all();
        } else {
            label.set_text("");
            inner.style_context().remove_class("active");
            inner.hide();
        }
    } else {
        label.set_text("");
        inner.hide();
    }
}

struct NydModule;

impl Module for NydModule {
    type Config = Config;

    fn init(info: &InitInfo, config: Config) -> Self {
        let root = info.get_root_widget();
        let inner = GtkBox::new(Orientation::Horizontal, 4);
        inner.set_widget_name("nyd-tracking");

        let label = Label::new(None);
        inner.add(&label);
        root.add(&inner);
        root.show_all();

        // One runtime and one adapter for the module's lifetime — the adapter
        // is created within the runtime (like `nyd`) and reused across ticks
        // (like the TUI), reloading from the DB on each `root()` call.
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => Rc::new(rt),
            Err(e) => {
                eprintln!("nyd-waybar: could not start tokio runtime: {e}");
                return NydModule;
            }
        };
        let adapter = resolve_trackings_adapter(&rt);

        let icon = config.icon;
        let max_chars = config.max_chars;

        // Initial update immediately.
        update_label(&label, &inner, &rt, &adapter, &icon, max_chars);

        // Periodic update via glib timeout.
        let label_ref = RefCell::new(label);
        let inner_ref = RefCell::new(inner);
        glib::timeout_add_local(
            std::time::Duration::from_millis(config.interval_ms as u64),
            move || {
                update_label(
                    &label_ref.borrow(),
                    &inner_ref.borrow(),
                    &rt,
                    &adapter,
                    &icon,
                    max_chars,
                );
                glib::ControlFlow::Continue
            },
        );

        NydModule
    }
}

waybar_module!(NydModule);

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_format_seconds() {
        assert_eq!(format_duration_short(Duration::seconds(0)), "0s");
        assert_eq!(format_duration_short(Duration::seconds(30)), "30s");
        assert_eq!(format_duration_short(Duration::seconds(59)), "59s");
    }

    #[test]
    fn test_format_minutes() {
        assert_eq!(format_duration_short(Duration::seconds(60)), "1min");
        assert_eq!(format_duration_short(Duration::seconds(90)), "1min");
        assert_eq!(format_duration_short(Duration::seconds(1320)), "22min");
        assert_eq!(format_duration_short(Duration::seconds(3599)), "59min");
    }

    #[test]
    fn test_format_hours() {
        assert_eq!(format_duration_short(Duration::seconds(3600)), "1h");
        assert_eq!(format_duration_short(Duration::seconds(5400)), "1.5h");
        assert_eq!(format_duration_short(Duration::seconds(7200)), "2h");
        assert_eq!(format_duration_short(Duration::seconds(9000)), "2.5h");
        assert_eq!(format_duration_short(Duration::seconds(36000)), "10h");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("Hello", 10), "Hello");
        assert_eq!(truncate("Hello World Long", 10), "Hello Wor\u{2026}");
        assert_eq!(truncate("AB", 2), "AB");
        assert_eq!(truncate("ABC", 2), "A\u{2026}");
    }
}
