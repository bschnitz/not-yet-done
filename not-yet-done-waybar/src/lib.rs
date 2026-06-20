use std::cell::RefCell;
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde::Deserialize;
use waybar_cffi::{
    gtk::{
        glib,
        prelude::{ContainerExt, LabelExt, StyleContextExt, WidgetExt},
        Label, Orientation, Box as GtkBox,
    },
    waybar_module, InitInfo, Module,
};

use not_yet_done_core::config::ConfigServiceImpl;
use not_yet_done_core::db;
use not_yet_done_task_core::module::TaskDomainModule;
use not_yet_done_task_core::repository::{
    TaskRepositoryImpl, TaskRepositoryImplParameters,
    TrackingRepositoryImpl, TrackingRepositoryImplParameters,
};

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
// DB access
// ---------------------------------------------------------------------------

fn connect_db() -> Option<Arc<TaskDomainModule>> {
    let config_service = ConfigServiceImpl::new();
    let rt = tokio::runtime::Runtime::new().ok()?;
    let db_url = rt.block_on(config_service.get_database_url()).ok()?;
    let db = rt.block_on(db::connect(&db_url, false)).ok()?;

    Some(Arc::new(
        TaskDomainModule::builder()
            .with_component_parameters::<TaskRepositoryImpl>(
                TaskRepositoryImplParameters { db: Some(db.clone()) },
            )
            .with_component_parameters::<TrackingRepositoryImpl>(
                TrackingRepositoryImplParameters { db: Some(db) },
            )
            .build(),
    ))
}

/// Query active trackings and return (task_description, started_at).
fn get_active_tracking(module: &TaskDomainModule) -> Option<(String, chrono::DateTime<Utc>)> {
    use not_yet_done_task_core::repository::TrackingRepository;
    use not_yet_done_task_core::service::TaskService;
    use shaku::HasComponent;

    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        let tracking_repo: &dyn TrackingRepository = module.resolve_ref();
        let active = tracking_repo.find_all_active().await.ok()?;
        let tracking = active.first()?;

        let task_service: &dyn TaskService = module.resolve_ref();
        let tasks = task_service.list_tasks(None).await.ok()?;
        let task = tasks.iter().find(|t| t.id == tracking.task_id)?;

        Some((task.description.clone(), tracking.started_at))
    })
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

fn update_label(
    label: &Label,
    inner: &GtkBox,
    module: &Option<Arc<TaskDomainModule>>,
    icon: &str,
    max_chars: usize,
) {
    if let Some(module) = module {
        if let Some((desc, started_at)) = get_active_tracking(module) {
            let elapsed = Utc::now() - started_at;
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

        let module = connect_db();
        if module.is_none() {
            eprintln!("nyd-waybar: could not connect to database");
        }

        let icon = config.icon;
        let max_chars = config.max_chars;

        // Initial update immediately.
        update_label(&label, &inner, &module, &icon, max_chars);

        // Periodic update via glib timeout.
        let label_ref = RefCell::new(label);
        let inner_ref = RefCell::new(inner);
        glib::timeout_add_local(
            std::time::Duration::from_millis(config.interval_ms as u64),
            move || {
                update_label(&label_ref.borrow(), &inner_ref.borrow(), &module, &icon, max_chars);
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
