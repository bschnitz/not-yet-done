//! The D-Bus side: one connection to a systemd manager, and the handful of
//! calls the read-only levels are built from.
//!
//! Everything here is *transport*. No node, no column and no query reaches this
//! module — [`crate::model`] turns what comes back into rows, so the shape of
//! the tab and the shape of the bus stay independent of each other.
//!
//! # Why a connection is made lazily
//!
//! A tab exists before anyone looks at it, and a manager that is not running
//! (a `--user` manager in a session without one) must not turn adapter
//! construction into an error. The connection is therefore opened on first use
//! and then kept: a failure surfaces where it can be shown — on the load that
//! wanted it — rather than at startup where nothing is on screen yet.
//!
//! # Deadlines
//!
//! Every call goes through [`Bus::deadline`], so a manager that stops answering
//! fails the load after `timeout_secs` instead of hanging the pane forever.
//! Local D-Bus traffic is a matter of milliseconds; a deadline that trips is
//! evidence of something wrong, not of a slow link.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use not_yet_done_content::{ContentError, Result};
use tokio::sync::OnceCell;
use zbus_systemd::systemd1::ManagerProxy;
use zbus_systemd::zbus::{self, fdo::PropertiesProxy, names::InterfaceName};
use zbus_systemd::zvariant::{OwnedObjectPath, OwnedValue};

use crate::config::Manager;

/// The D-Bus service every call here addresses.
const SERVICE: &str = "org.freedesktop.systemd1";

/// The interface a unit's generic properties live on.
pub const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";
/// The interface a `.service` unit's own properties live on.
pub const SERVICE_IFACE: &str = "org.freedesktop.systemd1.Service";
/// The interface a `.timer` unit's own properties live on.
pub const TIMER_IFACE: &str = "org.freedesktop.systemd1.Timer";

/// One interface's properties, as the bus returns them: the name of each
/// property to its still-typed value. Named because it travels between the
/// transport and the row builders, and a bare `HashMap<String, OwnedValue>` in
/// three signatures says nothing about what is in it.
pub type Props = HashMap<String, OwnedValue>;

/// How many property reads are in flight at once.
///
/// A level is a couple of hundred units at most and each read is a local
/// round trip, so the bound is not about protecting the manager — it keeps one
/// load from opening an unbounded number of concurrent futures on a machine
/// whose unit count we do not control.
pub const MAX_INFLIGHT: usize = 32;

/// One row of `ListUnits`, named. The bus returns a ten-tuple; carrying it
/// around positionally is how the wrong field ends up in a column.
#[derive(Clone, Debug)]
pub struct UnitEntry {
    /// Unit name including the suffix, e.g. `backup-photos.service`.
    pub name: String,
    pub description: String,
    /// `loaded` / `not-found` / `error` / `masked`.
    pub load_state: String,
    /// `active` / `inactive` / `failed` / `activating` / `deactivating`.
    pub active_state: String,
    /// The unit-type-specific sub state, e.g. `running`, `exited`, `dead`.
    pub sub_state: String,
    /// The unit's object path — where its properties are read from.
    pub path: OwnedObjectPath,
}

/// One row of `ListUnitFiles`: a unit file on disk, whether or not it is loaded.
#[derive(Clone, Debug)]
pub struct UnitFileEntry {
    /// Absolute path of the unit file.
    pub path: String,
    /// `enabled` / `disabled` / `static` / `masked` / `generated` / …
    pub state: String,
}

/// A lazily-connected handle on one systemd manager.
pub struct Bus {
    manager: Manager,
    timeout: Option<Duration>,
    conn: OnceCell<zbus::Connection>,
    /// Latches once `Subscribe` has been sent — see [`Bus::enable_signals`].
    subscribed: OnceCell<()>,
}

impl Bus {
    pub fn new(manager: Manager, timeout: Option<Duration>) -> Self {
        Self {
            manager,
            timeout,
            conn: OnceCell::new(),
            subscribed: OnceCell::new(),
        }
    }

    /// Which manager this handle talks to.
    pub fn manager(&self) -> Manager {
        self.manager
    }

    /// Run `fut` under the configured deadline, reporting what timed out.
    ///
    /// `what` names the call, because "timed out" on its own tells the user
    /// nothing they can act on.
    async fn deadline<T, E: std::fmt::Display>(
        &self,
        what: &str,
        fut: impl Future<Output = std::result::Result<T, E>>,
    ) -> Result<T> {
        let outcome = match self.timeout {
            None => fut.await,
            Some(limit) => tokio::time::timeout(limit, fut)
                .await
                .map_err(|_| ContentError::Other(format!("{what} timed out").into()))?,
        };
        outcome.map_err(|e| ContentError::Other(format!("{what}: {e}").into()))
    }

    /// The connection, opening it on first use.
    ///
    /// The user manager lives on the session bus and the system manager on the
    /// system bus — that, and nothing else, is what `manager:` selects.
    async fn connection(&self) -> Result<&zbus::Connection> {
        self.conn
            .get_or_try_init(|| async {
                let opening = async {
                    match self.manager {
                        Manager::User => zbus::Connection::session().await,
                        Manager::System => zbus::Connection::system().await,
                    }
                };
                self.deadline(
                    &format!("connecting to the {} manager", self.manager.as_str()),
                    opening,
                )
                .await
            })
            .await
    }

    async fn manager_proxy(&self) -> Result<ManagerProxy<'_>> {
        let conn = self.connection().await?;
        self.deadline("opening the manager proxy", ManagerProxy::new(conn))
            .await
    }

    /// Every unit the manager currently has loaded.
    ///
    /// "Loaded" is the honest scope for the Services and Timers levels: a unit
    /// file that has never been started has no runtime state to show. The Unit
    /// files level is the other half of that picture.
    pub async fn list_units(&self) -> Result<Vec<UnitEntry>> {
        let proxy = self.manager_proxy().await?;
        let raw = self.deadline("listing units", proxy.list_units()).await?;
        Ok(raw
            .into_iter()
            .map(
                |(name, description, load_state, active_state, sub_state, _following, path, ..)| {
                    UnitEntry {
                        name,
                        description,
                        load_state,
                        active_state,
                        sub_state,
                        path,
                    }
                },
            )
            .collect())
    }

    /// Every unit file the manager can see, loaded or not.
    pub async fn list_unit_files(&self) -> Result<Vec<UnitFileEntry>> {
        let proxy = self.manager_proxy().await?;
        let raw = self
            .deadline("listing unit files", proxy.list_unit_files())
            .await?;
        Ok(raw
            .into_iter()
            .map(|(path, state)| UnitFileEntry { path, state })
            .collect())
    }

    /// All properties of one interface on one unit.
    ///
    /// `GetAll` rather than a property each: the columns of a level want five
    /// or six values from the same interface, and one round trip that brings
    /// the whole map is cheaper than six that each bring one.
    ///
    /// A unit that vanished between the listing and this read (a transient unit
    /// that finished) yields an empty map rather than failing the whole level —
    /// its row then simply carries no runtime values.
    pub async fn properties(
        &self,
        path: &OwnedObjectPath,
        interface: &str,
    ) -> Props {
        self.try_properties(path, interface).await.unwrap_or_default()
    }

    async fn try_properties(
        &self,
        path: &OwnedObjectPath,
        interface: &str,
    ) -> Result<Props> {
        let conn = self.connection().await?;
        let iface = InterfaceName::try_from(interface)
            .map_err(|e| ContentError::Other(format!("bad interface name: {e}").into()))?;
        let proxy = PropertiesProxy::builder(conn)
            .destination(SERVICE)
            .and_then(|b| b.path(path.clone()))
            .map_err(|e| ContentError::Other(format!("addressing {path}: {e}").into()))?
            .build()
            .await
            .map_err(|e| ContentError::Other(format!("addressing {path}: {e}").into()))?;
        self.deadline(
            &format!("reading {interface} properties"),
            proxy.get_all(iface),
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Control
// ---------------------------------------------------------------------------

/// How long a job is watched before the adapter stops waiting for it.
///
/// Not a deadline in the [`Bus::deadline`] sense: the D-Bus call that *enqueues*
/// the job returns in milliseconds, and what this bounds is the job itself —
/// a `Type=notify` service that takes its time starting, a stop that waits out
/// `TimeoutStopSec=`. Tripping it is not an error and cancels nothing; it means
/// the adapter stops holding the pane's busy line and says the job is still
/// queued. Generous, because reporting "still running" for something that
/// finished two seconds later is the more annoying half of the trade.
pub const JOB_WAIT_SECS: u64 = 120;

/// Which manager method enqueues the job.
///
/// The job's *kind* is a transport fact — it picks the method — so it lives
/// here; which verb the user pressed, and whether they were asked first, is
/// [`crate::control`]'s business.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Start,
    Stop,
    Restart,
    Reload,
    ReloadOrRestart,
}

impl JobKind {
    /// The present participle, for the busy line ("Starting foo.service").
    pub fn gerund(self) -> &'static str {
        match self {
            JobKind::Start => "Starting",
            JobKind::Stop => "Stopping",
            JobKind::Restart => "Restarting",
            JobKind::Reload => "Reloading",
            JobKind::ReloadOrRestart => "Reloading or restarting",
        }
    }
}

/// How a job ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEnd {
    /// The manager reported the outcome: `done`, `failed`, `canceled`,
    /// `timeout`, `dependency`, `skipped`, `collected`, `once`.
    Reported(String),
    /// Still queued when [`JOB_WAIT_SECS`] ran out. The job was not cancelled.
    StillRunning,
}

/// Which unit-file operation to perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    Enable,
    Disable,
    Mask,
    Unmask,
}

/// What a unit-file operation did.
pub struct FileResult {
    /// `false` when the unit has no `[Install]` section — systemd then made no
    /// symlinks and the operation is a no-op the user has to be told about,
    /// because nothing else on screen would change.
    ///
    /// Only `EnableUnitFiles` reports this; the other operations always set it
    /// to `true`, since the question does not apply to them.
    pub carries_install_info: bool,
    /// One `(operation, filename, destination)` per symlink created or removed,
    /// exactly as the manager reports them.
    pub changes: Vec<(String, String, String)>,
}

impl Bus {
    /// The object path of a **loaded** unit, by name.
    ///
    /// `GetUnit`, not `LoadUnit`: asking for the properties of a unit is a
    /// read, and a read must not have the side effect of loading a unit the
    /// manager had deliberately let go.
    pub async fn unit_path(&self, name: &str) -> Result<OwnedObjectPath> {
        let proxy = self.manager_proxy().await?;
        self.deadline(
            &format!("looking up {name}"),
            proxy.get_unit(name.to_string()),
        )
        .await
    }

    /// Enqueue a job on `unit` and wait for the manager to say how it ended.
    ///
    /// The whole point of this phase: `StartUnit` returning a job path means
    /// "queued", not "started", and a tab that reports the former as the latter
    /// is lying at exactly the moment the user is watching. So the `JobRemoved`
    /// stream is opened **before** the call goes out — a fast job is removed
    /// again within a millisecond, and subscribing afterwards is a race that
    /// loses most of the time — and the result word comes from the manager.
    pub async fn run_job(&self, kind: JobKind, unit: &str) -> Result<JobEnd> {
        let proxy = self.manager_proxy().await?;
        self.enable_signals(&proxy).await;
        let mut removed = proxy.receive_job_removed().await.map_err(|e| {
            ContentError::Other(format!("watching jobs: {e}").into())
        })?;

        let name = unit.to_string();
        let mode = "replace".to_string();
        let enqueue = async {
            match kind {
                JobKind::Start => proxy.start_unit(name, mode).await,
                JobKind::Stop => proxy.stop_unit(name, mode).await,
                JobKind::Restart => proxy.restart_unit(name, mode).await,
                JobKind::Reload => proxy.reload_unit(name, mode).await,
                JobKind::ReloadOrRestart => proxy.reload_or_restart_unit(name, mode).await,
            }
        };
        let job = self
            .deadline(&format!("{} {unit}", kind.gerund().to_lowercase()), enqueue)
            .await?;

        let watch = async {
            while let Some(signal) = removed.next().await {
                let Ok(args) = signal.args() else { continue };
                if args.job() == &job {
                    return args.result().to_string();
                }
            }
            // The stream ended, which means the connection did. Say so rather
            // than claim an outcome.
            String::new()
        };
        match tokio::time::timeout(Duration::from_secs(JOB_WAIT_SECS), watch).await {
            Ok(result) if result.is_empty() => Err(ContentError::Other(
                format!("lost the connection while waiting for the {unit} job").into(),
            )),
            Ok(result) => Ok(JobEnd::Reported(result)),
            Err(_) => Ok(JobEnd::StillRunning),
        }
    }

    /// Create or remove the symlinks behind enable / disable / mask / unmask,
    /// then make the manager notice them.
    ///
    /// The `daemon-reload` is part of the operation, not a separate courtesy:
    /// without it the manager keeps serving the old picture, so the row the
    /// user is looking at would still say `disabled` after a successful enable.
    /// It is what `systemctl` does too, for the same reason.
    ///
    /// `runtime` is always `false` — everything this adapter writes is meant to
    /// survive a reboot. A `/run` variant is a phase-2 question, together with
    /// the rest of the write scope.
    pub async fn change_unit_file(&self, change: FileChange, unit: &str) -> Result<FileResult> {
        let proxy = self.manager_proxy().await?;
        let files = vec![unit.to_string()];
        let what = format!("{} {unit}", change.verb());
        let result = match change {
            FileChange::Enable => {
                let (install, changes) = self
                    .deadline(&what, proxy.enable_unit_files(files, false, false))
                    .await?;
                FileResult {
                    carries_install_info: install,
                    changes,
                }
            }
            FileChange::Disable => FileResult {
                carries_install_info: true,
                changes: self
                    .deadline(&what, proxy.disable_unit_files(files, false))
                    .await?,
            },
            FileChange::Mask => FileResult {
                carries_install_info: true,
                changes: self
                    .deadline(&what, proxy.mask_unit_files(files, false, false))
                    .await?,
            },
            FileChange::Unmask => FileResult {
                carries_install_info: true,
                changes: self
                    .deadline(&what, proxy.unmask_unit_files(files, false))
                    .await?,
            },
        };
        self.deadline("reloading the manager", proxy.reload()).await?;
        Ok(result)
    }

    /// Send `signal` to the unit's processes. `whom` is systemd's vocabulary:
    /// `main`, `control` or `all`.
    pub async fn kill(&self, unit: &str, whom: &str, signal: i32) -> Result<()> {
        let proxy = self.manager_proxy().await?;
        self.deadline(
            &format!("killing {unit}"),
            proxy.kill_unit(unit.to_string(), whom.to_string(), signal),
        )
        .await
    }

    /// Clear a unit's `failed` state so it can be started again.
    pub async fn reset_failed(&self, unit: &str) -> Result<()> {
        let proxy = self.manager_proxy().await?;
        self.deadline(
            &format!("resetting {unit}"),
            proxy.reset_failed_unit(unit.to_string()),
        )
        .await
    }

    /// Suspend (`freeze`) or resume (`thaw`) every process in the unit's cgroup.
    pub async fn freeze(&self, unit: &str, frozen: bool) -> Result<()> {
        let proxy = self.manager_proxy().await?;
        let name = unit.to_string();
        if frozen {
            self.deadline(&format!("freezing {unit}"), proxy.freeze_unit(name))
                .await
        } else {
            self.deadline(&format!("thawing {unit}"), proxy.thaw_unit(name))
                .await
        }
    }

    /// Ask the manager to emit unit and job signals, once per connection.
    ///
    /// systemd stays quiet until a client subscribes, and it answers a second
    /// `Subscribe` from the same client with an error — so this happens exactly
    /// once and its outcome is not worth failing an action over: if it did not
    /// take, the job wait falls through to its timeout and reports "still
    /// running" instead of an outcome, which is a degraded answer, not a wrong
    /// one.
    async fn enable_signals(&self, proxy: &ManagerProxy<'_>) {
        self.subscribed
            .get_or_init(|| async {
                let _ = proxy.subscribe().await;
            })
            .await;
    }
}

impl FileChange {
    /// The verb as the user knows it, for messages.
    pub fn verb(self) -> &'static str {
        match self {
            FileChange::Enable => "enabling",
            FileChange::Disable => "disabling",
            FileChange::Mask => "masking",
            FileChange::Unmask => "unmasking",
        }
    }
}
