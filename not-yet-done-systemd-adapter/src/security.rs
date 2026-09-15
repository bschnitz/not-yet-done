//! What `systemd-analyze security` says about a unit, as a level — and the
//! drop-in that answers one row of it.
//!
//! # Why a level and not a score
//!
//! `systemd-analyze security foo.service` prints a table of about eighty
//! checks and one number at the bottom, and the number is the part everyone
//! quotes. It is also the part that says least: on this machine every plain
//! user service scores within a tenth of every other, because a service that
//! never had a privilege cannot drop one. What separates two units is *which*
//! checks they pass, which is a table — so it is a level, with the checks as
//! rows, and the number is one column on the level above.
//!
//! # The shape of the JSON, measured
//!
//! `--json=short <unit>` is an array of `{set, name, json_field, description,
//! exposure}`, and two of those fields are not what their names suggest.
//!
//! `set` is **tri-state**. `true` is a check the unit passes, `false` one it
//! does not, and `null` is "this setting has no effect on a per-user service"
//! — one check here, `RemoveIPC=`. A reader that treats `set` as a boolean
//! reports a check as failed that cannot be passed at all.
//!
//! `exposure` is a **string** (`"0.2"`), and it is `null` exactly when `set` is
//! `true`. So an empty exposure cell is not missing data: it is the check
//! passing.
//!
//! What the per-unit JSON does **not** carry is the overall score. The
//! "→ Overall exposure level for X: 9.4 UNSAFE" line exists only in the table
//! output — which is why the column on the Services level comes from
//! [`overview`] instead, the no-unit form, whose rows are
//! `{unit, exposure, predicate, happy}` and which answers for every loaded
//! service in one call (53 ms for 27 units here, against 8 ms for a single
//! unit — so a column on the Services level is one shared call, and a column
//! on the Unit files level, with its several hundred rows, is not a thing that
//! can exist).
//!
//! Non-service units are refused by name ("Unit pipewire.socket is not a
//! service unit, refusing."), and that sentence is passed through rather than
//! translated. Units the manager has never **loaded** do work, which is why
//! this level hangs under the unit-files level too.
//!
//! # `name` is a label, not a directive
//!
//! The tempting shortcut — write the check's `name` into a drop-in and the
//! check goes away — is wrong, and wrong quietly. Many names are display
//! shorthand for a *group*: `CapabilityBoundingSet=~CAP_MAC_*`,
//! `CapabilityBoundingSet=~CAP_SET(UID|GID|PCAP)`, and — a literal ellipsis —
//! `RestrictAddressFamilies=~…`. Written to a file those are not configuration
//! at all; systemd drops them, mostly without a word in the journal, and the
//! check stays exactly where it was.
//!
//! Measured, on a throwaway unit: a drop-in built from the names alone left 23
//! checks untouched, and every capability that *did* flip had been dropped as
//! a side effect of some other directive (`ProtectClock=yes` removes
//! `CAP_SYS_TIME`, `PrivateDevices=yes` removes `CAP_MKNOD`) — not one of the
//! explicit lines took. With the groups expanded into real capability names,
//! the same unit went from 9.4 UNSAFE to 0.2 SAFE. So [`FIXES`] is a curated
//! table, every entry of which was applied on its own to a probe unit and
//! checked to flip its own check.
//!
//! # The two checks that cannot both pass
//!
//! Two entries in the table are honest about conflicting with a third:
//!
//! * `DeviceAllow=` needs an empty device list, but `ProtectClock=yes`
//!   *implies* `DeviceAllow=char-rtc r`. Either check can pass; not both, in
//!   any order.
//! * `RestrictAddressFamilies=~AF_UNIX` denies AF_UNIX, while the "all other
//!   address families" check wants an allow-list. `RestrictAddressFamilies=none`
//!   satisfies both, and is what the table offers — a unit that needs a socket
//!   will want an allow-list of its own there instead.
//!
//! Nothing here tries to resolve that: the fix is offered per row, in an
//! editor, for a person to read.

use std::collections::HashMap;

use not_yet_done_content::{ContentError, InputSpec, NodeAction, Result};
use serde_json::Value;

use crate::config::Manager;
use crate::model::SecurityRow;

/// Action id: open a drop-in prefilled with the directive that settles this
/// check.
pub const HARDEN: &str = "harden";

/// A check the unit passes.
pub const OK: &str = "ok";
/// A check the unit does not pass — the rows this level exists for.
pub const EXPOSED: &str = "exposed";
/// A check that cannot apply here at all (`set: null`).
pub const NO_EFFECT: &str = "no-effect";

/// What settles one check.
#[derive(Clone, Copy, Debug)]
pub enum Fix {
    /// The directives to add. Every one of these was written to a probe unit
    /// on its own and seen to flip its own check — see the module docs for why
    /// that had to be measured rather than derived from the check's name.
    Directives(&'static [&'static str]),
    /// No single directive settles this one, and why. The drop-in then carries
    /// the sentence as a comment instead of a line that would not work.
    Alternatives(&'static str),
}

/// The security actions a level offers.
///
/// Only on the security level itself: hardening is an answer to one row, and
/// the unit levels already have their own editing actions. And only against
/// the **user** manager, for the same reason the editing actions are — the
/// drop-in it opens is written to `~/.config/systemd/user`, and the system
/// manager is a privilege question rather than an editing one.
pub fn actions_for(type_id: &str, manager: Manager) -> Vec<NodeAction> {
    if type_id != "systemd:security" || manager != Manager::User {
        return Vec::new();
    }
    vec![NodeAction::new(
        HARDEN,
        "Harden this check",
        InputSpec::Editor,
    )]
}

/// Every check of one unit, in the order `systemd-analyze` reports them.
///
/// The order is systemd's own — roughly worst first — and is worth keeping:
/// it is the one ordering in the output that carries an opinion, and a user
/// who wants another sorts the column.
pub async fn checks(manager: Manager, unit: &str) -> Result<Vec<SecurityRow>> {
    let out = run(&[flag(manager), "security", "--json=short", unit]).await?;
    let Value::Array(items) = serde_json::from_str(&out).map_err(|e| {
        ContentError::Other(format!("systemd-analyze said something unexpected: {e}").into())
    })?
    else {
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .filter_map(|item| SecurityRow::parse(unit, item))
        .collect())
}

/// The overall exposure of every **loaded** service, as the raw string
/// systemd prints.
///
/// One call for the whole Services level. Failure is an empty map rather than
/// an error: a missing `systemd-analyze` should leave one column blank, not
/// take the level down.
pub async fn overview(manager: Manager) -> HashMap<String, String> {
    let Ok(out) = run(&[flag(manager), "security", "--json=short"]).await else {
        return HashMap::new();
    };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&out) else {
        return HashMap::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let unit = item.get("unit")?.as_str()?.to_string();
            let exposure = item.get("exposure")?.as_str()?.to_string();
            Some((unit, exposure))
        })
        .collect()
}

/// What to add to a drop-in to settle this check, header comment included.
///
/// The section heading is repeated unconditionally rather than merged into an
/// existing one: a unit file may hold `[Service]` more than once and systemd
/// folds them together (measured), while guessing where an existing section
/// ends is how a directive lands under `[Unit]`.
pub fn drop_in(row: &SecurityRow) -> String {
    let mut out = format!("\n# {} — {}\n", row.check, row.description);
    match fix(&row.id) {
        Some(Fix::Directives(lines)) => {
            out.push_str("[Service]\n");
            for line in lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        Some(Fix::Alternatives(why)) => {
            out.push_str(&format!(
                "# No single directive settles this one: {why}.\n\
                 # Nothing is filled in below on purpose — the choice is yours.\n"
            ));
        }
        None => {
            out.push_str(
                "# This adapter has no fix on file for this check. Its name above is a\n\
                 # display label, not necessarily a directive you can write verbatim.\n",
            );
        }
    }
    out
}

/// One check of one unit, by the id the level handed out.
///
/// Re-read rather than carried along: a row that has been on screen since
/// before an edit would otherwise offer to fix a check that already passes.
pub async fn check(manager: Manager, unit: &str, json_field: &str) -> Result<SecurityRow> {
    checks(manager, unit)
        .await?
        .into_iter()
        .find(|r| r.id == json_field)
        .ok_or_else(|| ContentError::NotFound(format!("{unit} has no security check {json_field}")))
}

/// Split a `security:<unit>:<json_field>` id into its two halves.
///
/// From the right: a `json_field` never holds a colon, a unit name may.
pub fn parse_id(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(crate::SECURITY_PREFIX)?.rsplit_once(':')
}

/// The unit a security row belongs to.
pub fn unit_of(id: &str) -> Option<&str> {
    parse_id(id).map(|(unit, _)| unit)
}

/// The fix for one check, by its `json_field`.
pub fn fix(json_field: &str) -> Option<Fix> {
    FIXES
        .iter()
        .find(|(field, _)| *field == json_field)
        .map(|(_, fix)| *fix)
}

/// The directives a fix would write, as one cell.
pub fn fix_cell(json_field: &str) -> String {
    match fix(json_field) {
        Some(Fix::Directives(lines)) => lines.join(" "),
        _ => String::new(),
    }
}

fn flag(manager: Manager) -> &'static str {
    match manager {
        Manager::User => "--user",
        Manager::System => "--system",
    }
}

/// Run systemd-analyze and hand back its stdout.
///
/// A refusal is reported in systemd's own words — "Unit foo.socket is not a
/// service unit, refusing." is a sentence the user can act on, an exit code is
/// not.
async fn run(args: &[&str]) -> Result<String> {
    let out = tokio::process::Command::new("systemd-analyze")
        .args(args)
        .output()
        .await
        .map_err(|e| ContentError::Other(format!("could not run systemd-analyze: {e}").into()))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        let said = said.trim();
        return Err(ContentError::Other(
            if said.is_empty() {
                format!("systemd-analyze failed ({})", out.status)
            } else {
                said.to_string()
            }
            .into(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The curated fix table, keyed by the check's `json_field`.
///
/// `json_field` and not `name`, because `name` is a display label that changes
/// shape between groups and singles, while the field is systemd's own stable
/// identifier for the check — and it is what a query filters on.
///
/// Every `Directives` entry here was applied alone to a probe unit and seen to
/// flip its own check; the whole table applied at once took that unit to 0.2
/// SAFE, with only the two documented conflicts left standing.
#[rustfmt::skip]
pub static FIXES: &[(&str, Fix)] = &[
    ("KeyringMode", Fix::Directives(&["KeyringMode=private"])),
    ("RootDirectoryOrRootImage", Fix::Alternatives(
        "RootDirectory= and RootImage= are alternatives, and which one fits depends on whether there is a directory tree or an image to run in",
    )),
    ("UserOrDynamicUser", Fix::Alternatives(
        "User= and DynamicUser= are alternatives, and a user service already runs as the calling user",
    )),
    ("CapabilityBoundingSet_CAP_SYS_TIME", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_TIME"])),
    ("NoNewPrivileges", Fix::Directives(&["NoNewPrivileges=yes"])),
    ("AmbientCapabilities", Fix::Directives(&["AmbientCapabilities="])),
    ("PrivateDevices", Fix::Directives(&["PrivateDevices=yes"])),
    ("ProtectClock", Fix::Directives(&["ProtectClock=yes"])),
    ("CapabilityBoundingSet_CAP_SYS_PACCT", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_PACCT"])),
    ("CapabilityBoundingSet_CAP_KILL", Fix::Directives(&["CapabilityBoundingSet=~CAP_KILL"])),
    ("ProtectKernelLogs", Fix::Directives(&["ProtectKernelLogs=yes"])),
    ("CapabilityBoundingSet_CAP_WAKE_ALARM", Fix::Directives(&["CapabilityBoundingSet=~CAP_WAKE_ALARM"])),
    ("CapabilityBoundingSet_CAP_DAC_FOWNER_IPC_OWNER", Fix::Directives(&["CapabilityBoundingSet=~CAP_DAC_OVERRIDE CAP_DAC_READ_SEARCH CAP_FOWNER CAP_IPC_OWNER"])),
    ("ProtectControlGroups", Fix::Directives(&["ProtectControlGroups=yes"])),
    ("CapabilityBoundingSet_CAP_LINUX_IMMUTABLE", Fix::Directives(&["CapabilityBoundingSet=~CAP_LINUX_IMMUTABLE"])),
    ("CapabilityBoundingSet_CAP_IPC_LOCK", Fix::Directives(&["CapabilityBoundingSet=~CAP_IPC_LOCK"])),
    ("ProtectKernelModules", Fix::Directives(&["ProtectKernelModules=yes"])),
    ("CapabilityBoundingSet_CAP_SYS_MODULE", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_MODULE"])),
    ("CapabilityBoundingSet_CAP_BPF", Fix::Directives(&["CapabilityBoundingSet=~CAP_BPF"])),
    ("CapabilityBoundingSet_CAP_SYS_TTY_CONFIG", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_TTY_CONFIG"])),
    ("CapabilityBoundingSet_CAP_SYS_BOOT", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_BOOT"])),
    ("CapabilityBoundingSet_CAP_SYS_CHROOT", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_CHROOT"])),
    ("PrivateMounts", Fix::Directives(&["PrivateMounts=yes"])),
    ("SystemCallArchitectures", Fix::Directives(&["SystemCallArchitectures=native"])),
    ("CapabilityBoundingSet_CAP_BLOCK_SUSPEND", Fix::Directives(&["CapabilityBoundingSet=~CAP_BLOCK_SUSPEND"])),
    ("MemoryDenyWriteExecute", Fix::Directives(&["MemoryDenyWriteExecute=yes"])),
    ("RestrictNamespaces_user", Fix::Directives(&["RestrictNamespaces=~user"])),
    ("RestrictNamespaces_pid", Fix::Directives(&["RestrictNamespaces=~pid"])),
    ("RestrictNamespaces_net", Fix::Directives(&["RestrictNamespaces=~net"])),
    ("RestrictNamespaces_uts", Fix::Directives(&["RestrictNamespaces=~uts"])),
    ("RestrictNamespaces_mnt", Fix::Directives(&["RestrictNamespaces=~mnt"])),
    ("CapabilityBoundingSet_CAP_LEASE", Fix::Directives(&["CapabilityBoundingSet=~CAP_LEASE"])),
    ("CapabilityBoundingSet_CAP_MKNOD", Fix::Directives(&["CapabilityBoundingSet=~CAP_MKNOD"])),
    ("RestrictNamespaces_cgroup", Fix::Directives(&["RestrictNamespaces=~cgroup"])),
    ("RestrictSUIDSGID", Fix::Directives(&["RestrictSUIDSGID=yes"])),
    ("RestrictNamespaces_ipc", Fix::Directives(&["RestrictNamespaces=~ipc"])),
    ("ProtectHostname", Fix::Directives(&["ProtectHostname=yes"])),
    ("CapabilityBoundingSet_CAP_CHOWN_FSETID_SETFCAP", Fix::Directives(&["CapabilityBoundingSet=~CAP_CHOWN CAP_FSETID CAP_SETFCAP"])),
    ("CapabilityBoundingSet_CAP_SET_UID_GID_PCAP", Fix::Directives(&["CapabilityBoundingSet=~CAP_SETUID CAP_SETGID CAP_SETPCAP"])),
    ("LockPersonality", Fix::Directives(&["LockPersonality=yes"])),
    ("ProtectKernelTunables", Fix::Directives(&["ProtectKernelTunables=yes"])),
    ("RestrictAddressFamilies_AF_PACKET", Fix::Directives(&["RestrictAddressFamilies=~AF_PACKET"])),
    ("RestrictAddressFamilies_AF_NETLINK", Fix::Directives(&["RestrictAddressFamilies=~AF_NETLINK"])),
    ("RestrictAddressFamilies_AF_UNIX", Fix::Directives(&["RestrictAddressFamilies=~AF_UNIX"])),
    ("RestrictAddressFamilies_OTHER", Fix::Directives(&["RestrictAddressFamilies=none"])),
    ("RestrictAddressFamilies_AF_INET_INET6", Fix::Directives(&["RestrictAddressFamilies=~AF_INET AF_INET6"])),
    ("CapabilityBoundingSet_CAP_MAC", Fix::Directives(&["CapabilityBoundingSet=~CAP_MAC_ADMIN CAP_MAC_OVERRIDE"])),
    ("RestrictRealtime", Fix::Directives(&["RestrictRealtime=yes"])),
    ("CapabilityBoundingSet_CAP_SYS_RAWIO", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_RAWIO"])),
    ("CapabilityBoundingSet_CAP_SYS_PTRACE", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_PTRACE"])),
    ("CapabilityBoundingSet_CAP_SYS_NICE_RESOURCE", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_NICE CAP_SYS_RESOURCE"])),
    ("SupplementaryGroups", Fix::Directives(&["SupplementaryGroups="])),
    ("DeviceAllow", Fix::Directives(&["DeviceAllow=", "DevicePolicy=closed"])),
    ("CapabilityBoundingSet_CAP_NET_ADMIN", Fix::Directives(&["CapabilityBoundingSet=~CAP_NET_ADMIN"])),
    ("ProtectSystem", Fix::Directives(&["ProtectSystem=strict"])),
    ("ProtectProc", Fix::Directives(&["ProtectProc=invisible"])),
    ("ProcSubset", Fix::Directives(&["ProcSubset=pid"])),
    ("ProtectHome", Fix::Directives(&["ProtectHome=yes"])),
    ("CapabilityBoundingSet_CAP_NET_BIND_SERVICE_BROADCAST_RAW)", Fix::Directives(&["CapabilityBoundingSet=~CAP_NET_BIND_SERVICE CAP_NET_BROADCAST CAP_NET_RAW"])),
    ("CapabilityBoundingSet_CAP_AUDIT", Fix::Directives(&["CapabilityBoundingSet=~CAP_AUDIT_CONTROL CAP_AUDIT_READ CAP_AUDIT_WRITE"])),
    ("CapabilityBoundingSet_CAP_SYS_ADMIN", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYS_ADMIN"])),
    ("PrivateNetwork", Fix::Directives(&["PrivateNetwork=yes"])),
    ("PrivateUsers", Fix::Directives(&["PrivateUsers=yes"])),
    ("PrivateTmp", Fix::Directives(&["PrivateTmp=yes"])),
    ("CapabilityBoundingSet_CAP_SYSLOG", Fix::Directives(&["CapabilityBoundingSet=~CAP_SYSLOG"])),
    ("Delegate", Fix::Directives(&["Delegate=no"])),
    ("SystemCallFilter_clock", Fix::Directives(&["SystemCallFilter=~@clock"])),
    ("SystemCallFilter_cpu_emulation", Fix::Directives(&["SystemCallFilter=~@cpu-emulation"])),
    ("SystemCallFilter_debug", Fix::Directives(&["SystemCallFilter=~@debug"])),
    ("SystemCallFilter_module", Fix::Directives(&["SystemCallFilter=~@module"])),
    ("SystemCallFilter_mount", Fix::Directives(&["SystemCallFilter=~@mount"])),
    ("SystemCallFilter_obsolete", Fix::Directives(&["SystemCallFilter=~@obsolete"])),
    ("SystemCallFilter_privileged", Fix::Directives(&["SystemCallFilter=~@privileged"])),
    ("SystemCallFilter_raw_io", Fix::Directives(&["SystemCallFilter=~@raw-io"])),
    ("SystemCallFilter_reboot", Fix::Directives(&["SystemCallFilter=~@reboot"])),
    ("SystemCallFilter_resources", Fix::Directives(&["SystemCallFilter=~@resources"])),
    ("SystemCallFilter_swap", Fix::Directives(&["SystemCallFilter=~@swap"])),
    ("IPAddressDeny", Fix::Directives(&["IPAddressDeny=any"])),
    ("NotifyAccess", Fix::Directives(&["NotifyAccess=none"])),
    ("RemoveIPC", Fix::Alternatives(
        "RemoveIPC= has no effect on a user service at all",
    )),
    ("UMask", Fix::Directives(&["UMask=0077"])),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug that made this table necessary: a check's `name` is a display
    /// label, and the group forms in it (`CAP_MAC_*`, `CAP_SET(UID|GID|PCAP)`,
    /// a bare `…`) are not configuration. Nothing the table would write may
    /// carry one of those shapes.
    #[test]
    fn no_fix_is_a_display_label_in_disguise() {
        for (field, fix) in FIXES {
            let Fix::Directives(lines) = fix else {
                continue;
            };
            for line in *lines {
                assert!(
                    !line.contains('*')
                        && !line.contains('(')
                        && !line.contains('|')
                        && !line.contains('…'),
                    "{field}: {line} is a display label, not a directive"
                );
                assert!(line.contains('='), "{field}: {line} is not an assignment");
            }
        }
    }

    #[test]
    fn every_check_is_keyed_once() {
        let mut seen: Vec<&str> = FIXES.iter().map(|(f, _)| *f).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(before, seen.len(), "a json_field is in the table twice");
        assert_eq!(
            before, 81,
            "the table should cover every check this systemd has"
        );
    }

    #[test]
    fn a_fix_with_no_single_directive_writes_comments_only() {
        let row = SecurityRow {
            unit: "probe.service".into(),
            id: "RootDirectoryOrRootImage".into(),
            check: "RootDirectory=/RootImage=".into(),
            description: "Service runs within the host's root directory".into(),
            status: EXPOSED.into(),
            exposure: Some(0.1),
            fix: String::new(),
        };
        let text = drop_in(&row);
        assert!(!text.contains("[Service]"), "{text}");
        assert!(text.contains("alternatives"), "{text}");
    }

    #[test]
    fn a_directive_fix_writes_a_section_and_the_lines() {
        let row = SecurityRow {
            unit: "probe.service".into(),
            id: "NoNewPrivileges".into(),
            check: "NoNewPrivileges=".into(),
            description: "Service processes may acquire new privileges".into(),
            status: EXPOSED.into(),
            exposure: Some(0.2),
            fix: fix_cell("NoNewPrivileges"),
        };
        let text = drop_in(&row);
        assert!(text.contains("[Service]\nNoNewPrivileges=yes\n"), "{text}");
    }

    /// The expansion that was the whole point: the group check must write real
    /// capability names, not the shorthand the label shows.
    #[test]
    fn a_group_check_expands_into_real_capabilities() {
        let cell = fix_cell("CapabilityBoundingSet_CAP_SET_UID_GID_PCAP");
        assert_eq!(
            cell,
            "CapabilityBoundingSet=~CAP_SETUID CAP_SETGID CAP_SETPCAP"
        );
    }
}
