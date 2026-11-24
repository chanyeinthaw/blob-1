//! Primary side of the `/ws/node` wire protocol: `blob-1 node` machines dial
//! out and present a `hello`. Unknown nodes are recorded in the environments
//! table with an empty credential (`pending:<node_id>`) until an admin
//! accepts them; accepted rows adopt the presented key so reconnects
//! authenticate immediately. `NodeRegistry` is also the `NodeDirectory`
//! `RemoteEnvironment` routes through.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket};
use dashmap::DashMap;
use environment::node::{
    Hello, NodeFrame, NodeReply, NodeRequest, PrimaryFrame, IDLE_TIMEOUT, NODE_PROTOCOL_VERSION,
    PING_INTERVAL,
};
use environment::ToolError;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use sha2::{Digest, Sha256};
use storage::{Environment, EnvironmentStore};
use storage_sqlite::SqliteStorage;
use tokio::sync::{mpsc, oneshot, watch};

const MAX_PENDING: usize = 32;
const PENDING_TTL: i64 = 15 * 60;
const SWEEP_INTERVAL: Duration = Duration::from_secs(2);
/// A connected socket that never sends `hello` is dropped.
const HELLO_TIMEOUT: Duration = Duration::from_secs(30);
/// In-flight requests older than this are failed and cancelled on the node
/// (a hair past `RemoteEnvironment`'s 120s execute timeout).
const REQUEST_TTL: Duration = Duration::from_secs(135);

/// A node awaiting accept — surfaced by `environments.list`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingNode {
    pub node_id: String,
    pub hostname: String,
    pub version: String,
    pub connected_at: i64,
}

/// An accepted environment — surfaced by `environments.list`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub name: String,
    pub node_id: String,
    pub version: Option<String>,
    pub last_seen: i64,
    /// A node session is authenticated for this env right now.
    pub online: bool,
}

/// One live `/ws/node` socket. `env` is `Some` once the session is
/// authenticated for an environment name.
struct Session {
    hello: Hello,
    /// sha256(node_key); compared against `environments.credential`.
    key_hash: Vec<u8>,
    connected_at: i64,
    /// The environment name this session's route is installed under.
    bound: Mutex<Option<String>>,
    /// Frames (already serialized) to the node; `Close` ends the writer task.
    tx: mpsc::Sender<Message>,
    /// Out-of-band socket kill — set when the frame queue can't carry a
    /// `Close`/`Cancel`; writer and reader select on it.
    kill: watch::Sender<bool>,
    /// In-flight `Request` ids awaiting a `Result`, timestamped for expiry.
    requests: DashMap<u64, (Instant, oneshot::Sender<NodeReply>)>,
    next_id: AtomicU64,
}

/// Live node sessions plus the env-name routing table. Spawned once at serve
/// startup; `handle_connection` runs per `/ws/node` connection.
pub struct NodeRegistry {
    store: Arc<SqliteStorage>,
    /// node_id → session (pending or authenticated).
    sessions: DashMap<String, Arc<Session>>,
    /// env name → the one live authenticated session.
    env_sessions: DashMap<String, Arc<Session>>,
    /// env name → routed enrollment ID (`None` when unrouted). Carrying
    /// identity lets consumers notice a re-enrollment under the same name,
    /// which a bool flap can coalesce away.
    env_watch: DashMap<String, watch::Sender<Option<Vec<u8>>>>,
    /// Serializes every route change, watch publication, and kill with
    /// `call`'s waiter registration. Acquire before `Session::bound`. Arc'd
    /// so `RequestGuard` can serialize its out-of-band kill too.
    route_lock: Arc<Mutex<()>>,
}

impl NodeRegistry {
    /// Starts the 2s sweeper. Call from inside the tokio runtime.
    pub fn new(store: Arc<SqliteStorage>) -> Arc<Self> {
        let registry = Arc::new(Self {
            store,
            sessions: DashMap::new(),
            env_sessions: DashMap::new(),
            env_watch: DashMap::new(),
            route_lock: Arc::new(Mutex::new(())),
        });
        let weak = Arc::downgrade(&registry);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(SWEEP_INTERVAL);
            loop {
                interval.tick().await;
                let Some(registry) = weak.upgrade() else {
                    break;
                };
                registry.sweep();
            }
        });
        registry
    }

    /// One `/ws/node` connection: reads `hello`, enforces protocol/caps,
    /// records or authenticates the node, then serves requests and ping/pong
    /// until close. Returns an owned future for `WebSocketUpgrade::on_upgrade`.
    pub fn handle_connection(
        self: &Arc<Self>,
        socket: WebSocket,
    ) -> impl std::future::Future<Output = ()> + Send + 'static {
        let registry = self.clone();
        async move { registry.serve_socket(socket).await }
    }

    async fn serve_socket(self: Arc<Self>, socket: WebSocket) {
        let (mut sink, mut stream) = socket.split();

        let hello = match tokio::time::timeout(HELLO_TIMEOUT, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<Hello>(&text) {
                Ok(hello) => hello,
                Err(_) => return,
            },
            _ => return,
        };

        if hello.protocol != NODE_PROTOCOL_VERSION {
            reject(&mut sink, "protocol version mismatch").await;
            return;
        }
        let key_hash = Sha256::digest(hello.node_key.as_bytes()).to_vec();

        // Reconnecting nodes authenticate immediately on their stored key.
        let known = match self.store.environment_for_key(&key_hash) {
            Ok(env) => env,
            Err(_) => {
                reject(&mut sink, "storage error").await;
                return;
            }
        };

        // An enrolled key only speaks for its own node_id; reject before the
        // session swap below so it can't displace another node's socket.
        if known
            .as_ref()
            .is_some_and(|env| env.node_id != hello.node_id)
        {
            reject(&mut sink, "node_key enrolled for a different node_id").await;
            return;
        }

        // Unknown nodes queue as pending; only those count against the cap.
        if known.is_none() {
            let row = match self.store.environment_for_node(&hello.node_id) {
                Ok(row) => row,
                Err(_) => {
                    reject(&mut sink, "storage error").await;
                    return;
                }
            };
            let enrolled = row.filter(|env| !env.credential.is_empty());
            match enrolled {
                // Accepted under this key since the lookup above: bind below.
                Some(env) if env.credential == key_hash => {}
                // A node_id enrolled under a different key can't silently
                // become pending; the env must be revoked first.
                Some(_) => {
                    reject(&mut sink, "node_id enrolled with a different key").await;
                    return;
                }
                None => {
                    if !self.register_pending(&mut sink, &hello, &key_hash).await {
                        return;
                    }
                }
            }
        }

        let (tx, rx) = mpsc::channel::<Message>(64);
        // Out-of-band kill switch: wakes writer and reader to drop the
        // socket when the bounded frame queue can't carry a Close/Cancel.
        let (kill, mut kill_writer) = watch::channel(false);
        let mut kill_reader = kill.subscribe();
        let session = Arc::new(Session {
            connected_at: now(),
            bound: Mutex::new(None),
            key_hash,
            hello,
            tx,
            kill,
            requests: DashMap::new(),
            next_id: AtomicU64::new(0),
        });

        // A redial replaces any older socket for this node_id, then binds
        // from the stored row (or stays pending) in the same critical section.
        let bound = {
            let _route = self.route_lock.lock().unwrap();
            if let Some(old) = self
                .sessions
                .insert(session.hello.node_id.clone(), session.clone())
            {
                old.kill.send_replace(true);
            }
            self.reconcile_locked(&session)
        };
        if !bound {
            send_frame(&session, &PrimaryFrame::Pending);
        }

        // Writer: outbound frames plus a protocol Ping every 30s; any send
        // failure marks the session dead.
        let (dead_tx, mut dead_rx) = watch::channel(false);
        let writer = tokio::spawn(async move {
            let mut rx = rx;
            let mut ping = tokio::time::interval(PING_INTERVAL);
            ping.tick().await; // first tick is immediate; skip it
            loop {
                let closing = tokio::select! {
                    _ = kill_writer.changed() => true,
                    _ = ping.tick() => {
                        let text = serde_json::to_string(&PrimaryFrame::Ping).unwrap();
                        sink.send(Message::Text(text.into())).await.is_err()
                    }
                    message = rx.recv() => match message {
                        Some(message) => {
                            let close = matches!(message, Message::Close(_));
                            let failed = sink.send(message).await.is_err();
                            close || failed
                        }
                        None => true,
                    },
                };
                if closing {
                    break;
                }
            }
            dead_tx.send_replace(true);
        });

        // Reader: `Result` frames resolve in-flight calls; `Pong` and the
        // socket-level Ping/Pong only count as liveness.
        loop {
            tokio::select! {
                _ = dead_rx.changed() => break,
                _ = kill_reader.changed() => break,
                _ = tokio::time::sleep(IDLE_TIMEOUT) => break,
                message = stream.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<NodeFrame>(&text) {
                            Ok(NodeFrame::Result { id, reply }) => {
                                if let Some((_, (_, waiter))) = session.requests.remove(&id) {
                                    let _ = waiter.send(reply);
                                }
                            }
                            Ok(NodeFrame::Pong) => {}
                            Err(_) => {}
                        }
                    }
                    Some(Ok(Message::Ping(bytes))) => {
                        let _ = session.tx.try_send(Message::Pong(bytes));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                },
            }
        }

        writer.abort();
        self.disconnect(&session);
    }

    /// Records an unknown node as pending. Returns false after refusing the
    /// socket. A capacity refusal closes without `Rejected`, so the node
    /// keeps redialing with backoff until a slot opens.
    async fn register_pending(
        &self,
        sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
        hello: &Hello,
        key_hash: &[u8],
    ) -> bool {
        // A live pending socket for this node_id pinned a different key;
        // the newcomer must not displace it (deny or TTL clears the slot).
        if self
            .sessions
            .get(&hello.node_id)
            .is_some_and(|s| s.key_hash != key_hash)
        {
            reject(sink, "node_id pending with a different key").await;
            return false;
        }
        if self.pending_count() >= MAX_PENDING {
            let _ = sink.send(Message::Close(None)).await;
            return false;
        }
        let stored = self.store.upsert_pending_node(
            &hello.node_id,
            &hello.hostname,
            &hello.version,
            key_hash,
            now(),
        );
        match stored {
            Ok(true) => true,
            // The row is pinned to another pending key, or was accepted in
            // the meantime. Only an accept of this key may proceed, so what
            // the operator reviewed is what gets enrolled.
            Ok(false) => {
                let accepted = self
                    .store
                    .environment_for_node(&hello.node_id)
                    .ok()
                    .flatten()
                    .is_some_and(|env| env.credential == key_hash);
                if !accepted {
                    reject(sink, "node_id pending with a different key").await;
                }
                accepted
            }
            Err(_) => {
                reject(sink, "storage error").await;
                false
            }
        }
    }

    /// Accepted environments for the environments list.
    pub fn environments(&self) -> Vec<EnvironmentInfo> {
        self.store
            .environments()
            .unwrap_or_else(|error| {
                eprintln!("environments list failed: {error}");
                Vec::new()
            })
            .into_iter()
            .filter(|env| !env.credential.is_empty())
            .map(|env| EnvironmentInfo {
                online: self.env_sessions.contains_key(&env.name),
                name: env.name,
                node_id: env.node_id,
                version: env.version,
                last_seen: env.last_seen,
            })
            .collect()
    }

    /// Pending nodes (stored rows with empty credential) for the environments
    /// list. Offline pendings stay listed until the sweeper expires them.
    pub fn pending(&self) -> Vec<PendingNode> {
        self.store
            .environments()
            .unwrap_or_else(|error| {
                eprintln!("environments list failed: {error}");
                Vec::new()
            })
            .into_iter()
            .filter(|env| env.credential.is_empty())
            .map(|env| PendingNode {
                hostname: env.hostname.unwrap_or(env.node_id.clone()),
                version: env.version.unwrap_or_default(),
                connected_at: env.last_seen,
                node_id: env.node_id,
            })
            .collect()
    }

    /// Whether `name` is a known (accepted) environment.
    pub fn environment_exists(&self, name: &str) -> bool {
        self.store
            .environment(name)
            .ok()
            .flatten()
            .is_some_and(|env| !env.credential.is_empty())
    }

    /// Accepts a pending node as environment `name` (default: its hostname).
    /// Delivers `Invited` immediately when the node is connected.
    pub async fn accept(&self, node_id: &str, name: Option<&str>) -> Result<(), NodesError> {
        accept_row(&*self.store, node_id, name)?;
        let _route = self.route_lock.lock().unwrap();
        self.reconcile_node_locked(node_id);
        Ok(())
    }

    /// Revokes an environment; its node goes back to pending on redial.
    pub async fn revoke(&self, name: &str) -> Result<(), NodesError> {
        let _route = self.route_lock.lock().unwrap();
        let env = revoke_row(&*self.store, name)?;
        self.reconcile_node_locked(&env.node_id);
        Ok(())
    }

    /// Discards a pending node: drops the stored row and closes its socket
    /// so a node_id pinned by a different key can re-enroll.
    pub async fn deny(&self, node_id: &str) -> Result<(), NodesError> {
        let _route = self.route_lock.lock().unwrap();
        deny_row(&*self.store, node_id)?;
        self.reconcile_node_locked(node_id);
        Ok(())
    }

    /// Renames an environment and moves its live route.
    pub async fn rename(&self, name: &str, to: &str) -> Result<(), NodesError> {
        let _route = self.route_lock.lock().unwrap();
        let env = rename_row(&*self.store, name, to)?;
        self.reconcile_node_locked(&env.node_id);
        Ok(())
    }

    /// Env route identity: `Some(enrollment)` of the enrollment currently
    /// routed, `None` when unrouted. `changed().await` fires on any
    /// transition including re-enrollment under the same name.
    pub fn availability(&self, env: &str) -> watch::Receiver<Option<Vec<u8>>> {
        self.env_sender(env).subscribe()
    }

    /// Absolute path of `workspace` on the env's node
    /// (`<workspaces>/<workspace>` as the node reported it) — used for the
    /// model's working-directory line. Falls back to the bare workspace name
    /// when no session is live.
    pub fn workspace_root(&self, env: &str, workspace: &str) -> std::path::PathBuf {
        self.env_sessions
            .get(env)
            .filter(|session| !session.hello.workspaces.is_empty())
            .map(|session| std::path::Path::new(&session.hello.workspaces).join(workspace))
            .unwrap_or_else(|| std::path::PathBuf::from(workspace))
    }

    fn pending_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|entry| entry.value().bound.lock().unwrap().is_none())
            .count()
    }

    /// The lazily-created liveness sender for an env name.
    fn env_sender(&self, env: &str) -> watch::Sender<Option<Vec<u8>>> {
        self.env_watch
            .entry(env.to_owned())
            .or_insert_with(|| watch::channel(None).0)
            .clone()
    }

    /// Brings one session in line with its stored row. Every route change
    /// goes through here, so accept, revoke, deny, rename (from the web or
    /// the CLI, which bypasses this registry) and redials converge the same
    /// way. A session whose row is accepted under its key holds the route
    /// for that row's name, publishing the row's enrollment. Any other
    /// session loses its route and is killed, unless it is a pending node
    /// still inside its TTL. Returns whether the session is bound.
    /// Caller holds `route_lock`.
    fn reconcile_locked(&self, session: &Arc<Session>) -> bool {
        let node_id = &session.hello.node_id;
        let current = !*session.kill.borrow()
            && self
                .sessions
                .get(node_id)
                .is_some_and(|s| Arc::ptr_eq(&s, session));
        let row = if current {
            match self.store.environment_for_node(node_id) {
                Ok(row) => row,
                // Leave routing as is; the next sweep retries.
                Err(_) => return session.bound.lock().unwrap().is_some(),
            }
        } else {
            None
        };
        let accepted = row
            .as_ref()
            .filter(|env| !env.credential.is_empty() && env.credential == session.key_hash);

        let mut bound = session.bound.lock().unwrap();
        if let Some(previous) = bound.take_if(|name| accepted.is_none_or(|env| env.name != *name)) {
            self.unroute(&previous, session);
        }
        let Some(env) = accepted else {
            let waiting = row.as_ref().is_some_and(|env| env.credential.is_empty())
                && now() - session.connected_at <= PENDING_TTL;
            if !waiting {
                session.kill.send_replace(true);
            }
            return false;
        };
        let newly_bound = bound.is_none();
        *bound = Some(env.name.clone());
        drop(bound);

        // A route left under this name by a session whose row moved is
        // replaced, not killed; that session's own reconcile rebinds it.
        let routed = self
            .env_sessions
            .get(&env.name)
            .is_some_and(|s| Arc::ptr_eq(&s, session));
        if !routed {
            self.env_sessions.insert(env.name.clone(), session.clone());
        }
        // Wake subscribers only on a real change: a new route or a
        // same-name re-accept's fresh enrollment.
        self.env_sender(&env.name).send_if_modified(|published| {
            let changed = !routed || published.as_ref() != Some(&env.enrollment);
            if changed {
                *published = Some(env.enrollment.clone());
            }
            changed
        });
        if newly_bound {
            send_frame(
                session,
                &PrimaryFrame::Invited {
                    name: env.name.clone(),
                },
            );
            let _ = self
                .store
                .touch_environment(node_id, &session.hello.version, now());
        }
        true
    }

    /// `reconcile_locked` for the live session of `node_id`, if any.
    fn reconcile_node_locked(&self, node_id: &str) {
        if let Some(session) = self.sessions.get(node_id).map(|s| s.clone()) {
            self.reconcile_locked(&session);
        }
    }

    /// Drops `name`'s route if `session` holds it and publishes it offline.
    fn unroute(&self, name: &str, session: &Arc<Session>) {
        if self
            .env_sessions
            .remove_if(name, |_, s| Arc::ptr_eq(s, session))
            .is_some()
        {
            self.env_sender(name).send_replace(None);
        }
    }

    /// Socket teardown: drop the session, its route, and its in-flight
    /// calls. Shares route_lock with `call`'s waiter registration so no
    /// waiter is stranded on a dead socket.
    fn disconnect(&self, session: &Arc<Session>) {
        self.sessions
            .remove_if(&session.hello.node_id, |_, s| Arc::ptr_eq(s, session));
        let _route = self.route_lock.lock().unwrap();
        if let Some(name) = session.bound.lock().unwrap().take() {
            self.unroute(&name, session);
        }
        session.requests.clear();
    }

    /// One sweeper pass: expire stale pending rows, reconcile every session
    /// against the store, and fail requests past their TTL.
    fn sweep(&self) {
        let _ = self.store.expire_pending_nodes(now() - PENDING_TTL);
        // Collected first: taking route_lock while holding a shard guard
        // could deadlock.
        let sessions: Vec<_> = self.sessions.iter().map(|e| e.value().clone()).collect();
        for session in &sessions {
            let _route = self.route_lock.lock().unwrap();
            self.reconcile_locked(session);
        }

        // Fail in-flight requests past TTL; the node aborts the work.
        // Kills are deferred past the `sessions` iteration — taking
        // route_lock while holding a shard guard could deadlock.
        let mut kill = Vec::new();
        for entry in self.sessions.iter() {
            let session = entry.value();
            let mut expired = Vec::new();
            for req in session.requests.iter() {
                if req.value().0.elapsed() > REQUEST_TTL {
                    expired.push(*req.key());
                }
            }
            for id in expired {
                if let Some((_, (_, waiter))) = session.requests.remove(&id) {
                    let _ = waiter.send(NodeReply::Error {
                        error: "environment request timed out".into(),
                    });
                    let text = serde_json::to_string(&PrimaryFrame::Cancel { id }).unwrap();
                    // Queue full = writer stuck; Close, then out-of-band
                    // kill so node teardown aborts the work.
                    if session.tx.try_send(Message::Text(text.into())).is_err()
                        && session.tx.try_send(Message::Close(None)).is_err()
                    {
                        kill.push(session.clone());
                    }
                }
            }
        }
        if !kill.is_empty() {
            // Kills go through route_lock like every other forced
            // teardown: serialized against call's waiter registration and
            // authenticate_locked's bind.
            let _route = self.route_lock.lock().unwrap();
            for session in kill {
                session.kill.send_replace(true);
            }
        }
    }
}

// Store-side halves of the admin operations, shared by `NodeRegistry` and
// the `agent environment` CLI. The CLI only writes rows; a running serve's
// sweep reconciles live sessions within one interval. Each write is pinned
// to the row just read (pending key or enrollment), so a concurrent change
// turns it into NotFound instead of touching a row the caller never saw.

/// Promotes a pending node to environment `name` (default: its hostname).
pub fn accept_row<S: EnvironmentStore + ?Sized>(
    store: &S,
    node_id: &str,
    name: Option<&str>,
) -> Result<Environment, NodesError> {
    let pending = pending_row(store, node_id)?;
    let name = match name {
        Some(name) => name.to_owned(),
        None => pending
            .hostname
            .clone()
            .unwrap_or_else(|| pending.node_id.clone()),
    };
    if store.environment(&name)?.is_some() {
        return Err(NodesError::Conflict(format!(
            "environment '{name}' already exists"
        )));
    }
    store
        .accept_pending_node(node_id, &name, pending_key(&pending))?
        .ok_or_else(|| not_pending(node_id))
}

/// Deletes an accepted environment, returning the removed row.
pub fn revoke_row<S: EnvironmentStore + ?Sized>(
    store: &S,
    name: &str,
) -> Result<Environment, NodesError> {
    let env = accepted_row(store, name)?;
    if !store.remove_environment(name, &env.enrollment)? {
        return Err(unknown(name));
    }
    Ok(env)
}

/// Deletes a pending node's row.
pub fn deny_row<S: EnvironmentStore + ?Sized>(store: &S, node_id: &str) -> Result<(), NodesError> {
    let pending = pending_row(store, node_id)?;
    if !store.drop_pending_node(node_id, pending_key(&pending))? {
        return Err(not_pending(node_id));
    }
    Ok(())
}

/// Renames an accepted environment, returning the renamed row.
pub fn rename_row<S: EnvironmentStore + ?Sized>(
    store: &S,
    name: &str,
    to: &str,
) -> Result<Environment, NodesError> {
    if store.environment(to)?.is_some() {
        return Err(NodesError::Conflict(format!(
            "environment '{to}' already exists"
        )));
    }
    let source = accepted_row(store, name)?;
    store
        .rename_environment(name, to, &source.enrollment)?
        .ok_or_else(|| unknown(name))
}

fn accepted_row<S: EnvironmentStore + ?Sized>(
    store: &S,
    name: &str,
) -> Result<Environment, NodesError> {
    store
        .environment(name)?
        .filter(|env| !env.credential.is_empty())
        .ok_or_else(|| unknown(name))
}

fn pending_row<S: EnvironmentStore + ?Sized>(
    store: &S,
    node_id: &str,
) -> Result<Environment, NodesError> {
    store
        .environment_for_node(node_id)?
        .filter(|env| env.credential.is_empty())
        .ok_or_else(|| not_pending(node_id))
}

fn pending_key(pending: &Environment) -> &[u8] {
    pending.pending_key.as_deref().unwrap_or_default()
}

fn unknown(name: &str) -> NodesError {
    NodesError::NotFound(format!("unknown environment '{name}'"))
}

fn not_pending(node_id: &str) -> NodesError {
    NodesError::NotFound(format!("no pending node '{node_id}'"))
}

/// Admin operations on pending nodes and environments.
#[derive(Debug, thiserror::Error)]
pub enum NodesError {
    #[error("{0}")]
    Store(#[from] storage::StoreError),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
}

/// Routes `RemoteEnvironment` calls to the env's live node session.
/// `Arc<NodeRegistry>` satisfies `NodeDirectory` via its blanket impl.
#[async_trait::async_trait]
impl environment_remote::NodeDirectory for NodeRegistry {
    async fn call(
        &self,
        env: &str,
        request: NodeRequest,
        enrollment: Option<&[u8]>,
    ) -> Result<NodeReply, ToolError> {
        let offline = || ToolError::Io(format!("environment '{env}' offline"));
        let Some(session) = self.env_sessions.get(env).map(|s| s.clone()) else {
            return Err(offline());
        };
        let id = session.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        // Validation and waiter registration share one critical section
        // with every route change and kill: the caller pins the enrollment
        // it bound at construction, and CLI revoke/rename bypasses this
        // registry — only a fresh row read inside the lock proves the live
        // session still owns an accepted row under this name AND that it
        // is the pinned enrollment.
        {
            let _route = self.route_lock.lock().unwrap();
            let still_valid = self
                .store
                .environment_for_node(&session.hello.node_id)
                .ok()
                .flatten()
                .is_some_and(|row| {
                    row.name == env
                        && !row.credential.is_empty()
                        && row.credential == session.key_hash
                        && Some(row.enrollment.as_slice()) == enrollment
                });
            if *session.kill.borrow()
                || !still_valid
                || !self
                    .env_sessions
                    .get(env)
                    .is_some_and(|s| Arc::ptr_eq(&s, &session))
            {
                return Err(offline());
            }
            session.requests.insert(id, (Instant::now(), tx));
        }
        let text = serde_json::to_string(&PrimaryFrame::Request { id, request })
            .map_err(|error| ToolError::Io(error.to_string()))?;
        // Dropping the guard (timeout, caller gone) retracts the waiter and
        // asks the node to abort the work.
        let mut guard = RequestGuard {
            session: session.clone(),
            route_lock: self.route_lock.clone(),
            id,
            armed: true,
        };
        if session.tx.send(Message::Text(text.into())).await.is_err() {
            return Err(offline());
        }
        let reply = rx.await.map_err(|_| offline());
        if reply.is_ok() {
            guard.armed = false;
        }
        reply
    }

    fn availability(&self, env: &str) -> watch::Receiver<Option<Vec<u8>>> {
        self.env_sender(env).subscribe()
    }
}

/// Retracts an in-flight request if the caller stops waiting: removes the
/// waiter and sends `Cancel` so the node aborts the work.
struct RequestGuard {
    session: Arc<Session>,
    route_lock: Arc<Mutex<()>>,
    id: u64,
    armed: bool,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.session.requests.remove(&self.id);
        let text = serde_json::to_string(&PrimaryFrame::Cancel { id: self.id }).unwrap();
        // A full outbound queue means the writer is stuck — the cancel can't
        // reach the node. Try Close on the same queue, then kill the socket
        // out of band; node teardown aborts all in-flight work. The kill is
        // serialized with call's waiter registration and every other
        // kill/route mutation under route_lock.
        if self
            .session
            .tx
            .try_send(Message::Text(text.into()))
            .is_err()
            && self.session.tx.try_send(Message::Close(None)).is_err()
        {
            let _route = self.route_lock.lock().unwrap();
            self.session.kill.send_replace(true);
        }
    }
}

/// Serializes a protocol frame onto the session's outbound queue.
fn send_frame(session: &Session, frame: &PrimaryFrame) {
    let Ok(text) = serde_json::to_string(frame) else {
        return;
    };
    let _ = session.tx.try_send(Message::Text(text.into()));
}

/// Pre-session refusal: send `Rejected` then close the socket.
async fn reject(sink: &mut futures_util::stream::SplitSink<WebSocket, Message>, reason: &str) {
    let reason = reason.to_owned();
    let frame = serde_json::to_string(&PrimaryFrame::Rejected { reason }).unwrap();
    let _ = sink.send(Message::Text(frame.into())).await;
    let _ = sink.send(Message::Close(None)).await;
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs() as i64
}
