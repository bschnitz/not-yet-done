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
///   placeholder for both Trackings and content-node invocations; the
///   JSON shape differs by context — Trackings: `{tracking_ids,
///   filter_min_date, filter_max_date}`, content nodes: `{node: {ref,
///   node_type, tab, instance, fields}}`).
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
/// 2. **Trackings**: `tracking.script_template` in `tui.yaml`
///    (Trackings tab only).
/// 3. **Global fallback**: `script.template` in `tui.yaml` (this field).
///
/// Layers 1 + 2 are optional; layer 3 always has a default. So most
/// users only ever touch `script.template` and let context-specific
/// scaffolds inherit the generic fallback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptConfig {
    /// Global fallback scaffold for new scripts created via the
    /// `:script` menu. Overridden per-view (`views[].script_template`)
    /// and for the Trackings tab (`tracking.script_template`).
    #[serde(default = "default_template")]
    pub template: String,

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
"#.to_string()
}

fn default_pause_tui() -> bool {
    true
}

fn default_busy_timeout_secs() -> f32 {
    3.0
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self {
            template: default_template(),
            interactive_command: String::new(),
            pause_tui: default_pause_tui(),
            busy_timeout_secs: default_busy_timeout_secs(),
        }
    }
}
