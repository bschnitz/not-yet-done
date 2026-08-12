//! Parse a workflow definition file (Markdown + YAML frontmatter) into a
//! [`WorkflowDef`].
//!
//! # File format
//!
//! A definition is YAML frontmatter (all keys optional) followed by `##`-headed
//! steps. Below, `<fence>` marks a triple-backtick line and the word after it is
//! the fence's *info string* — the parser keys off it:
//!
//! ```text
//! ---
//! title: Release cutting          # optional; falls back to a leading "# " or the file stem
//! mode: manual                    # optional workflow default: manual | auto | ai
//! log_runs: true                  # optional
//! ---
//!
//! Optional prose before the first step becomes the workflow description.
//!
//! ## Build                        # each "##" heading starts a step; text = its title
//! <fence>yaml meta                # per-step metadata (all keys optional)
//! id: build                       #   id defaults to the title slug when omitted
//! mode: auto                      #   overrides the workflow default for this step
//! optional: false
//! <fence>
//! Prose here is the step's description — the instruction a human or AI follows.
//! <fence>command
//! cargo build --release           # the `command` fence is the step's command
//! <fence>
//! <fence>yaml routing             # outgoing control flow (evaluated top-to-bottom)
//! exit == 0: tests-green          #   first matching guard wins
//! else: fail                      #   `else` is the fallback; targets `end`/`fail` end the run
//! <fence>
//!
//! ## Tests green?
//! ...
//! ```
//!
//! Recognised fences: `command` (the step's command), `yaml meta` (step metadata:
//! `id`, `mode`, `optional`), and `yaml routing` (the routing table). A routing
//! entry is `<guard>: <target>`; the guard is an expression, one of the
//! convenience guards `on_success` (= `exit == 0`) / `on_failure` (= `exit > 0`),
//! or the literal `else`; the target is a step id, the reserved `end`/`fail`, or
//! a `[a, b]` list that fans out into parallel branches. With no routing block a
//! step falls through to the next step in document order. Any other fenced block
//! (plain `yaml`, `rust`, `sh`, …) is preserved verbatim in the description, so
//! documentation snippets are never mistaken for configuration.

use serde::Deserialize;

use crate::model::{
    slug, Route, RouteCondition, RouteTarget, Step, StepMode, Trigger, WorkflowDef,
};

/// YAML frontmatter fields. All optional — a body-only file is valid.
#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    log_runs: Option<bool>,
    #[serde(default)]
    triggers: Vec<TriggerSpec>,
}

/// One `triggers:` list entry: exactly one of `cron`/`event` is expected.
/// Extra/absent keys are tolerated (an empty entry is dropped), matching the
/// parser's "malformed is ignored, never fatal" contract.
#[derive(Debug, Default, Deserialize)]
struct TriggerSpec {
    #[serde(default)]
    cron: Option<String>,
    #[serde(default)]
    event: Option<String>,
}

impl TriggerSpec {
    /// The [`Trigger`] this entry declares, if any. `cron` takes precedence when
    /// both are set; an entry with neither (or only blanks) yields `None`.
    fn into_trigger(self) -> Option<Trigger> {
        let cron = self
            .cron
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(expr) = cron {
            return Some(Trigger::Cron(expr));
        }
        let event = self
            .event
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        event.map(Trigger::Event)
    }
}

/// Parse `raw` (the full file contents) into a [`WorkflowDef`]. `name` is the
/// workflow id (the file stem); it seeds the title when nothing else supplies
/// one. Parsing never fails — malformed frontmatter is treated as absent and
/// unknown directives are ignored — so a half-written file still lists.
pub fn parse_workflow(name: &str, raw: &str) -> WorkflowDef {
    let (fm, body) = split_frontmatter(raw);

    let default_mode = fm
        .mode
        .as_deref()
        .and_then(StepMode::parse)
        .unwrap_or_default();

    let mut intro = String::new();
    let mut heading_title: Option<String> = None;
    let mut steps: Vec<Step> = Vec::new();
    let mut cur: Option<StepBuilder> = None;

    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // Step heading: `## Title` (exactly two hashes, to leave `#`/`###` for
        // prose structure). Finalises the previous step.
        if let Some(title) = trimmed.strip_prefix("## ") {
            if let Some(b) = cur.take() {
                steps.push(b.finish());
            }
            cur = Some(StepBuilder::new(title.trim()));
            i += 1;
            continue;
        }

        // A leading single `# Title` before any step seeds the workflow title.
        if cur.is_none() {
            if let Some(t) = trimmed.strip_prefix("# ") {
                if heading_title.is_none() {
                    heading_title = Some(t.trim().to_string());
                    i += 1;
                    continue;
                }
            }
        }

        // Fenced code block. Recognised info strings drive the step; any other
        // fence is verbatim description text.
        if let Some(info) = fence_info(trimmed) {
            let (inner, consumed) = read_fence(&lines, i);
            i += consumed;
            let mut toks = info.split_whitespace();
            match (toks.next(), toks.next(), cur.as_mut()) {
                (Some("command"), _, Some(b)) if b.command.is_none() => {
                    b.command = Some(inner.join("\n"));
                }
                (Some("yaml"), Some("meta"), Some(b)) => apply_meta(b, &inner),
                (Some("yaml"), Some("routing"), Some(b)) => apply_routing(b, &inner),
                _ => {
                    // Preserve the whole fence in the target description.
                    let mut block = String::new();
                    block.push_str(line);
                    block.push('\n');
                    for l in &inner {
                        block.push_str(l);
                        block.push('\n');
                    }
                    block.push_str("```");
                    push_desc(cur.as_mut(), &mut intro, &block);
                }
            }
            continue;
        }

        // Anything else is description text.
        push_desc(cur.as_mut(), &mut intro, line);
        i += 1;
    }
    if let Some(b) = cur.take() {
        steps.push(b.finish());
    }

    assign_ids(&mut steps);

    let title = fm
        .title
        .filter(|s| !s.trim().is_empty())
        .or(heading_title)
        .unwrap_or_else(|| name.to_string());

    let triggers = fm
        .triggers
        .into_iter()
        .filter_map(TriggerSpec::into_trigger)
        .collect();

    WorkflowDef {
        name: name.to_string(),
        title,
        description: intro.trim().to_string(),
        mode: default_mode,
        log_runs: fm.log_runs,
        steps,
        triggers,
    }
}

/// Split leading `---`-fenced YAML frontmatter from the body. Returns the parsed
/// (or default) frontmatter and the remaining body. A file that does not open
/// with a `---` line has no frontmatter and is returned whole as the body.
fn split_frontmatter(raw: &str) -> (Frontmatter, &str) {
    let rest = match raw.strip_prefix("---\n") {
        Some(r) => r,
        // Tolerate a leading BOM/whitespace-free CRLF variant.
        None => match raw.strip_prefix("---\r\n") {
            Some(r) => r,
            None => return (Frontmatter::default(), raw),
        },
    };
    // Find the closing `---` line.
    let mut idx = 0;
    for (start, line) in line_offsets(rest) {
        if line.trim_end() == "---" {
            let yaml = &rest[..start];
            let after = &rest[start + line.len()..];
            let after = after
                .strip_prefix('\n')
                .or_else(|| after.strip_prefix("\r\n"))
                .unwrap_or(after);
            let fm = serde_yaml::from_str::<Frontmatter>(yaml).unwrap_or_default();
            return (fm, after);
        }
        idx = start;
    }
    let _ = idx;
    // No closing fence: treat the whole thing as body (no frontmatter).
    (Frontmatter::default(), raw)
}

/// Yield `(byte_offset, line_including_terminator?)` — actually the line slice
/// without the terminator, paired with its start offset — for splitting.
fn line_offsets(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    s.split_inclusive('\n').map(move |chunk| {
        let start = offset;
        offset += chunk.len();
        (start, chunk.trim_end_matches('\n'))
    })
}

/// If `trimmed` opens a fenced code block (```` ``` ```` or ```` ~~~ ````),
/// return its info string (lowercased, trimmed; empty for a bare fence).
fn fence_info(trimmed: &str) -> Option<String> {
    let info = trimmed
        .strip_prefix("```")
        .or_else(|| trimmed.strip_prefix("~~~"))?;
    Some(info.trim().to_ascii_lowercase())
}

/// Read a fenced block starting at `lines[open]` (the opening fence). Returns
/// the inner lines (excluding both fences) and how many source lines were
/// consumed (opening + inner + closing, or to EOF if unclosed).
fn read_fence<'a>(lines: &[&'a str], open: usize) -> (Vec<&'a str>, usize) {
    let mut inner = Vec::new();
    let mut j = open + 1;
    while j < lines.len() {
        let t = lines[j].trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            return (inner, j - open + 1);
        }
        inner.push(lines[j]);
        j += 1;
    }
    (inner, j - open) // unclosed: consume to EOF
}

/// The `yaml meta` block: per-step metadata. All keys optional; malformed YAML
/// is ignored (never fatal), so a half-written block still lists.
#[derive(Debug, Default, Deserialize)]
struct StepMeta {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    optional: bool,
}

/// Apply a `yaml meta` block's fields to a step builder.
fn apply_meta(b: &mut StepBuilder, inner: &[&str]) {
    let meta: StepMeta = serde_yaml::from_str(&inner.join("\n")).unwrap_or_default();
    if let Some(id) = meta
        .id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        b.id = Some(id);
    }
    if let Some(m) = meta.mode.as_deref().and_then(StepMode::parse) {
        b.mode = Some(m);
    }
    b.optional |= meta.optional;
}

/// Apply a `yaml routing` block: one `<guard>: <target>` entry per line, in
/// order (first match wins at run time). The guard is `else` or an expression;
/// the target is a step id, `end`/`fail`, or a `[a, b]` list (parallel fan-out).
/// Parsed line-by-line rather than as a YAML map to keep entry order and to
/// tolerate `==` in guards. Blank and `#`-comment lines are skipped.
fn apply_routing(b: &mut StepBuilder, inner: &[&str]) {
    for raw in inner {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Split on the last colon: targets never contain one, guards rarely do.
        let Some((guard, target)) = line.rsplit_once(':') else {
            continue;
        };
        let guard = guard.trim();
        let condition = match guard.to_ascii_lowercase().as_str() {
            "else" => RouteCondition::Else,
            "on_success" => RouteCondition::OnSuccess,
            "on_failure" => RouteCondition::OnFailure,
            _ => RouteCondition::Expr(guard.to_string()),
        };
        let targets = parse_targets(target.trim());
        if targets.is_empty() {
            continue;
        }
        b.routes.push(Route { condition, targets });
    }
}

/// Parse a routing target: a `[a, b, c]` list (parallel fan-out) or a single
/// token. Each token becomes a [`RouteTarget`].
fn parse_targets(s: &str) -> Vec<RouteTarget> {
    let body = s
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(s);
    body.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(RouteTarget::parse)
        .collect()
}

/// Append a description line to the current step, or to the workflow intro when
/// no step is open yet.
fn push_desc(cur: Option<&mut StepBuilder>, intro: &mut String, line: &str) {
    match cur {
        Some(b) => {
            b.description.push_str(line);
            b.description.push('\n');
        }
        None => {
            intro.push_str(line);
            intro.push('\n');
        }
    }
}

/// Give every step a stable, unique id: its `id:` directive, else the title
/// slug, else a positional `step_<n>` fallback; collisions get a `_<n>` suffix.
fn assign_ids(steps: &mut [Step]) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (idx, step) in steps.iter_mut().enumerate() {
        let base = if step.id.is_empty() {
            let s = slug(&step.title);
            if s.is_empty() {
                format!("step_{}", idx + 1)
            } else {
                s
            }
        } else {
            step.id.clone()
        };
        let mut candidate = base.clone();
        let mut n = 2;
        while !seen.insert(candidate.clone()) {
            candidate = format!("{base}_{n}");
            n += 1;
        }
        step.id = candidate;
    }
}

/// Mutable accumulator for one step during parsing.
struct StepBuilder {
    title: String,
    description: String,
    command: Option<String>,
    id: Option<String>,
    mode: Option<StepMode>,
    optional: bool,
    routes: Vec<Route>,
}

impl StepBuilder {
    fn new(title: &str) -> Self {
        Self {
            title: title.to_string(),
            description: String::new(),
            command: None,
            id: None,
            mode: None,
            optional: false,
            routes: Vec::new(),
        }
    }

    fn finish(self) -> Step {
        Step {
            // `id` is resolved (and de-duplicated) later in `assign_ids`; an
            // empty string here means "derive from title".
            id: self.id.unwrap_or_default(),
            title: self.title,
            description: self.description.trim().to_string(),
            command: self
                .command
                .map(|c| c.trim_end().to_string())
                .filter(|c| !c.is_empty()),
            mode: self.mode,
            optional: self.optional,
            routes: self.routes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_title_mode_and_steps() {
        let raw = "\
---
title: Release cutting
mode: auto
log_runs: true
---

Intro prose describing the workflow.

## Build
Build the release.
```command
cargo build --release
```

## Tests green?
```yaml meta
mode: ai
optional: true
```
Check the suite.
";
        let wf = parse_workflow("release", raw);
        assert_eq!(wf.name, "release");
        assert_eq!(wf.title, "Release cutting");
        assert_eq!(wf.mode, StepMode::Auto);
        assert_eq!(wf.log_runs, Some(true));
        assert_eq!(wf.description, "Intro prose describing the workflow.");
        assert_eq!(wf.steps.len(), 2);

        let build = &wf.steps[0];
        assert_eq!(build.id, "build");
        assert_eq!(build.title, "Build");
        assert_eq!(build.description, "Build the release.");
        assert_eq!(build.command.as_deref(), Some("cargo build --release"));
        assert_eq!(build.mode, None);
        assert_eq!(build.resolved_mode(wf.mode), StepMode::Auto);

        let tests = &wf.steps[1];
        assert_eq!(tests.id, "tests_green");
        assert_eq!(tests.mode, Some(StepMode::Ai));
        assert!(tests.optional);
        assert_eq!(tests.description, "Check the suite.");
        assert!(tests.command.is_none());
    }

    #[test]
    fn body_only_file_uses_leading_heading_then_stem() {
        let with_h1 = "# My Flow\n\n## Only step\ndo it\n";
        let wf = parse_workflow("flow", with_h1);
        assert_eq!(wf.title, "My Flow");
        assert_eq!(wf.steps.len(), 1);
        assert_eq!(wf.steps[0].title, "Only step");

        let no_title = "## a\n## b\n";
        let wf = parse_workflow("stem_name", no_title);
        assert_eq!(wf.title, "stem_name");
        assert_eq!(wf.steps.len(), 2);
    }

    #[test]
    fn duplicate_titles_get_unique_ids_and_explicit_id_wins() {
        let raw = "\
## Deploy
```yaml meta
id: first
```

## Deploy

## Deploy
";
        let wf = parse_workflow("w", raw);
        let ids: Vec<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["first", "deploy", "deploy_2"]);
    }

    #[test]
    fn non_command_fence_is_preserved_in_description() {
        let raw = "\
## Note
Here is a snippet:
```rust
let x = 1; // ## not a heading
```
after
";
        let wf = parse_workflow("w", raw);
        let d = &wf.steps[0].description;
        assert!(d.contains("```rust"), "desc: {d}");
        assert!(d.contains("let x = 1; // ## not a heading"), "desc: {d}");
        assert!(d.contains("after"));
        // The fenced `##` line must not have started a new step.
        assert_eq!(wf.steps.len(), 1);
        assert!(wf.steps[0].command.is_none());
    }

    #[test]
    fn routing_parses_conditions_targets_fanout_and_terminals() {
        let raw = "\
## a
```yaml routing
exit == 0: [b, c]
else: recover
```

## b
```yaml routing
# comment lines are skipped
exit == 0: end
else: fail
```

## c

## recover
";
        let wf = parse_workflow("w", raw);

        // Step a: a conditional fan-out plus an else fallback.
        let a = &wf.steps[0];
        assert!(a.has_routing());
        assert_eq!(a.routes.len(), 2);
        assert_eq!(
            a.routes[0].condition,
            RouteCondition::Expr("exit == 0".into())
        );
        // Convenience guards map to their own variants.
        let conv = parse_workflow(
            "w",
            "## x\n```yaml routing\non_success: y\non_failure: z\n```\n",
        );
        assert_eq!(conv.steps[0].routes[0].condition, RouteCondition::OnSuccess);
        assert_eq!(conv.steps[0].routes[1].condition, RouteCondition::OnFailure);
        assert_eq!(
            a.routes[0].targets,
            vec![RouteTarget::Step("b".into()), RouteTarget::Step("c".into())]
        );
        assert_eq!(a.routes[1].condition, RouteCondition::Else);
        assert_eq!(
            a.routes[1].targets,
            vec![RouteTarget::Step("recover".into())]
        );

        // Step b: reserved terminals; comment line ignored.
        let b = &wf.steps[1];
        assert_eq!(b.routes[0].targets, vec![RouteTarget::End]);
        assert_eq!(b.routes[1].targets, vec![RouteTarget::Fail]);

        // Steps without a routing block are linear.
        assert!(!wf.steps[2].has_routing());
        assert!(!wf.steps[3].has_routing());
    }

    #[test]
    fn frontmatter_triggers_parse_to_cron_and_event() {
        let raw = "\
---
title: Nightly
triggers:
  - cron: \"0 2 * * *\"
  - event: ci:push
  - cron: \"   \"
  - {}
---
## step
do it
";
        let wf = parse_workflow("nightly", raw);
        assert_eq!(
            wf.triggers,
            vec![
                Trigger::Cron("0 2 * * *".into()),
                Trigger::Event("ci:push".into()),
            ]
        );
    }

    #[test]
    fn no_triggers_is_empty() {
        let wf = parse_workflow("w", "## step\ndo\n");
        assert!(wf.triggers.is_empty());
    }

    #[test]
    fn malformed_frontmatter_is_ignored_not_fatal() {
        let raw = "---\n: : : not yaml\n---\n## step\n";
        let wf = parse_workflow("w", raw);
        assert_eq!(wf.title, "w");
        assert_eq!(wf.steps.len(), 1);
    }

    #[test]
    fn no_frontmatter_body_starts_immediately() {
        let raw = "## only\ndo\n";
        let wf = parse_workflow("w", raw);
        assert_eq!(wf.steps.len(), 1);
        assert_eq!(wf.steps[0].description, "do");
    }
}
