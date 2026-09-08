use std::cell::RefCell;
use std::rc::Rc;

use chrono::Duration;
use serde::Deserialize;
use waybar_cffi::{
    InitInfo, Module,
    gtk::{
        Box as GtkBox, Label, Orientation, glib,
        prelude::{ContainerExt, LabelExt, StyleContextExt, WidgetExt},
    },
    waybar_module,
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
    /// Icon shown before the running-tracking count.
    #[serde(default = "default_icon")]
    icon: String,
    /// Update interval in milliseconds.
    #[serde(default = "default_interval_ms")]
    interval_ms: u32,
}

fn default_icon() -> String {
    "⏱".to_string()
}
fn default_interval_ms() -> u32 {
    5000
}

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

/// One running tracking as the bar shows it: the tracked task's full path and
/// the time elapsed since it started.
struct RunningTracking {
    /// `/Work/Project/Task` — the ancestor chain plus the task itself, in the
    /// same `/`-joined form the grouped tracking policy matches its
    /// `group_paths` against.
    path: String,
    elapsed: Duration,
}

/// The full task path from the adapter's `taskpath` column (the ancestors,
/// `/a/b`, empty for a top-level task) and the task's own description.
fn full_task_path(ancestors: &str, task: &str) -> String {
    format!("{ancestors}/{task}")
}

/// The bar text for `running`: the icon and how many trackings run. With
/// grouped tracking several can run at once, and the bar has no room to name
/// them all — the names go to the tooltip (see [`tooltip_text`]).
fn label_text(icon: &str, running: &[RunningTracking]) -> String {
    format!("{icon} {}", running.len())
}

/// The bar text when there is no adapter to ask. A module that hides on
/// failure looks exactly like a module with nothing to show, so a broken
/// setup passes for an idle one — the mark says the count is missing, the
/// tooltip says why (see [`Ui::show`]).
fn error_text(icon: &str) -> String {
    format!("{icon} !")
}

/// The hover text: one line per running tracking, `path — elapsed`, in the
/// order the adapter lists them.
fn tooltip_text(running: &[RunningTracking]) -> String {
    running
        .iter()
        .map(|t| format!("{} \u{2014} {}", t.path, format_duration_short(t.elapsed)))
        .collect::<Vec<_>>()
        .join("\n")
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

/// Resolve the `trackings` adapter via the host. The error is kept as text
/// rather than logged away: it is what the tooltip shows, and the host names
/// the view files its discovery had to skip in it — the usual reason a module
/// built against an older schema finds no `trackings` instance.
fn resolve_trackings_adapter(
    rt: &tokio::runtime::Runtime,
) -> Result<Box<dyn ContentAdapter>, String> {
    rt.block_on(async {
        let ctx = not_yet_done_host::host_context();
        not_yet_done_host::resolve_adapter(TRACKINGS_INSTANCE, &ctx)
    })
    .map_err(|e| e.to_string())
}

/// Query every running tracking via the adapter.
///
/// `root()` reloads the snapshot from the DB on every call, so a tracking
/// started or stopped after module init is picked up on the next tick. The
/// flat entry list marks a running tracking with a non-empty `marker` field
/// (the glyph is adapter-configurable via `tracking_marker`, so we match on
/// "non-empty" rather than a specific character), names the task in `task`
/// and its ancestors in `taskpath`, and carries the elapsed time (computed at
/// `now`) in `duration` (integer seconds). Under a grouped or
/// parallel policy more than one can run, so all of them are returned; an
/// unreadable adapter yields an empty list.
fn get_running_trackings(
    rt: &tokio::runtime::Runtime,
    adapter: &dyn ContentAdapter,
) -> Vec<RunningTracking> {
    rt.block_on(async {
        let root = adapter.root().await.ok()?;
        let result = not_yet_done_content::children::list(
            adapter,
            root.as_ref(),
            ListParams {
                node_type: tracking_entry_type(),
                query: None,
                sort: Vec::new(),
                page: None,
                download: false,
                group_by: None,
            },
        )
        .await
        .ok()?;

        let field = |row: &not_yet_done_content::NodeSummary, key: &str| {
            row.metadata
                .fields
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.value.clone())
        };

        let running = result
            .items
            .iter()
            .filter(|row| field(row, "marker").is_some_and(|m| !m.is_empty()))
            .map(|row| {
                let task = field(row, "task").unwrap_or_else(|| row.label.clone());
                let ancestors = field(row, "taskpath").unwrap_or_default();
                let secs: i64 = field(row, "duration")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                RunningTracking {
                    path: full_task_path(&ancestors, &task),
                    elapsed: Duration::seconds(secs),
                }
            })
            .collect();
        Some(running)
    })
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

/// The three widgets the module drives: the bar label, the tooltip label and
/// the box that carries both — and with it the style classes `active` and
/// `error`.
struct Ui {
    inner: GtkBox,
    label: Label,
    tip: Label,
}

impl Ui {
    /// Build the widgets under waybar's root and hand them to GTK.
    ///
    /// The tooltip is a label of our own, handed over from `query-tooltip`:
    /// the stock tooltip wraps long lines, and a task path is one line that
    /// must stay one line.
    fn install(info: &InitInfo) -> Self {
        let root = info.get_root_widget();
        let inner = GtkBox::new(Orientation::Horizontal, 4);
        inner.set_widget_name("nyd-tracking");

        let label = Label::new(None);
        inner.add(&label);

        let tip = Label::new(None);
        tip.set_line_wrap(false);
        tip.set_xalign(0.0);
        inner.set_has_tooltip(true);
        let custom = tip.clone();
        inner.connect_query_tooltip(move |_, _, _, _, tooltip| {
            tooltip.set_custom(Some(&custom));
            true
        });

        root.add(&inner);
        root.show_all();

        Ui { inner, label, tip }
    }

    /// Show `text` with `tooltip`, styled by `class` — the one class of
    /// [`STYLE_CLASSES`] that applies, the others removed.
    fn show(&self, text: &str, tooltip: &str, class: &str) {
        self.label.set_text(text);
        self.tip.set_text(tooltip);
        self.set_class(Some(class));
        self.inner.show_all();
    }

    /// Take the module off the bar: nothing runs, and nothing is wrong.
    fn hide(&self) {
        self.label.set_text("");
        self.set_class(None);
        self.inner.hide();
    }

    fn set_class(&self, class: Option<&str>) {
        let ctx = self.inner.style_context();
        for candidate in STYLE_CLASSES {
            match Some(*candidate) == class {
                true => ctx.add_class(candidate),
                false => ctx.remove_class(candidate),
            }
        }
    }
}

/// The classes the module puts on its box, at most one at a time: `active`
/// while trackings run, `error` while it has no adapter to ask.
const STYLE_CLASSES: &[&str] = &["active", "error"];

/// One tick. A failed resolve is retried here, so a bar that started before
/// its config was readable — or before the module was rebuilt to match it —
/// heals on the next tick instead of staying blank until the next restart.
fn update(
    ui: &Ui,
    rt: &tokio::runtime::Runtime,
    adapter: &RefCell<Result<Box<dyn ContentAdapter>, String>>,
    icon: &str,
) {
    // The borrow ends with this statement, so the retry below is free to
    // borrow mutably — held into the `if`, it would collide with itself.
    let unresolved = adapter.borrow().is_err();
    if unresolved && let Ok(resolved) = resolve_trackings_adapter(rt) {
        *adapter.borrow_mut() = Ok(resolved);
    }

    match adapter.borrow().as_ref() {
        Err(reason) => ui.show(&error_text(icon), reason, "error"),
        Ok(adapter) => {
            let running = get_running_trackings(rt, adapter.as_ref());
            match running.is_empty() {
                true => ui.hide(),
                false => ui.show(
                    &label_text(icon, &running),
                    &tooltip_text(&running),
                    "active",
                ),
            }
        }
    }
}

struct NydModule;

impl Module for NydModule {
    type Config = Config;

    fn init(info: &InitInfo, config: Config) -> Self {
        let ui = Ui::install(info);
        let icon = config.icon;

        // One runtime and one adapter for the module's lifetime — the adapter
        // is created within the runtime (like `nyd`) and reused across ticks
        // (like the TUI), reloading from the DB on each `root()` call.
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => Rc::new(rt),
            Err(e) => {
                let reason = format!("could not start tokio runtime: {e}");
                eprintln!("nyd-waybar: {reason}");
                ui.show(&error_text(&icon), &reason, "error");
                return NydModule;
            }
        };

        let adapter = RefCell::new(resolve_trackings_adapter(&rt));
        if let Err(reason) = adapter.borrow().as_ref() {
            eprintln!("nyd-waybar: could not resolve trackings adapter: {reason}");
        }

        // Initial update immediately, then one per interval.
        update(&ui, &rt, &adapter, &icon);
        glib::timeout_add_local(
            std::time::Duration::from_millis(config.interval_ms as u64),
            move || {
                update(&ui, &rt, &adapter, &icon);
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

    fn running(path: &str, secs: i64) -> RunningTracking {
        RunningTracking {
            path: path.to_string(),
            elapsed: Duration::seconds(secs),
        }
    }

    #[test]
    fn full_task_path_appends_the_task_to_its_ancestors() {
        assert_eq!(
            full_task_path("/Work/Project", "Task"),
            "/Work/Project/Task"
        );
        assert_eq!(full_task_path("", "Task"), "/Task");
    }

    #[test]
    fn label_counts_the_running_trackings() {
        assert_eq!(label_text("⏱", &[running("A", 5)]), "⏱ 1");
        assert_eq!(label_text("⏱", &[running("A", 5), running("B", 90)]), "⏱ 2");
    }

    #[test]
    fn a_module_without_an_adapter_marks_the_count_as_missing() {
        assert_eq!(error_text("⏱"), "⏱ !");
    }

    #[test]
    fn tooltip_names_each_running_tracking_with_its_elapsed_time() {
        assert_eq!(
            tooltip_text(&[running("/Work/Write report", 5400), running("/Call", 30)]),
            "/Work/Write report \u{2014} 1.5h\n/Call \u{2014} 30s"
        );
    }
}
