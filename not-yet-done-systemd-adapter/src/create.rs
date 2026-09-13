//! Making a unit that does not exist yet.
//!
//! # A timer is two files
//!
//! A `.timer` on its own does nothing: it names a `.service` and starts it on
//! a schedule. Written by hand that is two files, in two places, that have to
//! agree on a name — and half of one is what gets forgotten. So the timer form
//! writes **both**: one submission, one name, `foo.service` and `foo.timer`
//! next to each other. A command left empty says the service already exists
//! and only the schedule is missing.
//!
//! # Three ways in, one write path
//!
//! * [`NEW_SERVICE`] — a form for the fields a service actually has.
//! * [`NEW_TIMER`] — the pair above.
//! * [`NEW_FILE`] — the empty file in `$EDITOR`, for everything the forms do
//!   not cover (a socket, a path unit, a service with six `ExecStartPre=`
//!   lines). The buffer names itself: the `# unit:` line in its header is the
//!   file name, because a file created in an editor has to get its name from
//!   somewhere and the header is already there.
//!
//! All three end in the same place as [`crate::edit`]: staged, checked by
//! `systemd-analyze verify`, and only then written to
//! `~/.config/systemd/user`. Creating is editing that starts from nothing.
//!
//! # What is not overwritten
//!
//! A create never writes over an existing file. Editing a unit is what the
//! `e` leader is for, and it backs the old version up first; a create that
//! silently replaced a unit would be the one way to lose a file here.

use std::collections::HashMap;
use std::path::PathBuf;

use not_yet_done_content::{
    ActionOutcome, ContentError, FormFieldSpec, InputSpec, NodeAction, Result,
};

use crate::bus::Bus;
use crate::calendar;
use crate::config::Manager;
use crate::edit;

/// Action id: the service form.
pub const NEW_SERVICE: &str = "new-service";
/// Action id: the timer/service pair form.
pub const NEW_TIMER: &str = "new-timer";
/// Action id: an empty unit file in the editor.
pub const NEW_FILE: &str = "new-file";

/// The header line of a [`NEW_FILE`] buffer that carries the unit's name.
const UNIT_LINE: &str = "# unit:";
/// What that line says before the user has replaced it.
const UNIT_PLACEHOLDER: &str = "name-me.service";

/// The creating actions, which the manager node offers — creating has no row.
///
/// Nothing against the **system** manager, for the same reason editing offers
/// nothing there: this writes to `~/.config/systemd/user`, and an action that
/// can only fail is worse than one that is not offered.
pub fn actions_for(type_id: &str, manager: Manager) -> Vec<NodeAction> {
    if manager != Manager::User || type_id != "systemd:manager" {
        return Vec::new();
    }
    vec![
        NodeAction::new(NEW_SERVICE, "New service", service_form()),
        NodeAction::new(NEW_TIMER, "New timer (with its service)", timer_form()),
        NodeAction::new(NEW_FILE, "New unit file in the editor", InputSpec::Editor),
    ]
}

/// The fields a service has, in the order one thinks of them.
fn service_form() -> InputSpec {
    InputSpec::Form {
        fields: vec![
            FormFieldSpec::text("name", "Name"),
            FormFieldSpec::text("description", "Description").optional(),
            FormFieldSpec::text("command", "Command (ExecStart=)"),
            FormFieldSpec::select(
                "kind",
                "Type=",
                ["simple", "exec", "oneshot", "notify", "forking"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            )
            .with_default("simple"),
            FormFieldSpec::text("working_directory", "Working directory").optional(),
            FormFieldSpec::select(
                "restart",
                "Restart=",
                ["no", "on-failure", "always"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            )
            .with_default("no"),
            FormFieldSpec::toggle("enable", "Enable and start it now"),
        ],
    }
}

/// The pair: one name, one schedule, and the command the schedule runs.
fn timer_form() -> InputSpec {
    InputSpec::Form {
        fields: vec![
            FormFieldSpec::text("name", "Name"),
            FormFieldSpec::text("description", "Description").optional(),
            FormFieldSpec::text("command", "Command — empty: use the existing service").optional(),
            FormFieldSpec::text("schedule", "Schedule (OnCalendar=)").with_default("daily"),
            FormFieldSpec::toggle("once", "Once — the schedule is a single moment"),
            FormFieldSpec::toggle("persistent", "Catch up after downtime (Persistent=)")
                .with_default("true"),
            FormFieldSpec::text("delay", "Randomised delay (RandomizedDelaySec=)").optional(),
            FormFieldSpec::text("accuracy", "Accuracy (AccuracySec=)").optional(),
            FormFieldSpec::toggle("enable", "Enable and start the timer now").with_default("true"),
        ],
    }
}

/// What one submission produces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// Unit name → file content, in the order they are written. The unit the
    /// user is really making comes last, so a half-failed write leaves the
    /// dependency behind rather than the thing that depends on it.
    pub files: Vec<(String, String)>,
    /// The unit to enable and start once everything is on disk.
    pub enable: Option<String>,
    /// What systemd made of the schedule — for the notification, so the user
    /// sees what was written and when it will next fire.
    pub schedule: Option<calendar::Schedule>,
}

/// Build the files for a form submission, validating everything that can be
/// validated before anything is written.
pub async fn draft(action_id: &str, values: &HashMap<String, String>) -> Result<Draft> {
    let name = unit_base(required(values, "name")?)?;
    match action_id {
        NEW_SERVICE => {
            let unit = format!("{name}.service");
            let text = service_file(
                opt(values, "description").as_deref(),
                required(values, "command")?,
                opt(values, "kind").as_deref().unwrap_or("simple"),
                opt(values, "working_directory").as_deref(),
                opt(values, "restart").as_deref().unwrap_or("no"),
                // A service made on its own is one a person will want to
                // enable — the timer's is not, so only this one gets the
                // `[Install]` section that makes enabling possible.
                true,
            );
            Ok(Draft {
                enable: flag(values, "enable").then(|| unit.clone()),
                files: vec![(unit, text)],
                schedule: None,
            })
        }
        NEW_TIMER => {
            let service = format!("{name}.service");
            let timer = format!("{name}.timer");
            let schedule = schedule_for(values).await?;
            let mut files = Vec::new();
            // No command: the service is already there and only the schedule
            // was missing. Writing an empty one over it is the one thing this
            // must not do.
            if let Some(command) = opt(values, "command") {
                files.push((
                    service.clone(),
                    service_file(
                        opt(values, "description").as_deref(),
                        &command,
                        "oneshot",
                        opt(values, "working_directory").as_deref(),
                        "no",
                        false,
                    ),
                ));
            }
            files.push((
                timer.clone(),
                timer_file(
                    opt(values, "description").as_deref(),
                    &schedule.expression,
                    flag(values, "persistent") && !flag(values, "once"),
                    opt(values, "delay").as_deref(),
                    opt(values, "accuracy").as_deref(),
                ),
            ));
            Ok(Draft {
                files,
                enable: flag(values, "enable").then_some(timer),
                schedule: Some(schedule),
            })
        }
        other => Err(ContentError::NotSupported(format!(
            "{other} is not a creating action"
        ))),
    }
}

/// The schedule, by whichever of the two roads the `once` toggle chose.
///
/// Recurring goes through [`calendar`], which normalises and then lets systemd
/// have the last word. A single moment goes through `natural-date`, whose whole
/// job is resolving a phrase to one instant — and an instant written as
/// `OnCalendar=2026-09-14 09:00:00` is a timer that fires once.
async fn schedule_for(values: &HashMap<String, String>) -> Result<calendar::Schedule> {
    let raw = required(values, "schedule")?;
    if !flag(values, "once") {
        return calendar::check(raw).await;
    }
    // Through the same normaliser first: it strips the `at` and completes the
    // bare hour that `tomorrow at 9` is made of, which is the difference
    // between a phrase the resolver reads and one it returns nothing for.
    let phrase = calendar::normalize(raw);
    let when = natural_date::resolve_datetime(&phrase, chrono::Local::now()).ok_or_else(|| {
        ContentError::Other(format!("'{raw}' is not a moment I can work out").into())
    })?;
    let local = when.with_timezone(&chrono::Local);
    // Through systemd anyway: it has the last word on the written form here
    // exactly as it does on a recurring one, and it reports the elapse back.
    calendar::check(&local.format("%Y-%m-%d %H:%M:%S").to_string()).await
}

/// Render a `.service` file. Only the lines that were filled in.
fn service_file(
    description: Option<&str>,
    command: &str,
    kind: &str,
    working_directory: Option<&str>,
    restart: &str,
    installable: bool,
) -> String {
    let mut out = String::from("[Unit]\n");
    out.push_str(&format!(
        "Description={}\n\n",
        description.unwrap_or("Created with not-yet-done")
    ));
    out.push_str("[Service]\n");
    out.push_str(&format!("Type={kind}\n"));
    out.push_str(&format!("ExecStart={command}\n"));
    if let Some(dir) = working_directory {
        out.push_str(&format!("WorkingDirectory={dir}\n"));
    }
    if restart != "no" {
        out.push_str(&format!("Restart={restart}\n"));
    }
    if installable {
        out.push_str("\n[Install]\nWantedBy=default.target\n");
    }
    out
}

/// Render a `.timer` file.
///
/// No `Unit=`: without one a timer starts the `.service` of the same name,
/// which is exactly the pair this form writes. Spelling it out would be a
/// second place for the two names to disagree.
fn timer_file(
    description: Option<&str>,
    on_calendar: &str,
    persistent: bool,
    delay: Option<&str>,
    accuracy: Option<&str>,
) -> String {
    let mut out = String::from("[Unit]\n");
    out.push_str(&format!(
        "Description={}\n\n",
        description.unwrap_or("Created with not-yet-done")
    ));
    out.push_str("[Timer]\n");
    out.push_str(&format!("OnCalendar={on_calendar}\n"));
    if persistent {
        out.push_str("Persistent=true\n");
    }
    if let Some(delay) = delay {
        out.push_str(&format!("RandomizedDelaySec={delay}\n"));
    }
    if let Some(accuracy) = accuracy {
        out.push_str(&format!("AccuracySec={accuracy}\n"));
    }
    out.push_str("\n[Install]\nWantedBy=timers.target\n");
    out
}

/// The buffer [`NEW_FILE`] opens: a skeleton, and above the cut the line that
/// gives the file its name.
pub fn file_template() -> String {
    buffer(UNIT_PLACEHOLDER, &[], SKELETON)
}

/// Hand an unsaved buffer back with the reason on top.
///
/// Used for the two ways a `new-file` save can come to nothing: the buffer
/// still carries the placeholder name, or systemd refused what is in it. Both
/// times the user's own text goes back untouched below the cut, and the header
/// is rebuilt rather than patched, so a second and third round trip do not
/// stack up three notices.
pub fn reopen(text: &str, notice: &[String]) -> String {
    let unit = unit_line(text).unwrap_or_else(|| UNIT_PLACEHOLDER.to_string());
    buffer(&unit, notice, edit::strip_header(text))
}

/// The skeleton a brand-new buffer starts from.
const SKELETON: &str = "[Unit]\n\
     Description=\n\
     \n\
     [Service]\n\
     Type=simple\n\
     ExecStart=\n\
     \n\
     [Install]\n\
     WantedBy=default.target\n";

/// The header (stripped again on save) plus `body`. Mirrors
/// [`edit::template`], including how it words a refusal — the two buffers are
/// the same thing to the person reading them.
fn buffer(unit: &str, notice: &[String], body: &str) -> String {
    let mut out = format!(
        "# A new unit file in {dir}.\n\
         #\n\
         # Name it on the line below. The suffix is what decides the kind of\n\
         # unit this is — .service, .timer, .socket, .path, .target — and the\n\
         # sections below have to match it.\n\
         #\n\
         {UNIT_LINE} {unit}\n\
         #\n\
         # Nothing is written until the file survives `systemd-analyze verify`,\n\
         # and an existing unit of that name is never overwritten — edit that\n\
         # one instead.\n",
        dir = edit::user_unit_dir().display(),
    );
    if !notice.is_empty() {
        out.push_str("#\n# NOT WRITTEN:\n#\n");
        for line in notice {
            out.push_str(&format!("#   {line}\n"));
        }
        out.push_str("#\n");
    }
    out.push_str("# Everything above the next line is stripped before saving.\n");
    out.push_str(edit::CUT);
    out.push('\n');
    out.push_str(body);
    out
}

/// Read the unit's name out of a [`NEW_FILE`] buffer's header.
///
/// `None` when the line is gone or still says the placeholder — in both cases
/// the user has not named the file, and guessing a name for a unit is guessing
/// what it does.
pub fn name_from_buffer(buffer: &str) -> Option<String> {
    let name = unit_line(buffer)?;
    if name.is_empty() || name == UNIT_PLACEHOLDER {
        return None;
    }
    Some(name)
}

/// The raw value of the `# unit:` line, placeholder and all — what the header
/// should carry when the buffer is handed back. Read only above the cut: a
/// `# unit:` line further down is part of the file the user wrote.
fn unit_line(buffer: &str) -> Option<String> {
    for line in buffer.lines() {
        if line.trim_end() == edit::CUT {
            break;
        }
        if let Some(rest) = line.trim().strip_prefix(UNIT_LINE) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Where a new unit's file goes, as an [`edit::Target`] so the check and the
/// write are the ones editing already uses.
pub fn target(unit: &str) -> edit::Target {
    edit::Target {
        unit: unit.to_string(),
        layer: edit::Layer::Full,
        path: edit::user_unit_dir().join(unit),
        vendor: None,
    }
}

/// Check every file, then write them all, then reload the manager once.
///
/// Nothing is written until everything has passed: a pair whose timer is
/// rejected must not leave its service behind, because the leftover is
/// invisible in the timers list and will confuse whoever finds it next.
/// A draft that has passed every check and is ready to be put on disk: the
/// exact bytes per path, plus whatever `systemd-analyze verify` had to say
/// without refusing the unit outright.
///
/// The split exists for the editor road. Both roads must be refused by the
/// same checks, but they part company in what a refusal costs: the form can
/// be handed back with its fields filled in, while the editor buffer holds a
/// whole file the user wrote and nothing else holds a copy of it. So
/// [`stage`] answers "may this be written, and why not" *before* anything is
/// touched, and the caller decides how to say no.
pub struct Staged {
    files: Vec<(PathBuf, String)>,
    notes: Vec<String>,
}

/// Check a draft without writing anything: no file may exist yet, and every
/// one of them must survive `systemd-analyze verify`.
///
/// An `Err` here is a refusal in a sentence fit to show the user — nothing has
/// been written, and for a pair that is the point: a timer systemd rejects
/// must not leave its service behind.
pub async fn stage(draft: &Draft) -> Result<Staged> {
    let mut files = Vec::new();
    let mut notes = Vec::new();
    for (unit, text) in &draft.files {
        let target = target(unit);
        if target.path.exists() {
            return Err(ContentError::Other(
                format!(
                    "{} already exists — open it with the editor instead",
                    target.path.display()
                )
                .into(),
            ));
        }
        let findings = edit::verify(&target, text).await?;
        if findings.fatal {
            return Err(ContentError::Other(
                format!("systemd refuses {unit}: {}", findings.summary()).into(),
            ));
        }
        if !findings.is_clean() {
            notes.push(findings.summary());
        }
        files.push((target.path.clone(), text.clone()));
    }
    Ok(Staged { files, notes })
}

/// Put a [`Staged`] draft on disk and let the manager re-read its files once —
/// once for the whole draft, not once per file, because a pair is one change.
pub async fn commit(bus: &Bus, draft: &Draft, staged: Staged) -> Result<String> {
    let mut written: Vec<String> = Vec::new();
    for (path, text) in &staged.files {
        edit::write_file(path, text)?;
        written.push(path.display().to_string());
    }
    bus.daemon_reload().await?;

    let mut message = format!("Wrote {}", written.join(" and "));
    if let Some(schedule) = &draft.schedule {
        message.push_str(&format!(". {}", schedule.summary()));
    }
    if !staged.notes.is_empty() {
        message.push_str(&format!(
            ". systemd had a note: {}",
            staged.notes.join("; ")
        ));
    }
    Ok(message)
}

/// Check a draft and write it — the form road, where a refusal is one sentence
/// under the fields and the values are still on screen.
pub async fn write(bus: &Bus, draft: &Draft) -> Result<String> {
    let staged = stage(draft).await?;
    commit(bus, draft, staged).await
}

/// The new unit, once it is on disk and the user asked for it to run.
///
/// The same verb the `a` leader runs — `enable-now` — rather than a second
/// implementation of enabling.
pub async fn enable(bus: &Bus, unit: &str) -> Result<String> {
    let verb = crate::control::verb("enable-now")
        .ok_or_else(|| ContentError::Other("no enable-now verb".into()))?;
    crate::control::run(bus, verb, unit, None).await
}

/// The outcome a finished create reports.
pub fn outcome(message: String) -> ActionOutcome {
    ActionOutcome::Done {
        message: Some(message),
    }
}

/// A unit name without its suffix, validated.
///
/// systemd's own rules are wider than this (instances, escaped paths), but a
/// name typed into a form that contains a slash or a space is a mistake every
/// time, and the file it would make is one the user cannot address afterwards.
fn unit_base(raw: &str) -> Result<String> {
    let name = raw.trim();
    let base = name
        .rsplit_once('.')
        .filter(|(_, suffix)| {
            matches!(
                *suffix,
                "service" | "timer" | "socket" | "path" | "target" | "mount"
            )
        })
        .map(|(base, _)| base)
        .unwrap_or(name);
    if base.is_empty() {
        return Err(ContentError::Other("a unit needs a name".into()));
    }
    if !base
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':' | '\\'))
    {
        return Err(ContentError::Other(
            format!("'{base}' is not a unit name — letters, digits, '-', '_' and '.'").into(),
        ));
    }
    Ok(base.to_string())
}

/// A required form field, trimmed.
fn required<'a>(values: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    match values.get(key).map(|v| v.trim()) {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(ContentError::Other(format!("'{key}' is required").into())),
    }
}

/// An optional field: absent and whitespace-only are the same thing.
fn opt(values: &HashMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// A toggle delivers `"true"` / `"false"`; anything else (including absent)
/// is off.
fn flag(values: &HashMap<String, String>, key: &str) -> bool {
    values.get(key).is_some_and(|v| v == "true")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_name_survives_the_suffix_the_user_did_or_did_not_type() {
        assert_eq!(unit_base("backup").unwrap(), "backup");
        assert_eq!(unit_base("backup.service").unwrap(), "backup");
        assert_eq!(unit_base(" backup.timer ").unwrap(), "backup");
        // Not a unit suffix — part of the name, and it stays.
        assert_eq!(unit_base("backup.db").unwrap(), "backup.db");
        assert!(unit_base("").is_err());
        assert!(unit_base("../../etc/passwd").is_err());
        assert!(unit_base("two words").is_err());
    }

    #[test]
    fn a_service_file_carries_only_what_was_filled_in() {
        let text = service_file(
            Some("Nightly backup"),
            "/usr/bin/backup",
            "oneshot",
            None,
            "no",
            true,
        );
        assert!(text.contains("Description=Nightly backup"));
        assert!(text.contains("Type=oneshot"));
        assert!(text.contains("ExecStart=/usr/bin/backup"));
        assert!(
            !text.contains("WorkingDirectory"),
            "not filled in, not written"
        );
        assert!(
            !text.contains("Restart="),
            "the default is not worth a line"
        );
        assert!(
            text.contains("[Install]"),
            "a standalone service can be enabled"
        );
    }

    /// The pair is the point: one submission, two files, one name.
    #[tokio::test]
    async fn a_timer_and_its_service_are_written_together() {
        let draft = draft(
            NEW_TIMER,
            &values(&[
                ("name", "backup"),
                ("command", "/usr/bin/backup"),
                ("schedule", "every day at 9"),
                ("persistent", "true"),
                ("enable", "true"),
            ]),
        )
        .await
        .expect("systemd accepts the schedule");
        let names: Vec<&str> = draft.files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["backup.service", "backup.timer"]);
        let service = &draft.files[0].1;
        assert!(
            !service.contains("[Install]"),
            "a timer's service is started by the timer, not enabled on its own"
        );
        let timer = &draft.files[1].1;
        assert!(timer.contains("OnCalendar=*-*-* 09:00:00"), "{timer}");
        assert!(timer.contains("Persistent=true"));
        assert!(
            !timer.contains("Unit="),
            "the same name is the link; naming it twice is a way to get it wrong"
        );
        assert_eq!(draft.enable.as_deref(), Some("backup.timer"));
    }

    /// No command means the service is already there — and must be left alone.
    #[tokio::test]
    async fn a_schedule_for_an_existing_service_writes_only_the_timer() {
        let draft = draft(
            NEW_TIMER,
            &values(&[("name", "backup"), ("schedule", "weekly")]),
        )
        .await
        .expect("weekly is systemd's own word");
        let names: Vec<&str> = draft.files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["backup.timer"]);
        assert!(draft.enable.is_none(), "the toggle was off");
    }

    /// The one-shot: a moment, not a recurrence. `natural-date` resolves it,
    /// systemd writes it as an absolute date.
    #[tokio::test]
    async fn once_resolves_a_phrase_to_a_single_instant() {
        let draft = draft(
            NEW_TIMER,
            &values(&[
                ("name", "reminder"),
                ("schedule", "tomorrow at 9"),
                ("once", "true"),
                ("persistent", "true"),
            ]),
        )
        .await
        .expect("tomorrow at 9 is a moment");
        let timer = &draft.files[0].1;
        let tomorrow = (chrono::Local::now() + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        assert!(
            timer.contains(&format!("OnCalendar={tomorrow} 09:00:00")),
            "{timer}"
        );
        assert!(
            !timer.contains("Persistent="),
            "catching up on a moment that has passed is not a thing"
        );
    }

    #[tokio::test]
    async fn a_schedule_systemd_cannot_read_stops_the_whole_draft() {
        let err = draft(
            NEW_TIMER,
            &values(&[("name", "backup"), ("schedule", "whenever")]),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("whenever"), "{err}");
    }

    #[test]
    fn the_buffer_carries_the_name_and_says_when_it_does_not() {
        let template = file_template();
        assert!(
            name_from_buffer(&template).is_none(),
            "the placeholder is not a name"
        );
        let named = template.replace(UNIT_PLACEHOLDER, "my-thing.socket");
        assert_eq!(name_from_buffer(&named).as_deref(), Some("my-thing.socket"));
        // Below the cut it is file content, not a header line.
        let body = format!("{}\n# unit: sneaky.service\n", edit::CUT);
        assert!(name_from_buffer(&body).is_none());
        // And the template still strips down to a unit file.
        assert!(edit::strip_header(&template).starts_with("[Unit]"));
    }

    /// A refused save must cost the user nothing but a correction: the text
    /// comes back whole, under the name they gave it, with one reason on top —
    /// and a second refusal replaces that reason instead of piling on.
    #[test]
    fn a_refused_buffer_comes_back_whole_and_says_why() {
        let typed =
            file_template().replace(UNIT_PLACEHOLDER, "my-thing.service") + "Environment=KEEP=me\n";
        let body = edit::strip_header(&typed).to_string();

        let first = reopen(&typed, &["systemd refuses it: nope".to_string()]);
        assert_eq!(edit::strip_header(&first), body, "the file survives intact");
        assert_eq!(
            name_from_buffer(&first).as_deref(),
            Some("my-thing.service"),
            "the name the user typed is still there"
        );
        assert!(first.contains("# NOT WRITTEN:"));
        assert!(first.contains("systemd refuses it: nope"));

        let second = reopen(&first, &["and now something else".to_string()]);
        assert_eq!(second.matches("# NOT WRITTEN:").count(), 1, "{second}");
        assert!(
            !second.contains("nope"),
            "the stale reason is gone: {second}"
        );
        assert_eq!(edit::strip_header(&second), body);
    }

    #[test]
    fn creating_is_offered_where_it_can_work() {
        assert_eq!(actions_for("systemd:manager", Manager::User).len(), 3);
        assert!(actions_for("systemd:manager", Manager::System).is_empty());
        // Not on a row: creating has no row.
        assert!(actions_for("systemd:service", Manager::User).is_empty());
    }

    #[test]
    fn a_new_unit_goes_into_the_users_own_tree() {
        let t = target("backup.timer");
        assert_eq!(t.layer, edit::Layer::Full);
        assert_eq!(t.path, edit::user_unit_dir().join("backup.timer"));
        assert!(t.vendor.is_none(), "there is nothing underneath a new unit");
    }
}
