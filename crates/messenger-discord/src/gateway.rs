use crate::{attachments, backend, person, Connection, Discord, Inner};
use messenger::{Inbound, MessengerError, Place, Quote};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, ShardState, StreamExt};
use twilight_model::channel::{ChannelType, Message};

async fn quote(inner: &Inner, message: &Message) -> Option<Quote> {
    if let Some(quoted) = &message.referenced_message {
        return Some(Quote {
            message: quoted.id.to_string(),
            author: person(&quoted.author),
            text: quoted.content.clone(),
            attachments: attachments(&quoted.attachments),
        });
    }
    let reference = message.reference.as_ref()?;
    let (channel, id) = (reference.channel_id?, reference.message_id?);
    let quoted = match inner.http.message(channel, id).await {
        Ok(response) => response.model().await.map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    match quoted {
        Ok(quoted) => Some(Quote {
            message: quoted.id.to_string(),
            author: person(&quoted.author),
            attachments: attachments(&quoted.attachments),
            text: quoted.content,
        }),
        Err(error) => {
            eprintln!("discord quote: {error}");
            None
        }
    }
}

/// Runs the gateway for whichever connection `inner` currently holds. A token
/// change swaps `inner`, which tears down the shard and reconnects; a fatal
/// gateway error disconnects and reports the failure instead of exiting.
pub async fn listen(discord: &Discord, tx: mpsc::Sender<Inbound>) -> Result<(), MessengerError> {
    let intents = Intents::GUILDS
        | Intents::GUILD_MESSAGES
        | Intents::DIRECT_MESSAGES
        | Intents::MESSAGE_CONTENT;
    let mut inners = discord.inner.subscribe();
    loop {
        let inner = loop {
            if let Some(inner) = inners.borrow_and_update().clone() {
                break inner;
            }
            if inners.changed().await.is_err() {
                return Ok(());
            }
        };
        if let Outcome::Exit = run(discord, inner, &mut inners, &tx, intents).await {
            return Ok(());
        }
    }
}

enum Outcome {
    /// The connection was swapped or failed; take the current `inner` again.
    Swap,
    /// The caller is gone; stop listening.
    Exit,
}

async fn run(
    discord: &Discord,
    inner: Arc<Inner>,
    inners: &mut watch::Receiver<Option<Arc<Inner>>>,
    tx: &mpsc::Sender<Inbound>,
    intents: Intents,
) -> Outcome {
    let mut shard = Shard::new(ShardId::ONE, inner.token.clone(), intents);
    let mut bot = None;
    loop {
        let event = tokio::select! {
            _ = tx.closed() => return Outcome::Exit,
            changed = inners.changed() => {
                if changed.is_err() {
                    return Outcome::Exit;
                }
                return Outcome::Swap;
            },
            event = shard.next_event(EventTypeFlags::all()) => event,
        };
        match event {
            Some(Ok(Event::Ready(ready))) => {
                bot = Some(ready.user.id);
                inner
                    .ready
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                discord.set_connection(Connection::Online {
                    bot: ready.user.name.clone(),
                });
                let warm = inner.clone();
                tokio::spawn(async move {
                    if let Err(error) = warm.fetch_directory().await {
                        eprintln!("discord directory: {error}");
                    }
                });
            }
            Some(Ok(Event::MessageCreate(message))) => {
                let Some(bot) = bot else { continue };
                if message.author.bot || message.author.id == bot {
                    continue;
                }
                let dm = message.guild_id.is_none();
                if !dm
                    && !message.mentions.iter().any(|u| u.id == bot)
                    && !message
                        .referenced_message
                        .as_ref()
                        .is_some_and(|m| m.author.id == bot)
                {
                    continue;
                }
                let text = message
                    .content
                    .replace(&format!("<@{bot}>"), "")
                    .replace(&format!("<@!{bot}>"), "")
                    .trim()
                    .to_owned();
                let location_result: Result<_, MessengerError> = async {
                    Ok(if dm {
                        (
                            Place {
                                channel: message.channel_id.to_string(),
                                thread: None,
                                dm: true,
                            },
                            "DM".to_owned(),
                            None,
                        )
                    } else {
                        let channel = inner
                            .http
                            .channel(message.channel_id)
                            .await
                            .map_err(backend)?
                            .model()
                            .await
                            .map_err(backend)?;
                        if matches!(
                            channel.kind,
                            ChannelType::PublicThread
                                | ChannelType::PrivateThread
                                | ChannelType::AnnouncementThread
                        ) {
                            let parent = channel.parent_id.ok_or_else(|| {
                                backend(std::io::Error::other("thread has no parent"))
                            })?;
                            let parent_channel = inner
                                .http
                                .channel(parent)
                                .await
                                .map_err(backend)?
                                .model()
                                .await
                                .map_err(backend)?;
                            let location = format!(
                                "#{} › {}",
                                parent_channel.name.unwrap_or_default(),
                                channel.name.unwrap_or_default()
                            );
                            (
                                Place {
                                    channel: parent.to_string(),
                                    thread: Some(channel.id.to_string()),
                                    dm: false,
                                },
                                location,
                                channel.message_count.map(|n| n.saturating_sub(1)),
                            )
                        } else {
                            (
                                Place {
                                    channel: channel.id.to_string(),
                                    thread: None,
                                    dm: false,
                                },
                                format!("#{}", channel.name.unwrap_or_default()),
                                None,
                            )
                        }
                    })
                }
                .await;
                let (place, location, earlier) = match location_result {
                    Ok(value) => value,
                    Err(error) => {
                        eprintln!("discord message: {error}");
                        continue;
                    }
                };
                let inbound = Inbound {
                    place,
                    message: message.id.to_string(),
                    author: person(&message.author),
                    text,
                    quote: quote(&inner, &message).await,
                    attachments: attachments(&message.attachments),
                    location,
                    earlier,
                };
                if tx.send(inbound).await.is_err() {
                    return Outcome::Exit;
                }
            }
            Some(Ok(Event::GatewayClose(frame))) if shard.state() == ShardState::FatallyClosed => {
                eprintln!("discord gateway closed: {frame:?}");
                discord.error(match frame.as_ref().map(|frame| frame.code) {
                    Some(4004) => "Discord rejected this token",
                    Some(4013 | 4014) => {
                        "Enable the Message Content and Server Members intents for this bot"
                    }
                    _ => "Discord closed the connection",
                });
                return Outcome::Swap;
            }
            Some(Err(error)) => {
                eprintln!("discord gateway: {error}");
            }
            None => {
                discord.error("Discord connection stopped");
                return Outcome::Swap;
            }
            _ => {}
        }
    }
}
