use crate::{attachments, backend, person, Connection, Discord, Inner, PreFilterDecision};
use messenger::{Inbound, MessengerError, Place, Quote};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, ShardState, StreamExt};
use twilight_model::{
    channel::{message::MessageType, Message},
    id::{marker::ChannelMarker, Id},
};

#[derive(Clone)]
struct CachedChannel {
    name: String,
    parent: Option<Id<ChannelMarker>>,
}

fn cache_channel(
    discord: &Discord,
    known: &mut HashMap<Id<ChannelMarker>, CachedChannel>,
    ch: &twilight_model::channel::Channel,
) {
    let parent = ch.kind.is_thread().then_some(ch.parent_id).flatten();
    if let Some(parent_id) = parent {
        discord.notify_thread_seen(&parent_id.to_string(), &ch.id.to_string());
    }
    known.insert(
        ch.id,
        CachedChannel {
            name: ch.name.clone().unwrap_or_default(),
            parent,
        },
    );
}

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
    let mut known_channels: HashMap<Id<ChannelMarker>, CachedChannel> = HashMap::new();
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
                let _ = inner.user.set(ready.user.id);
                inner.clear_recent();
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
            Some(Ok(Event::GuildCreate(guild))) => {
                if let twilight_model::gateway::payload::incoming::GuildCreate::Available(guild) =
                    *guild
                {
                    for ch in guild.channels.iter().chain(guild.threads.iter()) {
                        cache_channel(discord, &mut known_channels, ch);
                    }
                }
            }
            Some(Ok(Event::ChannelCreate(ch))) => cache_channel(discord, &mut known_channels, &ch),
            Some(Ok(Event::ChannelUpdate(ch))) => cache_channel(discord, &mut known_channels, &ch),
            Some(Ok(Event::ThreadCreate(ch))) => cache_channel(discord, &mut known_channels, &ch),
            Some(Ok(Event::ThreadUpdate(ch))) => cache_channel(discord, &mut known_channels, &ch),
            Some(Ok(Event::ThreadListSync(sync))) => {
                for ch in &sync.threads {
                    cache_channel(discord, &mut known_channels, ch);
                }
            }
            Some(Ok(Event::MessageUpdate(update))) => {
                inner.invalidate_recent(update.channel_id);
            }
            Some(Ok(Event::MessageDelete(delete))) => {
                inner.invalidate_recent(delete.channel_id);
            }
            Some(Ok(Event::MessageDeleteBulk(delete))) => {
                inner.invalidate_recent(delete.channel_id);
            }
            Some(Ok(Event::MessageCreate(message))) => {
                let Some(bot) = bot else { continue };
                if matches!(message.kind, MessageType::Regular | MessageType::Reply) {
                    inner.record_gateway_message(&message, message.author.id == bot);
                }
                // Other bots' messages flow through the same access rules as
                // human ones (labeled bot-authored downstream); only our own
                // are always skipped — the bot already knows what it said.
                if message.author.id == bot {
                    continue;
                }
                let dm = message.guild_id.is_none();
                let addressed = dm
                    || message.mentions.iter().any(|u| u.id == bot)
                    || message
                        .referenced_message
                        .as_ref()
                        .is_some_and(|m| m.author.id == bot);
                // Unaddressed `always` mode only admits real conversation
                // turns (regular messages and replies), never system notices
                // such as pin or thread-created events.
                if !addressed
                    && !matches!(message.kind, MessageType::Regular | MessageType::Reply)
                {
                    continue;
                }
                let author_id = message.author.id.to_string();
                let guild_id = message.guild_id.map(|id| id.to_string());
                let channel_id = message.channel_id.to_string();
                let pre_filter = discord.pre_filter();
                let cached = (!dm)
                    .then(|| known_channels.get(&message.channel_id).cloned())
                    .flatten();
                let mut resolved_channel = None;
                let decision = match &pre_filter {
                    Some(filter) => {
                        if dm {
                            filter(&author_id, None, &channel_id, None, true, true)
                        } else if let Some(cached) = &cached {
                            match cached.parent {
                                Some(parent) => filter(
                                    &author_id,
                                    guild_id.as_deref(),
                                    &parent.to_string(),
                                    Some(&channel_id),
                                    addressed,
                                    true,
                                ),
                                None => filter(
                                    &author_id,
                                    guild_id.as_deref(),
                                    &channel_id,
                                    None,
                                    addressed,
                                    true,
                                ),
                            }
                        } else {
                            filter(
                                &author_id,
                                guild_id.as_deref(),
                                &channel_id,
                                None,
                                addressed,
                                false,
                            )
                        }
                    }
                    None => {
                        if addressed {
                            PreFilterDecision::Allow
                        } else {
                            PreFilterDecision::Reject
                        }
                    }
                };
                match decision {
                    PreFilterDecision::Reject => continue,
                    PreFilterDecision::NeedParent => {
                        // Channel was uncached and might be a thread whose
                        // parent has an `always` override: resolve it once,
                        // cache it, and re-check before any parent/quote REST.
                        let fetched = match inner.http.channel(message.channel_id).await {
                            Ok(resp) => resp.model().await.map_err(backend),
                            Err(error) => Err(backend(error)),
                        };
                        let ch = match fetched {
                            Ok(ch) => ch,
                            Err(error) => {
                                eprintln!("discord message: {error}");
                                continue;
                            }
                        };
                        let parent = ch.kind.is_thread().then_some(ch.parent_id).flatten();
                        cache_channel(discord, &mut known_channels, &ch);
                        if let Some(filter) = &pre_filter {
                            let recheck = match parent {
                                Some(parent) => filter(
                                    &author_id,
                                    guild_id.as_deref(),
                                    &parent.to_string(),
                                    Some(&channel_id),
                                    addressed,
                                    true,
                                ),
                                None => filter(
                                    &author_id,
                                    guild_id.as_deref(),
                                    &channel_id,
                                    None,
                                    addressed,
                                    true,
                                ),
                            };
                            if recheck != PreFilterDecision::Allow {
                                continue;
                            }
                        }
                        resolved_channel = Some(ch);
                    }
                    PreFilterDecision::Allow => {}
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
                                channel: channel_id.clone(),
                                thread: None,
                                dm: true,
                            },
                            "DM".to_owned(),
                            None,
                        )
                    } else {
                        let channel = match resolved_channel {
                            Some(ch) => ch,
                            None => {
                                let ch = inner
                                    .http
                                    .channel(message.channel_id)
                                    .await
                                    .map_err(backend)?
                                    .model()
                                    .await
                                    .map_err(backend)?;
                                cache_channel(discord, &mut known_channels, &ch);
                                ch
                            }
                        };
                        if channel.kind.is_thread() {
                            let parent = channel.parent_id.ok_or_else(|| {
                                backend(std::io::Error::other("thread has no parent"))
                            })?;
                            let parent_name = match known_channels.get(&parent) {
                                Some(cached_parent) => cached_parent.name.clone(),
                                None => {
                                    let parent_channel = inner
                                        .http
                                        .channel(parent)
                                        .await
                                        .map_err(backend)?
                                        .model()
                                        .await
                                        .map_err(backend)?;
                                    let name = parent_channel.name.unwrap_or_default();
                                    known_channels.insert(
                                        parent,
                                        CachedChannel {
                                            name: name.clone(),
                                            parent: None,
                                        },
                                    );
                                    name
                                }
                            };
                            let location = format!(
                                "#{} › {}",
                                parent_name,
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
                    guild: guild_id,
                    message: message.id.to_string(),
                    author: person(&message.author),
                    text,
                    quote: quote(&inner, &message).await,
                    attachments: attachments(&message.attachments),
                    location,
                    earlier,
                    addressed,
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
