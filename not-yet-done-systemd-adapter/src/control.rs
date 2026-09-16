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
//! * **[`Verb::protected`]** — whether [`crate::protect`] refuses it outright.
//!   Confirmation and protection are different instruments for different
//!   problems: one is for "did you mean it", the other for "this would take the
//!   session down with it", and a prompt is no answer to the second.
//!
//! Both policies read the manager. The same verb is a different act on the
//! system bus — `disable` there is a decision about the next boot of the whole
//! machine — so [`Verb::confirm_on`] has a sharper sentence for it, and
//! [`Verb::available_on`] withholds the one verb whose effect the runtime
//! columns cannot show.
//!
//! # Keys
//!
//! Deliberately absent. A verb here has an id and a label; which key reaches it
//! is the view YAML's business (`docs/examples/views/systemd.yaml` puts them on
//! the `a` leader), exactly as for every other adapter.

use not_yet_done_content::{ContentError, InputSpec, NodeAction, Result, ValueOption};

use crate::bus::{Bus, FileChange, JobEnd, JobKind, Props, SERVICE_IFACE, UNIT_IFACE};
use crate::config::Manager;

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
                    // A row on the critical chain is a unit like any other —
                    // and the level is where a slow start is found, so it is
                    // also where restarting the culprit belongs.
                    | "systemd:chain"
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

impl Op {
    /// Whether this masks or unmasks — the one file change [`Verb::available_on`]
    /// withholds from the system manager.
    ///
    /// Not because it is the most powerful. Because it is the one that *hides
    /// itself*: a masked unit looks, in every runtime column, exactly like a
    /// unit nobody happened to start. `active: inactive`, no failure, no
    /// journal line — and the reason is a symlink to `/dev/null` that only the
    /// Unit files level would show. Disabling is at least legible in the row
    /// that did it; masking is where hours go.
    pub fn masks_a_unit(self) -> bool {
        matches!(self, Op::Files(FileChange::Mask | FileChange::Unmask))
    }

    /// Whether this leaves the unit switched **off for the next boot**.
    ///
    /// The second shape of harm the protection list answers to. `disruptive`
    /// covers the immediate one — the unit stops now, and the session notices
    /// within the second. This one is quiet until the next login or reboot,
    /// and by then nothing on screen connects the missing unit to the key that
    /// was pressed. Both are refusals, not questions: see [`Verb::protected`].
    pub fn switches_off(self) -> bool {
        matches!(
            self,
            Op::Files(FileChange::Disable | FileChange::Mask)
                | Op::FilesThenJob(FileChange::Disable | FileChange::Mask, _)
        )
    }
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
    /// The question to ask instead when the verb runs against the **system**
    /// manager. `None` falls back to [`Verb::confirm`].
    ///
    /// Only the file verbs set it, and the difference is not emphasis. On the
    /// user manager "it will not come back on the next login" is the whole
    /// truth; on the system manager the same key press decides what the
    /// machine does at the next boot, for every account on it. A prompt that
    /// says the first while doing the second is worse than no prompt, because
    /// it was read and believed.
    ///
    /// Why this sentence rather than polkit's: the password dialog knows only
    /// that *something* wants `manage-unit-files`. It cannot name the unit.
    /// This is the only question in the chain that can.
    pub confirm_system: Option<&'static str>,
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
        confirm_system: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "stop",
        label: "Stop",
        scope: Scope::Any,
        op: Op::Job(JobKind::Stop),
        confirm: Some("Stop {unit}? Whatever depends on it stops too. (y/n)"),
        confirm_system: None,
        disruptive: true,
        takes_value: false,
    },
    Verb {
        id: "restart",
        label: "Restart",
        scope: Scope::Any,
        op: Op::Job(JobKind::Restart),
        confirm: None,
        confirm_system: None,
        disruptive: true,
        takes_value: false,
    },
    Verb {
        id: "reload",
        label: "Reload",
        scope: Scope::Processes,
        op: Op::Job(JobKind::Reload),
        confirm: None,
        confirm_system: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "reload-or-restart",
        label: "Reload or restart",
        scope: Scope::Processes,
        op: Op::Job(JobKind::ReloadOrRestart),
        confirm: None,
        confirm_system: None,
        disruptive: true,
        takes_value: false,
    },
    Verb {
        id: "enable",
        label: "Enable",
        scope: Scope::Any,
        op: Op::Files(FileChange::Enable),
        confirm: None,
        confirm_system: Some(
            "Enable {unit} on this machine? It starts on every boot from now on, for everyone \
             who uses it. (y/n)",
        ),
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "enable-now",
        label: "Enable and start",
        scope: Scope::Any,
        op: Op::FilesThenJob(FileChange::Enable, JobKind::Start),
        confirm: None,
        confirm_system: Some(
            "Enable and start {unit} on this machine? It starts now and on every boot from now \
             on, for everyone who uses it. (y/n)",
        ),
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "disable",
        label: "Disable",
        scope: Scope::Any,
        op: Op::Files(FileChange::Disable),
        confirm: Some("Disable {unit}? It will not come back on the next login. (y/n)"),
        confirm_system: Some(
            "Disable {unit} on this machine? It will not start at the next boot — for anyone, \
             not only for you. (y/n)",
        ),
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "disable-now",
        label: "Disable and stop",
        scope: Scope::Any,
        op: Op::FilesThenJob(FileChange::Disable, JobKind::Stop),
        confirm: Some("Disable and stop {unit}? It stops now and stays off. (y/n)"),
        confirm_system: Some(
            "Disable and stop {unit} on this machine? It stops now and will not start at the \
             next boot, for anyone. (y/n)",
        ),
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
        confirm_system: None,
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
        confirm_system: Some(
            "Apply the preset policy to {unit} on this machine? It is enabled or disabled to \
             match what the distribution ships — the Preset column says which — and that holds \
             from the next boot, for everyone. (y/n)",
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
        confirm_system: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "reset-failed",
        label: "Reset failed",
        scope: Scope::Loaded,
        op: Op::ResetFailed,
        confirm: None,
        confirm_system: None,
        disruptive: false,
        takes_value: false,
    },
    Verb {
        id: "kill",
        label: "Kill",
        scope: Scope::Processes,
        op: Op::Kill,
        confirm: Some("Send the signal to {unit}? Its processes get no chance to clean up. (y/n)"),
        confirm_system: None,
        disruptive: true,
        takes_value: true,
    },
    Verb {
        id: "freeze",
        label: "Freeze / thaw",
        scope: Scope::Processes,
        op: Op::FreezeToggle,
        confirm: None,
        confirm_system: None,
        disruptive: true,
        takes_value: false,
    },
];

/// The verb with this id, if it is one of ours.
///
/// Says nothing about whether it may run here — [`verb_on`] is the question
/// with the manager in it, and every dispatch path asks that one.
pub fn verb(id: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.id == id)
}

/// The verb with this id **if this manager offers it at all**.
///
/// The one chokepoint for the manager question, because a verb is reached by
/// two roads: the action list a frontend renders, and an id a caller names
/// outright — the CLI can invoke `stop` without ever having asked what the
/// level offers. Gating only the list would leave the second road open, and a
/// verb the tab refuses to show is not one the CLI may fire.
pub fn verb_on(manager: Manager, id: &str) -> Option<&'static Verb> {
    verb(id).filter(|v| v.available_on(manager))
}

impl Verb {
    /// Whether this verb may run against that manager.
    ///
    /// Every write against the **system** manager is a privileged call, and the
    /// adapter asks for that authorisation properly — see
    /// [`crate::bus::Bus::write`]. So the runtime verbs are offered there, and
    /// so are the file verbs that switch a unit on or off: `enable`, `disable`,
    /// their `--now` pair, and `preset`. What each of them does is written in
    /// the row afterwards — the `enabled` column changes, the `drift` column
    /// clears — so the tab can be believed about its own effect.
    ///
    /// **Except masking**, which is withheld on the system manager for a reason
    /// that is not about power: see [`Op::masks_a_unit`]. It is the one write
    /// whose result the runtime columns cannot show.
    ///
    /// Reading is unaffected either way: the levels, the journal and
    /// `systemd-analyze` need no privilege at all.
    pub fn available_on(&self, manager: Manager) -> bool {
        match manager {
            Manager::User => true,
            Manager::System => !self.op.masks_a_unit(),
        }
    }

    /// The question to ask before this verb runs on that manager, or `None`
    /// when it just happens. `{unit}` is still to be substituted.
    pub fn confirm_on(&self, manager: Manager) -> Option<&'static str> {
        match manager {
            Manager::System => self.confirm_system.or(self.confirm),
            Manager::User => self.confirm,
        }
    }

    /// Whether [`crate::protect`] refuses this verb outright on a unit its list
    /// covers.
    ///
    /// One question, two shapes of harm, because the protection list answers to
    /// both: [`Verb::disruptive`] is the unit going down *now*, and
    /// [`Op::switches_off`] is the unit not coming back *next time*. Neither is
    /// something a y/n prompt helps with — the hand that pressed the key
    /// presses `y` as well — which is why this is a refusal and not a
    /// confirmation.
    pub fn protected(&self) -> bool {
        self.disruptive || self.op.switches_off()
    }
}

/// The verbs offered on a level, as declared actions.
pub fn actions_for(type_id: &str, manager: Manager) -> Vec<NodeAction> {
    VERBS
        .iter()
        .filter(|v| v.available_on(manager) && v.scope.covers(type_id))
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
///
/// With one exception, and it is deliberate: a write the user **declined to
/// authorise** is reported as an ordinary sentence, not as an error. Dismissing
/// a password dialog is an answer, and answering "no" to a question the tab
/// itself asked is not a fault worth colouring red. The only refusals that
/// reach here come from the bus — the protection list refuses earlier, before
/// a call is ever made — so there is nothing else this could swallow.
pub async fn run(bus: &Bus, verb: &Verb, unit: &str, value: Option<&str>) -> Result<String> {
    match dispatch(bus, verb, unit, value).await {
        Err(ContentError::PermissionDenied(said)) => Ok(said),
        other => other,
    }
}

async fn dispatch(bus: &Bus, verb: &Verb, unit: &str, value: Option<&str>) -> Result<String> {
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
        //
        // "at login" and "at boot" are not decoration: the sentence explains
        // what enabling *would* have meant, and on the system manager that is
        // the machine coming up, not this session.
        let moment = match bus.manager() {
            Manager::User => "at login",
            Manager::System => "at boot",
        };
        return Ok(format!(
            "{unit} has no [Install] section, so there is nothing to enable — it is started by \
             something else, not {moment}"
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
        let kill = actions_for("systemd:service", Manager::User)
            .into_iter()
            .find(|a| a.id == "kill")
            .expect("services can be killed");
        assert!(matches!(kill.input, InputSpec::Picker));
        // And nothing else asks for input — the rest act on the row alone.
        assert!(
            actions_for("systemd:service", Manager::User)
                .iter()
                .filter(|a| a.id != "kill")
                .all(|a| matches!(a.input, InputSpec::None))
        );
    }

    #[test]
    fn a_level_that_is_not_a_unit_is_offered_nothing() {
        // The manager root and a property row are levels of this adapter too,
        // and neither is something you can stop.
        assert!(actions_for("systemd:manager", Manager::User).is_empty());
        assert!(actions_for("systemd:property", Manager::User).is_empty());
    }

    #[test]
    fn a_timer_is_not_offered_the_process_verbs() {
        let ids: Vec<&str> = actions_for("systemd:timer", Manager::User)
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
        let ids: Vec<String> = actions_for("systemd:unitfile", Manager::User)
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

    #[test]
    fn the_system_manager_is_offered_everything_but_masking() {
        let ids = |type_id| -> Vec<String> {
            actions_for(type_id, Manager::System)
                .into_iter()
                .map(|a| a.id)
                .collect()
        };
        let on_a_service = ids("systemd:service");
        // The runtime verbs, and the file verbs whose effect the row itself
        // reports back — polkit can authorise every one of them.
        for offered in [
            "start",
            "stop",
            "restart",
            "reload",
            "kill",
            "freeze",
            "enable",
            "enable-now",
            "disable",
            "disable-now",
            "preset",
        ] {
            assert!(
                on_a_service.contains(&offered.to_string()),
                "{offered} should be offered on the system manager"
            );
        }
        // Masking is the exception, in both directions: a machine-wide mask is
        // invisible in the runtime columns, and an unmask offered without it
        // would be a key for undoing something this tab cannot do.
        for withheld in ["mask", "unmask"] {
            assert!(
                !on_a_service.contains(&withheld.to_string()),
                "{withheld} should be withheld on the system manager"
            );
        }
        // The Unit files level draws the same line, and it is the level where
        // the file verbs earn their place: a disabled unit is only visible here.
        let on_a_file = ids("systemd:unitfile");
        assert!(on_a_file.contains(&"start".to_string()));
        assert!(on_a_file.contains(&"enable".to_string()));
        assert!(!on_a_file.contains(&"mask".to_string()));
        // Both roads ask the same question: naming the verb outright — what
        // the CLI does — agrees with what the list shows.
        assert!(verb_on(Manager::System, "enable").is_some());
        assert!(verb_on(Manager::System, "mask").is_none());
        assert!(verb_on(Manager::User, "mask").is_some());
        // `verb` itself still knows the whole table; it is the lookup without
        // the manager question in it.
        assert!(verb("mask").is_some());
    }

    #[test]
    fn a_file_verb_says_something_different_about_the_machine_than_about_the_session() {
        // The point of the second sentence: on the user manager `disable` is a
        // statement about the next login, on the system manager about the next
        // boot — and about everyone, not only about the person pressing the key.
        let disable = verb("disable").unwrap();
        let session = disable.confirm_on(Manager::User).unwrap();
        let machine = disable.confirm_on(Manager::System).unwrap();
        assert_ne!(session, machine);
        assert!(session.contains("login"));
        assert!(machine.contains("boot"));
        // `enable` does not ask at all on the user manager — it is cheap and
        // the row shows the result — but on the machine it is a decision.
        let enable = verb("enable").unwrap();
        assert!(enable.confirm_on(Manager::User).is_none());
        assert!(enable.confirm_on(Manager::System).is_some());
        // Every prompt still names the unit, on either manager.
        for v in VERBS {
            for manager in [Manager::User, Manager::System] {
                if let Some(prompt) = v.confirm_on(manager) {
                    assert!(
                        prompt.contains("{unit}"),
                        "{} on the {} manager: the prompt must name the unit",
                        v.id,
                        manager.as_str()
                    );
                }
            }
        }
        // A runtime verb has one sentence for both, because it means the same
        // thing on both: this unit stops now.
        let stop = verb("stop").unwrap();
        assert_eq!(stop.confirm_on(Manager::User), stop.confirm_on(Manager::System));
    }

    #[test]
    fn the_protection_list_answers_to_what_ends_a_unit_now_and_to_what_ends_it_next_time() {
        let protected: Vec<&str> = VERBS.iter().filter(|v| v.protected()).map(|v| v.id).collect();
        assert_eq!(
            protected,
            vec![
                "stop",
                "restart",
                "reload-or-restart",
                "disable",
                "disable-now",
                "mask",
                "kill",
                "freeze",
            ]
        );
        // The one that is easy to miss: plain `disable` takes nothing down
        // now, so it is not disruptive — and it still must not be aimed at
        // `dbus.socket`, because the session that notices is the next one.
        assert!(!verb("disable").unwrap().disruptive);
        assert!(verb("disable").unwrap().protected());
        // Enabling and unmasking are the reversals; nothing is protected from
        // being switched back on.
        assert!(!verb("enable").unwrap().protected());
        assert!(!verb("unmask").unwrap().protected());
    }
}
