mod gateway;
mod render;

use interface::{
    Fetched, Inbound, Interface, InterfaceError, Lookup, Mark, Outbound, Person, Place, Scope,
};
use std::collections::HashSet;
use tokio::sync::mpsc;
use twilight_http::Client;
use twilight_model::{
    channel::Message,
    id::{marker::ChannelMarker, Id},
    user::User,
};

pub struct Discord {
    token: String,
    http: Client,
}

fn backend(error: impl std::error::Error + Send + Sync + 'static) -> InterfaceError {
    InterfaceError::Backend(Box::new(error))
}

fn id<T>(value: &str) -> Result<Id<T>, InterfaceError> {
    value.parse().map_err(backend)
}

fn channel(place: &Place) -> Result<Id<ChannelMarker>, InterfaceError> {
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

fn fetched(message: Message) -> Fetched {
    Fetched {
        message: message.id.to_string(),
        author: person(&message.author),
        text: message.content,
        date: message.timestamp.iso_8601().to_string(),
    }
}

impl Discord {
    pub fn new(token: String) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self {
            http: Client::new(token.clone()),
            token,
        }
    }

    async fn messages(
        &self,
        place: &Place,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Message>, InterfaceError> {
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

    async fn history(
        &self,
        place: &Place,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Fetched>, InterfaceError> {
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
    ) -> Result<Vec<Fetched>, InterfaceError> {
        let Scope::Place(place) = scope else {
            return Err(InterfaceError::Unsupported(
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
    ) -> Result<Vec<Person>, InterfaceError> {
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
            Scope::Interface => {
                let query = query.ok_or_else(|| {
                    InterfaceError::Unsupported("listing all members; give a query".into())
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
impl Interface for Discord {
    async fn listen(&self, inbound: mpsc::Sender<Inbound>) -> Result<(), InterfaceError> {
        gateway::listen(self, inbound).await
    }
    async fn react(&self, place: &Place, message: &str, mark: Mark) -> Result<(), InterfaceError> {
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
    ) -> Result<(), InterfaceError> {
        self.http
            .delete_current_user_reaction(channel(place)?, id(message)?, &reaction(mark))
            .await
            .map_err(backend)?;
        Ok(())
    }
    async fn typing(&self, place: &Place) -> Result<(), InterfaceError> {
        self.http
            .create_typing_trigger(channel(place)?)
            .await
            .map_err(backend)?;
        Ok(())
    }
    async fn send(&self, outbound: Outbound) -> Result<(), InterfaceError> {
        render::send(self, outbound).await
    }
}
