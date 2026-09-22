//! The `# key: value` header a script declares itself with.
//!
//! Every script language in use writes comments differently, so the header is
//! defined by what follows the comment marker rather than by the marker: `#`,
//! `//`, `--` and `;;` all carry one, and only the first ten lines are read —
//! a header is a declaration, not something buried in the body.
//!
//! The parsers here are pure and say nothing about what a mode *does*. How a
//! mode is handled belongs to whoever runs the script: a frontend yielding its
//! terminal, a hook reading commands out of the answer file. What must not
//! diverge is the reading.

/// The value of a `# <key>: <value>` header line. The one place the header
/// syntax is defined; the typed parsers differ only in the key they ask for,
/// and a frontend with a key of its own uses this directly.
pub fn header_value<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    content.lines().take(10).find_map(|line| {
        let trimmed = line.trim();
        let after = trimmed
            .strip_prefix('#')
            .or_else(|| trimmed.strip_prefix("//"))
            .or_else(|| trimmed.strip_prefix("--"))
            .or_else(|| trimmed.strip_prefix(";;"))?;
        after.trim().strip_prefix(key).map(str::trim)
    })
}

/// How a script interacts with the terminal and what is done with what it
/// leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptMode {
    /// No terminal output. Stderr is shown as a notification.
    Background,
    /// Output captured and shown in an editor afterwards.
    Capture,
    /// The frontend yields the terminal to the script.
    Interactive,
    /// Interactive, and the output captured in an editor.
    InteractiveCapture,
    /// Background-style execution whose answer file is read as JSON
    /// `{ "commands": [".."], … }`, each entry then executed as a command
    /// line. Extra keys are tolerated for forward-compatibility.
    Commands,
    /// Interactive variant of [`ScriptMode::Commands`].
    InteractiveCommands,
    /// The script is asked in rounds: it declares a form, the frontend fills
    /// it in, and the values come back to a new process until the script
    /// answers with a result. See [`crate::form_run`].
    Form,
}

impl ScriptMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "background" => Some(Self::Background),
            "capture" => Some(Self::Capture),
            "interactive" => Some(Self::Interactive),
            "interactive+capture" => Some(Self::InteractiveCapture),
            "commands" => Some(Self::Commands),
            "interactive+commands" => Some(Self::InteractiveCommands),
            "form" => Some(Self::Form),
            _ => None,
        }
    }

    pub fn is_interactive(self) -> bool {
        matches!(
            self,
            Self::Interactive | Self::InteractiveCapture | Self::InteractiveCommands
        )
    }

    pub fn captures_output(self) -> bool {
        matches!(self, Self::Capture | Self::InteractiveCapture)
    }

    /// True when the script's answer file is a JSON command list rather than
    /// text to display.
    pub fn emits_commands(self) -> bool {
        matches!(self, Self::Commands | Self::InteractiveCommands)
    }

    /// True when the script is driven in rounds by [`crate::form_run`]. Such
    /// a script is neither interactive nor capturing: its process is short,
    /// silent and repeated, and everything the user sees is the form.
    pub fn is_form(self) -> bool {
        matches!(self, Self::Form)
    }
}

/// Parse `# mode: <mode>` from the script header.
///
/// An unknown mode falls back to [`ScriptMode::Background`] rather than
/// failing: modes are added over time, and a script written for a newer
/// version still running silently beats one that refuses to start.
pub fn parse_script_mode(content: &str) -> ScriptMode {
    header_value(content, "mode:")
        .and_then(ScriptMode::from_str)
        .unwrap_or(ScriptMode::Background)
}

/// Parse `# output: <ext>` — the file extension the captured-output viewer
/// gives its temp buffer. The extension drives syntax highlighting and
/// Markdown preview, so a report-style script emitting Markdown declares
/// `# output: md` to be rendered rather than shown as plain text. The leading
/// dot is optional; defaults to `.txt`. Only meaningful for capture modes.
pub fn parse_output_suffix(content: &str) -> String {
    header_value(content, "output:")
        .map(|ext| ext.trim_start_matches('.'))
        .filter(|ext| !ext.is_empty())
        .map(|ext| format!(".{ext}"))
        .unwrap_or_else(|| ".txt".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_comment_prefix_carries_a_header() {
        for prefix in ["#", "//", "--", ";;"] {
            assert_eq!(
                header_value(&format!("{prefix} scope: table\n"), "scope:"),
                Some("table"),
                "{prefix}"
            );
        }
    }

    #[test]
    fn a_header_below_the_first_ten_lines_is_not_one() {
        let script = format!("#!/bin/sh\n{}# mode: form\n", "\n".repeat(10));
        assert_eq!(parse_script_mode(&script), ScriptMode::Background);
    }

    #[test]
    fn every_mode_reads_back() {
        for (text, mode) in [
            ("background", ScriptMode::Background),
            ("capture", ScriptMode::Capture),
            ("interactive", ScriptMode::Interactive),
            ("interactive+capture", ScriptMode::InteractiveCapture),
            ("commands", ScriptMode::Commands),
            ("interactive+commands", ScriptMode::InteractiveCommands),
            ("form", ScriptMode::Form),
        ] {
            assert_eq!(
                parse_script_mode(&format!("#!/bin/sh\n# mode: {text}\n")),
                mode,
                "{text}"
            );
        }
    }

    #[test]
    fn an_unknown_mode_runs_silently_rather_than_not_at_all() {
        assert_eq!(
            parse_script_mode("# mode: telepathy\n"),
            ScriptMode::Background
        );
    }

    #[test]
    fn a_form_script_neither_captures_nor_takes_the_terminal() {
        let form = ScriptMode::Form;
        assert!(form.is_form());
        assert!(!form.is_interactive());
        assert!(!form.captures_output());
        assert!(!form.emits_commands());
    }

    #[test]
    fn the_output_suffix_defaults_to_text_and_takes_an_optional_dot() {
        assert_eq!(parse_output_suffix("# mode: capture\n"), ".txt");
        assert_eq!(parse_output_suffix("# output: md\n"), ".md");
        assert_eq!(parse_output_suffix("# output: .md\n"), ".md");
        assert_eq!(parse_output_suffix("# output:   \n"), ".txt");
    }
}
