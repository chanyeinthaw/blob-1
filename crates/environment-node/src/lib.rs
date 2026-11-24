//! `blob-1 node` client: dials out to a primary's `/ws/node` socket, presents
//! a `hello`, waits to be accepted as an environment, then serves environment
//! requests with cached per-workspace `LocalEnvironment`s. Reconnect/backoff
//! is the caller's job — `run_session` returns when the socket dies.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use environment::node::{
    Hello, NodeFrame, NodeReply, NodeRequest, PrimaryFrame, IDLE_TIMEOUT, MAX_FRAME_BYTES,
    NODE_PROTOCOL_VERSION,
};
use environment::{Environment, ToolError};
use environment_local::LocalEnvironment;
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("websocket: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid node config: {0}")]
    Config(String),
    /// Terminal refusal from the primary (e.g. protocol mismatch); the caller
    /// should not retry with the same credentials.
    #[error("rejected by primary: {0}")]
    Rejected(String),
}

/// Node identity and target. `node_id`/`node_key` are generated once and
/// persisted in `node.toml`; `hostname`/`version` are filled at load time.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Primary base URL (`http(s)` or `ws(s)`); `run_session` appends `/ws/node`.
    pub url: String,
    pub node_id: String,
    pub node_key: String,
    pub hostname: String,
    /// Binary version reported to the primary; callers should overwrite with
    /// their release version after `load_or_create`.
    pub version: String,
    /// Root under which each request's `workspace` gets a directory.
    pub workspaces: PathBuf,
}

/// The portion of `NodeConfig` stored in `node.toml`.
#[derive(Serialize, Deserialize)]
struct FileConfig {
    #[serde(default)]
    url: String,
    node_id: String,
    node_key: String,
    #[serde(default)]
    workspaces: Option<PathBuf>,
}

/// Loads `path` (`~/.blob-1/node.toml` by convention), creating it with fresh
/// random `node_id`/`node_key` when absent. The file is written 0600.
/// `workspaces` overrides the stored/default workspaces root.
pub fn load_or_create(path: &Path, workspaces: Option<PathBuf>) -> Result<NodeConfig, NodeError> {
    let default_workspaces = home_dir()?.join(".blob-1/workspaces");
    let file = match std::fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str::<FileConfig>(&text).map_err(|e| NodeError::Config(e.to_string()))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let file = FileConfig {
                url: String::new(),
                node_id: random_hex(16),
                node_key: random_hex(32),
                workspaces: workspaces.clone().or(Some(default_workspaces.clone())),
            };
            write_config(path, &file)?;
            file
        }
        Err(error) => return Err(error.into()),
    };
    Ok(NodeConfig {
        url: file.url,
        node_id: file.node_id,
        node_key: file.node_key,
        hostname: hostname(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        workspaces: workspaces.or(file.workspaces).unwrap_or(default_workspaces),
    })
}

fn write_config(path: &Path, file: &FileConfig) -> Result<(), NodeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string(file).map_err(|e| NodeError::Config(e.to_string()))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    std::io::Write::write_all(&mut options.open(path)?, text.as_bytes())?;
    Ok(())
}

fn home_dir() -> Result<PathBuf, NodeError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| NodeError::Config("HOME is not set".into()))
}

fn hostname() -> String {
    nix::unistd::gethostname()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "node".to_owned())
}

fn random_hex(bytes: usize) -> String {
    let mut data = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut data);
    hex::encode(data)
}

/// `blob-1 node` subcommand options.
#[derive(Debug, Clone)]
pub struct NodeOptions {
    /// Primary URL (`http(s)` or `ws(s)` base, or full `/ws/node` path).
    pub url: String,
    /// Workspaces root override (default `~/.blob-1/workspaces`).
    pub workspaces: Option<PathBuf>,
}

/// Node entry point: loads `~/.blob-1/node.toml`, then redials the primary
/// forever with exponential backoff (capped at 30s). A `rejected` hello is
/// terminal and returns an error.
pub async fn run(options: NodeOptions) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = home_dir()?.join(".blob-1/node.toml");
    let mut config = load_or_create(&path, options.workspaces)?;
    // Absolute once, so `hello` advertises the same root the executor uses.
    config.workspaces = std::path::absolute(&config.workspaces)?;
    if !options.url.is_empty() {
        config.url = options.url;
    }
    let url = url::Url::parse(&config.url)
        .map_err(|error| format!("invalid --url '{}': {error}", config.url))?;
    // Env cache and the skills FIFO outlive a single session: a dropped
    // socket's started apply keeps its place in the node-wide order.
    let shared = Arc::new(NodeShared::new(config.workspaces.clone()));
    let mut backoff = std::time::Duration::from_secs(1);
    loop {
        match run_session(&url, &config, &shared).await {
            Err(NodeError::Rejected(reason)) => return Err(format!("rejected: {reason}").into()),
            Ok(()) => eprintln!("connection closed; reconnecting in {}s", backoff.as_secs()),
            Err(error) => eprintln!("{error}; retrying in {}s", backoff.as_secs()),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
    }
}

/// A skills request and the session channel its reply goes to. `cancelled`
/// lets a `Cancel` frame drop the job while it still waits in the queue —
/// once the worker starts it, it runs to completion.
struct SkillsJob {
    id: u64,
    request: NodeRequest,
    out: mpsc::UnboundedSender<NodeFrame>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// The session's queued-job flags — the worker removes itself on
    /// completion so a long-lived socket doesn't accumulate entries.
    jobs: Arc<Mutex<HashMap<u64, Arc<std::sync::atomic::AtomicBool>>>>,
}

/// Node state that outlives one session: the per-workspace env cache plus a
/// single FIFO worker for managed-skills ops. Those ops mutate a node-wide
/// directory, so they run strictly in arrival order across redials — a
/// started apply finishes before the next session's manifest read, never
/// aborted mid-write (which would mark a managed skill "user-owned" on the
/// next sync).
pub struct NodeShared {
    envs: Arc<Mutex<HashMap<String, Arc<LocalEnvironment>>>>,
    skills: mpsc::Sender<SkillsJob>,
}

/// Pending skills jobs beyond this depth reject with a busy error; a wedged
/// apply can't grow memory or starve the queue silently.
const SKILLS_QUEUE: usize = 64;

impl NodeShared {
    fn new(root: PathBuf) -> Self {
        let envs = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::channel::<SkillsJob>(SKILLS_QUEUE);
        tokio::spawn({
            let envs = envs.clone();
            async move {
                while let Some(job) = rx.recv().await {
                    if !job.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                        let reply = dispatch(&root, &envs, job.request).await;
                        let _ = job.out.send(NodeFrame::Result { id: job.id, reply });
                    }
                    job.jobs.lock().unwrap().remove(&job.id);
                }
            }
        });
        Self { envs, skills: tx }
    }

    /// Queues a skills op in node-wide arrival order. The primary drops its
    /// waiter on timeout/cancel — a started op still runs to completion.
    /// Replies immediately with a busy error when the queue is full.
    fn submit(
        &self,
        id: u64,
        request: NodeRequest,
        out: mpsc::UnboundedSender<NodeFrame>,
        jobs: &Arc<Mutex<HashMap<u64, Arc<std::sync::atomic::AtomicBool>>>>,
    ) {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Register before try_send: the worker can finish a job before
        // submit returns, so it must find the entry to remove.
        jobs.lock().unwrap().insert(id, cancelled.clone());
        if self
            .skills
            .try_send(SkillsJob {
                id,
                request,
                out: out.clone(),
                cancelled,
                jobs: jobs.clone(),
            })
            .is_err()
        {
            jobs.lock().unwrap().remove(&id);
            let _ = out.send(NodeFrame::Result {
                id,
                reply: NodeReply::Error {
                    error: "node skills queue busy".into(),
                },
            });
        }
    }
}

/// Runs one node session against the primary at `url` (http(s) or ws(s) base)
/// until the socket closes. Sends `hello`, then:
/// `pending` → keep waiting, `invited` → serve requests, `rejected` → error
/// out so the caller can decide whether a retry makes sense.
/// `shared` carries the env cache and skills FIFO across redials.
pub async fn run_session(
    url: &url::Url,
    config: &NodeConfig,
    shared: &Arc<NodeShared>,
) -> Result<(), NodeError> {
    let mut socket = url.clone();
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        scheme => scheme,
    };
    socket
        .set_scheme(scheme)
        .map_err(|_| NodeError::Config(format!("bad url scheme '{scheme}'")))?;
    socket.set_path("/ws/node");

    let limits = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_frame_size(Some(MAX_FRAME_BYTES))
        .max_message_size(Some(MAX_FRAME_BYTES));
    let (stream, _) =
        tokio_tungstenite::connect_async_with_config(socket.as_str(), Some(limits), false).await?;
    let (mut sink, mut incoming) = stream.split();

    sink.send(Message::Text(
        serde_json::to_string(&Hello {
            node_id: config.node_id.clone(),
            hostname: config.hostname.clone(),
            version: config.version.clone(),
            protocol: NODE_PROTOCOL_VERSION,
            node_key: config.node_key.clone(),
            workspaces: config.workspaces.display().to_string(),
        })?
        .into(),
    ))
    .await?;

    // Requests run concurrently; replies funnel through one writer task so
    // the socket sink stays single-owner.
    let (out, mut frames) = mpsc::unbounded_channel::<NodeFrame>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = frames.recv().await {
            let Ok(text) = serde_json::to_string(&frame) else {
                continue;
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // Per-request tasks so `Cancel` and session teardown can abort work;
    // bash children die via LocalEnvironment's process-group guard. Skills
    // ops go to the shared FIFO worker instead — `skill_jobs` lets a
    // `Cancel` drop one still waiting in the queue.
    let tasks = Arc::new(Mutex::new(
        HashMap::<u64, tokio::task::JoinHandle<()>>::new(),
    ));
    let skill_jobs = Arc::new(Mutex::new(
        HashMap::<u64, Arc<std::sync::atomic::AtomicBool>>::new(),
    ));

    let mut result = Ok(());
    // Silence past IDLE_TIMEOUT (the primary pings every 30s) ends the
    // session so the node redials instead of waiting on a dead link.
    while let Ok(Some(message)) = tokio::time::timeout(IDLE_TIMEOUT, incoming.next()).await {
        let text = match message {
            Ok(Message::Text(text)) => text,
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Binary(_)) => continue,
            Ok(Message::Close(_)) | Ok(Message::Frame(_)) | Err(_) => break,
        };
        let Ok(frame) = serde_json::from_str::<PrimaryFrame>(&text) else {
            continue;
        };
        match frame {
            PrimaryFrame::Pending => eprintln!("waiting for invite"),
            PrimaryFrame::Invited { name } => eprintln!("accepted as environment '{name}'"),
            PrimaryFrame::Rejected { reason } => {
                result = Err(NodeError::Rejected(reason));
                break;
            }
            PrimaryFrame::Ping => {
                let _ = out.send(NodeFrame::Pong);
            }
            PrimaryFrame::Request { id, request } => {
                // Skills ops leave `tasks` entirely — the worker owns them,
                // so Cancel and teardown can't abort a started apply.
                if is_skills(&request) {
                    shared.submit(id, request, out.clone(), &skill_jobs);
                    continue;
                }
                let out = out.clone();
                let envs = shared.envs.clone();
                let root = config.workspaces.clone();
                // Spawn and insert under one lock so a fast-finishing task
                // cannot outrun its registration; finished handles are
                // reaped here since tasks no longer remove themselves.
                let mut tasks = tasks.lock().unwrap();
                tasks.retain(|_, handle| !handle.is_finished());
                let handle = tokio::spawn(async move {
                    let reply = dispatch(&root, &envs, request).await;
                    let _ = out.send(NodeFrame::Result { id, reply });
                });
                tasks.insert(id, handle);
            }
            PrimaryFrame::Cancel { id } => {
                // A queued skills job drops before it starts; a started one
                // (or any Execute task) finishes/aborts as before.
                if let Some(cancelled) = skill_jobs.lock().unwrap().remove(&id) {
                    cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                    continue;
                }
                if let Some(handle) = tasks.lock().unwrap().remove(&id) {
                    handle.abort();
                }
            }
        }
    }
    writer.abort();
    for (_, handle) in tasks.lock().unwrap().drain() {
        handle.abort();
    }
    // Queued-but-unstarted skills jobs must not mutate the node-wide dir
    // after this socket is gone — a started op still runs to completion.
    for (_, flag) in skill_jobs.lock().unwrap().drain() {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    result
}

/// Whether a request mutates or reads the node-wide managed-skills dir.
fn is_skills(request: &NodeRequest) -> bool {
    matches!(
        request,
        NodeRequest::ManagedSkills { .. }
            | NodeRequest::ApplySkills { .. }
            | NodeRequest::SkillCatalog { .. }
    )
}

/// Runs one environment method locally and wraps the outcome as a `NodeReply`.
/// Skills requests reach here only via the shared FIFO worker, so they are
/// inherently serialized node-wide.
async fn dispatch(
    root: &Path,
    envs: &Mutex<HashMap<String, Arc<LocalEnvironment>>>,
    request: NodeRequest,
) -> NodeReply {
    let workspace = match &request {
        NodeRequest::Execute { workspace, .. }
        | NodeRequest::ManagedSkills { workspace }
        | NodeRequest::ApplySkills { workspace, .. }
        | NodeRequest::SkillCatalog { workspace } => workspace.clone(),
    };
    let env = match local_env(root, envs, &workspace).await {
        Ok(env) => env,
        Err(error) => {
            return NodeReply::Error {
                error: error.to_string(),
            };
        }
    };
    let result = match request {
        NodeRequest::Execute { call, .. } => env
            .execute(call)
            .await
            .map(|result| NodeReply::Execute { result }),
        NodeRequest::ManagedSkills { .. } => env
            .managed_skills()
            .await
            .map(|manifest| NodeReply::ManagedSkills { manifest }),
        NodeRequest::ApplySkills { put, remove, .. } => env
            .apply_skills(put, remove)
            .await
            .map(|skipped| NodeReply::ApplySkills { skipped }),
        NodeRequest::SkillCatalog { .. } => env
            .skill_catalog()
            .await
            .map(|entries| NodeReply::SkillCatalog { entries }),
    };
    result.unwrap_or_else(|error| NodeReply::Error {
        error: error.to_string(),
    })
}

/// Per-workspace `LocalEnvironment` cache; `new` canonicalizes the root, so
/// the workspace dir is created first. Skills stay at the `None` default
/// (`~/.agents/skills`) — node-wide, mirroring the primary's shared dir; the
/// primary syncs its single global catalog to every workspace.
async fn local_env(
    root: &Path,
    envs: &Mutex<HashMap<String, Arc<LocalEnvironment>>>,
    workspace: &str,
) -> Result<Arc<LocalEnvironment>, ToolError> {
    if let Some(env) = envs.lock().unwrap().get(workspace) {
        return Ok(env.clone());
    }
    let dir = root.join(workspace);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|error| ToolError::Io(error.to_string()))?;
    let env = Arc::new(LocalEnvironment::new(dir, None)?);
    envs.lock()
        .unwrap()
        .insert(workspace.to_owned(), env.clone());
    Ok(env)
}
