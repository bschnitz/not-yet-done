//! Render a [`WorkflowDef`] as a [Mermaid](https://mermaid.js.org) flowchart
//! (Phase 6).
//!
//! A workflow *is* a diagram — this turns the parsed definition back into one.
//! Each step is a node; edges come from the step's declared [`Route`]s, falling
//! back to the linear "next step in document order" when a step declares none
//! (and to the reserved `end` terminal for the last step). Route guards become
//! edge labels; the reserved [`RouteTarget::End`] / [`RouteTarget::Fail`]
//! targets become the round `end` / `fail` terminal nodes.
//!
//! The output is plain Mermaid source (`flowchart TD …`): paste it into
//! <https://mermaid.live>, feed it to `mmdc`, or drop it in a fenced
//! ```` ```mermaid ```` block. It is a pure projection of the definition — no
//! run state — so it documents the *shape*, not any particular execution.
//!
//! [`Route`]: crate::model::Route

use crate::model::{RouteCondition, RouteTarget, WorkflowDef};

/// The round terminal node ids. Real step ids are slugs (lowercase alphanumeric
/// runs joined by single `_`, never leading/trailing `_`), so neither can ever
/// collide with a step's own id.
const END_NODE: &str = "__end__";
const FAIL_NODE: &str = "__fail__";

/// Render `def` as Mermaid flowchart source.
pub fn render(def: &WorkflowDef) -> String {
    let mut out = String::from("flowchart TD\n");
    if def.steps.is_empty() {
        out.push_str("    __empty__[\"(no steps)\"]\n");
        return out;
    }

    // Node declarations (id → labelled box), in document order.
    for step in &def.steps {
        out.push_str(&format!(
            "    {}[\"{}\"]\n",
            node_id(&step.id),
            label(&step.title)
        ));
    }

    // Edges: declared routes, or the linear fallthrough.
    let mut uses_end = false;
    let mut uses_fail = false;
    for (i, step) in def.steps.iter().enumerate() {
        let from = node_id(&step.id);
        if step.routes.is_empty() {
            match def.steps.get(i + 1) {
                Some(next) => out.push_str(&format!("    {from} --> {}\n", node_id(&next.id))),
                None => {
                    out.push_str(&format!("    {from} --> {END_NODE}\n"));
                    uses_end = true;
                }
            }
            continue;
        }
        for route in &step.routes {
            let lbl = guard_label(&route.condition);
            for target in &route.targets {
                let to = match target {
                    RouteTarget::Step(id) => node_id(id),
                    RouteTarget::End => {
                        uses_end = true;
                        END_NODE.to_string()
                    }
                    RouteTarget::Fail => {
                        uses_fail = true;
                        FAIL_NODE.to_string()
                    }
                };
                out.push_str(&format!("    {from} -->|{lbl}| {to}\n"));
            }
        }
    }

    if uses_end {
        out.push_str(&format!("    {END_NODE}((end))\n"));
    }
    if uses_fail {
        out.push_str(&format!("    {FAIL_NODE}((fail))\n"));
    }
    out
}

/// A short edge label for a route guard.
fn guard_label(c: &RouteCondition) -> String {
    match c {
        RouteCondition::Else => "else".to_string(),
        RouteCondition::OnSuccess => "ok".to_string(),
        RouteCondition::OnFailure => "fail".to_string(),
        RouteCondition::Expr(e) => edge_text(e),
    }
}

/// Sanitise a step id into a Mermaid-safe node id (identifier characters only).
fn node_id(id: &str) -> String {
    let s: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "n".to_string()
    } else {
        s
    }
}

/// Sanitise a node label: quotes break the `["…"]` box, so swap them, and
/// collapse newlines so a multi-line title stays on one line.
fn label(title: &str) -> String {
    title
        .replace('"', "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sanitise an edge label: `|` closes the `-->|…|` label and quotes/newlines
/// break it, so replace them.
fn edge_text(text: &str) -> String {
    text.replace('|', "/")
        .replace('"', "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_workflow;

    #[test]
    fn empty_workflow_renders_placeholder() {
        let def = parse_workflow("flow", "---\ntitle: X\n---\n");
        let out = render(&def);
        assert!(out.starts_with("flowchart TD"));
        assert!(out.contains("(no steps)"));
    }

    #[test]
    fn linear_workflow_chains_steps_to_end() {
        let def = parse_workflow("flow", "## First\na\n## Second\nb\n");
        let out = render(&def);
        // Two step nodes, chained, terminating at end.
        assert!(out.contains("first[\"First\"]"));
        assert!(out.contains("second[\"Second\"]"));
        assert!(out.contains("first --> second"));
        assert!(out.contains("second --> __end__"));
        assert!(out.contains("__end__((end))"));
        // No fail terminal was referenced.
        assert!(!out.contains("__fail__"));
    }

    #[test]
    fn routes_become_labelled_edges_with_terminals() {
        let md = "\
## Build
```command
make
```
```yaml routing
on_success: test
on_failure: fail
```
## Test
b
```yaml routing
else: end
```
";
        let def = parse_workflow("flow", md);
        let out = render(&def);
        assert!(out.contains("build -->|ok| test"));
        assert!(out.contains("build -->|fail| __fail__"));
        assert!(out.contains("test -->|else| __end__"));
        assert!(out.contains("__end__((end))"));
        assert!(out.contains("__fail__((fail))"));
    }
}
