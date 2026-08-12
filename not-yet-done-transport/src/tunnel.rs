//! In-process SSH local-port-forward via [`russh`].
//!
//! Supports a chain of jump hops: each subsequent SSH session is run
//! over a `direct-tcpip` channel of the previous one, so we never
//! shell out to `ssh -J` or `nc` for proxy-jumping. The free local
//! port is obtained by `TcpListener::bind("127.0.0.1:0")` before
//! opening the chain. This is a TOCTOU race in theory; in practice the
//! same race is what `kubectl port-forward`, `ssh -L … 0:…`, and
//! similar tools accept.
//!
//! For each incoming TCP connection on the local listener, the tunnel
//! opens a new `direct-tcpip` channel through the **last** hop's SSH
//! session and bidirectionally copies bytes between the two streams.
//! All upstream `Handle`s in the chain are kept alive in `Tunnel`; if
//! any of them is dropped the chain collapses, so the order of fields
//! and Drop are load-bearing.

use std::sync::Arc;

use russh::MethodSet;
use russh::client::{self, AuthResult, Handle, Handler};
use russh::keys::PrivateKeyWithHashAlg;
use russh::keys::agent::client::AgentClient;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use not_yet_done_content::CredentialProvider;

use crate::TransportError;
use crate::config::{Endpoint, SshAuth, SshHop};

/// Live SSH tunnel with a local listener forwarding to `target`.
///
/// Drop closes the listener and tears every SSH session in the chain
/// down (russh's `Handle` Drop closes the session).
pub struct Tunnel {
    pub local_port: u16,
    shutdown: watch::Sender<bool>,
    accept_task: JoinHandle<()>,
    /// Upstream hops in chain order. Only the last `Handle` is used to
    /// open per-connection `direct-tcpip` channels; the earlier ones
    /// are kept alive solely so the underlying tunneled streams stay
    /// open. Order matters for clean shutdown — dropping happens
    /// last-to-first via the implicit Vec drop.
    _chain: Vec<Arc<Handle<ClientHandler>>>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        self.accept_task.abort();
    }
}

/// Open the SSH chain, authenticate every hop, bind a local listener,
/// and spawn the accept loop forwarding to `target` via the last hop.
pub async fn start(hops: Vec<SshHop>, target: Endpoint) -> Result<Tunnel, TransportError> {
    if hops.is_empty() {
        return Err(TransportError::InvalidConfig(
            "ssh_tunnel mode requires at least one hop".into(),
        ));
    }

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| TransportError::Bind(format!("local listener: {e}")))?;
    let local_port = listener
        .local_addr()
        .map_err(|e| TransportError::Bind(format!("query local port: {e}")))?
        .port();

    let chain = open_chain(&hops).await?;
    let last = Arc::clone(chain.last().expect("non-empty by construction"));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let target = Arc::new(target);

    let accept_task = tokio::spawn(accept_loop(listener, last, target, shutdown_rx));

    Ok(Tunnel {
        local_port,
        shutdown: shutdown_tx,
        accept_task,
        _chain: chain,
    })
}

/// Build the SSH chain hop-by-hop. Hop 0 connects via TCP; hop N>0
/// runs `connect_stream` over a `direct-tcpip` channel of hop N-1
/// targeting hop N's `host:port`.
async fn open_chain(hops: &[SshHop]) -> Result<Vec<Arc<Handle<ClientHandler>>>, TransportError> {
    let config = Arc::new(client::Config {
        // russh defaults pick sensible window sizes / kex preferences.
        ..client::Config::default()
    });

    let mut chain: Vec<Arc<Handle<ClientHandler>>> = Vec::with_capacity(hops.len());

    // Hop 0 — direct TCP connect.
    let mut handle = client::connect(
        Arc::clone(&config),
        (hops[0].host.as_str(), hops[0].port),
        ClientHandler,
    )
    .await
    .map_err(|e| {
        TransportError::SshConnect(format!("hop #0 ({}:{}) — {e}", hops[0].host, hops[0].port))
    })?;
    authenticate(&mut handle, &hops[0], 0).await?;
    chain.push(Arc::new(handle));

    // Hops 1..n — each one is a fresh handshake over a direct-tcpip
    // channel of its predecessor. The previous hop resolves the
    // address, so e.g. `localhost:2222` on hop[1] means localhost on
    // hop[0]'s host.
    for (i, hop) in hops.iter().enumerate().skip(1) {
        let prev = chain
            .last()
            .expect("chain has at least one entry by induction");
        let channel = prev
            .channel_open_direct_tcpip(hop.host.clone(), hop.port as u32, "127.0.0.1", 0)
            .await
            .map_err(|e| {
                TransportError::Channel(format!("open hop #{i} ({}:{}): {e}", hop.host, hop.port))
            })?;
        let stream = channel.into_stream();

        let mut next = client::connect_stream(Arc::clone(&config), stream, ClientHandler)
            .await
            .map_err(|e| {
                TransportError::SshConnect(format!("hop #{i} ({}:{}) — {e}", hop.host, hop.port))
            })?;
        authenticate(&mut next, hop, i).await?;
        chain.push(Arc::new(next));
    }

    Ok(chain)
}

async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    ssh: &SshHop,
    hop_index: usize,
) -> Result<(), TransportError> {
    let user = ssh.user.clone();
    let tag = |msg: String| format!("hop #{hop_index}: {msg}");

    let result = match &ssh.auth {
        SshAuth::Password { password } => {
            let pw = resolve_provider(password, "ssh password").await?;
            handle
                .authenticate_password(user, pw)
                .await
                .map_err(|e| TransportError::SshAuth(tag(format!("password: {e}"))))?
        }
        SshAuth::PublicKey {
            identity,
            passphrase,
        } => {
            let pass = match passphrase {
                Some(p) => Some(resolve_provider(p, "ssh key passphrase").await?),
                None => None,
            };
            let key_path = identity.clone();
            let pass_owned = pass.clone();
            // load_secret_key is sync; keep the runtime non-blocking.
            let key = tokio::task::spawn_blocking(move || {
                russh::keys::load_secret_key(&key_path, pass_owned.as_deref())
            })
            .await
            .map_err(|e| TransportError::SshAuth(tag(format!("blocking task: {e}"))))?
            .map_err(|e| TransportError::SshAuth(tag(format!("load identity: {e}"))))?;

            // For RSA keys, `None` would default to legacy `ssh-rsa`
            // (SHA-1), which modern sshd reject. Ask the server for its
            // preferred RSA hash via the `server-sig-algs` extension
            // (RFC 8308) and use that. Non-RSA keys ignore the value.
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| TransportError::SshAuth(tag(format!("query rsa hash alg: {e}"))))?
                .flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg);
            handle
                .authenticate_publickey(user, key)
                .await
                .map_err(|e| TransportError::SshAuth(tag(format!("public_key: {e}"))))?
        }
        SshAuth::Agent => {
            let mut agent = AgentClient::connect_env()
                .await
                .map_err(|e| TransportError::SshAuth(tag(format!("agent connect: {e}"))))?;
            let identities = agent.request_identities().await.map_err(|e| {
                TransportError::SshAuth(tag(format!("agent request_identities: {e}")))
            })?;
            if identities.is_empty() {
                return Err(TransportError::SshAuth(tag(
                    "ssh-agent has no identities loaded".into(),
                )));
            }
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| TransportError::SshAuth(tag(format!("query rsa hash alg: {e}"))))?
                .flatten();
            let mut last = AuthResult::Failure {
                remaining_methods: MethodSet::empty(),
                partial_success: false,
            };
            for ident in identities {
                let pk = ident.public_key().into_owned();
                let res = handle
                    .authenticate_publickey_with(user.clone(), pk, hash_alg, &mut agent)
                    .await
                    .map_err(|e| TransportError::SshAuth(tag(format!("agent sign: {e:?}"))))?;
                if matches!(res, AuthResult::Success) {
                    last = res;
                    break;
                }
                last = res;
            }
            last
        }
    };

    match result {
        AuthResult::Success => Ok(()),
        AuthResult::Failure { .. } => Err(TransportError::SshAuth(tag(
            "server rejected credentials".into(),
        ))),
    }
}

async fn resolve_provider(
    provider: &CredentialProvider,
    label: &str,
) -> Result<String, TransportError> {
    let resolver = provider
        .build_resolver()
        .map_err(|e| TransportError::Provider(format!("{label}: {e}")))?;
    resolver
        .resolve()
        .await
        .map_err(|e| TransportError::Provider(format!("{label}: {e}")))
}

async fn accept_loop(
    listener: TcpListener,
    handle: Arc<Handle<ClientHandler>>,
    target: Arc<Endpoint>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, peer)) => {
                        let h = Arc::clone(&handle);
                        let t = Arc::clone(&target);
                        tokio::spawn(async move {
                            if let Err(e) = forward(stream, h, t, peer.port()).await {
                                // Per-connection failures are expected (peer
                                // hangups, slow targets, ...). Drop them
                                // silently; surfaces of the outer tunnel
                                // (e.g. auth failures) never reach this loop.
                                let _ = e;
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

async fn forward(
    mut local: tokio::net::TcpStream,
    handle: Arc<Handle<ClientHandler>>,
    target: Arc<Endpoint>,
    originator_port: u16,
) -> Result<(), TransportError> {
    let channel = handle
        .channel_open_direct_tcpip(
            target.host.clone(),
            target.port as u32,
            "127.0.0.1",
            originator_port as u32,
        )
        .await
        .map_err(|e| TransportError::Channel(format!("open: {e}")))?;

    let mut remote = channel.into_stream();

    // Bidirectional copy. `tokio::io::copy_bidirectional` is the right
    // primitive: it handles half-close cleanly so a client EOF is
    // forwarded as channel EOF and vice-versa.
    let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
    let _ = local.shutdown().await;
    Ok(())
}

/// SSH client handler. Accepts any host key — the equivalent of
/// `StrictHostKeyChecking=no`. A stricter known_hosts check could be
/// added here later without changing callers; for the initial cut we
/// match DBeaver's default behaviour.
struct ClientHandler;

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}
