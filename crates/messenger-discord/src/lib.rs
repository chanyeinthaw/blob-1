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

/// The Discord messenger. It starts disconnected and swaps its HTTP client
/// and gateway live when `connect` is called with a new token. Operations
/// while disconnected fail with "Discord is not connected".
pub struct Discord {
    inner: watch::Sender<Option<Arc<Inner>>>,
    connection: watch::Sender<Connection>,
}

/// One bot connection: its HTTP client plus the state the gateway fills in.
pub(crate) struct Inner {
    token: String,
    http: Client,
    user: tokio::sync::OnceCell<Id<UserMarker>>,
    ready: std::sync::atomic::AtomicBool,
    directory: tokio::sync::Mutex<Option<(std::time::Instant, Directory)>>,
}

/// What the web settings reports for the bot connection.
#[derive(Clone)]
pub enum Connection {
    Off,
    Connecting,
    Online { bot: String },
    Error { message: String },
}

type Directory = (Vec<(String, String, String)>, Vec<Person>);

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

    /// Channels and people the bot can see; empty while disconnected or not
    /// yet ready.
    pub async fn directory(&self) -> Result<Directory, MessengerError> {
        let inner = self.inner.borrow().clone();
        match inner {
            Some(inner) => inner.directory().await,
            None => Ok(Default::default()),
        }
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
        let mut channels = Vec::new();
        let mut people = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for guild in guilds {
            let members = match self.http.guild_members(guild.id).limit(1000).await {
                Ok(response) => response.models().await.map_err(backend)?,
                Err(error) => {
                    eprintln!("discord directory: members of {}: {error}", guild.name);
                    Vec::new()
                }
            };
            for member in members {
                let p = person(&member.user);
                if !member.user.bot && seen.insert(p.id.clone()) {
                    people.push(p);
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
                        channels.push((channel.id.to_string(), name, guild.name.clone()));
                    }
                }
            }
        }
        Ok((channels, people))
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
        render::send(&inner, outbound).await
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
