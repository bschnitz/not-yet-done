//! The verbs — what the tab can *do* to a unit, and the two policies that
//! govern them.
//!
//! One table, read three ways: it is what
//! [`actions_for_type`](not_yet_done_content::ContentAdapter::actions_for_type)
//! offers per level, it is what decides whether a verb asks first, and it is
//! what [`run`] dispatches. Three lists that must agree would be three lists
//! that eventually do not.
//!
//! # Which level gets which verb
//!
//! Not every verb means something everywhere, and offering one that cannot work
//! is worse than not offering it: the user presses it, systemd refuses, and the
//! tab looks broken. So each verb declares its [`Scope`] — the levels on which
//! it is meaningful — and a `.timer` row never shows `kill` (a timer has no
//! processes), while a unit-file row shows `enable` but not `reload`.
//!
//! The one that looks surprising and is not: **a unit file can be started**.
//! `StartUnit` takes a name, loaded or not, which is exactly how a disabled
//! timer gets switched on — and the unit-files level is the only one that can
//! see a disabled timer at all.
//!
//! # The two policies
//!
//! * **[`Verb::confirm`]** — a sentence, or nothing. Asked for what ends
//!   something running or switches it off for good; not asked for what is
//!   cheap and reversible. The adapter phrases it, because only the adapter
//!   knows what the verb will do. See [`ActionDispatch::Confirm`].
//! * **[`Verb::disruptive`]** — whether [`crate::protect`] refuses it outright.
//!   Confirmation and protection are different instruments for different
//!   problems: one is for "did you mean it", the other for "this would take the
//!   session down with it", and a prompt is no answer to the second.
//!
//! # Keys
//!
//! Deliberately absent. A verb here has an id and a label; which key reaches it
//! is the view YAML's business (`docs/examples/views/systemd.yaml` puts them on
//! the `a` leader), exactly as for every other adapter.

use not_yet_done_content::{ContentError, InputSpec, NodeAction, Result, ValueOption};

use crate::bus::{Bus, FileChange, JobEnd, JobKind, Props, SERVICE_IFACE, UNIT_IFACE};

/// The levels a verb is offered on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every level that stands for a unit, including unit files and the two
    /// dependency levels — the verb takes a unit *name* and does not care
    /// whether the manager has it loaded.
    Any,
    /// Loaded units only (services and timers): the verb acts on runtime state
    /// that a unit file on disk does not have.
    Loaded,
    /// Services only: the verb acts on the unit's *processes*, and a timer or a
    /// unit file has none.
    Processes,
}

impl Scope {
    /// Whether a verb of this scope belongs on the level `type_id`.
    ///
    /// Every arm names the levels it covers rather than excluding the ones it
    /// does not: the adapter has levels that are not units at all — the manager
    /// root, a property row — and "everything except the unit files" would hand
    /// them `stop`.
    fn covers(self, type_id: &str) -> bool {
        match self {
            // A dependency row is a unit of *any* kind — a `.target`, a
            // `.slice`, a `.mount` — so it gets the verbs that work by name
            // and none of the ones that assume a service or a loaded unit.
            Scope::Any => matches!(
                type_id,
                "systemd:service"
                    | "systemd:timer"
                    | "systemd:unitfile"
                    | "systemd:dep"
                    | "systemd:order"
            ),
            Scope::Loaded => matches!(type_id, "systemd:service" | "systemd:timer"),
            Scope::Processes => type_id == "systemd:service",
        }
    }
}

/// What a verb actually does on the bus.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    /// Enqueue a manager job and wait for its outcome.
    Job(JobKind),
    /// Change the unit's symlinks on disk.
    Files(FileChange),
    /// Both, in that order — the `--now` verbs. The file change comes first so
    /// that a failing enable does not leave a started-but-not-enabled unit.
    FilesThenJob(FileChange, JobKind),
    /// Send a signal to the unit's processes.
    Kill,
    /// Clear the `failed` state.
    ResetFailed,
    /// Suspend or resume the unit's cgroup, whichever it is not doing now.
    FreezeToggle,
}

/// One thing the tab can do to a unit.
pub struct Verb {
    /// Stable id — what the view YAML binds a key to.
    pub id: &'static str,
    pub label: &'static str,
    pub scope: Scope,
    pub op: Op,
    /// The question to ask before doing it, with `{unit}` standing in for the
    /// unit's name. `None` means it just happens.
    pub confirm: Option<&'static str>,
    /// Whether this interrupts a running unit — see [`crate::protect`].
    pub disruptive: bool,
    /// Whether the verb consumes a value (today: `kill`'s signal).
    pub takes_value: bool,
}

/// The whole vocabulary, in the order the action list shows it.
pub static VERBS: &[Verb] = &[
    Verb {
        id: "start",
        label: "Start",
        scope: Scope::Any,
        op: Op::Job(JobKind::Start),
        confirm: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "stop",
        label: "Stop",
        scope: Scope::Any,
        op: Op::Job(JobKind::Stop),
        confirm: Some("Stop {unit}? Whatever depends on it stops too. (y/n)"),
        disruptive: true,
        takes_value: false,
    },
    Verb {
        id: "restart",
        label: "Restart",
        scope: Scope::Any,
        op: Op::Job(JobKind::Restart),
        confirm: None,
        disruptive: true,
        takes_value: false,
    },
    Verb {
        id: "reload",
        label: "Reload",
        scope: Scope::Processes,
        op: Op::Job(JobKind::Reload),
        confirm: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "reload-or-restart",
        label: "Reload or restart",
        scope: Scope::Processes,
        op: Op::Job(JobKind::ReloadOrRestart),
        confirm: None,
        disruptive: true,
        takes_value: false,
    },
    Verb {
        id: "enable",
        label: "Enable",
        scope: Scope::Any,
        op: Op::Files(FileChange::Enable),
        confirm: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "enable-now",
        label: "Enable and start",
        scope: Scope::Any,
        op: Op::FilesThenJob(FileChange::Enable, JobKind::Start),
        confirm: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "disable",
        label: "Disable",
        scope: Scope::Any,
        op: Op::Files(FileChange::Disable),
        confirm: Some("Disable {unit}? It will not come back on the next login. (y/n)"),
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "disable-now",
        label: "Disable and stop",
        scope: Scope::Any,
        op: Op::FilesThenJob(FileChange::Disable, JobKind::Stop),
        confirm: Some("Disable and stop {unit}? It stops now and stays off. (y/n)"),
        disruptive: true,
        takes_value: false,
    },
    Verb {
        id: "mask",
        label: "Mask",
        scope: Scope::Any,
        op: Op::Files(FileChange::Mask),
        confirm: Some(
            "Mask {unit}? Nothing can start it again — not a dependency, not you — until it is \
             unmasked. (y/n)",
        ),
        disruptive: true,
        takes_value: false,
    },
    Verb {
        id: "preset",
        label: "Apply preset",
        scope: Scope::Any,
        op: Op::Files(FileChange::Preset),
        // The one verb whose outcome is not in its name — it enables or
        // disables depending on a policy file. The question therefore names the
        // policy's answer, which the row is already showing in its `preset`
        // column, so the confirmation and the table agree.
        confirm: Some(
            "Apply the preset policy to {unit}? It is enabled or disabled to match what the \
             distribution ships — the Preset column says which. (y/n)",
        ),
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "unmask",
        label: "Unmask",
        scope: Scope::Any,
        op: Op::Files(FileChange::Unmask),
        confirm: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "reset-failed",
        label: "Reset failed",
        scope: Scope::Loaded,
        op: Op::ResetFailed,
        confirm: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "kill",
        label: "Kill",
        scope: Scope::Processes,
        op: Op::Kill,
        confirm: Some("Send the signal to {unit}? Its processes get no chance to clean up. (y/n)"),
        disruptive: true,
        takes_value: true,
    },
    Verb {
        id: "freeze",
        label: "Freeze / thaw",
        scope: Scope::Processes,
        op: Op::FreezeToggle,
        confirm: None,
        disruptive: true,
        takes_value: false,
    },
];

/// The verb with this id, if it is one of ours.
pub fn verb(id: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.id == id)
}

/// The verbs offered on a level, as declared actions.
pub fn actions_for(type_id: &str) -> Vec<NodeAction> {
    VERBS
        .iter()
        .filter(|v| v.scope.covers(type_id))
        .map(|v| {
            // `kill` is the only verb that needs an answer before it acts, and
            // the answer is *which signal* — so it declares a picker and the
            // frontend fetches the list instead of firing blind.
            let input = if v.takes_value {
                InputSpec::Picker
            } else {
                InputSpec::None
            };
            NodeAction::new(v.id, v.label, input)
        })
        .collect()
}

/// The signals `kill` offers, as a value list the frontend can put in a menu.
///
/// Short on purpose. The full signal set is a `man 7 signal` table, and a menu
/// of sixty entries is not a menu; these are the ones anyone sends to a service
/// by hand.
pub const SIGNALS: &[(&str, i32, &str)] = &[
    ("SIGTERM", 15, "Ask it to stop"),
    ("SIGKILL", 9, "Stop it now, no cleanup"),
    ("SIGHUP", 1, "Reload its configuration"),
    ("SIGINT", 2, "Interrupt, as Ctrl+C would"),
    ("SIGQUIT", 3, "Quit and dump core"),
    ("SIGUSR1", 10, "Whatever the service made of it"),
    ("SIGUSR2", 12, "Whatever the service made of it"),
];

/// The signal list as selectable values.
pub fn signal_options() -> Vec<ValueOption> {
    SIGNALS
        .iter()
        .map(|(name, number, note)| ValueOption {
            value: (*name).to_string(),
            label: format!("{name} — {note}"),
            extra: [("number".to_string(), number.to_string())]
                .into_iter()
                .collect(),
        })
        .collect()
}

/// Resolve what the user picked into a signal number.
///
/// Accepts the name (`SIGTERM`), the bare name (`TERM`) or the number, because
/// the value may arrive from a menu, from a view YAML's `args:` or from the
/// CLI, and all three spell it differently. `None` means SIGTERM — the same
/// default `systemctl kill` has.
fn signal_number(value: Option<&str>) -> Result<i32> {
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(15);
    };
    if let Ok(n) = raw.parse::<i32>() {
        return (1..=64).contains(&n).then_some(n).ok_or_else(|| {
            ContentError::Other(format!("{n} is not a signal number (1-64)").into())
        });
    }
    let wanted = raw.to_ascii_uppercase();
    SIGNALS
        .iter()
        .find(|(name, ..)| *name == wanted || name.trim_start_matches("SIG") == wanted)
        .map(|(_, number, _)| *number)
        .ok_or_else(|| ContentError::Other(format!("unknown signal {raw:?}").into()))
}

/// Do it, and say what happened.
///
/// The returned string is what the user sees in the notification — the whole
/// point of the phase is that it reports the *outcome*, not the fact that a
/// message was sent. A verb that did not achieve what it was asked comes back
/// as an error, not as a cheerful message.
pub async fn run(bus: &Bus, verb: &Verb, unit: &str, value: Option<&str>) -> Result<String> {
    match verb.op {
        Op::Job(kind) => job(bus, kind, unit).await,
        Op::Files(change) => files(bus, change, unit).await,
        Op::FilesThenJob(change, kind) => {
            let first = files(bus, change, unit).await?;
            let second = job(bus, kind, unit).await?;
            Ok(format!("{first}; {second}"))
        }
        Op::Kill => {
            let number = signal_number(value)?;
            let name = SIGNALS
                .iter()
                .find(|(_, n, _)| *n == number)
                .map(|(name, ..)| *name)
                .unwrap_or("signal");
            bus.kill(unit, "all", number).await?;
            Ok(format!("Sent {name} to {unit}"))
        }
        Op::ResetFailed => {
            bus.reset_failed(unit).await?;
            Ok(format!("Cleared the failed state of {unit}"))
        }
        Op::FreezeToggle => {
            let path = bus.unit_path(unit).await?;
            let props = bus.properties(&path, UNIT_IFACE).await;
            // Anything but a plain "running" counts as frozen-or-freezing, so a
            // second press on a unit mid-freeze thaws it rather than queuing
            // another freeze.
            let frozen = !matches!(prop_str(&props, "FreezerState").as_str(), "running" | "");
            bus.freeze(unit, !frozen).await?;
            Ok(if frozen {
                format!("Thawed {unit}")
            } else {
                format!("Froze {unit}")
            })
        }
    }
}

/// Run a job and turn the manager's one-word verdict into a sentence.
async fn job(bus: &Bus, kind: JobKind, unit: &str) -> Result<String> {
    match bus.run_job(kind, unit).await? {
        JobEnd::StillRunning => Ok(format!(
            "{} {unit} — still running after {}s; it was not cancelled",
            kind.gerund(),
            crate::bus::JOB_WAIT_SECS
        )),
        JobEnd::Reported(result) if result == "done" => {
            Ok(format!("{} {unit} — done", kind.gerund()))
        }
        JobEnd::Reported(result) => {
            let detail = failure_detail(bus, unit).await;
            Err(ContentError::Other(
                format!(
                    "{} {unit} — {}{detail}",
                    kind.gerund().to_lowercase(),
                    explain(&result)
                )
                .into(),
            ))
        }
    }
}

/// The manager's result word, said in words.
fn explain(result: &str) -> String {
    match result {
        "failed" => "the job failed".into(),
        "timeout" => "the job timed out".into(),
        "canceled" => "the job was cancelled".into(),
        "dependency" => "a dependency of it failed".into(),
        "skipped" => "the job was skipped (a condition did not hold)".into(),
        "collected" => "the job was collected without running".into(),
        other => format!("the job ended as {other}"),
    }
}

/// What the unit itself says about why it is not running.
///
/// Read after the fact, from the unit's own post-mortem properties. The journal
/// lines that would explain *why* the process exited are phase 4; until then
/// this is the honest half — the exit code and status the manager recorded,
/// which is already the difference between "it failed" and "it failed with 2".
async fn failure_detail(bus: &Bus, unit: &str) -> String {
    let Ok(path) = bus.unit_path(unit).await else {
        return String::new();
    };
    let service = bus.properties(&path, SERVICE_IFACE).await;
    let mut parts = Vec::new();
    match prop_str(&service, "Result").as_str() {
        "" | "success" => {}
        result => parts.push(result.to_string()),
    }
    if let Some(status) = prop_i64(&service, "ExecMainStatus").filter(|s| *s != 0) {
        parts.push(format!("exit status {status}"));
    }
    let text = prop_str(&service, "StatusText");
    if !text.is_empty() {
        parts.push(text);
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

/// Change the unit's symlinks and report what moved — and where.
///
/// The manager answers with one `(operation, filename, destination)` per entry,
/// and the message hands all of it on: a headline that says what happened, then
/// one line per path. The first line is what the notification bar shows; the
/// rest appears when the entry is expanded, so naming every symlink costs
/// nothing on screen and saves the user the trip to `ls` that a bare count
/// would have forced.
async fn files(bus: &Bus, change: FileChange, unit: &str) -> Result<String> {
    let result = bus.change_unit_file(change, unit).await?;
    if !result.carries_install_info {
        // Not an error — systemd did exactly what was asked and it amounted to
        // nothing. Saying "enabled" here would be the lie.
        return Ok(format!(
            "{unit} has no [Install] section, so there is nothing to enable — it is started by \
             something else, not at login"
        ));
    }
    let made = count(&result.changes, Moved::Made);
    let removed = count(&result.changes, Moved::Removed);
    let notes = count(&result.changes, Moved::Note);
    Ok(with_paths(
        headline(change, unit, made, removed, notes),
        &result.changes,
    ))
}

/// Count the entries of one kind.
fn count(changes: &[(String, String, String)], kind: Moved) -> usize {
    changes.iter().filter(|c| moved(&c.0) == kind).count()
}

/// The one line the notification bar shows: what the operation did.
///
/// Split out from [`files`] because it is the part with the judgement in it and
/// the part worth testing — everything else is a bus round trip.
fn headline(change: FileChange, unit: &str, made: usize, removed: usize, notes: usize) -> String {
    let links = made + removed;
    match change {
        // The preset has no past participle of its own — it enabled or it
        // disabled, and which one was the policy's decision, not the user's.
        // That decision is the one thing the user cannot know beforehand, which
        // is why it belongs in the headline; the manager spells it out in the
        // operations, a written link meaning on and a removed one meaning off.
        FileChange::Preset => match (links, removed, made) {
            (0, _, _) if notes == 0 => format!("{unit} already matches its preset"),
            (0, _, _) => format!("The preset changed nothing for {unit}"),
            (_, 0, _) => format!("Enabled {unit} — the preset wants it on"),
            (_, _, 0) => format!("Disabled {unit} — the preset wants it off"),
            // Both directions at once: an alias moved, or an old link was
            // replaced. Neither verb would be the whole truth, so neither is
            // claimed and the paths below tell the story.
            _ => format!("Applied the preset to {unit}"),
        },
        _ => {
            let done = match change {
                FileChange::Enable => "Enabled",
                FileChange::Disable => "Disabled",
                FileChange::Mask => "Masked",
                FileChange::Unmask => "Unmasked",
                FileChange::Preset => unreachable!("handled above"),
            };
            match links {
                0 if notes == 0 => format!("{unit} was already {}", done.to_lowercase()),
                // Nothing moved, but systemd had something to say — a masked
                // unit refuses to be enabled through this very channel. The
                // note below is the reason; claiming the verb here would be the
                // second lie this function exists to avoid.
                0 => format!("Nothing changed for {unit}"),
                1 => format!("{done} {unit}"),
                n => format!("{done} {unit} ({n} symlinks)"),
            }
        }
    }
}

/// What one entry of the manager's change list actually says.
///
/// The list is not only symlinks: systemd reports its refusals through the same
/// array — `masked` when the unit is masked, `dangling` when the link points at
/// nothing — and counting those as work done is how a message ends up claiming
/// an enable that never happened.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Moved {
    /// A link was written. `copy` is the same event for the rare unit systemd
    /// copies instead of linking.
    Made,
    /// A link was taken away.
    Removed,
    /// Not a change at all: something systemd wants said about the unit.
    Note,
}

fn moved(op: &str) -> Moved {
    match op {
        "symlink" | "copy" => Moved::Made,
        "unlink" => Moved::Removed,
        _ => Moved::Note,
    }
}

/// The headline, then one line per path the manager touched.
fn with_paths(headline: String, changes: &[(String, String, String)]) -> String {
    let mut out = headline;
    for (op, file, _destination) in changes {
        // The destination is left out on purpose: for a written link it is the
        // unit file the row already names, and for everything else it is empty.
        match moved(op) {
            Moved::Made => out.push_str(&format!("\n+ {file}")),
            Moved::Removed => out.push_str(&format!("\n- {file}")),
            Moved::Note => out.push_str(&format!("\n! {file} — {}", note(op))),
        }
    }
    out
}

/// What systemd means by a change type that moved no symlink.
fn note(op: &str) -> String {
    match op {
        "masked" => "it is masked, so no symlink was written".into(),
        "dangling" => "the symlink points at a unit that is not there".into(),
        "dst-not-present" => "the unit file it would point at is missing".into(),
        "auxiliary-failed" => "a unit it also asked for could not be changed".into(),
        other => format!("systemd reported {other}"),
    }
}

pub(crate) fn prop_str(props: &Props, key: &str) -> String {
    use zbus_systemd::zvariant::Value;
    match props.get(key).map(|v| &**v) {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    }
}

fn prop_i64(props: &Props, key: &str) -> Option<i64> {
    use zbus_systemd::zvariant::Value;
    match props.get(key).map(|v| &**v)? {
        Value::I32(n) => Some(i64::from(*n)),
        Value::I64(n) => Some(*n),
        Value::U32(n) => Some(i64::from(*n)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verb_that_needs_a_signal_declares_a_picker() {
        let kill = actions_for("systemd:service")
            .into_iter()
            .find(|a| a.id == "kill")
            .expect("services can be killed");
        assert!(matches!(kill.input, InputSpec::Picker));
        // And nothing else asks for input — the rest act on the row alone.
        assert!(
            actions_for("systemd:service")
                .iter()
                .filter(|a| a.id != "kill")
                .all(|a| matches!(a.input, InputSpec::None))
        );
    }

    #[test]
    fn a_level_that_is_not_a_unit_is_offered_nothing() {
        // The manager root and a property row are levels of this adapter too,
        // and neither is something you can stop.
        assert!(actions_for("systemd:manager").is_empty());
        assert!(actions_for("systemd:property").is_empty());
    }

    #[test]
    fn a_timer_is_not_offered_the_process_verbs() {
        let ids: Vec<&str> = actions_for("systemd:timer")
            .iter()
            .map(|a| a.id.clone().leak() as &str)
            .collect();
        assert!(ids.contains(&"start"));
        assert!(ids.contains(&"enable-now"));
        assert!(!ids.contains(&"kill"), "a timer has no processes");
        assert!(!ids.contains(&"reload"));
    }

    #[test]
    fn a_unit_file_is_offered_only_what_works_without_being_loaded() {
        let ids: Vec<String> = actions_for("systemd:unitfile")
            .iter()
            .map(|a| a.id.clone())
            .collect();
        // The point of the level: switching on a timer that is not loaded
        // *because* it is disabled.
        assert!(ids.iter().any(|i| i == "enable-now"));
        assert!(ids.iter().any(|i| i == "start"));
        assert!(!ids.iter().any(|i| i == "reset-failed"));
    }

    #[test]
    fn every_disruptive_verb_is_either_asked_about_or_plainly_reversible() {
        for v in VERBS {
            if v.confirm.is_some() {
                assert!(
                    v.confirm.unwrap().contains("{unit}"),
                    "{}: the prompt must name the unit",
                    v.id
                );
            }
        }
        // Restart and freeze are disruptive without asking: both leave the unit
        // running afterwards, so the worst case is a blip, not a loss.
        let silent: Vec<&str> = VERBS
            .iter()
            .filter(|v| v.disruptive && v.confirm.is_none())
            .map(|v| v.id)
            .collect();
        assert_eq!(silent, vec!["restart", "reload-or-restart", "freeze"]);
    }

    #[test]
    fn signals_are_accepted_by_name_bare_name_or_number() {
        assert_eq!(signal_number(None).unwrap(), 15);
        assert_eq!(signal_number(Some("SIGKILL")).unwrap(), 9);
        assert_eq!(signal_number(Some("kill")).unwrap(), 9);
        assert_eq!(signal_number(Some("1")).unwrap(), 1);
        assert!(signal_number(Some("SIGNOPE")).is_err());
        assert!(signal_number(Some("99")).is_err());
    }

    fn change(op: &str, file: &str) -> (String, String, String) {
        (op.into(), file.into(), String::new())
    }

    #[test]
    fn the_preset_headline_names_the_direction_it_chose() {
        // The whole point of `a p` is that the policy decides, so the decision
        // is the one thing the user still has to be told.
        assert_eq!(
            headline(FileChange::Preset, "probe.service", 1, 0, 0),
            "Enabled probe.service — the preset wants it on"
        );
        assert_eq!(
            headline(FileChange::Preset, "probe.service", 0, 1, 0),
            "Disabled probe.service — the preset wants it off"
        );
        // Both directions at once claims neither.
        assert_eq!(
            headline(FileChange::Preset, "probe.service", 1, 1, 0),
            "Applied the preset to probe.service"
        );
        assert_eq!(
            headline(FileChange::Preset, "probe.service", 0, 0, 0),
            "probe.service already matches its preset"
        );
    }

    #[test]
    fn a_refusal_is_never_reported_as_the_verb() {
        // systemd sends its refusals through the same array as its symlinks.
        // Counting them as work is how a masked unit gets reported as enabled.
        assert_eq!(
            headline(FileChange::Enable, "probe.service", 0, 0, 1),
            "Nothing changed for probe.service"
        );
        // No entries at all is the other story, and keeps its own sentence.
        assert_eq!(
            headline(FileChange::Enable, "probe.service", 0, 0, 0),
            "probe.service was already enabled"
        );
    }

    #[test]
    fn every_path_the_manager_touched_reaches_the_message() {
        let text = with_paths(
            "Enabled probe.service — the preset wants it on".into(),
            &[
                change("symlink", "/wants/probe.service"),
                change("unlink", "/wants/old.service"),
                change("masked", "/units/probe.service"),
            ],
        );
        let lines: Vec<&str> = text.lines().collect();
        // The headline stays the first line: it is all the notification bar
        // shows, and the rest only appears when the entry is expanded.
        assert_eq!(lines[0], "Enabled probe.service — the preset wants it on");
        assert_eq!(lines[1], "+ /wants/probe.service");
        assert_eq!(lines[2], "- /wants/old.service");
        assert_eq!(
            lines[3],
            "! /units/probe.service — it is masked, so no symlink was written"
        );
    }
}
