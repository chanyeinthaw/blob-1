mod oauth;
mod tools;

pub use tools::{Outcome, SessionTools};

use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use environment::ToolSpec;
use rmcp::{
    serve_client,
    service::{Peer, RoleClient, RunningService},
    transport::{
        auth::OAuthClientConfig,
        child_process::TokioChildProcess,
        streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        },
        AuthClient, AuthorizationManager,
    },
};
use storage::{McpServer, McpStore, McpTransport};
use tokio::sync::Mutex;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_BACKOFF: Duration = Duration::from_secs(60);

/// Server ids must stay unambiguous inside `server__tool` wire names.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// OAuth client registration captured at login so `exchange_code` and later
/// token refreshes can rebuild the OAuth client with the same identity.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Registration {
    #[serde(rename = "clientId")]
    pub client_id: String,
    #[serde(rename = "clientSecret", skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(rename = "redirectUri")]
    pub redirect_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<String>,
}

impl From<OAuthClientConfig> for Registration {
    fn from(config: OAuthClientConfig) -> Self {
        Self {
            client_id: config.client_id,
            client_secret: config.client_secret,
            redirect_uri: config.redirect_uri,
            challenge: None,
        }
    }
}

impl From<Registration> for OAuthClientConfig {
    fn from(registration: Registration) -> Self {
        let mut config = OAuthClientConfig::new(registration.client_id, registration.redirect_uri);
        if let Some(secret) = registration.client_secret {
            config = config.with_client_secret(secret);
        }
        config
    }
}

/// One server's view for `mcp.list` and the system prompt.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub id: String,
    pub transport: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub enabled: bool,
    pub auth: &'static str,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<usize>,
}

#[derive(Clone)]
pub struct Mcp {
    inner: Arc<Inner>,
}

struct Inner {
    store: Arc<dyn McpStore>,
    servers: Mutex<BTreeMap<String, Arc<Entry>>>,
    next_conn: AtomicU64,
}

enum ConnectFail {
    Error(String),
    /// `WWW-Authenticate` challenge captured for the login flow.
    Challenge(String),
}

struct Entry {
    config: McpServer,
    state: Mutex<EntryState>,
}

struct Conn {
    service: RunningService<RoleClient, ()>,
    group: Option<Arc<Group>>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Some(group) = &self.group {
            group.kill();
        }
    }
}

/// A stdio server's process group. The id is forgotten once the group has been
/// killed, and the group is killed as soon as its leader exits, so a stale id
/// can never be signalled after the kernel reuses it.
struct Group(std::sync::atomic::AtomicU32);

impl Group {
    fn watch(pgid: u32) -> Arc<Self> {
        let group = Arc::new(Self(std::sync::atomic::AtomicU32::new(pgid)));
        #[cfg(target_os = "linux")]
        {
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pgid as libc::pid_t, 0) };
            if fd >= 0 {
                let fd = unsafe {
                    <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd as i32)
                };
                if let Ok(fd) = unsafe { tokio::io::unix::AsyncFd::register(fd) } {
                    let watched = Arc::downgrade(&group);
                    tokio::spawn(async move {
                        let _ = fd.readable().await;
                        if let Some(group) = watched.upgrade() {
                            group.kill();
                        }
                    });
                }
            }
        }
        group
    }

    fn kill(&self) {
        let pgid = self.0.swap(0, Ordering::SeqCst);
        if pgid != 0 {
            unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGKILL) };
        }
    }
}

struct EntryState {
    /// The shared connection plus its generation id; a caller holds the Arc
    /// across its request so an invalidated connection still finishes.
    conn: Option<(u64, Arc<Conn>)>,
    error: Option<String>,
    needs_login: bool,
    challenge: Option<String>,
    last_failure: Option<Instant>,
    tool_count: Option<usize>,
}

impl Entry {
    fn new(config: McpServer) -> Self {
        Self {
            config,
            state: Mutex::new(EntryState {
                conn: None,
                error: None,
                needs_login: false,
                challenge: None,
                last_failure: None,
                tool_count: None,
            }),
        }
    }
}

/// Why a request against a server failed. Transport-level failures have
/// already invalidated the connection by the time callers see them.
pub(crate) enum RequestFail {
    /// No usable connection (missing, disabled, or connect failed/throttled).
    Peer(String),
    /// The request exceeded its timeout.
    Timeout,
    /// rmcp-level error from the request itself.
    Service(rmcp::ServiceError),
}

impl RequestFail {
    /// A transport failure means an in-flight `tools/call` may or may not
    /// have run: its outcome is unknown and it must not be retried blindly.
    fn outcome_unknown(&self) -> bool {
        match self {
            Self::Timeout => true,
            Self::Service(error) => {
                Mcp::is_transport_error(error) || error.is_authorization_required()
            }
            Self::Peer(_) => false,
        }
    }
}

impl std::fmt::Display for RequestFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Peer(error) => error.fmt(f),
            Self::Service(error) => error.fmt(f),
            Self::Timeout => "request timed out".fmt(f),
        }
    }
}

impl Mcp {
    pub fn new(store: Arc<dyn McpStore>) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                servers: Mutex::new(BTreeMap::new()),
                next_conn: AtomicU64::new(0),
            }),
        }
    }

    /// True when at least one configured server is enabled; gates the
    /// `mcp_*` built-ins.
    pub fn any_enabled(&self) -> bool {
        self.inner
            .store
            .mcp_servers()
            .map(|servers| servers.iter().any(|server| server.enabled))
            .unwrap_or(false)
    }

    /// Reconcile in-memory entries with stored config so add/remove/enable/
    /// disable takes effect on the next call without restarting.
    async fn sync(&self) -> Result<(), String> {
        let configured = self
            .inner
            .store
            .mcp_servers()
            .map_err(|error| error.to_string())?;
        {
            let mut servers = self.inner.servers.lock().await;
            servers.retain(|id, _| configured.iter().any(|server| &server.id == id));
            for server in configured {
                match servers.get_mut(&server.id) {
                    Some(entry) if entry.config == server => {}
                    Some(entry) => *entry = Arc::new(Entry::new(server)),
                    None => {
                        servers.insert(server.id.clone(), Arc::new(Entry::new(server)));
                    }
                }
            }
        }
        Ok(())
    }

    /// Connect an entry. A failed attempt is retried at most once per
    /// RETRY_BACKOFF so a dead server doesn't stall every turn.
    async fn connect(&self, state: &mut EntryState, config: &McpServer) {
        let attempt = async {
            match &config.transport {
                McpTransport::Stdio { command, args, env } => {
                    let mut process = tokio::process::Command::new(command);
                    process
                        .args(args)
                        .env_remove("BLOB_1_APP_KEY")
                        .envs(env)
                        .kill_on_drop(true);
                    let mut process = process_wrap::tokio::CommandWrap::from(process);
                    process.wrap(process_wrap::tokio::ProcessGroup::leader());
                    match TokioChildProcess::new(process) {
                        Ok(transport) => {
                            let group = transport.id().map(Group::watch);
                            serve_client((), transport)
                                .await
                                .map(|service| (service, group))
                                .map_err(|error| ConnectFail::Error(error.to_string()))
                        }
                        Err(error) => Err(ConnectFail::Error(error.to_string())),
                    }
                }
                McpTransport::Http { url, headers } => {
                    match self.http_transport(url, headers, &config.id).await {
                        Ok(transport) => serve_client((), transport)
                            .await
                            .map(|service| (service, None))
                            .map_err(|error| match error.auth_challenge() {
                                Some(challenge) => ConnectFail::Challenge(challenge.to_string()),
                                None => ConnectFail::Error(error.to_string()),
                            }),
                        Err(error) => Err(ConnectFail::Error(error)),
                    }
                }
            }
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
            Ok(Ok((service, group))) => {
                let id = self.inner.next_conn.fetch_add(1, Ordering::Relaxed);
                state.conn = Some((id, Arc::new(Conn { service, group })));
                state.error = None;
                state.needs_login = false;
                state.challenge = None;
                state.last_failure = None;
            }
            Ok(Err(ConnectFail::Challenge(challenge))) => {
                state.needs_login = true;
                state.challenge = Some(challenge);
                state.error = Some("requires OAuth login".to_string());
                state.last_failure = Some(Instant::now());
            }
            Ok(Err(ConnectFail::Error(error))) => {
                state.needs_login = false;
                state.error = Some(error);
                state.last_failure = Some(Instant::now());
            }
            Err(_) => {
                state.needs_login = false;
                state.error = Some("connect timed out".to_string());
                state.last_failure = Some(Instant::now());
            }
        }
    }

    async fn http_transport(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        server: &str,
    ) -> Result<StreamableHttpClientTransport<AuthClient<reqwest::Client>>, String> {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, env_var) in headers {
            let value = std::env::var(env_var)
                .map_err(|_| format!("env var '{env_var}' for header '{name}' is not set"))?;
            map.insert(
                reqwest::header::HeaderName::try_from(name.as_str())
                    .map_err(|error| format!("header '{name}': {error}"))?,
                reqwest::header::HeaderValue::from_str(&value)
                    .map_err(|error| format!("header '{name}': {error}"))?,
            );
        }
        let client = reqwest::Client::builder()
            .default_headers(map)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        let mut manager = AuthorizationManager::new_allowing_private_addresses(url)
            .await
            .map_err(|error| error.to_string())?;
        manager.set_credential_store(oauth::Credentials {
            store: self.inner.store.clone(),
            server: server.to_string(),
        });
        manager.set_state_store(oauth::States {
            store: self.inner.store.clone(),
            server: server.to_string(),
        });
        // A stored registration lets expired tokens refresh without relogin;
        // skip quietly when the server advertises no metadata.
        if let Some(registration) = self.registration(server) {
            if let Ok(resolution) = manager
                .resolve_metadata_from_challenge(registration.challenge.as_deref())
                .await
            {
                manager.set_metadata(resolution.metadata);
                let _ = manager.configure_client(registration.into());
            }
        }
        Ok(StreamableHttpClientTransport::with_client(
            AuthClient::new(client, manager),
            StreamableHttpClientTransportConfig::with_uri(url).reinit_on_expired_session(false),
        ))
    }

    fn registration(&self, server: &str) -> Option<Registration> {
        self.inner
            .store
            .mcp_oauth(server)
            .ok()?
            .1
            .and_then(|value| serde_json::from_value(value).ok())
    }

    /// Ids of configured-and-enabled servers.
    pub(crate) async fn enabled_ids(&self) -> Vec<String> {
        if self.sync().await.is_err() {
            return Vec::new();
        }
        self.inner
            .servers
            .lock()
            .await
            .values()
            .filter(|entry| entry.config.enabled)
            .map(|entry| entry.config.id.clone())
            .collect()
    }

    /// Shared connection for an enabled server, connecting on first use.
    /// Only the per-server lock is held while connecting, so a hung server
    /// never stalls the others.
    pub(crate) async fn peer(&self, server: &str) -> Result<(Arc<Conn>, u64), String> {
        self.sync().await?;
        let entry = {
            self.inner
                .servers
                .lock()
                .await
                .get(server)
                .cloned()
                .ok_or_else(|| format!("no MCP server '{server}'"))?
        };
        if !entry.config.enabled {
            return Err(format!("MCP server '{server}' is disabled"));
        }
        let mut state = entry.state.lock().await;
        if let Some((_, conn)) = &state.conn {
            if !conn.service.is_closed() && !conn.service.peer().is_transport_closed() {
                return Ok((conn.clone(), state.conn.as_ref().unwrap().0));
            }
            if let Some((_, conn)) = state.conn.take() {
                conn.service.cancellation_token().cancel();
            }
        }
        let throttled = state
            .last_failure
            .is_some_and(|at| at.elapsed() < RETRY_BACKOFF);
        if throttled {
            return Err(format!(
                "MCP server '{server}' unavailable: {}",
                state.error.as_deref().unwrap_or("connect failed")
            ));
        }
        self.connect(&mut state, &entry.config).await;
        match &state.conn {
            Some((id, conn)) => Ok((conn.clone(), *id)),
            None => Err(format!(
                "MCP server '{server}' unavailable: {}",
                state.error.as_deref().unwrap_or("connect failed")
            )),
        }
    }

    /// Run `request` against the server's shared connection under one
    /// deadline that also covers connection acquisition; a request that
    /// expires after acquisition invalidates the connection. A transport
    /// failure invalidates the connection — but only if the map still holds
    /// the same generation, so a stale failure can't clear a newer
    /// connection.
    pub(crate) async fn request<T, Fut>(
        &self,
        server: &str,
        timeout: Duration,
        request: impl FnOnce(Peer<RoleClient>) -> Fut,
    ) -> Result<T, RequestFail>
    where
        Fut: std::future::Future<Output = Result<T, rmcp::ServiceError>>,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        let (conn, conn_id) = match tokio::time::timeout_at(deadline, self.peer(server)).await {
            Ok(result) => result.map_err(RequestFail::Peer)?,
            Err(_) => return Err(RequestFail::Timeout),
        };
        let result =
            match tokio::time::timeout_at(deadline, request(conn.service.peer().clone())).await {
                Ok(result) => result.map_err(RequestFail::Service),
                Err(_) => Err(RequestFail::Timeout),
            };
        if let Err(fail) = &result {
            let invalidate = match fail {
                RequestFail::Timeout => true,
                RequestFail::Service(error) => {
                    Self::is_transport_error(error) || error.is_authorization_required()
                }
                RequestFail::Peer(_) => false,
            };
            if invalidate {
                self.drop_conn(server, conn_id, fail).await;
            }
        }
        result
    }

    /// Drop a dead connection so the next call reconnects, and remember the
    /// failure (including a fresh auth challenge for the login flow).
    async fn drop_conn(&self, server: &str, conn_id: u64, fail: &RequestFail) {
        let Some(entry) = self.inner.servers.lock().await.get(server).cloned() else {
            return;
        };
        let mut state = entry.state.lock().await;
        if state.conn.as_ref().map(|(id, _)| *id) != Some(conn_id) {
            return;
        }
        if let Some((_, conn)) = state.conn.take() {
            conn.service.cancellation_token().cancel();
        }
        state.last_failure = None;
        if let RequestFail::Service(error) = fail {
            if let Some(challenge) = error.auth_challenge() {
                state.needs_login = true;
                state.challenge = Some(challenge.to_string());
            }
        }
        state.error = Some(fail.to_string());
    }

    fn is_transport_error(error: &rmcp::ServiceError) -> bool {
        matches!(
            error,
            rmcp::ServiceError::TransportSend(_)
                | rmcp::ServiceError::TransportClosed
                | rmcp::ServiceError::Cancelled { .. }
        )
    }

    /// Call `tool` on `server`. No automatic retry: a transport drop mid-call
    /// leaves the outcome unknown, so the failure is reported instead.
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: &str,
    ) -> Result<rmcp::model::CallToolResult, String> {
        let params = rmcp::model::CallToolRequestParams::new(tool.to_string()).with_arguments(
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(arguments)
                .map_err(|error| format!("arguments must be a JSON object: {error}"))?,
        );
        self.request(server, CALL_TIMEOUT, move |peer| async move {
            peer.call_tool(params).await
        })
        .await
        .map_err(|fail| match fail {
            RequestFail::Peer(error) => error,
            fail if fail.outcome_unknown() => {
                if matches!(fail, RequestFail::Service(error) if error.is_authorization_required())
                {
                    format!("MCP server '{server}' requires login (agent mcp login {server})")
                } else {
                    format!("MCP server '{server}' disconnected; the tool call outcome is unknown")
                }
            }
            fail => fail.to_string(),
        })
    }

    /// Attempts to connect a single configured server so Settings shows live
    /// status (`connected`, `needs-login`, or `error`) immediately after adding
    /// or enabling it.
    pub async fn probe(&self, id: &str) {
        if self.sync().await.is_ok() {
            let _ = self.peer(id).await;
        }
    }

    /// Probes enabled servers that are not yet connected (in parallel, obeying
    /// `RETRY_BACKOFF` so dead servers don't stall repeated status checks).
    pub async fn probe_enabled(&self) {
        if self.sync().await.is_err() {
            return;
        }
        let entries: Vec<Arc<Entry>> = self
            .inner
            .servers
            .lock()
            .await
            .values()
            .filter(|entry| entry.config.enabled)
            .cloned()
            .collect();
        let mut join = tokio::task::JoinSet::new();
        for entry in entries {
            let should_probe = entry
                .state
                .try_lock()
                .is_ok_and(|state| state.conn.is_none() && !state.needs_login);
            if should_probe {
                let mcp = self.clone();
                let id = entry.config.id.clone();
                join.spawn(async move {
                    let _ = mcp.peer(&id).await;
                });
            }
        }
        while join.join_next().await.is_some() {}
    }

    /// Servers configured in storage, with live status for `mcp.list`.
    pub async fn server_statuses(&self) -> Vec<ServerStatus> {
        if self.sync().await.is_err() {
            return Vec::new();
        }
        let entries: Vec<Arc<Entry>> = self.inner.servers.lock().await.values().cloned().collect();
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let (transport, command, args, url) = match &entry.config.transport {
                McpTransport::Stdio { command, args, .. } => {
                    ("stdio", Some(command.clone()), Some(args.clone()), None)
                }
                McpTransport::Http { url, .. } => ("http", None, None, Some(url.clone())),
            };
            let Ok(mut state) = entry.state.try_lock() else {
                let auth = match &entry.config.transport {
                    McpTransport::Http { headers, .. } if !headers.is_empty() => "headers",
                    _ if self.registration(&entry.config.id).is_some()
                        || self.credentials(&entry.config.id).is_some() =>
                    {
                        "oauth"
                    }
                    _ => "none",
                };
                out.push(ServerStatus {
                    id: entry.config.id.clone(),
                    transport,
                    command,
                    args,
                    url,
                    enabled: entry.config.enabled,
                    auth,
                    status: "connecting",
                    error: None,
                    tool_count: None,
                });
                continue;
            };
            let auth = match &entry.config.transport {
                McpTransport::Http { headers, .. } if !headers.is_empty() => "headers",
                _ if state.needs_login
                    || self.registration(&entry.config.id).is_some()
                    || self.credentials(&entry.config.id).is_some() =>
                {
                    "oauth"
                }
                _ => "none",
            };
            let mut status = if state.conn.is_some() {
                "connected"
            } else if !entry.config.enabled {
                "disconnected"
            } else if state.needs_login {
                "needs-login"
            } else if state.error.is_some() {
                "error"
            } else {
                "idle"
            };
            let mut tool_count = None;
            if let Some((_, conn)) = &state.conn {
                match tokio::time::timeout(STATUS_TIMEOUT, conn.service.peer().list_all_tools())
                    .await
                {
                    Ok(Ok(tools)) => {
                        state.tool_count = Some(tools.len());
                        tool_count = Some(tools.len());
                    }
                    Ok(Err(error)) => {
                        if Self::is_transport_error(&error) || error.is_authorization_required() {
                            if let Some((_, conn)) = state.conn.take() {
                                conn.service.cancellation_token().cancel();
                            }
                            state.last_failure = None;
                            if let Some(challenge) = error.auth_challenge() {
                                state.needs_login = true;
                                state.challenge = Some(challenge.to_string());
                            }
                        }
                        state.error = Some(error.to_string());
                        status = "error";
                        tool_count = state.tool_count;
                    }
                    Err(_) => tool_count = state.tool_count,
                }
            }
            out.push(ServerStatus {
                id: entry.config.id.clone(),
                transport,
                command,
                args,
                url,
                enabled: entry.config.enabled,
                auth,
                status,
                error: state.error.clone(),
                tool_count,
            });
        }
        out
    }

    fn credentials(&self, server: &str) -> Option<serde_json::Value> {
        self.inner.store.mcp_oauth(server).ok()?.0
    }

    /// `(id, one-line description)` for each enabled server: instructions or
    /// title when the server provides them, else its tool count. Connects
    /// lazily and in parallel; failures are reported as unavailable.
    pub async fn describe(&self) -> Vec<(String, String)> {
        if self.sync().await.is_err() {
            return Vec::new();
        }
        let ids: Vec<String> = {
            self.inner
                .servers
                .lock()
                .await
                .values()
                .filter(|entry| entry.config.enabled)
                .map(|entry| entry.config.id.clone())
                .collect()
        };
        let mut join = tokio::task::JoinSet::new();
        for id in ids.clone() {
            let mcp = self.clone();
            join.spawn(async move {
                let description = match mcp.peer(&id).await {
                    Ok((conn, _)) => {
                        let info = conn.service.peer_info();
                        let description = info
                            .and_then(|info| {
                                info.instructions.clone().or_else(|| {
                                    info.server_info.as_ref().and_then(|server| {
                                        server.title.clone().or_else(|| Some(server.name.clone()))
                                    })
                                })
                            })
                            .filter(|text| !text.is_empty());
                        match description {
                            Some(description) => description,
                            None => match mcp
                                .request(&id, REQUEST_TIMEOUT, |peer| async move {
                                    peer.list_all_tools().await
                                })
                                .await
                            {
                                Ok(tools) => format!("{} tools", tools.len()),
                                Err(error) => format!("unavailable: {error}"),
                            },
                        }
                    }
                    Err(error) => error,
                };
                (id, description)
            });
        }
        let mut results = BTreeMap::new();
        while let Some(Ok((id, description))) = join.join_next().await {
            results.insert(id, description);
        }
        ids.iter()
            .filter_map(|id| results.get(id).map(|d| (id.clone(), d.clone())))
            .collect()
    }

    /// Tools of a server for `agent mcp tools`.
    pub async fn tools(&self, server: &str) -> Result<Vec<(String, String)>, String> {
        self.request(server, REQUEST_TIMEOUT, |peer| async move {
            peer.list_all_tools().await
        })
        .await
        .map(|tools| {
            tools
                .into_iter()
                .map(|tool| {
                    (
                        tool.name.to_string(),
                        tool.description.as_deref().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .map_err(|error| error.to_string())
    }

    /// Kill stdio children and close HTTP clients at shutdown.
    pub async fn shutdown(&self) {
        let servers: Vec<Arc<Entry>> = self.inner.servers.lock().await.values().cloned().collect();
        for entry in servers {
            if let Some((_, conn)) = entry.state.lock().await.conn.take() {
                if let Some(group) = &conn.group {
                    group.kill();
                }
                match Arc::try_unwrap(conn) {
                    Ok(mut conn) => {
                        let _ = conn.service.close().await;
                    }
                    Err(conn) => conn.service.cancellation_token().cancel(),
                }
            }
        }
    }

    /// `server__tool` wire name for a tool; sanitizes and appends a hash
    /// suffix only when two tools sanitize to the same wire name.
    pub(crate) fn wire_names(
        server: &str,
        tools: &[rmcp::model::Tool],
    ) -> BTreeMap<String, rmcp::model::Tool> {
        fn sanitize(text: &str) -> String {
            text.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        }
        let mut out = BTreeMap::new();
        for tool in tools {
            let mut wire = format!("{server}__{}", sanitize(&tool.name));
            if out.contains_key(&wire) {
                wire = format!(
                    "{wire}-{:x}",
                    tool.name.bytes().fold(0u32, |hash, byte| {
                        hash.wrapping_mul(31).wrapping_add(byte as u32)
                    })
                );
            }
            out.insert(wire, tool.clone());
        }
        out
    }

    /// A server's current tools through the shared connection. `None` when
    /// the server is unreachable, so callers keep their captured specs.
    pub(crate) async fn tool_list(
        &self,
        server: &str,
        timeout: Duration,
    ) -> Option<Vec<rmcp::model::Tool>> {
        self.request(server, timeout, |peer| async move {
            peer.list_all_tools().await
        })
        .await
        .ok()
    }
}

/// ToolSpec for an rmcp Tool under its wire name.
pub(crate) fn spec_for(wire: &str, tool: &rmcp::model::Tool) -> ToolSpec {
    ToolSpec {
        name: Cow::Owned(wire.to_string()),
        description: Cow::Owned(
            tool.description
                .as_deref()
                .or(tool.title.as_deref())
                .unwrap_or_default()
                .to_string(),
        ),
        input_schema: Cow::Owned(
            serde_json::to_string(&tool.input_schema).unwrap_or_else(|_| "{}".to_string()),
        ),
    }
}
