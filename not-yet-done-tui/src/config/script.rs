use serde::{Deserialize, Serialize};

/// Configuration for the `:script` orchestration (Trackings tab and
/// per-view `type: script` actions on content nodes).
///
/// ```yaml
/// script:
///   template: |
///     #!/usr/bin/env python3
///     # mode: background
///     ...
///   interactive_command: "kitty @ launch --location=vsplit sh -c '{script} {json_file} {output_file}; touch {output_file}'"
///   pause_tui: true
///   busy_timeout_secs: 3
/// ```
///
/// Placeholders in `interactive_command`:
/// - `{script}` — path to the script file
/// - `{json_file}` — path to a temp file containing the JSON the
///   script receives (legacy `{tracking_json_file}` renamed to keep one
///   placeholder for both batch and content-node invocations; the
///   JSON shape differs by context — `scope: filtered_set` batch:
///   `{tracking_ids, filter_min_date, filter_max_date}`, content nodes:
///   `{node: {ref, id, label, node_type, tab, instance, fields}}`).
/// - `{output_file}` — path to output file; the TUI watches for it to
///   detect completion. Scripts can optionally write captured output
///   here. `touch` it at the end to signal completion.
///
/// ## Template resolution
///
/// When the user creates a new script through the `:script` menu, the
/// scaffold inserted into the new file is resolved in this order
/// (first hit wins):
///
/// 1. **Per-view**: `views[].script_template` in the active view's
///    `~/.config/not_yet_done/views/*.yaml` (content tabs only).
/// 2. **Global fallback for the level's scope**: `script.template`,
///    `script.batch_template` or `script.table_template` in `tui.yaml`,
///    picked by the `scope:` of the action that opened the menu (see
///    [`ScriptConfig::template_for`]).
///
/// Layer 1 is optional; layer 2 always has a default. So most users
/// only ever touch these three and let per-view scaffolds inherit the
/// fallback.
///
/// The scope decides because the payload decides: a scaffold that reads
/// `data["node"]` is wrong from its first line on a `scope: filtered_set`
/// level, which hands the script `{"tracking_ids": …}` instead. A
/// per-view `script_template` still wins over all three — it is set by
/// hand and knows its own level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptConfig {
    /// Global fallback scaffold for new scripts created via the
    /// `:script` menu. Overridden per-view (`views[].script_template`).
    #[serde(default = "default_template")]
    pub template: String,

    /// Fallback scaffold for a level whose script action is
    /// `scope: filtered_set` — the batch payload, not a node.
    #[serde(default = "default_batch_template")]
    pub batch_template: String,

    /// Fallback scaffold for a level whose script action is
    /// `scope: table` — the whole displayed table plus cursor context.
    #[serde(default = "default_table_template")]
    pub table_template: String,

    /// Command template for running interactive scripts in an external
    /// terminal window. When empty, the TUI yields its own terminal
    /// (legacy behaviour).
    #[serde(default)]
    pub interactive_command: String,

    /// Whether to pause the TUI while the interactive command launches.
    /// Required for commands like `kitty @` that need clean terminal access.
    #[serde(default = "default_pause_tui")]
    pub pause_tui: bool,

    /// Seconds to wait before showing a "script busy" indicator.
    #[serde(default = "default_busy_timeout_secs")]
    pub busy_timeout_secs: f32,

    /// How long the cursor has to sit still before a script bound to the
    /// `row_change` hook is run, in milliseconds.
    ///
    /// The hook fires per cursor move, so without a settle delay holding `j`
    /// would start a script per row. The right value depends on what the
    /// script costs: a preview that runs pandoc and a browser wants a
    /// quarter of a second or more, a script that only writes a file can sit
    /// at 50.
    #[serde(default = "default_row_change_delay_ms")]
    pub row_change_delay_ms: u64,
}

fn default_row_change_delay_ms() -> u64 {
    250
}

fn default_template() -> String {
    r#"#!/usr/bin/env python3
# mode: interactive
"""Generic node script.

Usage: script.py <json_file> [output_file]

Reads the selected node from a JSON file (first argument):
  {"node": {
      "ref": "<adapter>/<instance>/<id>",
      "id": "<node id>",
      "label": "<display label of the row>",
      "node_type": "<adapter>:<type>",
      "tab": "<adapter>",
      "instance": "<instance>",
      "fields": {"<key>": "<value>", ...}
  }}

Output (stdout+stderr) is shown in an editor window.
"""
import json
import sys


def main():
    if len(sys.argv) < 2:
        print("Usage: script.py <json_file> [output_file]", file=sys.stderr)
        sys.exit(1)
    with open(sys.argv[1]) as f:
        data = json.load(f)
    node = data.get("node", {})
    print(f"Got {node.get('ref', '<no ref>')}")


if __name__ == "__main__":
    main()
"#
    .to_string()
}

fn default_batch_template() -> String {
    r#"#!/usr/bin/env python3
# mode: interactive
"""Batch script (the level runs its scripts with `scope: filtered_set`).

Usage: script.py <json_file> [output_file]

Reads the rows the pane currently shows from a JSON file (first argument):
  {"tracking_ids": ["<row id>", ...],
   "filter_min_date": "<RFC3339>" or null,
   "filter_max_date": "<RFC3339>" or null}

The ids are what the user sees: an active fuzzy filter narrows them to the
hit set, an empty hit set yields an empty list. The bounds come from the
date clauses of the active query, with relative dates already resolved —
and they are instants, so an upper bound on a local midnight admits
nothing of that day. The key is named `tracking_ids` on every tab: the
shape is the historical trackings one, kept so the aggregate scripts run
unchanged.

Output (stdout+stderr) is shown in an editor window.
"""
import json
import sys


def main():
    if len(sys.argv) < 2:
        print("Usage: script.py <json_file> [output_file]", file=sys.stderr)
        sys.exit(1)
    with open(sys.argv[1]) as f:
        data = json.load(f)
    ids = data.get("tracking_ids", [])
    print(f"Got {len(ids)} rows")


if __name__ == "__main__":
    main()
"#
    .to_string()
}

fn default_table_template() -> String {
    r#"#!/usr/bin/env python3
# mode: interactive
"""Table script (the level runs its scripts with `scope: table`).

Usage: script.py <json_file> [output_file]

Reads the displayed table from a JSON file (first argument):
  {"rows": [{"id": "<row id>",
             "label": "<display label>",
             "fields": {"<key>": "<value>", ...}}, ...],
   "query": "<active query text>" or null,
   "selected_index": <index of the cursor row in rows>,
   "selected_field": "<column key under the column cursor>" or null}

The rows are in display order and narrowed by an active fuzzy filter. In
the transposed detail split every row is one field/value pair of the
record.

Output (stdout+stderr) is shown in an editor window.
"""
import json
import sys


def main():
    if len(sys.argv) < 2:
        print("Usage: script.py <json_file> [output_file]", file=sys.stderr)
        sys.exit(1)
    with open(sys.argv[1]) as f:
        data = json.load(f)
    rows = data.get("rows", [])
    cursor = data.get("selected_index", 0)
    print(f"Got {len(rows)} rows, cursor on {cursor}")


if __name__ == "__main__":
    main()
"#
    .to_string()
}

fn default_pause_tui() -> bool {
    true
}

fn default_busy_timeout_secs() -> f32 {
    3.0
}

impl ScriptConfig {
    /// The fallback scaffold for a level whose script action carries
    /// `scope`. Only reached when the view sets no `script_template` of
    /// its own — that one knows its level and wins.
    pub fn template_for(&self, scope: super::view_config::ScriptScope) -> &str {
        use super::view_config::ScriptScope;
        match scope {
            ScriptScope::Node => &self.template,
            ScriptScope::FilteredSet => &self.batch_template,
            ScriptScope::Table => &self.table_template,
        }
    }
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self {
            template: default_template(),
            batch_template: default_batch_template(),
            table_template: default_table_template(),
            interactive_command: String::new(),
            pause_tui: default_pause_tui(),
            busy_timeout_secs: default_busy_timeout_secs(),
            row_change_delay_ms: default_row_change_delay_ms(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::view_config::ScriptScope;

    /// A scaffold is only useful if it reads the payload its level actually
    /// hands over. Before this, every new script started from the node
    /// template — on a `scope: filtered_set` level that meant a script whose
    /// first `data["node"]` was already wrong.
    #[test]
    fn each_scope_scaffolds_the_payload_it_gets() {
        let cfg = ScriptConfig::default();
        assert!(cfg.template_for(ScriptScope::Node).contains("\"node\""));
        assert!(
            cfg.template_for(ScriptScope::FilteredSet)
                .contains("tracking_ids")
        );
        let table = cfg.template_for(ScriptScope::Table);
        assert!(table.contains("\"rows\"") && table.contains("selected_index"));
    }

    /// Each scaffold is runnable on its own: the `# mode:` header the menu
    /// parses, and a `main` that opens `sys.argv[1]`.
    #[test]
    fn every_scaffold_is_a_runnable_script() {
        let cfg = ScriptConfig::default();
        for scope in [ScriptScope::Node, ScriptScope::FilteredSet, ScriptScope::Table] {
            let t = cfg.template_for(scope);
            assert!(t.starts_with("#!/usr/bin/env python3"), "{scope:?}");
            assert!(t.contains("# mode: "), "{scope:?}");
            assert!(t.contains("open(sys.argv[1])"), "{scope:?}");
        }
    }
}
