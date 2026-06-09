# not-yet-done — Architektur & Konventionen

> **Dieses Dokument ist die maßgebliche Referenz für alle Entwicklungs- und KI-Assistenz-Aufgaben in diesem Projekt.**
> Jede KI, die an diesem Projekt arbeitet, soll dieses Dokument als primäre Quelle für Entscheidungen und Konventionen behandeln.

## ⚠️ Anweisungen für KI-Assistenten

Dieses Dokument beschreibt eine verbindliche Architektur. Folgende Verhaltensregeln gelten für jede KI, die an diesem Projekt arbeitet:

1. **Wenn du den Inhalt weiterer Dateien benötigst**, nenne sie in einer komma-separierten Liste ohne Leerzeichen, z.B.: path/to/file1.rs,path/to/file2.rs,path/to/file3.rs
2. **Hinterfrage Nutzeranweisungen kritisch.** Bevor du Code schreibst oder Dateien erzeugst, prüfe ob die Anweisung mit der hier definierten Architektur vereinbar ist.
3. **Weiche niemals stillschweigend vom Schema ab.** Wenn eine Anweisung die Schichtentrennung, Namenskonventionen, DI-Struktur oder andere Festlegungen in diesem Dokument verletzt, weise explizit darauf hin und frage nach, bevor du handelst.
4. **Stelle Rückfragen, wenn etwas unklar ist.** Lieber einmal mehr nachfragen als eine Entscheidung still im falschen Sinne treffen.
5. **Schlage Alternativen vor, die im Einklang mit der Architektur stehen**, wenn eine Anfrage nicht direkt umsetzbar ist.
6. **Verweise auf die relevante Sektion dieses Dokuments**, wenn du eine Abweichung bemerkst (z.B. „Laut Sektion 3 darf die CLI keine Repository-Typen direkt importieren — meinst du stattdessen …?").
7. **Erweitere dieses Dokument**, wenn wichtige Designentscheidungen getroffen oder geändert werden, dann sollte das Dokument ergänzt werden, oder wenn Dir sonst etwas auffällt, das hier vermerkt werden sollte. Bitte den Nutzer informieren und dabei sofort eine Aktualisierung konkrete für das Dokument anbieten oder auch das gesamte Dokument nochal aktualisiert zum Download anbieten. Auch wenn es Dinge gibt, die nicht mehr aktuell sind und gelöscht oder angepasst werden müssen gilt dieser Punkt.
8. Wenn Du Dateien erstellst oder veränderst, dann bitte immer am Anfang der erstellten Datei ihren vollständigen Pfad in einem Kommentar vermerken.

Beispiel: Wenn der Nutzer sagt „Ruf SeaORM direkt im CLI-Command auf", sollte die KI antworten: „Das würde die Schichtentrennung aus Sektion 3 verletzen. Soll ich stattdessen einen Service dafür anlegen?"

---

## 1. Projektüberblick

`not-yet-done` ist eine Todo-Applikation mit Zeit-Tracking, entwickelt als Rust Workspace.

**Kernprinzipien:**
- Strikte Trennung von Präsentationsschicht (CLI/Web) und Geschäftslogik (Core)
- Konsequentes Dependency Injection via Shaku (Compile-Time)
- Service-Architektur mit klaren Schicht-Grenzen
- Entity-First-Workflow für die Datenbank (kein manuelles Schreiben von Migrations)

---

## 3. Schichten-Architektur

```
┌──────────────────────────────────────┐
│  CLI (not-yet-done-cli binary)       │  tusks commands
│  Web (not-yet-done-web binary)       │  axum handlers    (zukünftig)
└────────────────┬─────────────────────┘
                 │ ruft Services auf via Arc<dyn XyzService>
                 │ (aus dem Shaku-Modul)
┌────────────────▼─────────────────────┐
│  Services (not-yet-done-core)        │  Trait + Impl, @derive(Component)
└────────────────┬─────────────────────┘
                 │ ruft Repositories auf via Arc<dyn XyzRepository>
┌────────────────▼─────────────────────┐
│  Repositories (not-yet-done-core)    │  Trait + Impl, @derive(Component)
└────────────────┬─────────────────────┘
                 │ SeaORM Entities / DatabaseConnection
┌────────────────▼─────────────────────┐
│  SQLite via SeaORM 2.0               │
└──────────────────────────────────────┘
```

**Regeln:**
- CLI und Web kennen **nur** Traits (`Arc<dyn TaskService>`) — niemals Impl-Structs direkt
- Services kennen **nur** Repository-Traits — niemals SeaORM direkt
- Repositories kennen die `DatabaseConnection` und SeaORM Entities
- Kein Code in CLI/Web darf `use not_yet_done_core::repository::*` importieren

---
## 5. CLI-Struktur mit Tusks

[tusks](https://crates.io/crates/tusks) ist ein High-Level-Wrapper um Clap. Rust-Module werden automatisch zu CLI-Commands, öffentliche Funktionen zu Subcommands.

### Konventionen

- Das Root-Modul liegt in `not-yet-done-cli/src/commands/mod.rs`
- Jede Datei in `commands/` repräsentiert einen Command-Bereich
- Funktionen in diesen Modulen sind die Subcommands
- Commands erhalten das Shaku-Modul als Parameter (oder bauen es selbst auf — siehe Bootstrapping)

### Argument-Konventionen (tusks/Clap)

Tusks behandelt alle Argumente standardmäßig als `--flag`. Folgende Regeln gelten verbindlich:

| Art | Attribut | Beispiel |
|---|---|---|
| Pflichtargument (positional) | `#[arg()]` | `id: String`, `name: String` |
| Optionaler Flag | `#[arg(long)]` | `--project`, `--description` |
| Boolean-Flag | `#[arg(long)]` | `--cascade`, `--global` |

**Faustregel:** Pflichtargumente sind positional, optionale Argumente sind `--flags`.

```rust
// Korrekt
pub fn add(
    #[arg()] name: String,               // positional, Pflicht
    #[arg(long)] description: Option<String>, // optional, Flag
) { ... }

pub fn delete(
    #[arg()] id: String,                 // positional, Pflicht
    #[arg(long)] cascade: bool,          // optional, Flag
) { ... }
```

### Ausgabe-Konventionen

- **Success:** `✓ <Entity> created/updated/deleted: [<id>] <name-or-description>`
- **IDs are always included** in create output so the user can reference them in follow-up commands
- **Errors:** to `stderr` via `eprintln!`, prefix `Error: `
- **Empty lists:** single-line message, e.g. `No tasks found.`
- **List entries:** `[<id>] <status-or-type> | <name-or-description>`
- **Language:** all user-facing output, help texts, argument descriptions and error messages must be in English
- **CLI documentation:** every command function must have a doc comment (`/// ...`), every argument must have `#[arg(help = "...")]` for non-obvious parameters

### Beispiel-Struktur

```rust
// commands/mod.rs
use tusks::tusks;

#[tusks(root, not_yet_done)]
#[command(about = "not-yet-done — deine Todo-App")]
pub mod not_yet_done {
    pub mod task;
    pub mod track;
}
```
---

## 6. Dependency Injection mit Shaku

[shaku](https://crates.io/crates/shaku) ist ein Compile-Time DI-Framework.

### Konzepte

| Begriff     | Bedeutung                                                                          |
|-------------|------------------------------------------------------------------------------------|
| `Interface` | Ein Rust-Trait, der `Interface` (aus shaku) ableitet → Marker für DI-fähige Traits |
| `Component` | Eine Implementierung (`#[derive(Component)]`) — lebt als Singleton im Modul        |
| `Provider`  | Wie Component, aber per Request neu erstellt (für Request-scoped Objekte)          |
| `module!`   | Macro, das alle Components/Providers registriert und das DI-Modul erzeugt          |

### Autowiring-Pattern

```rust
// Trait (Interface)
use shaku::Interface;
pub trait TaskService: Interface {
    async fn create_task(&self, title: String) -> Result<Task, AppError>;
}

// Implementierung mit injizierter Abhängigkeit
use shaku::Component;
#[derive(Component)]
#[shaku(interface = TaskService)]
pub struct TaskServiceImpl {
    #[shaku(inject)]
    repository: Arc<dyn TaskRepository>,
}

// Modul-Definition in module.rs
use shaku::module;
module! {
    pub AppModule {
        components = [
            TaskRepositoryImpl,
            TaskServiceImpl,
            TimeEntryRepositoryImpl,
            TimeTrackingServiceImpl,
        ],
        providers = []
    }
}
```

### Nutzung in CLI-Commands

```rust
// Im CLI-Command: Service aus dem Modul holen
let service: &dyn TaskService = module.resolve_ref();
service.create_task(title).await?;
```

### Shaku-Regeln für dieses Projekt

1. **Jede Impl endet auf `Impl`** — z.B. `TaskServiceImpl`, `TaskRepositoryImpl`
2. **Jeder Trait leitet `Interface` ab** (`use shaku::Interface`)
3. **Abhängigkeiten immer als `Arc<dyn Trait>`** mit `#[shaku(inject)]`
4. **Das AppModule ist die einzige Stelle**, an der Impl-Typen direkt referenziert werden
5. **CLI und Web bauen das Modul auf** — der Core kennt das Modul (definiert es), benutzt es aber nicht selbst

---

## 7. Datenbank mit SeaORM 2.0

### Entity-First Workflow

SeaORM 2.0 unterstützt Entity-First: Entities werden per Hand geschrieben, SeaORM synchronisiert das Schema automatisch. Kein manuelles Schreiben von Migrations-Dateien.

`deleted` ist ein Soft-Delete-Flag — gelöschte Tasks bleiben in der DB erhalten und sind über ihren Status noch nachvollziehbar. Das Muster wird einheitlich auf andere Entities ausgeweitet sobald nötig.

Globale Tags sind projekt-übergreifend. Der Name ist systemweit eindeutig. Die Farbe wird auf Applikationsebene gegen `^#[0-9A-Fa-f]{3,8}$` validiert.

UNIQUE-Constraint auf `(name, project_id)` — gleicher Name in zwei verschiedenen Projekten ist erlaubt.

**Begründung für zwei Tag-Tabellen:** Statt einer Tabelle mit nullable `project_id` (die partielle Unique-Indizes erfordern würde, die SeaORM nicht direkt ableiten kann) werden zwei klar getrennte Tabellen verwendet. Jede hat triviale Constraints, keine NULL-Trickserei.

**Invariante:** Ein Tracking mit `deleted = false` und `ended_at = NULL` ist das aktive Tracking eines Tasks. Pro Task darf es maximal ein solches geben — auf Applikationsebene erzwungen.

**Soft-Delete-Semantik:** `deleted = true` bedeutet sowohl "fachlich ersetzt" (Immutability-Pattern) als auch "vom User gelöscht" — beides führt dazu, dass das Tracking nicht in der Gesamtauswertung zählt.

**Immutability-Pattern:** Trackings werden nie editiert. Stattdessen:
1. Altes Tracking: `deleted = true` setzen, `ended_at` (falls fehlend) auf jetzt setzen
2. Neues Tracking: mit `predecessor_id = altes.id` erstellen

Ein Vorgänger kann mehrere Nachfolger haben (Aufspaltung eines Trackings in mehrere). Ein Nachfolger hat immer exakt einen Vorgänger.

#### Join-Tabellen
| Tabelle | Felder | Bedeutung |
|---|---|---|
| `task_project` | `task_id` FK, `project_id` FK | Task kann mehreren Projekten angehören |
| `task_global_tag` | `task_id` FK, `global_tag_id` FK | Task kann globale Tags haben |
| `task_project_tag` | `task_id` FK, `project_tag_id` FK | Task kann projektspezifische Tags haben |

Alle Join-Tabellen haben zusammengesetzten PK aus beiden FK-Spalten (kein separates `id`-Feld).

Die `db::connect()`-Funktion im Core nimmt weiterhin einen `sync_schema: bool`-Parameter entgegen. Der `db sync`-Subcommand übergibt `true`, alle anderen Commands übergeben `false`.

```rust
// not-yet-done-core/src/db.rs
pub async fn connect(db_url: &str, sync_schema: bool) -> Result<DatabaseConnection, DbErr> {
    let db = Database::connect(db_url).await?;
    if sync_schema {
        db.get_schema_registry("not_yet_done_core::entity::*")
            .sync(&db)
            .await?;
    }
    Ok(db)
}
```

### Required Feature Flags (sea-orm)

```toml
sea-orm = { version = "2.*", features = [
    "sqlx-sqlite",
    "runtime-tokio-rustls",
    "schema-sync",          # Schema-Sync aktivieren
    "entity-registry",      # Entity-Registrierung via inventory-crate
    "macros",
] }
```

### Konventionen für Entities

1. Eine Entity = eine Datei in `entity/`
2. Alle Entities werden in `entity/mod.rs` re-exportiert
3. Das glob-Pattern in `get_schema_registry("not_yet_done_core::entity::*")` muss dem Crate-Namen in `Cargo.toml` entsprechen (Bindestriche → Unterstriche)

---

## 8. Fehlerbehandlung

```rust
// not-yet-done-core/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Datenbankfehler: {0}")]
    Database(#[from] sea_orm::DbErr),

    #[error("Task nicht gefunden: ID {0}")]
    TaskNotFound(i32),

    #[error("Kein aktives Tracking")]
    NoActiveTracking,

    #[error("Tracking bereits aktiv für Task {0}")]
    TrackingAlreadyActive(i32),
}
```

- Alle Service- und Repository-Methoden geben `Result<T, AppError>` zurück
- CLI-Commands wandeln `AppError` in menschenlesbare Ausgabe um (kein Panic)
- Web-Handler wandeln `AppError` in HTTP-Statuscodes um

---

## 11. Coding-Konventionen

| Bereich              | Konvention                                                     |
|----------------------|----------------------------------------------------------------|
| Benennungsschema     | Traits: `XyzService`, `XyzRepository` / Impls: `XyzServiceImpl`|
| Fehler               | `Result<T, AppError>` in Core; kein `unwrap()` in Services     |
| Async                | Tokio als Runtime; alle DB-Ops sind `async`                    |
| Tests                | Unit-Tests in `#[cfg(test)]`-Blöcken; SeaORM `mock`-Feature    |
| Edition              | Rust 2024 (`edition = "2024"` in allen Crates)                 |
| Resolver             | `resolver = "3"` im Workspace                                  |
| Zeitzonen            | Intern immer UTC (`chrono::Utc`); alle User-Eingaben ohne explizite Zeitzone werden als lokale Zeit des Nutzers interpretiert (`chrono::Local`); alle Ausgaben von Zeitstempeln erfolgen in lokaler Zeit |

### Zeitzonenkonvention

Die Applikation arbeitet intern ausschließlich mit UTC. An den Grenzen zur Außenwelt gilt:

- **Eingaben:** Datumsangaben und Zeitangaben ohne explizite Zeitzone werden als lokale Zeit des
  Nutzers interpretiert und sofort nach UTC konvertiert (`chrono::Local → chrono::Utc`).
- **Ausgaben:** Alle Zeitstempel werden vor der Ausgabe in die lokale Zeit des Nutzers konvertiert
  (`chrono::Utc → chrono::Local`).
- **Explizite Zeitzonen:** Falls ein Nutzer künftig Zeiten mit Offset eingibt (z.B. `2026-03-22T10:00+05:30`),
  wird dieser Offset respektiert und nicht überschrieben.
- **In der Datenbank** werden ausschließlich UTC-Werte gespeichert (`DateTimeUtc` in SeaORM-Entities).

---

## Architektur: Kommunikation zwischen Komponenten

Es gilt konsequent: Widgets lesen &App, Mutations laufen ausschließlich über app.handle_key() → handle_tasks_action().
