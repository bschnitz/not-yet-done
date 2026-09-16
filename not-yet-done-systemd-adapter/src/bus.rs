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
//!
//! A *privileged* write is the one exception, and it gets its own budget. See
//! [`Bus::write`]: when polkit is asking a human for a password, silence is
//! the expected state, not a fault, and the read deadline would cut the dialog
//! off mid-typing.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use not_yet_done_content::{ContentError, Result};
use tokio::sync::OnceCell;
use zbus_systemd::systemd1::ManagerProxy;
use zbus_systemd::zbus::{self, fdo::PropertiesProxy, names::InterfaceName, proxy::MethodFlags};
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
    /// The deadline privileged writes run under — see [`Bus::write`].
    auth_timeout: Option<Duration>,
    conn: OnceCell<zbus::Connection>,
    /// Latches once `Subscribe` has been sent — see [`Bus::enable_signals`].
    subscribed: OnceCell<()>,
}

impl Bus {
    pub fn new(
        manager: Manager,
        timeout: Option<Duration>,
        auth_timeout: Option<Duration>,
    ) -> Self {
        Self {
            manager,
            timeout,
            auth_timeout,
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

    /// A cloned handle on the same connection, for a task that outlives the
    /// call that started it.
    ///
    /// [`crate::live`] runs for as long as the adapter does and cannot borrow
    /// from `&self`. A `zbus::Connection` is an `Arc` inside, so this shares
    /// the one connection rather than opening a second one — which matters
    /// because `Subscribe` is per client, and a second connection would be a
    /// second client the manager has to feed.
    pub async fn connection_handle(&self) -> Result<zbus::Connection> {
        self.connection().await.cloned()
    }

    /// Ask the manager to emit unit and job signals, from outside a call that
    /// already holds a proxy. Idempotent — see [`Bus::enable_signals`].
    pub async fn subscribe_signals(&self) -> Result<()> {
        let proxy = self.manager_proxy().await?;
        self.enable_signals(&proxy).await;
        Ok(())
    }

    async fn manager_proxy(&self) -> Result<ManagerProxy<'_>> {
        let conn = self.connection().await?;
        self.deadline("opening the manager proxy", ManagerProxy::new(conn))
            .await
    }

    /// Make one manager call that may have to be *authorised* before it runs.
    ///
    /// Two things separate this from [`Bus::deadline`], and both are about the
    /// same fact: on the system manager, every write is a polkit action.
    ///
    /// **The flag.** D-Bus lets a caller say whether it is willing to wait
    /// while the human at the keyboard is asked. A caller that does not say so
    /// is refused outright with
    /// `org.freedesktop.DBus.Error.InteractiveAuthorizationRequired` — the
    /// manager will not start a dialog behind a client's back. The generated
    /// [`ManagerProxy`] methods carry no flags, so the call goes out through
    /// the untyped [`zbus::Proxy`] underneath, which is the only reason this
    /// helper names its method as a string. Sending it on the *user* manager
    /// too costs nothing: with no policy to check, nobody is asked.
    ///
    /// **The deadline.** [`Bus::deadline`] exists to catch a manager that
    /// stopped answering. A call waiting on a password dialog is silent for
    /// the opposite reason — it is working exactly as intended — so it runs
    /// under `auth_timeout` instead, which is minutes rather than seconds.
    ///
    /// A refusal comes back as [`ContentError::PermissionDenied`], not as
    /// `Other`: declining a password is a decision the user made, and
    /// [`crate::control::run`] turns it back into a plain sentence rather than
    /// an adapter error.
    async fn write<B, R>(
        &self,
        proxy: &ManagerProxy<'_>,
        what: &str,
        method: &'static str,
        body: &B,
    ) -> Result<R>
    where
        B: serde::Serialize + zbus_systemd::zvariant::DynamicType,
        R: for<'d> zbus_systemd::zvariant::DynamicDeserialize<'d>,
    {
        let call =
            proxy
                .inner()
                .call_with_flags(method, MethodFlags::AllowInteractiveAuth.into(), body);
        let replied = match self.auth_timeout {
            None => call.await,
            Some(limit) => match tokio::time::timeout(limit, call).await {
                Ok(replied) => replied,
                Err(_) => {
                    return Err(ContentError::Other(
                        format!(
                            "{what} timed out after {}s — if an authentication dialog is open, \
                             it was never answered",
                            limit.as_secs()
                        )
                        .into(),
                    ));
                }
            },
        };
        match replied {
            Ok(Some(value)) => Ok(value),
            // Only reachable with `NoReplyExpected`, which this never sets —
            // but claiming success for a call whose outcome we never saw is
            // the one thing this whole module exists not to do.
            Ok(None) => Err(ContentError::Other(
                format!("{what}: the manager sent no reply").into(),
            )),
            Err(e) => Err(write_error(what, e)),
        }
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

    /// When this manager's own startup began and when it ended, on the
    /// monotonic clock.
    ///
    /// The two numbers `systemd-analyze time` subtracts from each other, read
    /// as properties instead of parsed out of its output. The first is the zero
    /// every `@` on the critical chain counts from; the second is what
    /// separates a unit that came up during the startup from one that was
    /// started long afterwards.
    ///
    /// The end is `None` while the manager is still starting up —
    /// `FinishTimestampMonotonic` is `0` until it finishes, and a startup that
    /// has not ended is a different answer from one that ended at time zero.
    pub async fn startup_window(&self) -> Result<(u64, Option<u64>)> {
        let proxy = self.manager_proxy().await?;
        let began = self
            .deadline("reading the startup time", proxy.userspace_timestamp_monotonic())
            .await?;
        let ended = self
            .deadline("reading the startup time", proxy.finish_timestamp_monotonic())
            .await?;
        Ok((began, (ended > 0).then_some(ended)))
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
    pub async fn properties(&self, path: &OwnedObjectPath, interface: &str) -> Props {
        self.try_properties(path, interface)
            .await
            .unwrap_or_default()
    }

    async fn try_properties(&self, path: &OwnedObjectPath, interface: &str) -> Result<Props> {
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

    /// The manager method this job is enqueued with.
    ///
    /// A string, not a generated proxy method, because the call goes out
    /// through [`Bus::write`] — see there for why the typed methods cannot be
    /// used for anything that may need authorising.
    pub fn method(self) -> &'static str {
        match self {
            JobKind::Start => "StartUnit",
            JobKind::Stop => "StopUnit",
            JobKind::Restart => "RestartUnit",
            JobKind::Reload => "ReloadUnit",
            JobKind::ReloadOrRestart => "ReloadOrRestartUnit",
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
    /// Enable or disable, whichever the preset policy says — the one operation
    /// whose outcome the row itself tells you, now that the `preset` column
    /// shows what the policy wants. See [`crate::preset`].
    Preset,
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
        let mut removed = proxy
            .receive_job_removed()
            .await
            .map_err(|e| ContentError::Other(format!("watching jobs: {e}").into()))?;

        let body = (unit.to_string(), "replace".to_string());
        let job: OwnedObjectPath = self
            .write(
                &proxy,
                &format!("{} {unit}", kind.gerund().to_lowercase()),
                kind.method(),
                &body,
            )
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
    /// It is also not skipped when the change list comes back empty, tempting
    /// as that is on the system manager where it would save a second password
    /// dialog. An empty list is not the same as "nothing happened": since the
    /// drop-in and preset work it has meant, more than once, that the symlinks
    /// were already as asked while the *manager's* picture was not. The
    /// dialog is the cheaper of the two mistakes.
    ///
    /// Every call here goes through [`Bus::write`] — both the file operation
    /// and the reload. On the system manager they are two separate polkit
    /// actions (`manage-unit-files` and `reload-daemon`), each `auth_admin_keep`,
    /// so a cold session is asked twice for one verb and then not again.
    ///
    /// `runtime` is always `false` — everything this adapter writes is meant to
    /// survive a reboot. A `/run` variant is a phase-2 question, together with
    /// the rest of the write scope.
    pub async fn change_unit_file(&self, change: FileChange, unit: &str) -> Result<FileResult> {
        let proxy = self.manager_proxy().await?;
        let files = vec![unit.to_string()];
        let what = format!("{} {unit}", change.verb());
        let result = match change {
            // Enable and preset share a shape as well as a signature: both may
            // decide to enable a unit that has no `[Install]` section, make no
            // symlinks at all, and have to say so.
            FileChange::Enable | FileChange::Preset => {
                let method = match change {
                    FileChange::Enable => "EnableUnitFiles",
                    _ => "PresetUnitFiles",
                };
                let (carries_install_info, changes) =
                    self.write(&proxy, &what, method, &(files, false, false)).await?;
                FileResult {
                    carries_install_info,
                    changes,
                }
            }
            FileChange::Disable => FileResult {
                carries_install_info: true,
                changes: self
                    .write(&proxy, &what, "DisableUnitFiles", &(files, false))
                    .await?,
            },
            FileChange::Mask => FileResult {
                carries_install_info: true,
                changes: self
                    .write(&proxy, &what, "MaskUnitFiles", &(files, false, false))
                    .await?,
            },
            FileChange::Unmask => FileResult {
                carries_install_info: true,
                changes: self
                    .write(&proxy, &what, "UnmaskUnitFiles", &(files, false))
                    .await?,
            },
        };
        self.daemon_reload().await?;
        Ok(result)
    }

    /// Send `signal` to the unit's processes. `whom` is systemd's vocabulary:
    /// `main`, `control` or `all`.
    pub async fn kill(&self, unit: &str, whom: &str, signal: i32) -> Result<()> {
        let proxy = self.manager_proxy().await?;
        let body = (unit.to_string(), whom.to_string(), signal);
        self.write(&proxy, &format!("killing {unit}"), "KillUnit", &body)
            .await
    }

    /// Have the manager re-read every unit file on disk.
    ///
    /// What makes an edited file *configuration*. It changes nothing that is
    /// running: a unit that is up keeps the settings it started with until it
    /// is restarted, which is exactly the question the edit flow asks
    /// afterwards rather than answering for the user.
    pub async fn daemon_reload(&self) -> Result<()> {
        let proxy = self.manager_proxy().await?;
        self.write(&proxy, "reloading the manager", "Reload", &())
            .await
    }

    /// Clear a unit's `failed` state so it can be started again.
    pub async fn reset_failed(&self, unit: &str) -> Result<()> {
        let proxy = self.manager_proxy().await?;
        let body = (unit.to_string(),);
        self.write(
            &proxy,
            &format!("resetting {unit}"),
            "ResetFailedUnit",
            &body,
        )
        .await
    }

    /// Suspend (`freeze`) or resume (`thaw`) every process in the unit's cgroup.
    pub async fn freeze(&self, unit: &str, frozen: bool) -> Result<()> {
        let proxy = self.manager_proxy().await?;
        let body = (unit.to_string(),);
        let (what, method) = if frozen {
            (format!("freezing {unit}"), "FreezeUnit")
        } else {
            (format!("thawing {unit}"), "ThawUnit")
        };
        self.write(&proxy, &what, method, &body).await
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

/// Say what a failed privileged call means, in the user's terms.
///
/// The two names below are polkit's answer arriving as a D-Bus error, and they
/// are the only ones that are not a fault:
///
///   * `InteractiveAuthorizationRequired` — the call was refused *before*
///     anyone was asked. On this code path that should not happen, since every
///     write carries the flag; it is kept because a policy set to
///     `auth_admin` with no agent running (a bare tty, a session without a
///     polkit agent) produces exactly this, and "no agent" is worth saying.
///   * `AccessDenied` — the request reached a human and did not come back
///     authorised. systemd sends the same name whether the dialog was
///     dismissed, the password was wrong, or the policy says no outright, so
///     the sentence covers all three rather than guessing which one it was.
///
/// Anything else is a real error and stays one.
fn write_error(what: &str, err: zbus::Error) -> ContentError {
    let zbus::Error::MethodError(name, detail, _) = &err else {
        return ContentError::Other(format!("{what}: {err}").into());
    };
    match name.as_str() {
        "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired" => {
            ContentError::PermissionDenied(format!(
                "{what} needs authorisation and nothing could ask for it — \
                 no polkit agent is running for this session"
            ))
        }
        "org.freedesktop.DBus.Error.AccessDenied" => ContentError::PermissionDenied(format!(
            "{what} was not authorised — the request was dismissed or refused"
        )),
        _ => match detail {
            Some(said) => ContentError::Other(format!("{what}: {said}").into()),
            None => ContentError::Other(format!("{what}: {name}").into()),
        },
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
            FileChange::Preset => "presetting",
        }
    }
}
