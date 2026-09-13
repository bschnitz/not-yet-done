//! Editing a unit — where the buffer goes, what checks it before it lands, and
//! what is left to decide afterwards.
//!
//! # Where a change lands
//!
//! Three files could hold the same change, and they are not equivalent:
//!
//! * `~/.config/systemd/user/<unit>.d/override.conf` — a **drop-in**, layered
//!   on top of whatever the distribution ships. The vendor unit keeps being
//!   updated by the package manager, and `systemctl --user revert <unit>`
//!   takes the change back out in one word. This is the default, and it is
//!   what `systemctl edit` does.
//! * `~/.config/systemd/user/<unit>` — the **whole file**, copied into the
//!   user's own tree on first edit. It shadows the vendor unit completely,
//!   which also means it stops tracking it.
//! * The vendor file itself — never. Nothing here writes outside
//!   `~/.config/systemd/user`; the system manager is a privilege question
//!   (phase 7), not an editing one.
//!
//! The one case where the default bends: if the unit already loads from a file
//! in the user's own tree, [`resolve`] edits **that file** rather than laying a
//! drop-in over it. There is nothing underneath to preserve, and the
//! alternative splits one unit's configuration across two files for no gain.
//!
//! # The check runs before the file exists
//!
//! `systemd-analyze verify` is the gate, and it runs against a **staging
//! directory** rather than against the real one: the pending text is written
//! to `<tmp>/<unit>.d/override.conf`, and `SYSTEMD_UNIT_PATH=<tmp>:` puts that
//! directory ahead of the defaults. A drop-in of the same name in a
//! higher-priority directory shadows the real one while every *other* drop-in
//! and the vendor fragment still merge — so what is checked is the effective
//! unit, and a file that does not survive the check never reaches
//! `~/.config`.
//!
//! # What "verify" actually says
//!
//! Less than its name suggests, and the difference matters:
//!
//! | written | `systemd-analyze verify` says | exit |
//! |---|---|---|
//! | `NoSuchKey=1` | `Unknown key … ignoring.` | 0 |
//! | `Restart=nonsense` | `Failed to parse … ignoring` | 0 |
//! | a line outside any section | `Assignment outside of section. Ignoring.` | 0 |
//! | `ExecStart=/does/not/exist` | `Command … is not executable` | **1** |
//!
//! So the exit code alone is not a gate — it only separates "systemd would
//! load this anyway" from "systemd refuses the unit". The usable signal is
//! that a clean file produces **no output at all**: every line is a finding,
//! and [`Findings::fatal`] decides whether it blocks the write or is merely
//! reported.

use std::path::{Path, PathBuf};

use not_yet_done_content::{ActionOption, ContentError, InputSpec, NodeAction, Result};

use crate::bus::{Bus, UNIT_IFACE};
use crate::config::Manager;

/// Action id: edit the drop-in (or the user's own fragment — see [`resolve`]).
pub const EDIT: &str = "edit";
/// Action id: edit the whole unit file, copying the vendor one on first use.
pub const EDIT_FULL: &str = "edit-full";
/// Action id: the follow-up question after a write — see [`apply_options`].
pub const APPLY: &str = "apply";
/// The `apply` choice that changes nothing about the running unit.
pub const NOTHING: &str = "nothing";

/// How many backups of one unit's file are kept before the oldest goes.
pub const BACKUPS_KEPT: usize = 10;

/// The line that ends the buffer's header. Everything above it is stripped
/// before the file is written; everything below it is the file.
const CUT: &str = "# ---------------------------------------------------------------- 8< ---";

/// The editing actions a level offers.
///
/// Nothing at all against the **system** manager: writing there needs
/// privileges this adapter does not ask for yet (phase 7), and an action that
/// can only fail is worse than an action that is not offered.
pub fn actions_for(type_id: &str, manager: Manager) -> Vec<NodeAction> {
    if manager != Manager::User {
        return Vec::new();
    }
    if !matches!(
        type_id,
        "systemd:service" | "systemd:timer" | "systemd:unitfile"
    ) {
        return Vec::new();
    }
    vec![
        NodeAction::new(EDIT, "Edit drop-in", InputSpec::Editor),
        NodeAction::new(EDIT_FULL, "Edit whole unit file", InputSpec::Editor),
        NodeAction::new(APPLY, "Apply to the running unit", InputSpec::Picker),
    ]
}

/// Which file an edit writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// A fragment layered on the vendor unit.
    DropIn,
    /// The unit file itself, in the user's own tree.
    Full,
}

/// The file an edit action writes, resolved against what is actually on disk.
#[derive(Clone, Debug)]
pub struct Target {
    pub unit: String,
    pub layer: Layer,
    /// Where the buffer is written on save. May not exist yet.
    pub path: PathBuf,
    /// The file the unit loads from today, when that is a *different* file —
    /// what a full copy is seeded from, and what a drop-in layers over.
    pub vendor: Option<PathBuf>,
}

/// `~/.config/systemd/user` — the only tree this phase writes to.
pub fn user_unit_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("systemd")
        .join("user")
}

/// Work out which file this edit means.
pub async fn resolve(bus: &Bus, unit: &str, layer: Layer) -> Result<Target> {
    let root = user_unit_dir();
    let vendor = fragment_path(bus, unit).await;
    let owned = vendor.as_deref().is_some_and(|p| p.starts_with(&root));
    let (layer, path) = match layer {
        // The unit already loads from a file the user wrote. A drop-in on top
        // of it would split one unit's configuration across two files and
        // preserve nothing, because there is no vendor version underneath.
        Layer::DropIn if owned => (Layer::Full, vendor.clone().unwrap_or_default()),
        Layer::DropIn => (
            Layer::DropIn,
            root.join(format!("{unit}.d")).join("override.conf"),
        ),
        Layer::Full => (Layer::Full, root.join(unit)),
    };
    Ok(Target {
        unit: unit.to_string(),
        layer,
        vendor: vendor.filter(|v| *v != path),
        path,
    })
}

/// The file the manager currently loads this unit from, if any.
///
/// Two ways, because the levels can see two different kinds of unit: a loaded
/// one has an object on the bus that names its `FragmentPath`, and a unit the
/// manager has never touched has only a file on disk — which is precisely what
/// the unit-files level exists to show.
async fn fragment_path(bus: &Bus, unit: &str) -> Option<PathBuf> {
    if let Ok(object) = bus.unit_path(unit).await {
        let props = bus.properties(&object, UNIT_IFACE).await;
        let path = crate::control::prop_str(&props, "FragmentPath");
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    let files = bus.list_unit_files().await.ok()?;
    files
        .into_iter()
        .find(|f| f.path.rsplit('/').next() == Some(unit))
        .map(|f| PathBuf::from(f.path))
}

/// What the header tells the user about the unit as it stands.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub active: String,
    pub sub: String,
    pub unit_file: String,
}

impl State {
    /// The one-line summary for the buffer header. Empty when the manager has
    /// no object for this unit — a unit file that was never loaded has no
    /// state to report, and inventing one would be worse than saying nothing.
    fn describe(&self) -> String {
        match (self.active.as_str(), self.sub.as_str()) {
            ("", _) => "not loaded".into(),
            (active, "") => active.into(),
            (active, sub) => format!("{active} ({sub})"),
        }
    }
}

/// Read the three properties the header quotes.
pub async fn state(bus: &Bus, unit: &str) -> State {
    let Ok(object) = bus.unit_path(unit).await else {
        return State::default();
    };
    let props = bus.properties(&object, UNIT_IFACE).await;
    State {
        active: crate::control::prop_str(&props, "ActiveState"),
        sub: crate::control::prop_str(&props, "SubState"),
        unit_file: crate::control::prop_str(&props, "UnitFileState"),
    }
}

/// Whether the running unit is still using the configuration it started with.
///
/// The only case in which the follow-up question is worth asking: a unit that
/// is not up has nothing to restart, and asking anyway trains the user to
/// dismiss the prompt.
pub fn needs_applying(state: &State) -> bool {
    matches!(state.active.as_str(), "active" | "activating" | "reloading")
}

/// What the buffer starts out as: the target file if it exists, otherwise the
/// vendor file for a full copy, otherwise the empty section the unit's kind
/// calls for.
pub fn body(target: &Target) -> String {
    if let Ok(text) = std::fs::read_to_string(&target.path) {
        return text;
    }
    if target.layer == Layer::Full
        && let Some(vendor) = &target.vendor
        && let Ok(text) = std::fs::read_to_string(vendor)
    {
        return text;
    }
    match target.unit.rsplit_once('.').map(|(_, kind)| kind) {
        Some("timer") => "[Timer]\n".into(),
        Some("socket") => "[Socket]\n".into(),
        Some("path") => "[Path]\n".into(),
        Some("mount") => "[Mount]\n".into(),
        _ => "[Service]\n".into(),
    }
}

/// The file suffix that gets the editor to highlight the buffer.
///
/// The unit's own extension, not the target file's: a drop-in is named
/// `override.conf`, and `.conf` tells an editor nothing, while every editor
/// that knows systemd at all knows `.service` and `.timer`.
pub fn suffix(unit: &str) -> String {
    match unit.rsplit_once('.') {
        Some((_, kind)) if !kind.is_empty() => format!(".{kind}"),
        _ => ".service".into(),
    }
}

/// A token that changes whenever the target file does, so a save can tell
/// whether the file moved under the buffer.
pub fn version(target: &Target) -> String {
    use std::hash::{Hash, Hasher};
    match std::fs::read(&target.path) {
        Ok(bytes) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        }
        // A file that is not there yet is a state like any other, and one the
        // next save must be able to notice has changed.
        Err(_) => "absent".into(),
    }
}

/// Render the buffer: a header that is stripped again on save, then the file.
///
/// `notice` is what the last attempt had to say — the findings that rejected
/// it, or the news that the file changed underneath. Empty on the first open.
pub fn template(target: &Target, state: &State, body: &str, notice: &[String]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# {} — {}{}\n",
        target.unit,
        state.describe(),
        if state.unit_file.is_empty() {
            String::new()
        } else {
            format!(", {}", state.unit_file)
        }
    ));
    out.push_str(&format!("# Writing: {}\n#\n", target.path.display()));

    match target.layer {
        Layer::DropIn => {
            out.push_str(
                "# This is a drop-in: it is read *in addition to* the unit file, not\n\
                 # instead of it. A directive that takes a list — ExecStart=,\n\
                 # ExecStartPre=, Environment=, After= — therefore ADDS an entry, and\n\
                 # replacing one means clearing the list first:\n\
                 #\n\
                 #     [Service]\n\
                 #     ExecStart=\n\
                 #     ExecStart=/the/new/command\n\
                 #\n",
            );
            if let Some(vendor) = &target.vendor {
                out.push_str(&format!("# Layered over: {}\n#\n", vendor.display()));
            }
        }
        Layer::Full => {
            out.push_str(
                "# This is the whole unit file, in your own tree. It replaces the\n\
                 # packaged one outright, including any future update to it;\n\
                 # `systemctl --user revert` puts the packaged version back.\n#\n",
            );
            if let Some(vendor) = &target.vendor {
                out.push_str(&format!("# Copied from: {}\n#\n", vendor.display()));
            }
        }
    }

    if !notice.is_empty() {
        out.push_str("# NOT WRITTEN — systemd had something to say about it:\n#\n");
        for line in notice {
            out.push_str(&format!("#   {line}\n"));
        }
        out.push_str("#\n");
    }

    out.push_str("# Everything above the next line is stripped before saving.\n");
    out.push_str(CUT);
    out.push('\n');
    out.push_str(body);
    out
}

/// Drop the header again, leaving exactly what belongs in the file.
///
/// A buffer whose marker line is gone — the user deleted it — is taken as
/// content in full. The alternative, guessing which leading comments were ours,
/// would eventually eat a comment the user wrote.
pub fn strip_header(buffer: &str) -> &str {
    let mut offset = 0;
    for line in buffer.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end() == CUT {
            return &buffer[offset..];
        }
    }
    buffer
}

/// What `systemd-analyze verify` had to say.
#[derive(Clone, Debug, Default)]
pub struct Findings {
    /// systemd refuses to load the unit as written — the write must not happen.
    pub fatal: bool,
    /// Every line the check printed. A clean file prints none.
    pub lines: Vec<String>,
}

impl Findings {
    pub fn is_clean(&self) -> bool {
        self.lines.is_empty()
    }

    /// The one-line version for a notification, when the findings were
    /// warnings and the file was written anyway.
    pub fn summary(&self) -> String {
        match self.lines.len() {
            0 => String::new(),
            1 => self.lines[0].clone(),
            n => format!("{} (and {} more)", self.lines[0], n - 1),
        }
    }
}

/// Check the pending text as the unit systemd would actually load.
///
/// Never touches the real configuration: see the module header for the staging
/// directory and why a same-named drop-in there shadows the real one.
pub async fn verify(target: &Target, text: &str) -> Result<Findings> {
    let staging = tempfile::Builder::new()
        .prefix("nyd-systemd-verify-")
        .tempdir()
        .map_err(|e| ContentError::Other(format!("staging the check: {e}").into()))?;
    let staged = match target.layer {
        Layer::DropIn => staging
            .path()
            .join(format!("{}.d", target.unit))
            .join("override.conf"),
        Layer::Full => staging.path().join(&target.unit),
    };
    write_file(&staged, text)?;

    let output = tokio::process::Command::new("systemd-analyze")
        .arg("--user")
        .arg("verify")
        .arg(&target.unit)
        // Trailing colon: the staging directory goes *in front of* the normal
        // search path rather than replacing it, so the vendor fragment and
        // every other drop-in still merge in.
        .env(
            "SYSTEMD_UNIT_PATH",
            format!("{}:", staging.path().display()),
        )
        // systemd speaks the caller's language; this buffer is English.
        .env("LC_ALL", "C")
        .output()
        .await;

    let output = match output {
        Ok(o) => o,
        // A machine without systemd-analyze still has a systemd. Refusing to
        // write because the *checker* is missing helps nobody — but the user
        // should know the check did not happen.
        Err(e) => {
            return Ok(Findings {
                fatal: false,
                lines: vec![format!("systemd-analyze could not be run ({e}) — not checked")],
            });
        }
    };

    let staged_prefix = staged.display().to_string();
    let real = target.path.display().to_string();
    let mut lines = Vec::new();
    for stream in [&output.stdout, &output.stderr] {
        for line in String::from_utf8_lossy(stream).lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // The user never chose the staging path and should not have to
            // read it; say the name the file is about to have.
            lines.push(line.replace(&staged_prefix, &real));
        }
    }
    Ok(Findings {
        fatal: !output.status.success(),
        lines,
    })
}

/// Copy the target file aside before it is overwritten, and keep the last
/// [`BACKUPS_KEPT`].
///
/// Deliberately **outside** the systemd tree. A drop-in directory is read
/// wholesale — every `*.conf` in it is configuration — so a backup kept next to
/// the file is only safe for as long as nobody names one `.conf`, which is a
/// convention, not a guarantee. `~/.local/share` cannot be read by systemd at
/// all.
///
/// Returns `None` when there was nothing to back up, which is the normal case
/// for the first edit of a unit.
pub fn backup(target: &Target) -> Result<Option<PathBuf>> {
    if !target.path.exists() {
        return Ok(None);
    }
    let dir = backup_dir(&target.unit);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ContentError::Other(format!("creating {}: {e}", dir.display()).into()))?;
    let name = format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        target
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unit".into())
    );
    let to = dir.join(name);
    std::fs::copy(&target.path, &to)
        .map_err(|e| ContentError::Other(format!("backing up to {}: {e}", to.display()).into()))?;
    prune(&dir);
    Ok(Some(to))
}

/// Where one unit's backups live.
pub fn backup_dir(unit: &str) -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("not_yet_done")
        .join("systemd-backups")
        .join(unit)
}

/// Keep the newest [`BACKUPS_KEPT`]. The timestamp leads the file name, so
/// sorting by name is sorting by age.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    names.sort();
    let surplus = names.len().saturating_sub(BACKUPS_KEPT);
    for old in names.into_iter().take(surplus) {
        let _ = std::fs::remove_file(old);
    }
}

/// Write `text` to the target, creating the drop-in directory if this is the
/// first override of this unit.
pub fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ContentError::Other(format!("creating {}: {e}", parent.display()).into())
        })?;
    }
    let mut text = text.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    std::fs::write(path, text)
        .map_err(|e| ContentError::Other(format!("writing {}: {e}", path.display()).into()))
}

/// The three answers to "the unit is still running the old configuration".
///
/// The first two are [`crate::control`] verb ids on purpose: the follow-up does
/// not get a second implementation of restart, it runs the same verb the `a`
/// leader runs, protection list and all.
pub fn apply_options() -> Vec<ActionOption> {
    vec![
        ActionOption {
            label: "restart — stop it and bring it back on the new configuration".into(),
            value: "restart".into(),
        },
        ActionOption {
            label: "reload-or-restart — reload if the unit can, restart if it cannot".into(),
            value: "reload-or-restart".into(),
        },
        ActionOption {
            label: "nothing — leave it running; the change takes effect next time".into(),
            value: NOTHING.into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target {
            unit: "probe.service".into(),
            layer: Layer::DropIn,
            path: PathBuf::from("/tmp/probe.service.d/override.conf"),
            vendor: Some(PathBuf::from("/usr/lib/systemd/user/probe.service")),
        }
    }

    #[test]
    fn the_header_goes_out_the_way_it_came_in() {
        let body = "[Service]\nExecStart=\nExecStart=/bin/true\n";
        let buffer = template(&target(), &State::default(), body, &[]);
        assert!(buffer.contains("probe.service"), "header names the unit");
        assert!(
            buffer.contains("ExecStart=\n#     ExecStart="),
            "a drop-in buffer warns that lists append"
        );
        // What is written is the file, not the explanation of it.
        assert_eq!(strip_header(&buffer), body);
    }

    #[test]
    fn a_buffer_without_the_marker_is_content_in_full() {
        // The user deleted the header. Guessing which comment lines were ours
        // would eventually eat one of theirs.
        let text = "# my own note\n[Service]\nExecStart=/bin/true\n";
        assert_eq!(strip_header(text), text);
    }

    #[test]
    fn a_rejected_buffer_carries_the_reason_and_still_round_trips() {
        let body = "[Service]\nExecStart=/nope\n";
        let notice = vec!["probe.service: Command /nope is not executable".to_string()];
        let buffer = template(&target(), &State::default(), body, &notice);
        assert!(buffer.contains("not executable"), "the reason is in view");
        assert!(buffer.contains("NOT WRITTEN"), "and so is the consequence");
        // The next save must still find the file underneath the banner.
        assert_eq!(strip_header(&buffer), body);
    }

    #[test]
    fn only_a_running_unit_is_asked_about() {
        for (active, expected) in [
            ("active", true),
            ("activating", true),
            ("reloading", true),
            ("inactive", false),
            ("failed", false),
            ("", false),
        ] {
            let state = State {
                active: active.into(),
                ..Default::default()
            };
            assert_eq!(needs_applying(&state), expected, "ActiveState={active}");
        }
    }

    #[test]
    fn the_editor_is_told_the_units_kind_not_the_files() {
        // The drop-in is called override.conf; `.conf` highlights nothing.
        assert_eq!(suffix("backup.timer"), ".timer");
        assert_eq!(suffix("app.service"), ".service");
        assert_eq!(suffix("odd"), ".service");
    }

    #[test]
    fn a_fresh_drop_in_opens_on_the_section_its_kind_needs() {
        let mut t = target();
        assert_eq!(body(&t), "[Service]\n");
        t.unit = "backup.timer".into();
        assert_eq!(body(&t), "[Timer]\n");
    }

    #[test]
    fn the_apply_choices_are_verbs_this_adapter_already_has() {
        for option in apply_options() {
            if option.value == NOTHING {
                continue;
            }
            assert!(
                crate::control::verb(&option.value).is_some(),
                "{} must be a real verb, not a second implementation",
                option.value
            );
        }
    }

    #[test]
    fn backups_live_where_systemd_cannot_read_them() {
        let dir = backup_dir("probe.service");
        assert!(
            !dir.starts_with(user_unit_dir()),
            "a backup inside the unit tree is one rename away from being configuration"
        );
    }

    #[test]
    fn the_system_manager_is_not_offered_an_editor() {
        assert!(actions_for("systemd:service", Manager::System).is_empty());
        assert_eq!(actions_for("systemd:service", Manager::User).len(), 3);
        // Not a unit: nothing to edit.
        assert!(actions_for("systemd:property", Manager::User).is_empty());
        assert!(actions_for("systemd:manager", Manager::User).is_empty());
    }
}
