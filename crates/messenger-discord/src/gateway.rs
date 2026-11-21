use crate::{attachments, backend, person, Discord};
use messenger::{Inbound, MessengerError, Place, Quote};
use tokio::sync::mpsc;
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, ShardState, StreamExt};
use twilight_model::channel::{ChannelType, Message};

async fn quote(discord: &Discord, message: &Message) -> Option<Quote> {
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
    let quoted = match discord.http.message(channel, id).await {
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

pub async fn listen(discord: &Discord, tx: mpsc::Sender<Inbound>) -> Result<(), MessengerError> {
    let intents = Intents::GUILDS
        | Intents::GUILD_MESSAGES
        | Intents::DIRECT_MESSAGES
        | Intents::MESSAGE_CONTENT;
    let mut shard = Shard::new(ShardId::ONE, discord.token.clone(), intents);
    let mut bot = None;
    loop {
        let event = tokio::select! {
            _ = tx.closed() => return Ok(()),
            event = shard.next_event(EventTypeFlags::all()) => event,
        };
        match event {
            Some(Ok(Event::Ready(ready))) => {
                bot = Some(ready.user.id);
                discord
                    .ready
                    .store(true, std::sync::atomic::Ordering::Relaxed);
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
                        let channel = discord
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
                            let parent_channel = discord
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
                    quote: quote(discord, &message).await,
                    attachments: attachments(&message.attachments),
                    location,
                    earlier,
                };
                if tx.send(inbound).await.is_err() {
                    return Ok(());
                }
            }
            Some(Ok(Event::GatewayClose(frame))) if shard.state() == ShardState::FatallyClosed => {
                return Err(backend(std::io::Error::other(format!(
                    "gateway closed: {frame:?}"
                ))));
            }
            Some(Err(error)) => {
                eprintln!("discord gateway: {error}");
            }
            None => return Err(backend(std::io::Error::other("gateway stopped"))),
            _ => {}
        }
    }
}
