mod gateway;
mod render;

use messenger::{
    Attachment, Fetched, Inbound, Lookup, Mark, Messenger, MessengerError, Outbound, Person, Place,
    Scope,
};
use std::collections::HashSet;
use tokio::sync::mpsc;
use twilight_http::Client;
use twilight_model::{
    channel::{message::MessageType, Message},
    id::{
        marker::{ChannelMarker, UserMarker},
        Id,
    },
    user::User,
};

pub struct Discord {
    token: String,
    http: Client,
    user: tokio::sync::OnceCell<Id<UserMarker>>,
}

fn backend(error: impl std::error::Error + Send + Sync + 'static) -> MessengerError {
    MessengerError::Backend(Box::new(error))
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

impl Discord {
    pub fn new(token: String) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self {
            http: Client::new(token.clone()),
            token,
            user: tokio::sync::OnceCell::new(),
        }
    }

    pub async fn ping_state(
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

#[async_trait::async_trait]
impl Lookup for Discord {
    fn name(&self) -> &'static str {
        "discord"
    }

    async fn message(&self, place: &Place, message: &str) -> Result<Fetched, MessengerError> {
        let message = self
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
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        match scope {
            Scope::Place(place) => {
                let query = query.map(str::to_lowercase);
                for message in self.messages(place, None, 100).await? {
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
                    let mut request = self.http.current_user_guilds().limit(200);
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
                        let members = self
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
        self.http
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
        self.http
            .delete_current_user_reaction(channel(place)?, id(message)?, &reaction(mark))
            .await
            .map_err(backend)?;
        Ok(())
    }
    async fn typing(&self, place: &Place) -> Result<(), MessengerError> {
        match self.http.create_typing_trigger(channel(place)?).await {
            Ok(_) => Ok(()),
            Err(error) if place.thread.is_some() && render::has_code(&error, 10003) => {
                self.http
                    .create_typing_trigger(id(&place.channel)?)
                    .await
                    .map_err(backend)?;
                Ok(())
            }
            Err(error) => Err(backend(error)),
        }
    }
    async fn send(&self, outbound: Outbound) -> Result<messenger::Sent, MessengerError> {
        render::send(self, outbound).await
    }
    async fn rename(&self, place: &Place, name: &str) -> Result<(), MessengerError> {
        let thread = place
            .thread
            .as_deref()
            .ok_or_else(|| MessengerError::Unsupported("thread required".into()))?;
        self.http
            .update_thread(id(thread)?)
            .name(name)
            .await
            .map_err(backend)?;
        Ok(())
    }
}
