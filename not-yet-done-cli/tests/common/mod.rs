use assert_cmd::Command;
use tempfile::TempDir;

/// Creates a temporary SQLite database, runs `db sync`, and returns
/// the TempDir (keep it alive for the test's duration) and the DATABASE_URL.
pub fn setup() -> (TempDir, String) {
    let dir = tempfile::tempdir().expect("Failed to create temp dir");
    let db_path = dir.path().join("test.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

    nyd(&db_url)
        .args(["db", "sync"])
        .assert()
        .success();

    (dir, db_url)
}

/// Returns a `Command` for the `nyd` binary with `DATABASE_URL` set.
pub fn nyd(db_url: &str) -> Command {
    let mut cmd = Command::cargo_bin("not-yet-done-cli").expect("Binary not found");
    cmd.env("DATABASE_URL", db_url);
    cmd
}

/// Creates a task with the given description and returns its UUID.
pub fn create_task(db_url: &str, description: &str) -> String {
    let output = nyd(db_url)
        .args(["task", "add", description])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("Invalid UTF-8");
    parse_bracketed_uuid(&stdout)
        .unwrap_or_else(|| panic!("Could not parse task UUID from: {stdout}"))
}

/// Starts tracking a task and returns the tracking UUID.
/// NOT DEAD CODE! This function is indeed used, but the compiler checks it for every test file.
#[allow(dead_code)]
pub fn start_tracking(db_url: &str, task_id: &str) -> String {
    let output = nyd(db_url)
        .args(["track", "start", task_id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("Invalid UTF-8");
    parse_bracketed_uuid(&stdout)
        .unwrap_or_else(|| panic!("Could not parse tracking UUID from: {stdout}"))
}

/// Parses the first UUID inside `[...]` from a string like
/// `✓ Tracking started: [<uuid>] started at ...`
pub fn parse_bracketed_uuid(s: &str) -> Option<String> {
    let start = s.find('[')?;
    let end = s.find(']')?;
    if end > start + 1 {
        Some(s[start + 1..end].to_string())
    } else {
        None
    }
}
