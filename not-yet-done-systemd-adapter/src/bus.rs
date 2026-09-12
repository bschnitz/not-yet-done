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
}

impl Bus {
    pub fn new(manager: Manager, timeout: Option<Duration>) -> Self {
        Self {
            manager,
            timeout,
            conn: OnceCell::new(),
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
