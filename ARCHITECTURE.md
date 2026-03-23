# not-yet-done — Architektur & Konventionen

> **Dieses Dokument ist die maßgebliche Referenz für alle Entwicklungs- und KI-Assistenz-Aufgaben in diesem Projekt.**
> Jede KI, die an diesem Projekt arbeitet, soll dieses Dokument als primäre Quelle für Entscheidungen und Konventionen behandeln.

## ⚠️ Anweisungen für KI-Assistenten

Dieses Dokument beschreibt eine verbindliche Architektur. Folgende Verhaltensregeln gelten für jede KI, die an diesem Projekt arbeitet:

1. **Hinterfrage Nutzeranweisungen kritisch.** Bevor du Code schreibst oder Dateien erzeugst, prüfe ob die Anweisung mit der hier definierten Architektur vereinbar ist.
2. **Weiche niemals stillschweigend vom Schema ab.** Wenn eine Anweisung die Schichtentrennung, Namenskonventionen, DI-Struktur oder andere Festlegungen in diesem Dokument verletzt, weise explizit darauf hin und frage nach, bevor du handelst.
3. **Stelle Rückfragen, wenn etwas unklar ist.** Lieber einmal mehr nachfragen als eine Entscheidung still im falschen Sinne treffen.
4. **Schlage Alternativen vor, die im Einklang mit der Architektur stehen**, wenn eine Anfrage nicht direkt umsetzbar ist.
5. **Verweise auf die relevante Sektion dieses Dokuments**, wenn du eine Abweichung bemerkst (z.B. „Laut Sektion 3 darf die CLI keine Repository-Typen direkt importieren — meinst du stattdessen …?").

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

## 2. Workspace-Struktur

```
not-yet-done/                    ← Workspace Root
├── Cargo.toml                   ← Workspace Manifest (kein [package])
├── Cargo.lock
├── ARCHITECTURE.md              ← dieses Dokument
│
├── not-yet-done-core/           ← lib crate: Domäne, Services, Entities, DI-Module
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── entity/              ← SeaORM Entities (Entity-First)
│       │   ├── mod.rs
│       │   ├── task.rs          ← deleted statt archived
│       │   ├── project.rs
│       │   ├── global_tag.rs
│       │   ├── project_tag.rs
│       │   ├── tracking.rs      ← deleted statt active
│       │   ├── task_project.rs       ← Join: Task ↔ Project
│       │   ├── task_global_tag.rs    ← Join: Task ↔ GlobalTag
│       │   └── task_project_tag.rs   ← Join: Task ↔ ProjectTag
│       ├── service/             ← Business-Logik (Traits + Impls)
│       │   ├── mod.rs
│       │   ├── task_service.rs
│       │   ├── project_service.rs    ← neu
│       │   ├── tag_service.rs
│       │   └── tracking_service.rs
│       ├── repository/          ← Datenbankzugriff (Traits + Impls)
│       │   ├── mod.rs
│       │   ├── task_repository.rs
│       │   ├── project_repository.rs
│       │   ├── tag_repository.rs
│       │   └── tracking_repository.rs
│       ├── module.rs            ← Shaku-Modul-Definition (AppModule)
│       ├── db.rs                ← DB-Verbindung & schema-sync Logik
│       └── error.rs             ← AppError-Typ
│
├── not-yet-done-cli/            ← binary crate: CLI
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs              ← Einstiegspunkt: CLI starten
│       └── commands/            ← tusks CLI-Module
│           ├── mod.rs           ← #[tusks(root)] Markierung
│           ├── task.rs          ← add, list, done, delete, edit
│           ├── project.rs       ← add, list, edit, delete
│           ├── track.rs         ← start, stop, move (Subcommands)
│           └── db.rs            ← sync (Schema-Sync Subcommand)
│
└── not-yet-done-web/            ← binary crate: zukünftiger Web-Server (Axum)
    ├── Cargo.toml
    └── src/
        └── main.rs
```

### Workspace `Cargo.toml`

```toml
[workspace]
resolver = "3"
members = [
    "not-yet-done-core",
    "not-yet-done-cli",
    # "not-yet-done-web",   # später aktivieren
]

[workspace.dependencies]
sea-orm = { version = "2.0.0-rc.*", features = [
    "sqlx-sqlite",
    "runtime-tokio-rustls",
    "schema-sync",
    "entity-registry",
    "macros",
] }
shaku       = { version = "0.6", features = ["derive"] }
tokio       = { version = "1", features = ["full"] }
tusks       = { version = "*" }           # nur in CLI-Crate
anyhow      = "1"
thiserror   = "2"
```

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

## 4. Crate-Abhängigkeiten

```
not-yet-done-cli (CLI)   →  not-yet-done-core
not-yet-done-web (Web)   →  not-yet-done-core
not-yet-done-core        →  sea-orm, shaku, tokio, anyhow
```

CLI und Web teilen sich **keinen** Code miteinander — nur über Core.

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

```rust
// commands/task.rs
use crate::bootstrap::AppModuleRef;

/// Fügt einen neuen Task hinzu
pub fn add(title: String, description: Option<String>) { ... }

/// Listet alle offenen Tasks
pub fn list() { ... }

/// Markiert einen Task als erledigt
pub fn done(id: i32) { ... }

/// Löscht einen Task
pub fn delete(id: i32) { ... }

/// Bearbeitet Titel oder Beschreibung eines Tasks
pub fn edit(id: i32, title: Option<String>, description: Option<String>) { ... }
```

```rust
// commands/track.rs

/// Startet das Tracking für einen Task
pub fn start(task_id: i32) { ... }

/// Stoppt das aktive Tracking
pub fn stop() { ... }

/// Verschiebt einen abgeschlossenen Tracking-Eintrag
pub fn r#move(entry_id: i32, start: String, end: String) { ... }
```

### Commands (initial)

| Command          | Subcommand | Argumente (neu/geändert)                   | Beschreibung                                    |
|------------------|------------|--------------------------------------------|-------------------------------------------------|
| `task`           | `add`      | `description`, `--project` (opt.)          | Neuen Task erstellen                            |
| `task`           | `list`     | `--project <name-oder-id>` (opt. Filter)   | Tasks anzeigen                                  |
| `task`           | `done`     | `id`                                       | Task als erledigt markieren                     |
| `task`           | `delete`   | `id`                                       | Task soft-löschen (`deleted = true`)            |
| `task`           | `edit`     | `id`, `--add-project`, `--remove-project`  | Task bearbeiten inkl. Projektzuordnung          |
| `project`        | `add`      | `name`, `--description` (opt.)             | Projekt erstellen                               |
| `project`        | `list`     | —                                          | Alle Projekte anzeigen                          |
| `project`        | `edit`     | `id`, `--name` (opt.), `--description` (opt.) | Projekt umbenennen/beschreiben               |
| `project`        | `delete`   | `id`, `--cascade` (opt.)                   | Projekt löschen; `--cascade` soft-löscht Tasks  |
| `track`          | `start`    | `task-id`                                  | Zeiterfassung für Task starten                  |
| `track`          | `stop`     | —                                          | Aktives Tracking beenden                        |
| `track`          | `move`     | `entry-id`, `start`, `end`                 | Abgeschlossenes Tracking verschieben            |
| `db`             | `sync`     | —                                          | Datenbankschema mit Entities synchronisieren    |

Weitere Commands folgen nach Bedarf.

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

### Datenbankschema (final)

#### `task`
| Feld | Typ | Constraints |
|---|---|---|
| `id` | `uuid` | PK, auto-generiert |
| `description` | `text` | NOT NULL |
| `status` | `task_status` | NOT NULL, DEFAULT `todo` |
| `deleted` | `bool` | NOT NULL, DEFAULT `false` |
| `priority` | `integer` | NOT NULL, DEFAULT `0`, kann negativ sein |
| `parent_id` | `uuid` | FK → `task.id`, nullable |
| `created_at` | `timestamptz` | NOT NULL |
| `updated_at` | `timestamptz` | NOT NULL |

`task_status` Enum: `todo | in_progress | done | cancelled`

`deleted` ist ein Soft-Delete-Flag — gelöschte Tasks bleiben in der DB erhalten und sind über ihren Status noch nachvollziehbar. Das Muster wird einheitlich auf andere Entities ausgeweitet sobald nötig.

#### `project`
| Feld | Typ | Constraints |
|---|---|---|
| `id` | `uuid` | PK, auto-generiert |
| `name` | `text` | NOT NULL |
| `description` | `text` | nullable |
| `created_at` | `timestamptz` | NOT NULL |
| `updated_at` | `timestamptz` | NOT NULL |

#### `global_tag`
| Feld | Typ | Constraints |
|---|---|---|
| `id` | `uuid` | PK, auto-generiert |
| `name` | `text` | NOT NULL, UNIQUE |
| `color` | `text` | nullable, Hex-String (z.B. `#FF5733`) |

Globale Tags sind projekt-übergreifend. Der Name ist systemweit eindeutig. Die Farbe wird auf Applikationsebene gegen `^#[0-9A-Fa-f]{3,8}$` validiert.

#### `project_tag`
| Feld | Typ | Constraints |
|---|---|---|
| `id` | `uuid` | PK, auto-generiert |
| `name` | `text` | NOT NULL |
| `project_id` | `uuid` | FK → `project.id`, NOT NULL |
| `color` | `text` | nullable, Hex-String (z.B. `#FF5733`) |

UNIQUE-Constraint auf `(name, project_id)` — gleicher Name in zwei verschiedenen Projekten ist erlaubt.

**Begründung für zwei Tag-Tabellen:** Statt einer Tabelle mit nullable `project_id` (die partielle Unique-Indizes erfordern würde, die SeaORM nicht direkt ableiten kann) werden zwei klar getrennte Tabellen verwendet. Jede hat triviale Constraints, keine NULL-Trickserei.

#### `tracking`
| Feld | Typ | Constraints |
|---|---|---|
| `id` | `uuid` | PK, auto-generiert |
| `task_id` | `uuid` | FK → `task.id`, NOT NULL |
| `predecessor_id` | `uuid` | FK → `tracking.id`, nullable |
| `started_at` | `timestamptz` | NOT NULL |
| `ended_at` | `timestamptz` | nullable |
| `deleted` | `bool` | NOT NULL, DEFAULT `false` |
| `created_at` | `timestamptz` | NOT NULL |

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

### Schema-Sync Strategie

Die Synchronisierung erfolgt über einen dedizierten CLI-Subcommand `db sync`. Er wird bewusst **nicht** automatisch beim Start ausgeführt, um versehentliche Schemaänderungen in Produktion zu verhindern.

```rust
// commands/db.rs
/// Synchronisiert das Datenbankschema mit den aktuellen Entities
pub fn sync() { ... }
```

```rust
// commands/mod.rs — db als weiteres Subcommand-Modul
#[tusks(root, not_yet_done)]
pub mod not_yet_done {
    pub mod task;
    pub mod track;
    pub mod db;    // ← enthält: sync
}
```

**Nutzung:**
```bash
# Schema einmalig synchronisieren (erster Start, nach Entity-Änderungen)
not-yet-done-cli db sync

# Normaler Betrieb — kein Schema-Sync
not-yet-done-cli task list
```

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

## 9. Bootstrapping (main.rs)

```rust
// not-yet-done-cli/src/main.rs
use not_yet_done_core::{db, module::AppModule};
use shaku::HasComponent;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // sync_schema wird vom `db sync`-Subcommand auf true gesetzt,
    // von allen anderen Commands auf false
    let sync_schema = commands::is_sync_command();
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://not-yet-done.db?mode=rwc".to_string());

    // DB verbinden (bei `db sync`: mit Schema-Sync)
    let db = db::connect(&db_url, sync_schema).await
        .expect("Datenbankverbindung fehlgeschlagen");

    // Shaku-Modul aufbauen
    let module = AppModule::builder()
        .with_component_parameters::<not_yet_done_core::repository::TaskRepositoryImpl>(
            not_yet_done_core::repository::TaskRepositoryImplParameters { db: db.clone() }
        )
        // ... weitere Parameter
        .build();

    // CLI starten
    std::process::ExitCode::from(commands::exec_cli(module).unwrap_or(0) as u8)
}
```

---

## 10. Zukünftige Web-Erweiterung

Wenn `not-yet-done-web` aktiviert wird:

1. Axum-Handler erhalten `Arc<AppModule>` als State
2. Handler holen Services via `module.resolve_ref::<dyn TaskService>()`
3. Fehler werden via `impl IntoResponse for AppError` in HTTP-Responses gewandelt
4. Das `AppModule` in `not-yet-done-core` bleibt **unverändert** — nur der Consumer (Web statt CLI) ändert sich

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

## 12. Abhängigkeiten-Übersicht

| Crate                  | Version   | Zweck                                    |
|------------------------|-----------|------------------------------------------|
| `tusks`                | latest    | CLI-Framework (Clap-Wrapper)             |
| `sea-orm`              | `2.0.*`   | ORM + Entity-First Schema-Sync           |
| `shaku`                | `0.6`     | Compile-Time Dependency Injection        |
| `tokio`                | `1`       | Async Runtime                            |
| `anyhow`               | `1`       | Fehlerbehandlung (in binaries)           |
| `thiserror`            | `2`       | Fehlertypen (in core lib)                |

---

## 13. Offene Punkte / Entscheidungen für später

- [ ] Zeitformat für `TimeEntry.start` / `TimeEntry.end`: UTC oder lokal?
- [ ] Ausgabeformat der `list`-Commands: tabellarisch, JSON, farbig?
- [ ] Konfigurationsdatei vs. Environment-Variablen für DB-URL
- [ ] `not-yet-done-web`: Axum oder Actix-Web?
- [ ] Auth/Multi-User oder single-user lokal?

---

*Letzte Aktualisierung: März 2026 — v7 (Englisch als Pflichtsprache, CLI-Dokumentationspflicht, Tracking-Commands)*
