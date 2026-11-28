//! Supervision of the `blob-1-browser` companion: one shared child process
//! per `serve`, connected over a Unix socket speaking `browser_protocol`.
//! Serve enforces tab ownership here: a task can only touch tabs it opened.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use browser_protocol::{
    ClientFrame, Reply, ReplyResult, Request, ServerFrame, PROTOCOL_VERSION,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, watch};

/// Companion state the web UI shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StateKind {
    Unavailable,
    Starting,
    Ready,
    Error,
}

impl StateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StateKind::Unavailable => "unavailable",
            StateKind::Starting => "starting",
            StateKind::Ready => "ready",
            StateKind::Error => "error",
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Reply, String>>>>>;

struct Inner {
    state: Mutex<(StateKind, Option<String>)>,
    conn: Mutex<Option<Arc<Conn>>>,
    /// tab id -> owning task session id; cleared when the companion restarts.
    tabs: Mutex<HashMap<String, String>>,
    /// owner -> (opens in flight, closed meanwhile). A tab whose open finishes
    /// after its owner's `CloseOwner` is closed on arrival.
    opening: Mutex<HashMap<String, (u32, bool)>>,
    /// `<data>/browser/profiles`, shared with the companion.
    profiles: std::path::PathBuf,
    shutdown: watch::Sender<bool>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct Conn {
    tx: tokio::sync::mpsc::UnboundedSender<ClientFrame>,
    pending: Pending,
    next: AtomicU64,
}

/// Shared browser companion handle. Cloning is cheap; all clones supervise
/// the same child process.
#[derive(Clone)]
pub struct Browser {
    inner: Arc<Inner>,
}

impl Browser {
    /// Starts the supervisor. Without a companion binary (env `BLOB_1_BROWSER`,
    /// or `browser/blob-1-browser` / `blob-1-browser` next to the current
    /// executable) the feature is off:
    /// state stays `unavailable` and no tools appear.
    pub fn start(data_dir: &Path) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let inner = Arc::new(Inner {
            state: Mutex::new((StateKind::Unavailable, None)),
            conn: Mutex::new(None),
            tabs: Mutex::new(HashMap::new()),
            opening: Mutex::new(HashMap::new()),
            profiles: data_dir.join("browser").join("profiles"),
            shutdown: shutdown_tx,
            task: Mutex::new(None),
        });
        let Some(exe) = find_binary() else {
            return Self { inner };
        };
        let runtime = data_dir.join("runtime");
        let profiles = data_dir.join("browser").join("profiles");
        let socket = runtime.join("browser.sock");
        match (std::fs::create_dir_all(&runtime), std::fs::create_dir_all(&profiles)) {
            (Ok(()), Ok(())) => {}
            (Err(error), _) | (_, Err(error)) => {
                set_state(&inner, StateKind::Error, Some(format!("browser directories: {error}")));
                return Self { inner };
            }
        }
        // A leftover socket from a killed serve is dead; the companion unlinks
        // before binding, and removing it here keeps connects from hitting a
        // stale listener.
        let _ = std::fs::remove_file(&socket);
        set_state(&inner, StateKind::Starting, None);
        let task = tokio::spawn(supervise(inner.clone(), exe, socket, profiles, shutdown_rx));
        *inner.task.lock().expect("browser task mutex poisoned") = Some(task);
        Self { inner }
    }

    pub fn state(&self) -> (StateKind, Option<String>) {
        self.inner
            .state
            .lock()
            .expect("browser state mutex poisoned")
            .clone()
    }

    pub fn ready(&self) -> bool {
        self.inner
            .conn
            .lock()
            .expect("browser conn mutex poisoned")
            .is_some()
    }

    /// Runs one request for `owner`, enforcing that tab-scoped operations
    /// only reach tabs the owner opened through this handle.
    pub async fn execute(&self, owner: &str, request: Request) -> Result<Reply, String> {
        let tab = match &request {
            Request::Navigate { tab, .. }
            | Request::Snapshot { tab }
            | Request::Act { tab, .. }
            | Request::Evaluate { tab, .. }
            | Request::Screenshot { tab, .. }
            | Request::Close { tab } => Some(tab.clone()),
            _ => None,
        };
        if let Some(tab) = &tab {
            let tabs = self.inner.tabs.lock().expect("browser tabs mutex poisoned");
            if tabs.get(tab).map(String::as_str) != Some(owner) {
                return Err(format!("unknown tab '{tab}'"));
            }
        }
        if let Request::CloseOwner { owner: close } = &request {
            if close != owner {
                return Err("cannot close another owner's tabs".into());
            }
            if let Some(entry) = self.opening().get_mut(owner) {
                entry.1 = true;
            }
        }
        if let Request::Open { .. } = request {
            // Counted before handing off, so a `CloseOwner` sent from now on
            // sees it. The open and its bookkeeping run in their own task: a
            // stopped task drops this future, and a tab created afterwards
            // must still be recorded or closed.
            self.opening().entry(owner.to_owned()).or_default().0 += 1;
            let this = self.clone();
            let owner = owner.to_owned();
            return tokio::spawn(async move { this.finish_open(&owner, request).await })
                .await
                .map_err(|error| format!("browser open failed: {error}"))?;
        }
        let reply = self.request(&request).await?;
        let mut tabs = self.inner.tabs.lock().expect("browser tabs mutex poisoned");
        match (&request, &reply) {
            (Request::Close { tab }, Reply::Closed) => {
                tabs.remove(tab);
            }
            (Request::CloseOwner { .. }, Reply::Closed) => {
                tabs.retain(|_, tab_owner| tab_owner != owner);
            }
            _ => {}
        }
        Ok(reply)
    }

    /// Sends a counted `Open`, then records the new tab for `owner` — or
    /// closes it when the owner's tabs were closed while it was opening.
    async fn finish_open(&self, owner: &str, request: Request) -> Result<Reply, String> {
        let reply = self.request(&request).await;
        let closed_meanwhile = {
            let mut map = self.opening();
            let entry = map.get_mut(owner).expect("open counted before the hand-off");
            entry.0 -= 1;
            let closed = entry.1;
            if entry.0 == 0 {
                map.remove(owner);
            }
            closed
        };
        let reply = reply?;
        let Reply::Opened { tab, .. } = &reply else {
            return Ok(reply);
        };
        if closed_meanwhile {
            let _ = self.request(&Request::Close { tab: tab.clone() }).await;
            return Err("the task's browser tabs were closed".into());
        }
        self.inner
            .tabs
            .lock()
            .expect("browser tabs mutex poisoned")
            .insert(tab.clone(), owner.to_owned());
        Ok(reply)
    }

    fn opening(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u32, bool)>> {
        self.inner
            .opening
            .lock()
            .expect("browser opening mutex poisoned")
    }

    /// Deletes a profile's cookies and storage: `DeleteProfile` closes its tabs
    /// when the companion is running, then serve removes the directory itself
    /// (they share it), so a profile removed while the companion is down does
    /// not come back with its old logins.
    pub async fn delete_profile(&self, profile: &str) -> Result<(), String> {
        if !browser_protocol::valid_profile_name(profile) {
            return Err(format!("invalid profile name '{profile}'"));
        }
        let closed = match self.ready() {
            true => self
                .request(&Request::DeleteProfile {
                    profile: profile.to_owned(),
                })
                .await
                .map(|_| ()),
            false => Ok(()),
        };
        match std::fs::remove_dir_all(self.inner.profiles.join(profile)) {
            Ok(()) => closed,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => closed,
            Err(error) => Err(format!("could not delete profile data: {error}")),
        }
    }

    /// Sends one request and awaits its reply, with a 45s timeout for opens
    /// and navigations (60s otherwise); a late reply is abandoned with
    /// `Cancel`. Frame contents are never logged: `type` actions can carry
    /// resolved secret values.
    async fn request(&self, request: &Request) -> Result<Reply, String> {
        let conn = self
            .inner
            .conn
            .lock()
            .expect("browser conn mutex poisoned")
            .clone()
            .ok_or_else(|| "browser companion is not connected".to_owned())?;
        let id = conn.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        conn.pending
            .lock()
            .expect("browser pending mutex poisoned")
            .insert(id, tx);
        if conn
            .tx
            .send(ClientFrame::Request {
                id,
                request: request.clone(),
            })
            .is_err()
        {
            conn.pending
                .lock()
                .expect("browser pending mutex poisoned")
                .remove(&id);
            return Err("browser companion is not connected".into());
        }
        let timeout = match request {
            Request::Open { .. } | Request::Navigate { .. } => std::time::Duration::from_secs(45),
            _ => std::time::Duration::from_secs(60),
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("browser companion connection lost".into()),
            Err(_) => {
                conn.pending
                    .lock()
                    .expect("browser pending mutex poisoned")
                    .remove(&id);
                let _ = conn.tx.send(ClientFrame::Cancel { id });
                Err("browser companion request timed out".into())
            }
        }
    }

    /// Stops the supervisor and the companion. Closing the child's stdin asks
    /// it to exit; the kill afterwards is the deadline.
    pub async fn shutdown(&self) {
        let _ = self.inner.shutdown.send(true);
        let task = self
            .inner
            .task
            .lock()
            .expect("browser task mutex poisoned")
            .take();
        if let Some(task) = task {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        }
    }
}

fn set_state(inner: &Inner, kind: StateKind, message: Option<String>) {
    *inner.state.lock().expect("browser state mutex poisoned") = (kind, message);
}

fn find_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("BLOB_1_BROWSER") {
        let path = PathBuf::from(path.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    // `blob-1 update --browser` unpacks the companion into `browser/` beside
    // the server binary; a plain sibling works for local builds.
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [dir.join("browser").join("blob-1-browser"), dir.join("blob-1-browser")]
        .into_iter()
        .find(|path| path.is_file())
}

/// Reads one newline-terminated frame, refusing frames over
/// `MAX_FRAME_BYTES` so a broken companion can't grow serve's memory.
async fn next_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    use tokio::io::AsyncReadExt;
    let limit = browser_protocol::MAX_FRAME_BYTES;
    let mut buf = Vec::new();
    let read = (&mut *reader)
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut buf)
        .await?;
    if read == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "companion frame exceeds the size limit",
        ));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

enum Exit {
    Shutdown,
    Failed(String),
}

async fn supervise(
    inner: Arc<Inner>,
    exe: PathBuf,
    socket: PathBuf,
    profiles: PathBuf,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut backoff = std::time::Duration::from_secs(1);
    loop {
        set_state(&inner, StateKind::Starting, None);
        match run_once(&inner, &exe, &socket, &profiles, &mut shutdown).await {
            Exit::Shutdown => break,
            Exit::Failed(message) => {
                eprintln!("browser companion: {message}");
                set_state(&inner, StateKind::Error, Some(message));
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = shutdown.changed() => break,
                }
                backoff = (backoff * 2).min(std::time::Duration::from_secs(15));
            }
        }
    }
    *inner.conn.lock().expect("browser conn mutex poisoned") = None;
    inner
        .tabs
        .lock()
        .expect("browser tabs mutex poisoned")
        .clear();
    set_state(&inner, StateKind::Unavailable, None);
}

/// One child lifetime: spawn, handshake, serve until the child exits, the
/// connection breaks, or serve shuts down.
async fn run_once(
    inner: &Arc<Inner>,
    exe: &Path,
    socket: &Path,
    profiles: &Path,
    shutdown: &mut watch::Receiver<bool>,
) -> Exit {
    let spawned = tokio::process::Command::new(exe)
        .arg("--socket")
        .arg(socket)
        .arg("--profiles-dir")
        .arg(profiles)
        .stdin(std::process::Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return Exit::Failed(format!("spawn {}: {error}", exe.display())),
    };
    // `Child::wait` drops the child's stdin (deadlock avoidance), and the
    // maintenance loop waits on the child for its whole lifetime — so stdin
    // must live outside the child, or the companion would read its stop
    // signal immediately. It closes in the teardown below.
    let stdin = child.stdin.take();
    let result = connect_and_run(inner, socket, &mut child, shutdown).await;
    // Closing stdin is the companion's stop signal; give it a moment before
    // the kill so profile data flushes cleanly.
    drop(stdin);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
    let _ = child.start_kill();
    let _ = child.wait().await;
    result
}

async fn connect_and_run(
    inner: &Arc<Inner>,
    socket: &Path,
    child: &mut tokio::process::Child,
    shutdown: &mut watch::Receiver<bool>,
) -> Exit {
    // The companion creates its socket after startup; poll briefly, bailing
    // out early when the child dies before listening.
    let stream = {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Exit::Failed(format!("companion exited before listening ({status})"));
            }
            match tokio::net::UnixStream::connect(socket).await {
                Ok(stream) => break stream,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                        _ = shutdown.changed() => return Exit::Shutdown,
                    }
                }
                Err(error) => return Exit::Failed(format!("connect {}: {error}", socket.display())),
            }
        }
    };
    let (read, mut write) = stream.into_split();
    let hello = serde_json::to_string(&ClientFrame::Hello {
        protocol: PROTOCOL_VERSION,
    })
    .expect("hello frame serializes");
    if write
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .is_err()
    {
        return Exit::Failed("companion hello write failed".into());
    }
    let mut lines = BufReader::new(read);
    let ready = tokio::time::timeout(std::time::Duration::from_secs(10), next_frame(&mut lines)).await;
    match ready {
        Ok(Ok(Some(line))) => match serde_json::from_str::<ServerFrame>(&line) {
            Ok(ServerFrame::Ready { .. }) => {}
            _ => return Exit::Failed("companion did not answer Hello with Ready".into()),
        },
        Ok(Ok(None)) => return Exit::Failed("companion closed the connection during hello".into()),
        Ok(Err(error)) => return Exit::Failed(format!("companion read: {error}")),
        Err(_) => return Exit::Failed("companion hello timed out".into()),
    }

    // Connection established: a writer task serializes frames, a reader task
    // routes replies to their waiters. Neither ever logs frame contents.
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ClientFrame>();
    let (lost_tx, mut lost) = watch::channel(false);
    {
        let lost_tx = lost_tx.clone();
        tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let Ok(mut text) = serde_json::to_string(&frame) else {
                    continue;
                };
                text.push('\n');
                if write.write_all(text.as_bytes()).await.is_err() {
                    break;
                }
            }
            let _ = lost_tx.send(true);
        });
    }
    {
        let pending = pending.clone();
        let lost_tx = lost_tx.clone();
        tokio::spawn(async move {
            while let Ok(Some(line)) = next_frame(&mut lines).await {
                if let Ok(ServerFrame::Reply { id, result }) =
                    serde_json::from_str::<ServerFrame>(&line)
                {
                    let waiter = pending
                        .lock()
                        .expect("browser pending mutex poisoned")
                        .remove(&id);
                    if let Some(waiter) = waiter {
                        let result = match result {
                            ReplyResult::Ok(reply) => Ok(reply),
                            ReplyResult::Error(error) => Err(error),
                        };
                        let _ = waiter.send(result);
                    }
                }
            }
            let _ = lost_tx.send(true);
        });
    }

    let conn = Arc::new(Conn {
        tx,
        pending: pending.clone(),
        next: AtomicU64::new(1),
    });
    *inner.conn.lock().expect("browser conn mutex poisoned") = Some(conn.clone());
    set_state(inner, StateKind::Ready, None);

    // Serve until shutdown, child exit, or a broken connection. A ping every
    // 15s keeps dead-but-quiet connections visible through the write error.
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(15));
    ping.tick().await;
    let exit = loop {
        tokio::select! {
            _ = shutdown.changed() => break Exit::Shutdown,
            status = child.wait() => {
                break match status {
                    Ok(status) => Exit::Failed(format!("companion exited ({status})")),
                    Err(error) => Exit::Failed(format!("companion wait: {error}")),
                };
            }
            result = lost.changed() => {
                if result.is_err() || *lost.borrow() {
                    break Exit::Failed("companion connection lost".into());
                }
            }
            _ = ping.tick() => {
                if conn.tx.send(ClientFrame::Ping).is_err() {
                    break Exit::Failed("companion connection lost".into());
                }
            }
        }
    };

    // Teardown: fail every pending request by dropping the connection, and
    // forget tab ownership since all tabs die with the companion.
    let mut guard = inner.conn.lock().expect("browser conn mutex poisoned");
    if guard.as_ref().is_some_and(|current| Arc::ptr_eq(current, &conn)) {
        *guard = None;
    }
    drop(guard);
    // Dropping the senders fails every in-flight request with
    // "connection lost" on the waiting side.
    conn.pending
        .lock()
        .expect("browser pending mutex poisoned")
        .clear();
    inner
        .tabs
        .lock()
        .expect("browser tabs mutex poisoned")
        .clear();
    exit
}

/// Per-bot harness backend: resolves the bot's profile from the roster and
/// its secrets from the bot store, and routes requests through the shared
/// supervised companion with the task as owner.
pub struct BotBrowser {
    browser: Browser,
    secrets: Arc<dyn storage::SecretStore>,
    bots: Arc<storage_sqlite::SqliteStorage>,
    bot_id: String,
}

impl BotBrowser {
    pub fn tools(
        browser: &Browser,
        secrets: Arc<dyn storage::SecretStore>,
        bots: Arc<storage_sqlite::SqliteStorage>,
        bot_id: String,
    ) -> harness::BrowserTools {
        harness::BrowserTools::new(Arc::new(Self {
            browser: browser.clone(),
            secrets,
            bots,
            bot_id,
        }))
    }
}

#[async_trait::async_trait]
impl harness::browser::BrowserBackend for BotBrowser {
    fn ready(&self) -> bool {
        self.browser.ready()
    }

    fn profile(&self) -> Result<Option<String>, String> {
        use storage::BotStore;
        self.bots
            .bot(&self.bot_id)
            .map(|bot| bot.and_then(|bot| bot.browser_profile))
            .map_err(|error| error.to_string())
    }

    fn secret(&self, name: &str) -> Result<Option<String>, String> {
        self.secrets
            .secret_value(name)
            .map_err(|error| error.to_string())
    }

    async fn execute(&self, owner: &str, request: Request) -> Result<Reply, String> {
        self.browser.execute(owner, request).await
    }
}
