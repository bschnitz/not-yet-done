use std::io::{self, Read, Write};
use std::process::Command;

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub enum EditorError {
    Io(io::Error),
    EditorFailed(std::process::ExitStatus),
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditorError::Io(e) => write!(f, "IO error: {}", e),
            EditorError::EditorFailed(s) => write!(f, "Editor exited with status: {}", s),
        }
    }
}

impl From<io::Error> for EditorError {
    fn from(e: io::Error) -> Self {
        EditorError::Io(e)
    }
}

/// Pausiert ratatui, öffnet einen Editor mit `initial_content` in einem Tempfile,
/// und gibt den bearbeiteten Inhalt nach dem Schließen zurück.
///
/// # Arguments
/// * `editor` - Optionaler Pfad/Name des Editors. Fällt zurück auf `$EDITOR`, dann `vi`.
/// * `initial_content` - Inhalt, der vor dem Öffnen ins Tempfile geschrieben wird.
///
/// # Example
/// ```rust
/// let result = open_editor(None, "# Bearbeite mich\n")?;
/// let result = open_editor(Some("nvim"), "# Bearbeite mich\n")?;
/// ```
pub fn open_editor(
    editor: Option<&str>,
    initial_content: &str,
) -> Result<String, EditorError> {
    // Tempfile erstellen und Startinhalt schreiben
    let mut tmpfile = NamedTempFile::new()?;
    tmpfile.write_all(initial_content.as_bytes())?;
    tmpfile.flush()?;
    let path = tmpfile.path().to_owned();

    // ratatui pausieren
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;

    // Editor bestimmen: Argument → $EDITOR → vi
    let editor_cmd = editor
        .map(str::to_owned)
        .or_else(|| std::env::var("EDITOR").ok())
        .unwrap_or_else(|| "vi".to_string());

    // Editor starten (blockierend)
    let status = Command::new(&editor_cmd)
        .arg(&path)
        .status()?;

    // ratatui wiederherstellen — auch bei Fehler!
    let restore_result = (|| -> io::Result<()> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(())
    })();

    // Erst nach Restore Fehler werfen
    restore_result?;

    if !status.success() {
        return Err(EditorError::EditorFailed(status));
    }

    // Inhalt lesen
    let mut content = String::new();
    std::fs::File::open(&path)?.read_to_string(&mut content)?;

    Ok(content)
}
