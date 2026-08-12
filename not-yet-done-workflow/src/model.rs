//! The in-memory model of a workflow **definition** — the parsed form of one
//! `.md` file. This is a read-only projection: the file on disk is the single
//! source of truth (edited verbatim), and [`parse`](crate::parse) rebuilds this
//! model from it. Nothing here serialises back to markdown.
//!
//! # Shape
//!
//! A [`WorkflowDef`] is an ordered list of [`Step`]s plus workflow-level
//! metadata (title, description, default [`StepMode`], run-logging flag). Each
//! step carries up to three *execution facets* — a `title`, an optional prose
//! `description` (the instruction a human **or** an AI follows), and an optional
//! `command` (the deterministic automation) — and an optional per-step mode that
//! overrides the workflow default.
//!
//! Control flow is a DAG: each step declares its **own** outgoing [`Route`]s
//! (guard condition → target step(s)), so a step is coupled only to what comes
//! *after* it, never to its predecessor. The common linear case leaves the
//! routes empty and the runner (a later phase) falls back to "the next step in
//! document order", so a plain checklist needs no routing syntax at all while
//! the model already carries the full branching/parallel structure for when it
//! is needed. A route may fan out to several targets (parallel branches) and may
//! point at the reserved terminals [`RouteTarget::End`] / [`RouteTarget::Fail`].

/// How a step is carried out. The workflow declares a default; a step may
/// override it. A step whose resolved mode needs a facet it lacks (an `Auto`
/// step with no `command`, an `Ai` step with no `description`) degrades to
/// `Manual` at run time rather than failing — "as automatic as possible".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepMode {
    /// Show the title/description; the user performs the step and marks it done.
    Manual,
    /// Run the step's `command` (deterministic automation, no AI).
    Auto,
    /// Hand the `description` (plus run context) to the configured AI runner,
    /// which drives the app's own CLI to carry the step out.
    Ai,
}

impl Default for StepMode {
    fn default() -> Self {
        StepMode::Manual
    }
}

impl StepMode {
    /// Parse a mode token (`manual` / `auto` / `ai`), case-insensitively.
    /// Returns `None` for anything else so the caller can decide whether an
    /// unknown token is an error or a silently-ignored default.
    pub fn parse(s: &str) -> Option<StepMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(StepMode::Manual),
            "auto" => Some(StepMode::Auto),
            "ai" => Some(StepMode::Ai),
            _ => None,
        }
    }

    /// The canonical token for this mode (round-trips with [`StepMode::parse`]).
    pub fn as_str(self) -> &'static str {
        match self {
            StepMode::Manual => "manual",
            StepMode::Auto => "auto",
            StepMode::Ai => "ai",
        }
    }

    /// Whether this mode runs without user interaction (`auto` or `ai`).
    pub fn is_automatic(self) -> bool {
        matches!(self, StepMode::Auto | StepMode::Ai)
    }
}

/// Where a [`Route`] points. A plain step id, or one of the reserved terminals
/// that end the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteTarget {
    /// Continue at the step with this id.
    Step(String),
    /// End the run successfully.
    End,
    /// End the run as failed.
    Fail,
}

impl RouteTarget {
    /// Parse a target token: `end`/`fail` (case-insensitive) are the reserved
    /// terminals; anything else is a step id (kept verbatim).
    pub fn parse(s: &str) -> RouteTarget {
        match s.trim().to_ascii_lowercase().as_str() {
            "end" => RouteTarget::End,
            "fail" => RouteTarget::Fail,
            _ => RouteTarget::Step(s.trim().to_string()),
        }
    }

    /// The step id this target names, if it is a step (not a terminal).
    pub fn step_id(&self) -> Option<&str> {
        match self {
            RouteTarget::Step(id) => Some(id),
            _ => None,
        }
    }
}

/// The guard on a [`Route`]. Either a named convenience guard, the `else`
/// fallback, or an expression evaluated against the step's result at run time (a
/// later phase; kept as raw text here). The variables an expression may
/// reference depend on the step's mode — e.g. `exit`, `stdout`, `stderr` for an
/// `auto` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteCondition {
    /// The fallback taken when no earlier route matched.
    Else,
    /// The step succeeded — convenience for `exit == 0` on a command step (and
    /// the AI reporting done / the user marking a manual step done).
    OnSuccess,
    /// The step failed — convenience for `exit > 0` on a command step (and the
    /// AI reporting failure).
    OnFailure,
    /// A guard expression, evaluated at run time.
    Expr(String),
}

/// One outgoing edge of a step: a guard and where it routes to. A route with
/// more than one target fans out into parallel branches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The guard that selects this route.
    pub condition: RouteCondition,
    /// The successor(s). Multiple targets run concurrently.
    pub targets: Vec<RouteTarget>,
}

/// A declared reason to start a run of this workflow **without** a person asking
/// (Phase 6c). Triggers live in the frontmatter and are evaluated by the
/// adapter's background scheduler; they never affect a manually started run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// Fire on a cron schedule (a standard 5-field cron expression, evaluated in
    /// local time). The raw text is kept verbatim; the scheduler parses it.
    Cron(String),
    /// Fire when a matching event is seen on the host event bus, keyed by its
    /// `topic` (e.g. `"ci:push"`).
    Event(String),
}

/// One step of a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Stable identifier, unique within the workflow. Taken from the `id:` key of
    /// the step's `yaml meta` block when present, otherwise derived from the
    /// title (slugified) and de-duplicated. Used to key run state and to name
    /// route targets.
    pub id: String,
    /// Human title (the `##` heading text).
    pub title: String,
    /// Optional prose instruction — what to do, for a human or an AI. May be
    /// empty.
    pub description: String,
    /// Optional deterministic command for `auto` execution.
    pub command: Option<String>,
    /// Per-step mode override; `None` inherits the workflow default.
    pub mode: Option<StepMode>,
    /// Whether the step may be skipped without failing the workflow.
    pub optional: bool,
    /// Outgoing routes (empty = linear: fall through to the next step in
    /// document order). Evaluated top-to-bottom, first matching route wins.
    pub routes: Vec<Route>,
}

impl Step {
    /// The mode this step runs in, given the workflow default — the per-step
    /// override if set, else `default`.
    pub fn resolved_mode(&self, default: StepMode) -> StepMode {
        self.mode.unwrap_or(default)
    }

    /// Whether the step declares explicit routing (opts out of linear flow).
    pub fn has_routing(&self) -> bool {
        !self.routes.is_empty()
    }
}

/// A parsed workflow definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDef {
    /// Stable id of the workflow — the file stem (without `.md`). Addresses the
    /// workflow across frontends and keys its runs.
    pub name: String,
    /// Human title. Falls back to `name` when neither frontmatter nor a leading
    /// `# ` heading supplies one.
    pub title: String,
    /// Optional workflow-level description (the prose before the first step).
    pub description: String,
    /// Default execution mode applied to steps that don't override it.
    pub mode: StepMode,
    /// Whether runs of this workflow are recorded to the run store. `None` in
    /// the file means "unset"; the adapter resolves it against its config
    /// default. Kept as `Option` so an unset flag is distinguishable.
    pub log_runs: Option<bool>,
    /// Ordered steps, in document order.
    pub steps: Vec<Step>,
    /// Declared triggers that start a run without a person asking (Phase 6c).
    /// Empty for a plain workflow that is only ever run by hand.
    pub triggers: Vec<Trigger>,
}

impl WorkflowDef {
    /// Whether every step can run without user interaction — i.e. the whole
    /// workflow could be executed fully automatically. An empty workflow is not
    /// considered auto-runnable.
    pub fn is_auto_runnable(&self) -> bool {
        !self.steps.is_empty()
            && self
                .steps
                .iter()
                .all(|s| s.resolved_mode(self.mode).is_automatic())
    }

    /// Look up a step by its id.
    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }
}

/// Slugify a title into an id fragment: lowercase, runs of non-alphanumeric
/// characters collapsed to a single `_`, trimmed of leading/trailing `_`. An
/// all-punctuation title yields an empty string (the caller substitutes a
/// positional fallback).
pub fn slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_us = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_us = false;
        } else if !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parse_round_trips_and_rejects_unknown() {
        for m in [StepMode::Manual, StepMode::Auto, StepMode::Ai] {
            assert_eq!(StepMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(StepMode::parse("  AUTO "), Some(StepMode::Auto));
        assert_eq!(StepMode::parse("nope"), None);
        assert!(StepMode::Auto.is_automatic() && StepMode::Ai.is_automatic());
        assert!(!StepMode::Manual.is_automatic());
    }

    #[test]
    fn slug_collapses_and_trims() {
        assert_eq!(slug("Build the Release!"), "build_the_release");
        assert_eq!(slug("Tests green?"), "tests_green");
        assert_eq!(slug("  a — b  "), "a_b");
        assert_eq!(slug("***"), "");
    }

    #[test]
    fn auto_runnable_requires_every_step_automatic() {
        let mut wf = WorkflowDef {
            name: "w".into(),
            title: "W".into(),
            description: String::new(),
            mode: StepMode::Auto,
            log_runs: None,
            steps: vec![],
            triggers: vec![],
        };
        // Empty is not auto-runnable.
        assert!(!wf.is_auto_runnable());

        let step = |id: &str, mode: Option<StepMode>| Step {
            id: id.into(),
            title: id.into(),
            description: String::new(),
            command: None,
            mode,
            optional: false,
            routes: Vec::new(),
        };
        // All inherit the auto default → auto-runnable.
        wf.steps = vec![step("a", None), step("b", None)];
        assert!(wf.is_auto_runnable());
        // One manual override breaks it.
        wf.steps.push(step("c", Some(StepMode::Manual)));
        assert!(!wf.is_auto_runnable());
    }
}
