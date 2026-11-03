//! The trackings adapter's `edit` action: set a tracking's start and end in
//! an editor buffer.
//!
//! [`prepare_edit`] renders the current times as `start:` / `end:` lines in
//! local time; [`execute_edit`] parses the saved buffer and hands the new
//! times to [`TrackingService::edit_tracking`], which owns every rule (no
//! future, end after start, no end on a running tracking, no overlap inside
//! the tracking group). A value may be written as in the buffer
//! (`2026-03-22 09:15:00`) or in the natural form `split`/`move` accept
//! (`9:15`, `yesterday 14:00`).
//!
//! [`TrackingService::edit_tracking`]: not_yet_done_task_core::service::TrackingService::edit_tracking

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use not_yet_done_content::{
    ActionArgs, ActionOutcome, ContentError, EditorPrep, InputSpec, NodeAction, Result,
};
use uuid::Uuid;

use super::{emit_tracking_changed, to_content_err};
use crate::CoreHandle;
use crate::datetime::LocalDateTime;
use crate::form::invalid_input;

/// How the buffer writes a time, and the first format it reads back.
const BUFFER_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// The `edit` entry action.
pub(super) fn edit_action() -> NodeAction {
    NodeAction::new("edit", "Edit start/end", InputSpec::Editor)
}

/// `prepare("edit")` — the buffer with the tracking's current start and end.
/// The version token is the times it shows, so a save after the tracking
/// changed elsewhere is refused instead of overwriting that change.
pub(super) async fn prepare_edit(handle: &CoreHandle, tracking_id: Uuid) -> Result<EditorPrep> {
    let tracking = handle
        .tracking_repo
        .find_by_id(tracking_id)
        .await
        .map_err(to_content_err)?
        .ok_or_else(|| ContentError::NotFound(tracking_id.to_string()))?;
    let path = handle
        .tracking_service
        .label_path(tracking.task_id)
        .await
        .map_err(to_content_err)?;
    let template = render_buffer(&path, tracking.started_at, tracking.ended_at);
    Ok(EditorPrep {
        cursor_line: template.lines().position(|l| l.starts_with("start:")),
        template,
        version: version_token(tracking.started_at, tracking.ended_at),
        suffix: ".yaml".into(),
        file_path: None,
        args: Default::default(),
    })
}

/// `execute("edit")` — apply the saved buffer. The tracking's group comes from
/// the adapter's policy (overridable per invocation via `group_paths`, see
/// [`crate::task::tracking_policy_for`]).
pub(super) async fn execute_edit(
    handle: &CoreHandle,
    tracking_id: Uuid,
    text: &str,
    version: &str,
    args: &ActionArgs,
) -> Result<ActionOutcome> {
    let current = handle
        .tracking_repo
        .find_by_id(tracking_id)
        .await
        .map_err(to_content_err)?
        .ok_or_else(|| ContentError::NotFound(tracking_id.to_string()))?;
    if !version.is_empty() && version != version_token(current.started_at, current.ended_at) {
        return Err(invalid_input(
            "the tracking changed while it was being edited — reopen the editor".to_string(),
        ));
    }

    let (start, end) = parse_buffer(text)?;
    // The buffer shows whole seconds; a value left as shown keeps the stored
    // sub-second precision instead of counting as a change.
    let start = keep_if_same_second(start, current.started_at);
    let end = match (end, current.ended_at) {
        (Some(e), Some(old)) => Some(keep_if_same_second(e, old)),
        (e, _) => e,
    };

    let policy = crate::task::tracking_policy_for(handle, args).map_err(invalid_input)?;
    let edited = handle
        .tracking_service
        .edit_tracking(tracking_id, start, end, &policy)
        .await
        .map_err(to_content_err)?;
    if !edited.changed {
        return Ok(ActionOutcome::Done {
            message: Some("Tracking unchanged".to_string()),
        });
    }
    emit_tracking_changed(handle, tracking_id);
    Ok(ActionOutcome::Done {
        message: Some("Tracking edited".to_string()),
    })
}

fn version_token(start: DateTime<Utc>, end: Option<DateTime<Utc>>) -> String {
    match end {
        Some(end) => format!("{}/{}", start.to_rfc3339(), end.to_rfc3339()),
        None => format!("{}/", start.to_rfc3339()),
    }
}

fn format_local(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format(BUFFER_FORMAT).to_string()
}

fn render_buffer(path: &str, start: DateTime<Utc>, end: Option<DateTime<Utc>>) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Tracking of {path}\n"));
    out.push_str("# Times are local, e.g. '2026-03-22 09:15:00', '9:15', 'yesterday 14:00'.\n");
    match end {
        Some(_) => out.push_str("# Edit start and end. Lines starting with '#' are ignored.\n"),
        None => out.push_str("# Running tracking: only the start can be edited, leave end empty.\n"),
    }
    out.push_str(&format!("start: {}\n", format_local(start)));
    match end {
        Some(end) => out.push_str(&format!("end: {}\n", format_local(end))),
        None => out.push_str("end:\n"),
    }
    out
}

/// The `start` and `end` the buffer names. `start` is required; an empty or
/// missing `end` means "no end" (a running tracking).
fn parse_buffer(text: &str) -> Result<(DateTime<Utc>, Option<DateTime<Utc>>)> {
    let mut start = None;
    let mut end = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(invalid_input(format!("expected 'start: …' or 'end: …', got '{line}'")));
        };
        let value = value.trim();
        match key.trim() {
            "start" => start = Some(value.to_string()),
            "end" => end = Some(value.to_string()),
            other => return Err(invalid_input(format!("unknown key '{other}'"))),
        }
    }
    let start = match start.as_deref() {
        Some(v) if !v.is_empty() => parse_time(v)?,
        _ => return Err(invalid_input("'start' is required".to_string())),
    };
    let end = match end.as_deref() {
        Some(v) if !v.is_empty() => Some(parse_time(v)?),
        _ => None,
    };
    Ok((start, end))
}

/// A buffer value: the exact buffer format first (so seconds survive), then
/// the natural forms of [`LocalDateTime`].
fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, BUFFER_FORMAT)
        && let Some(local) = Local.from_local_datetime(&naive).earliest()
    {
        return Ok(local.with_timezone(&Utc));
    }
    value
        .parse::<LocalDateTime>()
        .map(|dt| dt.utc)
        .map_err(invalid_input)
}

fn keep_if_same_second(parsed: DateTime<Utc>, stored: DateTime<Utc>) -> DateTime<Utc> {
    if parsed.timestamp() == stored.timestamp() {
        stored
    } else {
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn a_rendered_buffer_parses_back_to_its_times() {
        let start = utc("2026-03-22T08:15:30Z");
        let end = utc("2026-03-22T10:00:00Z");
        let buffer = render_buffer("/Work/Doku", start, Some(end));
        assert_eq!(parse_buffer(&buffer).unwrap(), (start, Some(end)));
    }

    #[test]
    fn a_running_tracking_renders_an_empty_end() {
        let start = utc("2026-03-22T08:15:30Z");
        let buffer = render_buffer("/Work/Doku", start, None);
        assert!(buffer.contains("\nend:\n"), "{buffer}");
        assert_eq!(parse_buffer(&buffer).unwrap(), (start, None));
    }

    #[test]
    fn a_missing_start_is_refused() {
        let err = parse_buffer("end: 2026-03-22 10:00:00\n").unwrap_err();
        assert!(format!("{err}").contains("'start' is required"), "{err}");
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let err = parse_buffer("start: 2026-03-22 10:00:00\ntask: x\n").unwrap_err();
        assert!(format!("{err}").contains("unknown key 'task'"), "{err}");
    }

    #[test]
    fn the_same_second_keeps_the_stored_precision() {
        let stored = utc("2026-03-22T08:15:30.123456Z");
        let shown = utc("2026-03-22T08:15:30Z");
        assert_eq!(keep_if_same_second(shown, stored), stored);
        let moved = utc("2026-03-22T08:15:31Z");
        assert_eq!(keep_if_same_second(moved, stored), moved);
    }
}
