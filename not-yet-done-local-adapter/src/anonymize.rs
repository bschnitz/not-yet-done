//! Domain anonymizer for the local Tasks / Trackings / Projects adapters.
//!
//! The content layer ships a [`StandardAnonymizer`](not_yet_done_content::StandardAnonymizer)
//! that is always *safe* (it never leaks real text) but domain-blind: it
//! replaces every free-text token with a neutral pool word, so a task name
//! comes out as `"Falcon Harbor"` and the *same* task referenced from a
//! tracking row comes out word-for-word identically only by luck of tokenising.
//!
//! These three entities want more than safety — they want **plausibility** and
//! **referential consistency**: a screenshot should show task names that read
//! like task names, and the task `"Quarterly review"` must appear as the *same*
//! pseudo-name everywhere it surfaces (the Tasks tree, a tracking's `task`
//! column, a tracking's `taskpath`, a task's `ancestors` chain). We get both by
//! keying a fixed lookup list on the **hash of the real name string** (not the
//! DB id): `pseudo = LIST[stable_hash(real) % LIST.len()]`. Two rows naming the
//! same real task hash to the same slot, so they share a pseudo-name for free,
//! deterministically and across runs.
//!
//! Everything not domain-specific (an unknown future column, free-text tags,
//! a project description) falls back to the [`StandardAnonymizer`], so adding a
//! column can never silently leak: the worst case is a neutral pool word.
//!
//! The lookup lists below are **fully invented** neutral strings — no real
//! customer, person or project term — because they ship in the repository.

use not_yet_done_content::anonymize::stable_hash;
use not_yet_done_content::{Anonymizer, StandardAnonymizer};

/// Which local entity an anonymizer instance scrubs. Selects the per-key
/// strategy (a task name vs. a project name vs. their structural columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalDomain {
    Task,
    Tracking,
    Project,
}

/// The anonymizer the local adapters return from `ContentAdapter::anonymizer`.
/// Maps the sensitive columns of its [`LocalDomain`] to invented pseudo-names,
/// passes structural columns (markers, dates, durations, ids) through verbatim,
/// and delegates anything unrecognised to the safe [`StandardAnonymizer`].
#[derive(Debug, Clone, Copy)]
pub struct LocalAnonymizer {
    domain: LocalDomain,
    std: StandardAnonymizer,
}

impl LocalAnonymizer {
    pub fn task() -> Self {
        Self::new(LocalDomain::Task)
    }
    pub fn tracking() -> Self {
        Self::new(LocalDomain::Tracking)
    }
    pub fn project() -> Self {
        Self::new(LocalDomain::Project)
    }

    fn new(domain: LocalDomain) -> Self {
        Self {
            domain,
            std: StandardAnonymizer::new(),
        }
    }

    fn scrub_task(&self, key: &str, value: &str) -> String {
        match key {
            // The task name — the heart of the consistency contract.
            "label" => pseudo_task_name(value),
            // JSON `[{id, description}]` ancestor chain: scrub each
            // `description` (a task name) the same way, keep every `id`.
            "ancestors" => scrub_ancestors(value, &self.std),
            // Comma-separated free-text tag names → neutral pool words.
            "tag_names" => scrub_csv(value, &self.std),
            // Glyph icons; structural under Standard, but route it through in
            // case a tag symbol is ever configured as plain text.
            "tag_symbols" => self.std.scrub_value(key, value),
            // Structural / addressing columns — verbatim.
            "tracking" | "tracking_rollup" | "status" | "priority" | "notes"
            | "created" | "updated" | "last_tracked" | "id" | "deleted" | "tag_ids" => {
                value.to_string()
            }
            // Unknown future column → safe fallback, never a leak.
            _ => self.std.scrub_value(key, value),
        }
    }

    fn scrub_tracking(&self, key: &str, value: &str) -> String {
        match key {
            // Both name the same underlying task → same lookup as Tasks, so a
            // tracking shows the pseudo-name its task carries in the Tasks tab.
            "label" | "task" => pseudo_task_name(value),
            // `/a/b/c` chain of task names → map each segment.
            "taskpath" => scrub_task_path(value),
            // Structural / addressing columns — verbatim.
            "marker" | "started" | "ended" | "duration" | "id" | "task_id" | "deleted" => {
                value.to_string()
            }
            _ => self.std.scrub_value(key, value),
        }
    }

    fn scrub_project(&self, key: &str, value: &str) -> String {
        match key {
            "label" | "name" => pseudo_project_name(value),
            // Free-text blurb → neutral pool words (no project-name list).
            "description" => self.std.scrub_value(key, value),
            "created" | "id" => value.to_string(),
            _ => self.std.scrub_value(key, value),
        }
    }
}

impl Anonymizer for LocalAnonymizer {
    fn scrub_value(&self, key: &str, value: &str) -> String {
        match self.domain {
            LocalDomain::Task => self.scrub_task(key, value),
            LocalDomain::Tracking => self.scrub_tracking(key, value),
            LocalDomain::Project => self.scrub_project(key, value),
        }
    }
}

/// Map a real task name to a stable invented pseudo-name. Values carrying no
/// ascii letter (a date or duration bucket header that reaches the `label`
/// column of a grouped row) are left verbatim — they are never sensitive and
/// must not be turned into a random task name.
fn pseudo_task_name(value: &str) -> String {
    if !value.chars().any(|c| c.is_ascii_alphabetic()) {
        return value.to_string();
    }
    let idx = (stable_hash(value) % TASK_NAMES.len() as u64) as usize;
    TASK_NAMES[idx].to_string()
}

/// Map a real project name to a stable invented pseudo-name. Same letter guard
/// as [`pseudo_task_name`].
fn pseudo_project_name(value: &str) -> String {
    if !value.chars().any(|c| c.is_ascii_alphabetic()) {
        return value.to_string();
    }
    let idx = (stable_hash(value) % PROJECT_NAMES.len() as u64) as usize;
    PROJECT_NAMES[idx].to_string()
}

/// Scrub a `/`-separated task path segment-by-segment, preserving the leading
/// slash and any empty segments (`/a/b` → `/<pseudo a>/<pseudo b>`).
fn scrub_task_path(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    value
        .split('/')
        .map(|seg| {
            if seg.is_empty() {
                String::new()
            } else {
                pseudo_task_name(seg)
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Scrub the `ancestors` JSON array: replace each element's `description` (a
/// task name) with its pseudo-name, keep every `id` (addressing). On any
/// unexpected shape, fall back to the safe [`StandardAnonymizer`] rather than
/// risk passing real text through.
fn scrub_ancestors(value: &str, std: &StandardAnonymizer) -> String {
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::Array(items)) => {
            let mapped: Vec<serde_json::Value> = items
                .into_iter()
                .map(|mut item| {
                    if let Some(desc) = item.get("description").and_then(|d| d.as_str()) {
                        let pseudo = pseudo_task_name(desc);
                        item["description"] = serde_json::Value::String(pseudo);
                    }
                    item
                })
                .collect();
            serde_json::Value::Array(mapped).to_string()
        }
        _ => std.scrub_value("ancestors", value),
    }
}

/// Scrub a comma-separated free-text list (tag names) element-by-element via
/// the standard anonymizer, re-joining with `", "`.
fn scrub_csv(value: &str, std: &StandardAnonymizer) -> String {
    if value.is_empty() {
        return String::new();
    }
    value
        .split(',')
        .map(|tok| std.scrub_value("tag", tok.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Invented, neutral pseudo task names. Generic enough to read like real task
/// descriptions on a screenshot, but referencing no real customer/person/work.
const TASK_NAMES: &[&str] = &[
    "Backup-Strategie überprüfen",
    "Onboarding-Dokumentation aktualisieren",
    "Release-Notes vorbereiten",
    "Monitoring-Dashboard einrichten",
    "Datenbank-Migration planen",
    "API-Endpunkt dokumentieren",
    "Testabdeckung erhöhen",
    "CI-Pipeline reparieren",
    "Logrotation konfigurieren",
    "Zertifikate erneuern",
    "Performance-Profiling durchführen",
    "Code-Review nachziehen",
    "Abhängigkeiten aktualisieren",
    "Fehlerbericht reproduzieren",
    "Konfiguration vereinheitlichen",
    "Cache-Invalidierung prüfen",
    "Benutzerhandbuch ergänzen",
    "Schnittstelle abstimmen",
    "Wartungsfenster ankündigen",
    "Rechteverwaltung aufräumen",
    "Suchindex neu aufbauen",
    "Verschlüsselung nachrüsten",
    "Lasttest auswerten",
    "Alarmierung feinjustieren",
    "Datenexport automatisieren",
    "Formularvalidierung ergänzen",
    "Übersetzung nachpflegen",
    "Statusseite einrichten",
    "Rollback-Verfahren testen",
    "Speicherleck untersuchen",
    "Build-Zeiten optimieren",
    "Health-Check erweitern",
    "Webhook anbinden",
    "Audit-Log einführen",
    "Feature-Flag entfernen",
    "Endnutzer-Feedback sichten",
    "Inventar abgleichen",
    "Servertausch vorbereiten",
    "Notfallplan überarbeiten",
    "Schulungsunterlagen erstellen",
    "Datenschutzcheck durchführen",
    "Lizenzen inventarisieren",
    "Backup einspielen testen",
    "Netzwerksegmentierung prüfen",
    "Ticket-Vorlagen pflegen",
    "Deployment-Skript härten",
    "Metriken konsolidieren",
    "Altsystem abschalten",
    "Reporting verschlanken",
    "Zugriffsprotokoll auswerten",
    "Schema-Versionierung einführen",
    "Bereitschaftsplan erstellen",
    "Konfigdrift beseitigen",
    "Sekundärindex ergänzen",
    "Fehlerseite gestalten",
    "Archivierung anstoßen",
    "Kapazitätsplanung aktualisieren",
    "Telemetrie reduzieren",
    "Vertragsdaten prüfen",
    "Quartalsbericht zusammenstellen",
];

/// Invented, neutral pseudo project / client names. Company-shaped but fully
/// fictional.
const PROJECT_NAMES: &[&str] = &[
    "Nordlicht Systeme",
    "Auenbach Logistik",
    "Kranzberg Medien",
    "Talwind Energie",
    "Steinach Werke",
    "Morgentau Verlag",
    "Fernblick Software",
    "Hügelland Bau",
    "Brunnental Pharma",
    "Wiesengrund Handel",
    "Eschenhof Technik",
    "Lindenau Beratung",
    "Felsbach Maschinen",
    "Ahornweg Dienste",
    "Seeblick Reederei",
    "Birkenfeld Druck",
    "Rotbuche Labor",
    "Silberbach Finanz",
    "Tannberg Studio",
    "Klee & Partner",
    "Habicht Sicherheit",
    "Marlow Industrie",
    "Veste Datentechnik",
    "Kornfeld Genossenschaft",
    "Uferweg Mobilität",
    "Distelhof Catering",
    "Granit Versicherung",
    "Espenried Recycling",
    "Wolkenstein Reisen",
    "Quellwasser Kommunal",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_label_is_a_plausible_pseudo_name_and_stable() {
        let a = LocalAnonymizer::task();
        let once = a.scrub_value("label", "Geheimer Kundenauftrag");
        let twice = a.scrub_value("label", "Geheimer Kundenauftrag");
        assert_eq!(once, twice, "deterministic");
        assert!(TASK_NAMES.contains(&once.as_str()), "drawn from the pool");
        assert!(!once.contains("Kunden"), "real text gone");
    }

    #[test]
    fn same_task_maps_consistently_across_task_and_tracking() {
        let task = LocalAnonymizer::task();
        let trk = LocalAnonymizer::tracking();
        let real = "Quartalsabschluss erstellen";
        // The pseudo-name a task carries must equal what a tracking's `task`
        // column and a taskpath segment carry for the same real name.
        let from_task_label = task.scrub_value("label", real);
        let from_trk_task = trk.scrub_value("task", real);
        let from_trk_label = trk.scrub_value("label", real);
        assert_eq!(from_task_label, from_trk_task);
        assert_eq!(from_task_label, from_trk_label);
        let from_path = trk.scrub_value("taskpath", &format!("/{real}"));
        assert_eq!(from_path, format!("/{from_task_label}"));
    }

    #[test]
    fn structural_columns_pass_through_verbatim() {
        let a = LocalAnonymizer::task();
        for (k, v) in [
            ("id", "538d5583-31ad-4ddc-9395-b037afd581c2"),
            ("created", "2026-06-22T10:00:00+00:00"),
            ("priority", "3"),
            ("status", "✓"),
            ("tracking", "⏱"),
            ("deleted", "true"),
            ("tag_ids", "11111111-2222-3333-4444-555555555555"),
        ] {
            assert_eq!(a.scrub_value(k, v), v, "{k} must survive verbatim");
        }
    }

    #[test]
    fn ancestors_json_scrubs_descriptions_keeps_ids() {
        let a = LocalAnonymizer::task();
        let input = r#"[{"id":"abc","description":"Real Root"},{"id":"def","description":"Real Child"}]"#;
        let out = a.scrub_value("ancestors", input);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr[0]["id"], "abc", "ids preserved for addressing");
        assert_eq!(arr[1]["id"], "def");
        assert!(!out.contains("Real Root"), "real names gone");
        assert!(!out.contains("Real Child"));
        // Consistency: the root description maps to the same pseudo as a label.
        assert_eq!(arr[0]["description"], a.scrub_value("label", "Real Root"));
    }

    #[test]
    fn taskpath_maps_each_segment_keeps_slashes() {
        let a = LocalAnonymizer::tracking();
        let out = a.scrub_value("taskpath", "/Alpha/Beta");
        assert!(out.starts_with('/'));
        assert_eq!(out.split('/').count(), 3, "leading empty + 2 segments");
        assert!(!out.contains("Alpha") && !out.contains("Beta"));
        // Each segment is a task-name pseudo.
        let parts: Vec<&str> = out.split('/').skip(1).collect();
        assert!(parts.iter().all(|p| TASK_NAMES.contains(p)));
    }

    #[test]
    fn date_bucket_label_is_left_verbatim() {
        // A grouped tracking row whose label is a date bucket must not be
        // turned into a random task name.
        let a = LocalAnonymizer::tracking();
        assert_eq!(a.scrub_value("label", "2026-06-22"), "2026-06-22");
    }

    #[test]
    fn project_name_drawn_from_project_pool_not_task_pool() {
        let a = LocalAnonymizer::project();
        let out = a.scrub_value("name", "Echte Kundenfirma GmbH");
        assert!(PROJECT_NAMES.contains(&out.as_str()));
        assert!(!out.contains("Kunden"));
    }

    #[test]
    fn unknown_key_falls_back_to_standard_not_passthrough() {
        let a = LocalAnonymizer::task();
        // A column we never enumerated must still be scrubbed (safe default).
        let out = a.scrub_value("some_future_text_column", "Confidential note");
        assert!(!out.contains("Confidential"));
    }
}
