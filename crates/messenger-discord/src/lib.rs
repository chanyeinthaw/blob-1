mod gateway;
mod render;

use messenger::{
    Attachment, Fetched, Inbound, Lookup, Mark, Messenger, MessengerError, Outbound, Person, Place,
    Scope,
};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use twilight_http::Client;
use twilight_model::{
    channel::{message::MessageType, ChannelType, Message},
    id::{
        marker::{ChannelMarker, UserMarker},
        Id,
    },
    user::User,
};

/// Decision returned by the gateway pre-filter before any REST call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreFilterDecision {
    /// Admit the message and proceed with REST lookups as needed.
    Allow,
    /// Drop the message immediately with no further REST calls.
    Reject,
    /// Channel parent is unknown and an inherited channel override could apply;
    /// resolve the channel once, then re-check with the resolved parent.
    NeedParent,
}

/// Cheap gate consulted per incoming gateway message before any REST call:
/// `(author_id, guild_id, channel_id, thread_id, addressed, channel_known)`.
/// When `thread_id` is `Some`, `channel_id` is the resolved parent channel;
/// when `None` and `channel_known` is `true`, `channel_id` is a known
/// top-level channel (or DM); when `channel_known` is `false`, `channel_id` is
/// uncached and may be a thread. Without a pre-filter, only addressed messages
/// run.
type PreFilter = Arc<
    dyn Fn(&str, Option<&str>, &str, Option<&str>, bool, bool) -> PreFilterDecision + Send + Sync,
>;

/// Callback invoked whenever Discord reports or confirms that a thread exists
/// `(parent_channel_id, thread_id)`.
type ThreadSeen = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The Discord messenger. It starts disconnected and swaps its HTTP client
/// and gateway live when `connect` is called with a new token. Operations
/// while disconnected fail with "Discord is not connected".
pub struct Discord {
    inner: watch::Sender<Option<Arc<Inner>>>,
    connection: watch::Sender<Connection>,
    pre_filter: std::sync::RwLock<Option<PreFilter>>,
    on_thread_seen: std::sync::RwLock<Option<ThreadSeen>>,
}

/// One bot connection: its HTTP client plus the state the gateway fills in.
pub(crate) struct Inner {
    token: String,
    http: Client,
    user: tokio::sync::OnceCell<Id<UserMarker>>,
    ready: std::sync::atomic::AtomicBool,
    directory: tokio::sync::Mutex<Option<(std::time::Instant, Directory)>>,
    recent: std::sync::Mutex<std::collections::HashMap<String, RecentChannel>>,
}

/// Process-wide monotonic token bound to each live cache entry so seeds from
/// invalidated channels, evicted entries, earlier `Ready` sessions, or replaced
/// connections can never match a newer entry.
static NEXT_CACHE_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct RecentChannel {
    token: u64,
    messages: std::collections::VecDeque<Fetched>,
    expires_at: std::time::Instant,
    ttl: std::time::Duration,
    warming: bool,
    warming_holders: usize,
    /// True only when the retained window starts at the very beginning of the
    /// channel/thread history (no older messages exist on Discord before `front`).
    complete_from_start: bool,
}

const RECENT_WARMING_TTL: std::time::Duration = std::time::Duration::from_secs(15);
const RECENT_CHANNEL_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);
const RECENT_THREAD_TTL: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);
const RECENT_MAX_MESSAGES: usize = 20;
const RECENT_MAX_CHANNELS: usize = 128;

fn snowflake_key(id: &str) -> u64 {
    id.parse::<u64>().unwrap_or(0)
}

fn insert_ordered(
    messages: &mut std::collections::VecDeque<Fetched>,
    complete_from_start: &mut bool,
    item: Fetched,
) {
    if messages.iter().any(|m| m.message == item.message) {
        return;
    }
    let key = snowflake_key(&item.message);
    let pos = messages
        .iter()
        .position(|m| snowflake_key(&m.message) > key)
        .unwrap_or(messages.len());
    messages.insert(pos, item);
    while messages.len() > RECENT_MAX_MESSAGES {
        messages.pop_front();
        *complete_from_start = false;
    }
}

/// What the web settings reports for the bot connection.
#[derive(Clone)]
pub enum Connection {
    Off,
    Connecting,
    Online { bot: String },
    Error { message: String },
}

/// `(guilds, channels, people)` where each channel carries `(id, name, server, guild_id, is_thread)`
/// and each person carries the guild IDs they belong to so pickers can filter accurately.
type Directory = (
    Vec<(String, String)>,
    Vec<(String, String, String, String, bool)>,
    Vec<(Person, Vec<String>)>,
);

const DIRECTORY_TTL: std::time::Duration = std::time::Duration::from_secs(600);

fn backend(error: impl std::error::Error + Send + Sync + 'static) -> MessengerError {
    MessengerError::Backend(Box::new(error))
}

fn disconnected() -> MessengerError {
    backend(std::io::Error::other("Discord is not connected"))
}

fn id<T>(value: &str) -> Result<Id<T>, MessengerError> {
    value.parse().map_err(backend)
}

fn channel(place: &Place) -> Result<Id<ChannelMarker>, MessengerError> {
    id(place.thread.as_deref().unwrap_or(&place.channel))
}

fn reaction(mark: Mark) -> twilight_http::request::channel::reaction::RequestReactionType<'static> {
    twilight_http::request::channel::reaction::RequestReactionType::Unicode {
        name: match mark {
            Mark::Seen => "👀",
            Mark::Working => "⏳",
            Mark::Done => "✅",
            Mark::Failed => "⚠️",
        },
    }
}

fn person(user: &User) -> Person {
    Person {
        id: user.id.to_string(),
        name: user
            .global_name
            .clone()
            .unwrap_or_else(|| user.name.clone()),
    }
}

fn attachments(files: &[twilight_model::channel::Attachment]) -> Vec<Attachment> {
    files
        .iter()
        .map(|file| Attachment {
            name: file.filename.clone(),
            content_type: file.content_type.clone(),
            size: file.size,
            url: file.url.clone(),
        })
        .collect()
}

fn fetched(message: Message) -> Fetched {
    Fetched {
        message: message.id.to_string(),
        author: person(&message.author),
        text: message.content,
        date: message.timestamp.iso_8601().to_string(),
        attachments: attachments(&message.attachments),
    }
}

impl Default for Discord {
    fn default() -> Self {
        Self::new()
    }
}

impl Discord {
    pub fn new() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self {
            inner: watch::channel(None).0,
            connection: watch::channel(Connection::Off).0,
            pre_filter: std::sync::RwLock::new(None),
            on_thread_seen: std::sync::RwLock::new(None),
        }
    }

    /// Installs the per-message pre-filter the gateway runs before any REST
    /// call, so busy servers can't rate-limit the bot on skipped messages.
    pub fn set_pre_filter(&self, filter: PreFilter) {
        *self.pre_filter.write().expect("pre_filter lock poisoned") = Some(filter);
    }

    pub(crate) fn pre_filter(&self) -> Option<PreFilter> {
        self.pre_filter
            .read()
            .expect("pre_filter lock poisoned")
            .clone()
    }

    /// Installs a callback notified whenever Discord reports or creates a
    /// thread `(parent_channel_id, thread_id)`.
    pub fn set_on_thread_seen(&self, cb: ThreadSeen) {
        *self
            .on_thread_seen
            .write()
            .expect("on_thread_seen lock poisoned") = Some(cb);
    }

    pub(crate) fn notify_thread_seen(&self, parent: &str, thread: &str) {
        if let Some(cb) = self
            .on_thread_seen
            .read()
            .expect("on_thread_seen lock poisoned")
            .clone()
        {
            cb(parent, thread);
        }
    }

    /// Connects with a bot token, replacing any current connection.
    pub fn connect(&self, token: String) {
        self.inner.send_replace(Some(Arc::new(Inner::new(token))));
        self.connection.send_replace(Connection::Connecting);
    }

    /// Disconnects; the connection state reports off.
    pub fn disconnect(&self) {
        self.inner.send_replace(None);
        self.connection.send_replace(Connection::Off);
    }

    /// Disconnects and reports a failure, e.g. a dead gateway or no main model.
    pub fn error(&self, message: impl Into<String>) {
        self.inner.send_replace(None);
        self.connection.send_replace(Connection::Error {
            message: message.into(),
        });
    }

    /// Reports connecting until the supervisor picks up a newly stored token.
    pub fn connecting(&self) {
        self.connection.send_replace(Connection::Connecting);
    }

    /// Whether the live connection already uses this token.
    pub fn uses_token(&self, token: &str) -> bool {
        self.inner
            .borrow()
            .as_ref()
            .is_some_and(|inner| inner.token == token)
    }

    pub(crate) fn set_connection(&self, connection: Connection) {
        self.connection.send_replace(connection);
    }

    pub fn connection(&self) -> Connection {
        self.connection.borrow().clone()
    }

    fn current(&self) -> Result<Arc<Inner>, MessengerError> {
        self.inner.borrow().clone().ok_or_else(disconnected)
    }

    /// Channels, servers, and people the bot can see; empty while disconnected
    /// or not yet ready.
    pub async fn directory(&self) -> Result<Directory, MessengerError> {
        let inner = self.inner.borrow().clone();
        match inner {
            Some(inner) => inner.directory().await,
            None => Ok(Default::default()),
        }
    }

    /// Resolve a single channel or thread id (e.g. a stored thread override
    /// not in the top-level channel list) to `(id, name, server_name, guild_id, is_thread)`.
    pub async fn resolve_channel(
        &self,
        channel_id: &str,
        guilds: &[(String, String)],
    ) -> Result<Option<(String, String, String, String, bool)>, MessengerError> {
        let Some(inner) = self.inner.borrow().clone() else {
            return Ok(None);
        };
        if !inner.is_ready() {
            return Ok(None);
        }
        let Ok(cid) = id::<ChannelMarker>(channel_id) else {
            return Ok(None);
        };
        let channel = match inner.http.channel(cid).await {
            Ok(resp) => resp.model().await.map_err(backend)?,
            Err(_) => return Ok(None),
        };
        let guild_id = channel.guild_id.map(|g| g.to_string()).unwrap_or_default();
        let server = if guild_id.is_empty() {
            String::new()
        } else {
            guilds
                .iter()
                .find(|(id, _)| id == &guild_id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| guild_id.clone())
        };
        let name = channel.name.unwrap_or_else(|| channel_id.to_owned());
        Ok(Some((
            channel.id.to_string(),
            name,
            server,
            guild_id,
            channel.kind.is_thread(),
        )))
    }
}

impl Inner {
    fn new(token: String) -> Self {
        Self {
            http: Client::new(token.clone()),
            token,
            user: tokio::sync::OnceCell::new(),
            ready: std::sync::atomic::AtomicBool::new(false),
            directory: tokio::sync::Mutex::new(None),
            recent: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn recent_key(place: &Place) -> &str {
        place.thread.as_deref().unwrap_or(&place.channel)
    }

    fn ttl_for(place: &Place) -> std::time::Duration {
        if place.thread.is_some() {
            RECENT_THREAD_TTL
        } else {
            RECENT_CHANNEL_TTL
        }
    }

    /// Appends a live gateway message into the ring buffer ONLY when this channel/thread
    /// is actively tracked or transiently warming. Bot messages refresh participating TTL.
    pub(crate) fn record_gateway_message(&self, message: &Message, is_bot: bool) {
        let now = std::time::Instant::now();
        let key = message.channel_id.to_string();
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = guard.get_mut(&key) else {
            return;
        };
        if now >= entry.expires_at {
            guard.remove(&key);
            return;
        }
        if is_bot && !entry.warming {
            entry.expires_at = now + entry.ttl;
        }
        insert_ordered(
            &mut entry.messages,
            &mut entry.complete_from_start,
            fetched(message.clone()),
        );
    }

    pub(crate) fn invalidate_recent(&self, channel_id: Id<ChannelMarker>) {
        let key = channel_id.to_string();
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(&key);
    }

    pub(crate) fn clear_recent(&self) {
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        guard.clear();
    }

    fn begin_warming(&self, place: &Place) -> u64 {
        let now = std::time::Instant::now();
        let key = Self::recent_key(place).to_owned();
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|_, entry| now < entry.expires_at);
        if let Some(entry) = guard.get_mut(&key) {
            if entry.warming {
                entry.warming_holders += 1;
                entry.expires_at = now + RECENT_WARMING_TTL;
            }
            return entry.token;
        }
        if guard.len() >= RECENT_MAX_CHANNELS {
            if let Some(oldest_key) = guard
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(k, _)| k.clone())
            {
                guard.remove(&oldest_key);
            }
        }
        let token = NEXT_CACHE_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        guard.insert(
            key,
            RecentChannel {
                token,
                messages: std::collections::VecDeque::with_capacity(RECENT_MAX_MESSAGES),
                expires_at: now + RECENT_WARMING_TTL,
                ttl: Self::ttl_for(place),
                warming: true,
                warming_holders: 1,
                complete_from_start: false,
            },
        );
        token
    }

    fn cancel_warming(&self, place: &Place, token: u64) {
        let key = Self::recent_key(place);
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        let remove = if let Some(entry) = guard.get_mut(key) {
            if entry.warming && entry.token == token {
                entry.warming_holders = entry.warming_holders.saturating_sub(1);
                entry.warming_holders == 0
            } else {
                false
            }
        } else {
            false
        };
        if remove {
            guard.remove(key);
        }
    }

    fn touch_history(&self, place: &Place) {
        let now = std::time::Instant::now();
        let key = Self::recent_key(place);
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = guard.get_mut(key) {
            if !entry.warming && now < entry.expires_at {
                entry.expires_at = now + entry.ttl;
            }
        }
    }

    fn recent_history(
        &self,
        place: &Place,
        before: Option<&str>,
        limit: usize,
    ) -> Option<Vec<Fetched>> {
        let now = std::time::Instant::now();
        let key = Self::recent_key(place);
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.get(key)?;
        if now >= entry.expires_at {
            guard.remove(key);
            return None;
        }
        if entry.warming {
            return None;
        }
        let filtered: Vec<&Fetched> = match before {
            Some(before_id) => {
                let before_num = before_id.parse::<u64>().ok()?;
                let list: Vec<&Fetched> = entry
                    .messages
                    .iter()
                    .filter(|m| snowflake_key(&m.message) < before_num)
                    .collect();
                // Fall back to REST whenever the retained window doesn't cover
                // `limit` messages and may have older history before it.
                if !entry.complete_from_start && list.len() < limit {
                    return None;
                }
                list
            }
            None => {
                let list: Vec<&Fetched> = entry.messages.iter().collect();
                if !entry.complete_from_start && list.len() < limit {
                    return None;
                }
                list
            }
        };
        let start = filtered.len().saturating_sub(limit);
        Some(filtered[start..].iter().map(|m| (*m).clone()).collect())
    }

    fn seed_history(
        &self,
        place: &Place,
        messages: &[Fetched],
        complete_from_start: bool,
        token: u64,
    ) {
        let now = std::time::Instant::now();
        let key = Self::recent_key(place);
        let ttl = Self::ttl_for(place);
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        // Only promote/seed a still-live entry matching `token`. If the entry was
        // invalidated (edit/delete), cleared (`Ready`), evicted, or belongs to a
        // previous connection, its token will not match and the stale seed is rejected.
        let Some(entry) = guard.get_mut(key) else {
            return;
        };
        if now >= entry.expires_at {
            guard.remove(key);
            return;
        }
        if entry.token != token {
            return;
        }
        if entry.warming {
            entry.complete_from_start = complete_from_start;
        } else if complete_from_start {
            entry.complete_from_start = true;
        }
        entry.warming = false;
        entry.warming_holders = 0;
        entry.ttl = ttl;
        entry.expires_at = now + ttl;
        for msg in messages {
            insert_ordered(
                &mut entry.messages,
                &mut entry.complete_from_start,
                msg.clone(),
            );
        }
    }

    fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Relaxed)
    }

    async fn directory(&self) -> Result<Directory, MessengerError> {
        if !self.is_ready() {
            return Ok(Default::default());
        }
        let mut cache = self.directory.lock().await;
        if let Some((at, directory)) = cache.as_ref() {
            if at.elapsed() < DIRECTORY_TTL {
                return Ok(directory.clone());
            }
        }
        match self.fetch_directory().await {
            Ok(directory) => {
                *cache = Some((std::time::Instant::now(), directory.clone()));
                Ok(directory)
            }
            Err(error) => match cache.as_ref() {
                Some((_, directory)) => {
                    eprintln!("discord directory: {error}");
                    Ok(directory.clone())
                }
                None => Err(error),
            },
        }
    }

    async fn fetch_directory(&self) -> Result<Directory, MessengerError> {
        let guilds = self
            .http
            .current_user_guilds()
            .limit(200)
            .await
            .map_err(backend)?
            .models()
            .await
            .map_err(backend)?;
        let mut guild_list = Vec::new();
        let mut channels = Vec::new();
        let mut people_map: std::collections::BTreeMap<String, (Person, Vec<String>)> =
            std::collections::BTreeMap::new();
        for guild in guilds {
            let gid = guild.id.to_string();
            guild_list.push((gid.clone(), guild.name.clone()));
            let members = match self.http.guild_members(guild.id).limit(1000).await {
                Ok(response) => response.models().await.map_err(backend)?,
                Err(error) => {
                    eprintln!("discord directory: members of {}: {error}", guild.name);
                    Vec::new()
                }
            };
            for member in members {
                if member.user.bot {
                    continue;
                }
                let p = person(&member.user);
                let entry = people_map
                    .entry(p.id.clone())
                    .or_insert_with(|| (p, Vec::new()));
                if !entry.1.contains(&gid) {
                    entry.1.push(gid.clone());
                }
            }
            for channel in self
                .http
                .guild_channels(guild.id)
                .await
                .map_err(backend)?
                .models()
                .await
                .map_err(backend)?
            {
                if matches!(
                    channel.kind,
                    ChannelType::GuildText | ChannelType::GuildAnnouncement
                ) {
                    if let Some(name) = channel.name {
                        channels.push((
                            channel.id.to_string(),
                            name,
                            guild.name.clone(),
                            gid.clone(),
                            false,
                        ));
                    }
                }
            }
        }
        guild_list.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let mut people: Vec<(Person, Vec<String>)> = people_map.into_values().collect();
        people.sort_by(|a, b| a.0.name.cmp(&b.0.name).then_with(|| a.0.id.cmp(&b.0.id)));
        Ok((guild_list, channels, people))
    }

    async fn ping_state(
        &self,
        place: &Place,
    ) -> Result<(Option<Id<UserMarker>>, Id<UserMarker>, bool), MessengerError> {
        let user = *self
            .user
            .get_or_try_init(|| async {
                Ok::<_, MessengerError>(
                    self.http
                        .current_user()
                        .await
                        .map_err(backend)?
                        .model()
                        .await
                        .map_err(backend)?
                        .id,
                )
            })
            .await?;
        let latest = self
            .messages(place, None, 10)
            .await?
            .iter()
            .find(|message| matches!(message.kind, MessageType::Regular | MessageType::Reply))
            .map(|message| message.author.id);
        Ok((latest, user, latest != Some(user)))
    }

    async fn messages(
        &self,
        place: &Place,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Message>, MessengerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let request = self.http.channel_messages(channel(place)?);
        let limit = limit.min(100) as u16;
        let response = if let Some(before) = before {
            request.before(id(before)?).limit(limit).await
        } else {
            request.limit(limit).await
        }
        .map_err(backend)?;
        response.models().await.map_err(backend)
    }
}

/// Checks a bot token against `GET /users/@me` before it is stored.
pub async fn validate_token(token: &str) -> Result<(), String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::new(token.to_owned())
        .current_user()
        .await
        .map_err(|_| "Discord rejected this token".to_owned())?
        .model()
        .await
        .map_err(|_| "Discord rejected this token".to_owned())?;
    Ok(())
}

#[async_trait::async_trait]
impl Lookup for Discord {
    fn name(&self) -> &'static str {
        "discord"
    }

    fn self_id(&self) -> Option<String> {
        self.current()
            .ok()
            .and_then(|inner| inner.user.get().map(ToString::to_string))
    }

    fn recent_history(
        &self,
        place: &Place,
        before: Option<&str>,
        limit: usize,
    ) -> Option<Vec<Fetched>> {
        self.current()
            .ok()
            .and_then(|inner| inner.recent_history(place, before, limit))
    }

    fn begin_warming(&self, place: &Place) -> u64 {
        self.current()
            .ok()
            .map_or(0, |inner| inner.begin_warming(place))
    }

    fn cancel_warming(&self, place: &Place, token: u64) {
        if let Ok(inner) = self.current() {
            inner.cancel_warming(place, token);
        }
    }

    fn seed_history(
        &self,
        place: &Place,
        messages: &[Fetched],
        complete_from_start: bool,
        token: u64,
    ) {
        if let Ok(inner) = self.current() {
            inner.seed_history(place, messages, complete_from_start, token);
        }
    }

    fn touch_history(&self, place: &Place) {
        if let Ok(inner) = self.current() {
            inner.touch_history(place);
        }
    }

    async fn channel_guild(&self, channel_id: &str) -> Result<Option<String>, MessengerError> {
        let channel = self
            .current()?
            .http
            .channel(id::<ChannelMarker>(channel_id)?)
            .await
            .map_err(backend)?
            .model()
            .await
            .map_err(backend)?;
        Ok(channel.guild_id.map(|g| g.to_string()))
    }

    async fn message(&self, place: &Place, message: &str) -> Result<Fetched, MessengerError> {
        let message = self
            .current()?
            .http
            .message(channel(place)?, id(message)?)
            .await
            .map_err(backend)?
            .model()
            .await
            .map_err(backend)?;
        Ok(fetched(message))
    }

    fn parse_link(&self, link: &str) -> Option<(Place, String)> {
        let link = link.strip_prefix("https://")?;
        let link = link.split(['?', '#']).next()?;
        let (host, path) = link.split_once('/')?;
        if !matches!(
            host,
            "discord.com"
                | "discordapp.com"
                | "ptb.discord.com"
                | "canary.discord.com"
                | "ptb.discordapp.com"
                | "canary.discordapp.com"
        ) {
            return None;
        }
        let mut parts = path.split('/');
        if parts.next()? != "channels" {
            return None;
        }
        let guild = parts.next()?;
        let channel = parts.next()?;
        let message = parts.next()?;
        let numeric = |value: &str| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
        if !(guild == "@me" || numeric(guild))
            || !numeric(channel)
            || !numeric(message)
            || parts.next().is_some()
        {
            return None;
        }
        Some((
            Place {
                channel: channel.into(),
                thread: None,
                dm: guild == "@me",
            },
            message.into(),
        ))
    }

    async fn history(
        &self,
        place: &Place,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Fetched>, MessengerError> {
        Ok(self
            .current()?
            .messages(place, before, limit)
            .await?
            .into_iter()
            .rev()
            .map(fetched)
            .collect())
    }

    async fn search(
        &self,
        scope: &Scope,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Fetched>, MessengerError> {
        let Scope::Place(place) = scope else {
            return Err(MessengerError::Unsupported(
                "search across all of discord".into(),
            ));
        };
        let query = query.to_lowercase();
        Ok(self
            .current()?
            .messages(place, None, 100)
            .await?
            .into_iter()
            .filter(|m| m.content.to_lowercase().contains(&query))
            .take(limit as usize)
            .map(fetched)
            .collect())
    }

    async fn participants(
        &self,
        scope: &Scope,
        query: Option<&str>,
    ) -> Result<Vec<Person>, MessengerError> {
        let inner = self.current()?;
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        match scope {
            Scope::Place(place) => {
                let query = query.map(str::to_lowercase);
                for message in inner.messages(place, None, 100).await? {
                    let user = message.author;
                    let p = person(&user);
                    if !user.bot
                        && query
                            .as_ref()
                            .is_none_or(|q| p.name.to_lowercase().contains(q) || p.id.contains(q))
                        && seen.insert(p.id.clone())
                    {
                        result.push(p);
                    }
                }
            }
            Scope::Messenger => {
                let query = query.ok_or_else(|| {
                    MessengerError::Unsupported("listing all members; give a query".into())
                })?;
                let mut after = None;
                loop {
                    let mut request = inner.http.current_user_guilds().limit(200);
                    if let Some(id) = after {
                        request = request.after(id);
                    }
                    let guilds = request
                        .await
                        .map_err(backend)?
                        .models()
                        .await
                        .map_err(backend)?;
                    for guild in &guilds {
                        let members = inner
                            .http
                            .search_guild_members(guild.id, query)
                            .limit(50)
                            .await
                            .map_err(backend)?
                            .models()
                            .await
                            .map_err(backend)?;
                        for member in members {
                            let p = person(&member.user);
                            if !member.user.bot && seen.insert(p.id.clone()) {
                                result.push(p);
                                if result.len() == 50 {
                                    return Ok(result);
                                }
                            }
                        }
                    }
                    if guilds.len() < 200 {
                        break;
                    }
                    after = guilds.last().map(|g| g.id);
                }
            }
        }
        Ok(result)
    }
}

#[async_trait::async_trait]
impl Messenger for Discord {
    async fn direct(&self, user: &str) -> Result<Place, MessengerError> {
        let channel = self
            .current()?
            .http
            .create_private_channel(id(user)?)
            .await
            .map_err(backend)?
            .model()
            .await
            .map_err(backend)?;
        Ok(Place {
            channel: channel.id.to_string(),
            thread: None,
            dm: true,
        })
    }
    async fn listen(&self, inbound: mpsc::Sender<Inbound>) -> Result<(), MessengerError> {
        gateway::listen(self, inbound).await
    }
    async fn react(&self, place: &Place, message: &str, mark: Mark) -> Result<(), MessengerError> {
        self.current()?
            .http
            .create_reaction(channel(place)?, id(message)?, &reaction(mark))
            .await
            .map_err(backend)?;
        Ok(())
    }
    async fn unreact(
        &self,
        place: &Place,
        message: &str,
        mark: Mark,
    ) -> Result<(), MessengerError> {
        self.current()?
            .http
            .delete_current_user_reaction(channel(place)?, id(message)?, &reaction(mark))
            .await
            .map_err(backend)?;
        Ok(())
    }
    async fn typing(&self, place: &Place) -> Result<(), MessengerError> {
        let inner = self.current()?;
        match inner.http.create_typing_trigger(channel(place)?).await {
            Ok(_) => Ok(()),
            Err(error) if place.thread.is_some() && render::has_code(&error, 10003) => {
                inner
                    .http
                    .create_typing_trigger(id(&place.channel)?)
                    .await
                    .map_err(backend)?;
                Ok(())
            }
            Err(error) => Err(backend(error)),
        }
    }
    async fn send(&self, outbound: Outbound) -> Result<messenger::Sent, MessengerError> {
        let inner = self.current()?;
        render::send(self, &inner, outbound).await
    }
    async fn rename(&self, place: &Place, name: &str) -> Result<(), MessengerError> {
        let thread = place
            .thread
            .as_deref()
            .ok_or_else(|| MessengerError::Unsupported("thread required".into()))?;
        self.current()?
            .http
            .update_thread(id(thread)?)
            .name(name)
            .await
            .map_err(backend)?;
        Ok(())
    }
}

#[cfg(test)]
mod recent_history_tests {
    use super::*;

    fn mk_fetched(id: &str, text: &str) -> Fetched {
        Fetched {
            message: id.into(),
            author: Person {
                id: "u1".into(),
                name: "Chan".into(),
            },
            text: text.into(),
            date: String::new(),
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn warming_retains_cold_burst_and_orders_by_snowflake() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let inner = Inner::new("token".into());
        let place = Place {
            channel: "1".into(),
            thread: None,
            dm: false,
        };
        // Cold warming collects arrivals B(200) and C(300) without serving cache hits yet.
        let token = inner.begin_warming(&place);
        {
            let mut guard = inner.recent.lock().unwrap();
            let entry = guard.get_mut("1").unwrap();
            insert_ordered(
                &mut entry.messages,
                &mut entry.complete_from_start,
                mk_fetched("200", "B"),
            );
            insert_ordered(
                &mut entry.messages,
                &mut entry.complete_from_start,
                mk_fetched("300", "C"),
            );
        }
        assert!(inner.recent_history(&place, Some("300"), 10).is_none());

        // Overlapping warming holds refcount safely: cancelling one does not drop the buffer.
        let token_hold2 = inner.begin_warming(&place);
        assert_eq!(token_hold2, token);
        inner.cancel_warming(&place, token_hold2);
        assert!(inner.recent.lock().unwrap().contains_key("1"));

        // Seeding with complete_from_start=true promotes warming -> participating and serves <10 cleanly.
        inner.seed_history(&place, &[mk_fetched("100", "A")], true, token);
        let before_c = inner.recent_history(&place, Some("300"), 10).unwrap();
        let ids: Vec<&str> = before_c.iter().map(|m| m.message.as_str()).collect();
        assert_eq!(ids, vec!["100", "200"]);

        // Out-of-order older message (50) inserts in snowflake order and never leaks after before=100.
        inner.seed_history(&place, &[mk_fetched("50", "Z")], true, token);
        let before_a = inner.recent_history(&place, Some("100"), 10).unwrap();
        let ids: Vec<&str> = before_a.iter().map(|m| m.message.as_str()).collect();
        assert_eq!(ids, vec!["50"]);

        // cancel_warming does not remove an already promoted participating channel.
        inner.cancel_warming(&place, token);
        assert!(inner.recent_history(&place, Some("300"), 10).is_some());

        // Edit/delete invalidation removes the entry so stale in-flight seeds are rejected
        // without retaining any per-channel map state for uncached channels.
        inner.invalidate_recent(Id::new(1));
        assert!(inner.recent_history(&place, Some("300"), 10).is_none());
        inner.seed_history(&place, &[mk_fetched("100", "stale")], true, token);
        assert!(inner.recent_history(&place, Some("300"), 10).is_none());

        // Fresh gateway Ready clears entries so pre-reconnect seeds are rejected.
        let token2 = inner.begin_warming(&place);
        inner.clear_recent();
        inner.seed_history(&place, &[mk_fetched("100", "pre-ready")], true, token2);
        assert!(inner.recent_history(&place, Some("300"), 10).is_none());

        // Replaced Inner (new connection) rejects seeds issued by an older Inner.
        let old_token = inner.begin_warming(&place);
        let inner_new = Inner::new("token".into());
        let _new_token = inner_new.begin_warming(&place);
        inner_new.seed_history(&place, &[mk_fetched("100", "old-conn")], true, old_token);
        assert!(inner_new.recent_history(&place, Some("300"), 10).is_none());

        // Partial seed (5 items from a limit=5 fetch, complete_from_start=false) falls back to REST
        // when a later caller asks for 10, but hits cache for limit=5.
        let partial_place = Place {
            channel: "2".into(),
            thread: None,
            dm: false,
        };
        let p_token = inner.begin_warming(&partial_place);
        let partial: Vec<Fetched> = (10u64..15)
            .map(|n| mk_fetched(&n.to_string(), "p"))
            .collect();
        inner.seed_history(&partial_place, &partial, false, p_token);
        assert!(inner
            .recent_history(&partial_place, Some("20"), 10)
            .is_none());
        assert_eq!(
            inner
                .recent_history(&partial_place, Some("20"), 5)
                .unwrap()
                .len(),
            5
        );

        // Evicted ring clears complete_from_start and falls back to None when < limit remain.
        let token3 = inner.begin_warming(&place);
        let mut full = Vec::new();
        for n in 1000u64..1025 {
            full.push(mk_fetched(&n.to_string(), "m"));
        }
        inner.seed_history(&place, &full, false, token3);
        assert!(inner.recent_history(&place, Some("500"), 10).is_none());
        assert!(inner.recent_history(&place, Some("1007"), 10).is_none());
        let hit = inner.recent_history(&place, Some("1020"), 10).unwrap();
        assert_eq!(hit.len(), 10);
        assert_eq!(hit.first().unwrap().message, "1010");
        assert_eq!(hit.last().unwrap().message, "1019");

        // cancel_warming cleans up an unpromoted warming channel once the last holder drops.
        let cold = Place {
            channel: "3".into(),
            thread: None,
            dm: false,
        };
        let cold_tok = inner.begin_warming(&cold);
        inner.cancel_warming(&cold, cold_tok);
        assert!(!inner.recent.lock().unwrap().contains_key("3"));
    }
}
