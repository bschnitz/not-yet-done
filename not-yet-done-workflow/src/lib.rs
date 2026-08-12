//! Local **workflow** definitions — an ordered set of steps, kept as
//! git-friendly Markdown files and (later) surfaced as a content adapter with
//! per-run execution state persisted in SQLite.
//!
//! A workflow is a series of steps to carry out; it can be drawn as a diagram.
//! Each step has a title, an optional prose instruction (for a human **or** an
//! AI), and an optional command (deterministic automation) — so the same
//! definition can be run manually, fully automatically, or by an AI that drives
//! the app's own CLI. See [`model`] for the shape and [`parse`] for the file
//! format.
//!
//! # Phase 1 (this module set)
//!
//! * [`model`] — the parsed [`WorkflowDef`] / [`Step`] shape and [`StepMode`].
//! * [`parse`] — Markdown + YAML-frontmatter → [`WorkflowDef`].
//! * [`repo`] — flat filesystem CRUD over the `.md` files, plus `load`.
//!
//! Execution (manual/auto/AI runners), the run/protocol SQLite store, and the
//! `ContentAdapter` surface (`root → workflow → run → step-log`) arrive in
//! later phases and build on exactly these types.

pub mod adapter;
pub mod config;
pub mod entity;
pub mod exec;
pub mod factory;
pub mod guard;
pub mod mermaid;
pub mod model;
pub mod parse;
pub mod repo;
pub mod scheduler;
pub mod store;

pub use adapter::WorkflowAdapter;
pub use config::WorkflowConfig;
pub use factory::WorkflowAdapterFactory;
pub use guard::{eval as eval_guard, GuardVars};
pub use model::{slug, Route, RouteCondition, RouteTarget, Step, StepMode, Trigger, WorkflowDef};
pub use parse::parse_workflow;
pub use repo::{default_root, normalize_name, WorkflowEntry, WorkflowRepo};
pub use store::{connect, default_sqlite_url, NewStep, RunRow, RunStore, StepRow};
