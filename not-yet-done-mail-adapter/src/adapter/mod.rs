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
mod attachment;
mod factory;
mod folder;
mod message;
mod root;
mod scope;
mod types;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{OnceCell, RwLock, broadcast, watch};

use not_yet_done_content::{
    ActionInput, AdapterCapabilities, AdapterStatus, Child, ColumnSchema, ContentAdapter,
    ContentError, Invalidation, ListParams, ListResult, MetadataField, Node, NodeAction, NodeType,
    PageInfo, Result, StatusReporter, apply_sort,
};

use crate::config::{AccountConfig, MailConfig};
use crate::credentials::{AccountCredentials, LoginLane};
use crate::error::MailError;
use crate::ids::MessageId;
use crate::imap::conn::Connection;
use crate::model::{EnvelopeRow, FolderInfo};
use account::MailAccountNode;
use attachment::MailAttachmentNode;
use folder::MailFolderNode;
use message::{BodyCache, MailMessageNode};
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

/// One value out of a form action's input, refused when it is blank. A
/// download into `""` would silently land in the process's working
/// directory, which is not where the user meant.
fn form_field(input: &ActionInput, key: &str) -> Result<String> {
    match input {
        ActionInput::Form(values) => {
            let value = values.get(key).map(|s| s.trim()).unwrap_or("");
            if value.is_empty() {
                return Err(other_err(format!("`{key}` must not be empty")));
            }
            Ok(value.to_string())
        }
        _ => Err(ContentError::NotSupported("expected form input".into())),
    }
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
    /// The last page of messages listed per folder, so a cursor restored onto
    /// a row resolves without a round trip. One page per folder and replaced
    /// on every listing — a mailbox of a hundred thousand messages must not
    /// become a hundred thousand cached envelopes.
    messages: RwLock<HashMap<String, Arc<Vec<EnvelopeRow>>>>,
    /// Messages per page when a view asks for no window of its own.
    page_size: Option<u32>,
    /// The last few message bodies, shared by every message node this
    /// instance hands out — a node lives for one call, the cache has to
    /// outlive it or reading the same mail twice fetches it twice.
    bodies: Arc<BodyCache>,
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
            messages: RwLock::new(HashMap::new()),
            page_size: cfg.page_size,
            bodies: Arc::new(BodyCache::default()),
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

    /// One page of a folder's messages.
    ///
    /// The whole query string is IMAP SEARCH here — unlike the folder level,
    /// nothing is parsed out of it first. It does not need to be: the account
    /// and the mailbox are already fixed by the folder this level hangs
    /// under, so there is nothing left for the adapter to read.
    async fn message_level(
        &self,
        account: &str,
        folder: &str,
        params: &ListParams,
    ) -> Result<ListResult> {
        let limit = params
            .page
            .map(|p| p.limit)
            .or_else(|| self.runtime(account).ok()?.cfg.page_size)
            .or(self.page_size)
            .unwrap_or(crate::config::DEFAULT_PAGE_SIZE)
            .max(1);
        let offset = params.page.map(|p| p.offset).unwrap_or(0);
        let page = self
            .connection(account)
            .await?
            .messages(folder, params.query.as_deref().unwrap_or(""), offset, limit)
            .await
            .map_err(mail_err)?;

        let rows = Arc::new(page.rows);
        self.messages
            .write()
            .await
            .insert(crate::ids::folder_id(account, folder), Arc::clone(&rows));

        // Which flag slots this page needs, decided once for the whole page:
        // every row gets the same mask, so the gutter is a column that can be
        // read downward, and a slot nobody uses costs no width at all.
        let slots = message::FlagSlots::of(&rows);
        let mut items: Vec<_> = rows
            .iter()
            .map(|row| message::message_row(account, folder, row, slots))
            .collect();
        // Sorting is over the page, not the mailbox: the rows a sort could
        // reach are the ones already fetched. `applied_sort` is what says so
        // to the frontend rather than leaving the user to infer it.
        let applied = apply_sort(&mut items, &params.sort, &message::columns());
        Ok(ListResult {
            items,
            applied_sort: applied,
            page: Some(PageInfo {
                offset,
                limit,
                total: Some(page.total as u64),
                has_next: offset.saturating_add(limit) < page.total,
                has_prev: offset > 0,
            }),
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

    /// The envelope behind a message id, from the page it was listed on.
    ///
    /// A miss is not worth a fetch: `get_by_id` is what a restored cursor
    /// calls at startup, and fetching there would log every account in before
    /// the user has looked at anything. The row is built from the id instead,
    /// and says as much.
    async fn known_message(&self, id: &MessageId) -> EnvelopeRow {
        if let Some(hit) = self
            .messages
            .read()
            .await
            .get(&id.folder_id())
            .and_then(|page| {
                page.iter()
                    .find(|r| r.uid == id.uid && r.uid_validity == id.uid_validity)
            })
        {
            return hit.clone();
        }
        EnvelopeRow {
            uid: id.uid,
            uid_validity: id.uid_validity,
            subject: format!("message {}", id.uid),
            // Not knowing is not the same as having read it: an unloaded
            // message must not paint itself as seen.
            seen: true,
            ..Default::default()
        }
    }

    /// The envelope behind a message id, asking the server when the page
    /// that listed it is gone.
    ///
    /// The counterpart to [`MailAdapter::known_message`], and the difference
    /// is who is asking: this one is reached by *drilling into* a message,
    /// which the user just did, so a round trip is the answer to something
    /// they requested rather than a login behind their back.
    async fn fetch_message(&self, id: &MessageId) -> Result<EnvelopeRow> {
        if let Some(hit) = self
            .messages
            .read()
            .await
            .get(&id.folder_id())
            .and_then(|page| {
                page.iter()
                    .find(|r| r.uid == id.uid && r.uid_validity == id.uid_validity)
            })
        {
            return Ok(hit.clone());
        }
        self.connection(&id.account)
            .await?
            .envelope(&id.folder, id.uid_validity, id.uid)
            .await
            .map_err(mail_err)
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
        // Read from the most specific id outwards. Each of these patterns is
        // a prefix of the next: an attachment id also parses as a message id
        // (whose folder happens to end in `/part/2`), and that in turn as a
        // folder id. Whichever is tried first wins, so the order is the
        // meaning.
        if let Some((msg, part)) = crate::ids::parse_attachment_id(id) {
            self.runtime(&msg.account)?;
            let conn = self.connection(&msg.account).await?.clone();
            // The cache, never the server: this is also what a restored
            // cursor calls, and the part path in the id is all `open` needs.
            let known = self.known_message(&msg).await;
            let att = known
                .attachments
                .iter()
                .find(|a| a.part == part)
                .cloned()
                .unwrap_or_else(|| attachment::placeholder(&part));
            return Ok(Box::new(MailAttachmentNode::new(
                &msg,
                att,
                known.attachments,
                conn,
            )));
        }
        if let Some(msg) = crate::ids::parse_message_id(id) {
            self.runtime(&msg.account)?;
            let conn = self.connection(&msg.account).await?.clone();
            let row = self.known_message(&msg).await;
            return Ok(Box::new(MailMessageNode::new(
                &msg,
                &row,
                conn,
                Arc::clone(&self.bodies),
            )));
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
                let (msg_account, msg_path) = (account.clone(), path.clone());
                vec![
                    Child {
                        node_type: types::folder_type().clone(),
                        columns: folder::columns(),
                        list: Box::new(move |params| {
                            Box::pin(async move {
                                self.folder_level(&account, Some(&path), &params).await
                            })
                        }),
                    },
                    Child {
                        node_type: types::message_type().clone(),
                        columns: message::columns(),
                        list: Box::new(move |params| {
                            Box::pin(async move {
                                self.message_level(&msg_account, &msg_path, &params).await
                            })
                        }),
                    },
                ]
            }
            "mail:message" => {
                let Some(msg) = crate::ids::parse_message_id(node.id()) else {
                    return Vec::new();
                };
                vec![Child {
                    node_type: types::attachment_type().clone(),
                    columns: attachment::columns(),
                    list: Box::new(move |params| {
                        Box::pin(async move {
                            let row = self.fetch_message(&msg).await?;
                            let mut listed = attachment::list(&msg, &row.attachments);
                            listed.applied_sort = apply_sort(
                                &mut listed.items,
                                &params.sort,
                                &attachment::columns(),
                            );
                            Ok(listed)
                        })
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
            "mail:message" => message::columns(),
            "mail:attachment" => attachment::columns(),
            _ => Vec::new(),
        }
    }

    /// Only the attachment level acts. Everything above it is a listing —
    /// the write actions (seen, flag, move) arrive with phase 5.
    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "mail:attachment" => attachment::actions(),
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
        self.messages.write().await.clear();
        // A body is only valid for the UID it was fetched under, and a
        // reconnect is exactly when a mailbox may have been renumbered.
        self.bodies.clear();
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
        assert_eq!(cell(inbox, "unread_count"), "1");
        assert_eq!(cell(inbox, "total"), "3");
        // `unread` is the key the frontend paints the highlight from, so it
        // carries the FLAG and the count had to move aside. Getting these
        // two the wrong way round costs nothing at compile time and leaves
        // the unread highlight silently dead.
        assert_eq!(cell(inbox, "unread"), "true");
        assert_eq!(
            res.items[1].has_children,
            Some(true),
            "Archive has children"
        );

        // A `\Noselect` folder cannot be counted; an empty cell says so,
        // where a `0` would claim the folder is empty.
        assert_eq!(cell(&res.items[1], "unread_count"), "");
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

    /// Reading a message: a header block the server never sends, then the
    /// decoded text. And it is read *once* — the cache is what makes a
    /// preview pane that follows the cursor affordable.
    #[tokio::test]
    async fn a_message_reads_as_a_header_block_over_decoded_text() {
        let server = FakeServer::start(scripted()).await;
        let adapter = adapter_for(server.addr.port());
        let folder = adapter.get_by_id("work/INBOX").await.expect("resolves");
        children::list(
            &adapter,
            folder.as_ref(),
            params(types::message_type(), None),
        )
        .await
        .expect("lists");
        let listed = server.fetches();

        let node = adapter
            .get_by_id("work/INBOX#42.1")
            .await
            .expect("resolves");
        let text = node
            .content()
            .expect("a message has a body")
            .read_text()
            .await
            .expect("reads");

        assert!(text.starts_with("From: Jürgen <juergen@example.org>\n"), "{text}");
        assert!(text.contains("Subject: Grüße\n"), "{text}");
        assert!(
            text.contains("Grüße aus München"),
            "quoted-printable ISO-8859-1 arrived readable: {text}"
        );
        assert!(
            !text.contains("quoted-printable"),
            "the raw headers are not part of what is shown: {text}"
        );
        assert_eq!(server.fetches(), listed + 1, "one fetch for the body");

        // The same message again comes out of the cache.
        let again = adapter
            .get_by_id("work/INBOX#42.1")
            .await
            .expect("resolves");
        again
            .content()
            .expect("body")
            .read_text()
            .await
            .expect("reads");
        assert_eq!(
            server.fetches(),
            listed + 1,
            "a second read of the same message costs nothing"
        );
    }

    /// A message with attachments names them in its header block, so a
    /// reader knows what is there before drilling in.
    #[tokio::test]
    async fn a_message_with_files_says_so_above_its_text() {
        let server = FakeServer::start(scripted()).await;
        let adapter = adapter_for(server.addr.port());
        let folder = adapter.get_by_id("work/INBOX").await.expect("resolves");
        children::list(
            &adapter,
            folder.as_ref(),
            params(types::message_type(), None),
        )
        .await
        .expect("lists");

        let node = adapter
            .get_by_id("work/INBOX#42.4")
            .await
            .expect("resolves");
        let text = node
            .content()
            .expect("body")
            .read_text()
            .await
            .expect("reads");
        assert!(text.contains("Attachments: invoice.pdf\n"), "{text}");
        assert!(text.contains("Die Rechnung haengt an."), "{text}");
    }

    /// The attachment level is a projection of a row the adapter already
    /// holds: drilling into a listed message must not cost a round trip.
    #[tokio::test]
    async fn attachments_come_from_the_envelope_without_a_second_fetch() {
        let server = FakeServer::start(scripted()).await;
        let adapter = adapter_for(server.addr.port());
        let folder = adapter.get_by_id("work/INBOX").await.expect("resolves");
        children::list(
            &adapter,
            folder.as_ref(),
            params(types::message_type(), None),
        )
        .await
        .expect("lists");
        let listed = server.fetches();

        let message = adapter
            .get_by_id("work/INBOX#42.4")
            .await
            .expect("resolves");
        let res = children::list(
            &adapter,
            message.as_ref(),
            params(types::attachment_type(), None),
        )
        .await
        .expect("lists");

        let ids: Vec<&str> = res.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["work/INBOX#42.4/part/2"]);
        assert_eq!(res.items[0].label, "invoice.pdf");
        assert_eq!(cell(&res.items[0], "size"), "4096");
        assert_eq!(
            server.fetches(),
            listed,
            "the BODYSTRUCTURE came with the envelope"
        );
        assert!(children::check_rows(&attachment::columns(), &res.items).is_empty());
    }

    /// A message with nothing attached says so in its row, which is what
    /// keeps the drill arrow honest.
    #[tokio::test]
    async fn a_message_without_files_is_a_leaf() {
        let server = FakeServer::start(scripted()).await;
        let adapter = adapter_for(server.addr.port());
        let folder = adapter.get_by_id("work/INBOX").await.expect("resolves");
        let res = children::list(
            &adapter,
            folder.as_ref(),
            params(types::message_type(), None),
        )
        .await
        .expect("lists");

        let with_file = res.items.iter().find(|i| i.id.ends_with("#42.4")).unwrap();
        let without = res.items.iter().find(|i| i.id.ends_with("#42.1")).unwrap();
        assert_eq!(with_file.has_children, Some(true));
        assert_eq!(without.has_children, Some(false));
    }

    /// An attachment id resolves to a node that can open its file — the
    /// path is in the id, so a restored cursor works without the listing.
    #[tokio::test]
    async fn an_attachment_id_resolves_and_its_actions_are_published() {
        let server = FakeServer::start(scripted()).await;
        let adapter = adapter_for(server.addr.port());

        let node = adapter
            .get_by_id("work/INBOX#42.4/part/2")
            .await
            .expect("resolves");
        assert_eq!(node.node_type().type_id, "mail:attachment");
        assert_eq!(
            node.label(),
            "part 2",
            "nothing listed it, so the part path is the name it has"
        );
        assert_eq!(server.connections(), 0, "and nothing was connected over it");

        let ids: Vec<String> = adapter
            .actions_for_type(types::attachment_type())
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, ["open", "download_all"]);
        assert!(
            adapter.actions_for_type(types::message_type()).is_empty(),
            "the message level is read-only until phase 5"
        );
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
