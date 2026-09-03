//! `MailAdapter` — the `ContentAdapter` implementation.
//!
//! One instance, many accounts (see `docs/plan-mail-adapter.md` §4.2): the
//! root lists the configured accounts, each account its folder tree, each
//! folder its messages. Every id below the root names its account first, so
//! a call routes to a connection by reading its id (see [`crate::ids`]).
//!
//! Two things follow from *many* accounts under *one* instance, and both are
//! solved here rather than in the IMAP layer:
//!
//! - **One status channel.** Each account reports on its own
//!   [`StatusReporter`]; this adapter merges them onto the single channel a
//!   frontend subscribes to, naming the account in the credential dialog's
//!   header so a password prompt says whose it is.
//! - **One credential channel.** `submit_credentials` addresses the instance,
//!   not the account. Logins are therefore serialised by the
//!   [`LoginLane`](crate::credentials::LoginLane), and whoever holds the lane
//!   is the addressee of the answer.

mod account;
mod factory;
mod folder;
mod root;
mod scope;
mod types;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{OnceCell, RwLock, broadcast, watch};

use not_yet_done_content::{
    AdapterCapabilities, AdapterStatus, Child, ColumnSchema, ContentAdapter, ContentError,
    Invalidation, ListParams, ListResult, MetadataField, Node, Result, StatusReporter, apply_sort,
};

use crate::config::{AccountConfig, MailConfig};
use crate::credentials::{AccountCredentials, LoginLane};
use crate::error::MailError;
use crate::imap::conn::Connection;
use crate::model::FolderInfo;
use account::MailAccountNode;
use folder::MailFolderNode;
use root::MailRoot;

pub use factory::MailAdapterFactory;

/// Id of the root node — the instance itself, above any account.
pub(crate) const ROOT_ID: &str = "root";

/// One read-only metadata field. Everything the folder tree shows is the
/// server's; nothing on these levels is edited in place.
pub(crate) fn field(key: &str, value: String, label: &str) -> MetadataField {
    MetadataField {
        key: key.into(),
        value,
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    }
}

fn other_err(e: impl std::fmt::Display) -> ContentError {
    ContentError::Other(e.to_string().into())
}

/// Everything one account needs, plus the connection it gets on first use.
struct AccountRuntime {
    cfg: Arc<AccountConfig>,
    creds: Arc<AccountCredentials>,
    /// The account's own status channel, merged onto the instance's by the
    /// forwarder started alongside the connection.
    status: StatusReporter,
    /// Built on first use — opening one account's subtab must not log into
    /// the other five.
    conn: OnceCell<Connection>,
}

pub struct MailAdapter {
    instance_id: String,
    name: String,
    /// Configuration order, which is the order the user wrote and the order
    /// the account rows appear in.
    order: Vec<String>,
    accounts: HashMap<String, AccountRuntime>,
    /// The one channel a frontend subscribes to.
    status_tx: watch::Sender<AdapterStatus>,
    /// A `watch` sender with no receivers cannot publish, so the adapter
    /// keeps one of its own — otherwise the very first status report, sent
    /// before any frontend subscribed, would be dropped on the floor.
    _status_keepalive: watch::Receiver<AdapterStatus>,
    inv_tx: broadcast::Sender<Invalidation>,
    /// Whoever holds this is the addressee of the next `submit_credentials`.
    lane: Arc<LoginLane>,
    /// Last folder listing per account. A tree expands one level at a time,
    /// and re-`LIST`ing the whole mailbox set for every expanded node would
    /// be a round trip per keystroke; a listing of a *top* level refreshes
    /// it, so a reload is still a reload.
    folders: RwLock<HashMap<String, Arc<Vec<FolderInfo>>>>,
}

impl MailAdapter {
    pub(crate) fn from_config(
        instance_id: &str,
        cfg: MailConfig,
    ) -> std::result::Result<Self, String> {
        cfg.validate()?;
        let lane = LoginLane::new();
        let (status_tx, status_rx) = watch::channel(AdapterStatus::Idle);
        let (inv_tx, _) = broadcast::channel(64);

        let mut order = Vec::new();
        let mut accounts = HashMap::new();
        for account in cfg.accounts {
            let account = Arc::new(account);
            let status = StatusReporter::new();
            let creds = AccountCredentials::new(
                account.id.clone(),
                account.auth.clone(),
                status.clone(),
                Arc::clone(&lane),
            )
            .map_err(|e| format!("account `{}`: {e}", account.id))?;
            order.push(account.id.clone());
            accounts.insert(
                account.id.clone(),
                AccountRuntime {
                    cfg: account,
                    creds,
                    status,
                    conn: OnceCell::new(),
                },
            );
        }

        Ok(Self {
            instance_id: instance_id.to_string(),
            name: cfg.name.unwrap_or_else(|| "Mail".to_string()),
            order,
            accounts,
            status_tx,
            _status_keepalive: status_rx,
            inv_tx,
            lane,
            folders: RwLock::new(HashMap::new()),
        })
    }

    /// The accounts in configuration order.
    fn configured(&self) -> Vec<Arc<AccountConfig>> {
        self.order
            .iter()
            .filter_map(|id| self.accounts.get(id))
            .map(|rt| Arc::clone(&rt.cfg))
            .collect()
    }

    fn runtime(&self, account: &str) -> Result<&AccountRuntime> {
        self.accounts.get(account).ok_or_else(|| {
            ContentError::NotFound(format!("no account `{account}` in this instance"))
        })
    }

    /// This account's connection, started on first use together with the task
    /// that merges its status onto the instance's channel.
    async fn connection(&self, account: &str) -> Result<&Connection> {
        let rt = self.runtime(account)?;
        Ok(rt
            .conn
            .get_or_init(|| async {
                self.forward_status(rt);
                Connection::spawn(
                    Arc::clone(&rt.cfg),
                    Arc::clone(&rt.creds),
                    rt.status.clone(),
                )
            })
            .await)
    }

    /// Merge one account's status into the instance's channel.
    ///
    /// The only thing rewritten on the way is the credential dialog's header:
    /// the auth layer writes it without knowing which of six mailboxes it is
    /// asking for, and an unnamed password prompt in a six-account instance is
    /// a question the user cannot answer.
    fn forward_status(&self, rt: &AccountRuntime) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut rx = rt.status.subscribe();
        let tx = self.status_tx.clone();
        let who = rt.cfg.label().to_string();
        handle.spawn(async move {
            while rx.changed().await.is_ok() {
                let status = match rx.borrow_and_update().clone() {
                    AdapterStatus::NeedsCreds {
                        fields,
                        header,
                        error,
                    } => AdapterStatus::NeedsCreds {
                        fields,
                        header: Some(match header {
                            Some(h) => format!("{who}: {h}"),
                            None => format!("{who}: log in"),
                        }),
                        error,
                    },
                    other => other,
                };
                if tx.send(status).is_err() {
                    break;
                }
            }
        });
    }

    /// Every mailbox of an account. `refresh` asks the server; otherwise the
    /// snapshot the level above left behind is reused.
    async fn folders_of(&self, account: &str, refresh: bool) -> Result<Arc<Vec<FolderInfo>>> {
        if !refresh {
            if let Some(hit) = self.folders.read().await.get(account) {
                return Ok(Arc::clone(hit));
            }
        }
        let listed = self
            .connection(account)
            .await?
            .folders()
            .await
            .map_err(mail_err)?;
        let listed = Arc::new(listed);
        self.folders
            .write()
            .await
            .insert(account.to_string(), Arc::clone(&listed));
        Ok(listed)
    }

    /// One level of an account's folder tree: the mailboxes directly below
    /// `under` (the top level when it is `None`), with their counts.
    async fn folder_level(
        &self,
        account: &str,
        under: Option<&str>,
        params: &ListParams,
    ) -> Result<ListResult> {
        // A top-level listing is the one the user triggers (opening the tab,
        // pressing reload); the deeper ones come from expanding what it
        // returned, and re-listing the whole mailbox set for each of them
        // would be a round trip per expanded node.
        let all = self.folders_of(account, under.is_none()).await?;
        let mut items = Vec::new();
        for info in all.iter().filter(|f| f.parent_path() == under) {
            let has_children = all.iter().any(|c| c.parent_path() == Some(&info.path));
            let mut info = info.clone();
            if info.selectable {
                // One `STATUS` round trip per folder — for this level only,
                // never for the whole tree.
                let (total, unread) = self
                    .connection(account)
                    .await?
                    .folder_status(&info.path)
                    .await
                    .map_err(mail_err)?;
                info.total = Some(total);
                info.unread = Some(unread);
            }
            items.push(folder::folder_row(account, &info, has_children));
        }
        let applied = apply_sort(&mut items, &params.sort, &folder::columns());
        Ok(ListResult {
            items,
            applied_sort: applied,
            page: None,
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }

    /// Which account a folder listing is about: the one the query names, or —
    /// when the instance holds exactly one — that one.
    fn scoped_account(&self, named: Option<String>) -> Result<String> {
        if let Some(id) = named {
            self.runtime(&id)?;
            return Ok(id);
        }
        match self.order.as_slice() {
            [only] => Ok(only.clone()),
            _ => Err(other_err(
                "this instance holds several accounts — scope the level with `query: \"account:<id>\"`",
            )),
        }
    }

    /// The `FolderInfo` behind an id, from the last listing.
    async fn known_folder(&self, account: &str, path: &str) -> FolderInfo {
        if let Some(hit) = self
            .folders
            .read()
            .await
            .get(account)
            .and_then(|all| all.iter().find(|f| f.path == path))
        {
            return hit.clone();
        }
        // Not listed yet — a restored cursor, say. Resolving it must not
        // connect, so the node is built from the id alone; the label is the
        // whole path because without a `LIST` we do not know where the
        // server's own delimiter splits it.
        FolderInfo {
            path: path.to_string(),
            name: crate::mutf7::decode(path),
            ..Default::default()
        }
    }
}

/// An IMAP-layer error as the content layer sees it.
fn mail_err(e: MailError) -> ContentError {
    match e {
        MailError::Cancelled => other_err("the login was cancelled"),
        other => other_err(other),
    }
}

#[async_trait]
impl ContentAdapter for MailAdapter {
    fn adapter_type(&self) -> &str {
        "mail"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(MailRoot {
            name: self.name.clone(),
        }))
    }

    /// Resolve an id without touching the network: every id says which
    /// account it belongs to, and the folder tree it names was listed by
    /// whoever produced the id.
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        if id == ROOT_ID {
            return self.root().await;
        }
        if let Some((account, path)) = crate::ids::split_folder_id(id) {
            self.runtime(account)?;
            let info = self.known_folder(account, path).await;
            return Ok(Box::new(MailFolderNode::new(account, &info)));
        }
        let rt = self.runtime(id)?;
        Ok(Box::new(MailAccountNode::new(&rt.cfg)))
    }

    fn capabilities(&self) -> AdapterCapabilities {
        // Reading only, until phase 5 brings seen/flag/move.
        AdapterCapabilities::default()
    }

    /// What lives under a mail node.
    ///
    /// The root offers **both** `mail:account` and `mail:folder`: the first is
    /// the tree over all accounts, the second is what a per-account subtab
    /// pins itself to with `query: "account:<id>"`. Without a scope the folder
    /// child is only unambiguous when the instance holds a single account, and
    /// says so otherwise instead of guessing.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        match node.node_type().type_id.as_str() {
            "mail:root" => vec![
                Child {
                    node_type: types::account_type().clone(),
                    columns: account::columns(),
                    list: Box::new(move |params| {
                        Box::pin(
                            async move { Ok(account::list_accounts(&self.configured(), &params)) },
                        )
                    }),
                },
                Child {
                    node_type: types::folder_type().clone(),
                    columns: folder::columns(),
                    list: Box::new(move |params| {
                        Box::pin(async move {
                            let scope = scope::parse_folder_scope(params.query.as_deref())
                                .map_err(other_err)?;
                            let account = self.scoped_account(scope.account)?;
                            self.folder_level(&account, scope.under.as_deref(), &params)
                                .await
                        })
                    }),
                },
            ],
            "mail:account" => {
                let account = node.id().to_string();
                vec![Child {
                    node_type: types::folder_type().clone(),
                    columns: folder::columns(),
                    list: Box::new(move |params| {
                        Box::pin(async move { self.folder_level(&account, None, &params).await })
                    }),
                }]
            }
            "mail:folder" => {
                let Some((account, path)) = crate::ids::split_folder_id(node.id()) else {
                    return Vec::new();
                };
                let (account, path) = (account.to_string(), path.to_string());
                vec![Child {
                    node_type: types::folder_type().clone(),
                    columns: folder::columns(),
                    list: Box::new(move |params| {
                        Box::pin(
                            async move { self.folder_level(&account, Some(&path), &params).await },
                        )
                    }),
                }]
            }
            _ => Vec::new(),
        }
    }

    async fn describe_columns(&self, node_type: &str) -> Vec<ColumnSchema> {
        match node_type {
            "mail:account" => account::columns(),
            "mail:folder" => folder::columns(),
            _ => Vec::new(),
        }
    }

    fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.status_tx.subscribe()
    }

    fn subscribe_invalidations(&self) -> broadcast::Receiver<Invalidation> {
        self.inv_tx.subscribe()
    }

    /// Hand the answer to whoever asked. The lane guarantees there is at most
    /// one asker, which is what makes an instance-wide credential channel
    /// enough for six accounts.
    async fn submit_credentials(&self, fields: HashMap<String, String>) -> Result<()> {
        let asking = self
            .lane
            .holder()
            .await
            .ok_or_else(|| other_err("no account is currently asking for credentials"))?;
        self.runtime(&asking)?
            .creds
            .submit(fields)
            .await
            .map_err(other_err)
    }

    async fn cancel_credentials(&self) -> Result<()> {
        let Some(asking) = self.lane.holder().await else {
            return Ok(());
        };
        self.runtime(&asking)?
            .creds
            .cancel()
            .await
            .map_err(other_err)
    }

    /// Drop every account's session. The next request reconnects the accounts
    /// that are actually used — which, with one instance per six mailboxes, is
    /// usually one of them.
    async fn invalidate_session(&self) -> Result<()> {
        for id in &self.order {
            if let Some(conn) = self.accounts.get(id).and_then(|rt| rt.conn.get()) {
                let _ = conn.disconnect().await;
            }
        }
        self.folders.write().await.clear();
        Ok(())
    }

    async fn invalidate_credentials(&self) -> Result<()> {
        for id in &self.order {
            if let Some(rt) = self.accounts.get(id) {
                rt.creds.invalidate().await;
            }
        }
        self.invalidate_session().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::testserver::{FakeServer, PASSWORD, scripted};
    use not_yet_done_content::{ListParams, NodeSummary, NodeType, children};

    /// A two-account instance against one fake server. The second account is
    /// deliberately never reached: it is what makes "opening one subtab must
    /// not log into the others" a testable claim.
    fn adapter_for(port: u16) -> MailAdapter {
        let yaml = format!(
            r#"
name: Mail
accounts:
  - id: work
    name: Work
    host: 127.0.0.1
    port: {port}
    security: none
    auth:
      mechanism: password
      bindings:
        - field: username
          provider: {{ type: literal, value: someone }}
        - field: password
          provider: {{ type: literal, value: {PASSWORD} }}
  - id: private
    name: Private
    host: 127.0.0.1
    port: {port}
    security: none
    auth:
      mechanism: password
      bindings:
        - field: username
          provider: {{ type: literal, value: someone }}
        - field: password
          provider: {{ type: literal, value: {PASSWORD} }}
"#
        );
        let cfg: MailConfig = serde_yaml::from_str(&yaml).expect("config parses");
        MailAdapter::from_config("mail", cfg).expect("adapter builds")
    }

    fn params(node_type: &NodeType, query: Option<&str>) -> ListParams {
        ListParams {
            node_type: node_type.clone(),
            query: query.map(str::to_string),
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        }
    }

    fn cell<'a>(row: &'a NodeSummary, key: &str) -> &'a str {
        row.metadata
            .fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.as_str())
            .unwrap_or("<missing>")
    }

    /// The shape the shipped view uses: a subtab pinned to one account with
    /// `query: "account:work"`, showing that account's top-level mailboxes.
    #[tokio::test]
    async fn a_subtab_scoped_to_an_account_lists_its_top_level_folders() {
        let server = FakeServer::start(scripted()).await;
        let adapter = adapter_for(server.addr.port());
        let root = adapter.root().await.expect("root");

        let res = children::list(
            &adapter,
            root.as_ref(),
            params(types::folder_type(), Some("account:work")),
        )
        .await
        .expect("lists");

        let ids: Vec<&str> = res.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "work/INBOX",
                "work/Archive",
                "work/Entw&APw-rfe",
                "work/Sent Items"
            ],
            "the top level only — `Archive/2019` hangs under Archive"
        );

        let inbox = &res.items[0];
        assert_eq!(cell(inbox, "unread"), "3");
        assert_eq!(cell(inbox, "total"), "12");
        assert_eq!(cell(inbox, "unread_marker"), "true");
        assert_eq!(
            res.items[1].has_children,
            Some(true),
            "Archive has children"
        );

        // A `\Noselect` folder cannot be counted; an empty cell says so,
        // where a `0` would claim the folder is empty.
        assert_eq!(cell(&res.items[1], "unread"), "");
        assert_eq!(res.items[2].label, "Entwürfe", "the label is decoded");
        assert_eq!(cell(&res.items[2], "name"), "Entwürfe");

        // The columns the adapter describes must be present in every row —
        // that promise is what local sorting and filtering are built on.
        assert!(
            children::check_rows(&folder::columns(), &res.items).is_empty(),
            "every described column is carried in every row"
        );

        // Only the account the subtab named was ever contacted.
        assert_eq!(server.connections(), 1);
    }

    /// Expanding a node must not re-`LIST` the whole mailbox set: the tree
    /// walks the snapshot the top-level listing left behind.
    #[tokio::test]
    async fn drilling_into_a_folder_reuses_the_snapshot() {
        let server = FakeServer::start(scripted()).await;
        let adapter = adapter_for(server.addr.port());
        let root = adapter.root().await.expect("root");
        children::list(
            &adapter,
            root.as_ref(),
            params(types::folder_type(), Some("account:work")),
        )
        .await
        .expect("top level");
        assert_eq!(server.lists(), 1);

        let archive = adapter.get_by_id("work/Archive").await.expect("resolves");
        assert_eq!(
            archive.label(),
            "Archive",
            "the label comes from the listing"
        );
        let res = children::list(
            &adapter,
            archive.as_ref(),
            params(types::folder_type(), None),
        )
        .await
        .expect("children");
        let ids: Vec<&str> = res.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["work/Archive/2019"]);
        assert_eq!(server.lists(), 1, "no second LIST for an expanded node");
    }

    /// With several accounts and no scope there is no right answer, and
    /// picking one silently would show the wrong mailbox under the right
    /// title.
    #[tokio::test]
    async fn an_unscoped_folder_level_says_it_needs_an_account() {
        let server = FakeServer::start(scripted()).await;
        let adapter = adapter_for(server.addr.port());
        let root = adapter.root().await.expect("root");

        let listed =
            children::list(&adapter, root.as_ref(), params(types::folder_type(), None)).await;
        let err = match listed {
            Err(e) => e,
            Ok(res) => panic!("expected a refusal, got {} rows", res.items.len()),
        };
        assert!(err.to_string().contains("account:"), "{err}");
        assert_eq!(server.connections(), 0, "and nothing was connected over it");
    }

    /// The account level is pure config: it renders before anything is
    /// logged in, which is what lets a six-account tab come up instantly.
    #[tokio::test]
    async fn accounts_list_without_touching_the_network() {
        let server = FakeServer::start(scripted()).await;
        let adapter = adapter_for(server.addr.port());
        let root = adapter.root().await.expect("root");

        let res = children::list(&adapter, root.as_ref(), params(types::account_type(), None))
            .await
            .expect("lists");
        let labels: Vec<&str> = res.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["Work", "Private"], "in configuration order");
        assert_eq!(server.connections(), 0);
        assert!(children::check_rows(&account::columns(), &res.items).is_empty());
    }
}
