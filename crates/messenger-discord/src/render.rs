use crate::{backend, channel, id, Discord, Inner};
use messenger::{MessengerError, Outbound, Sent, Target};
use twilight_http::{api_error::ApiError, error::ErrorType};
use twilight_model::{
    channel::message::AllowedMentions,
    id::{
        marker::{ChannelMarker, MessageMarker, UserMarker},
        Id,
    },
};

pub(crate) fn has_code(error: &twilight_http::Error, code: u64) -> bool {
    matches!(error.kind(), ErrorType::Response { error: ApiError::General(error), .. } if error.code == code)
}

async fn thread(
    discord: &Discord,
    inner: &Inner,
    parent: Id<ChannelMarker>,
    message: Id<MessageMarker>,
) -> Result<bool, MessengerError> {
    let source = inner
        .http
        .message(parent, message)
        .await
        .map_err(backend)?
        .model()
        .await
        .map_err(backend)?;
    let content = source
        .mentions
        .iter()
        .fold(source.content, |content, user| {
            let name = format!("@{}", user.name);
            content
                .replace(&format!("<@{}>", user.id), &name)
                .replace(&format!("<@!{}>", user.id), &name)
        });
    let name: String = content.trim().chars().take(100).collect();
    let name = if name.is_empty() { "thread" } else { &name };
    let created = match inner
        .http
        .create_thread_from_message(parent, message, name)
        .await
    {
        Ok(_) => true,
        Err(error) if has_code(&error, 160004) => false,
        Err(error) => return Err(backend(error)),
    };
    discord.notify_thread_seen(&parent.to_string(), &message.to_string());
    Ok(created)
}

enum MissingThreadAction {
    CreateThread(Id<ChannelMarker>, Id<MessageMarker>),
    FallbackParent(Id<ChannelMarker>),
}

pub async fn send(
    discord: &Discord,
    inner: &Inner,
    outbound: Outbound,
) -> Result<Sent, MessengerError> {
    let mut created_thread = false;
    let mut used_thread = false;
    let mut mentions = outbound.mention;
    let mut reference = None;
    let mut reply_author = None;
    let mut missing_thread = None;
    let mut continuing = false;
    let destination = match outbound.target {
        Target::Reply {
            place,
            message,
            author,
            first,
            thread: true,
        } if !place.dm && place.thread.is_none() => {
            continuing = !first;
            used_thread = true;
            let parent = channel(&place)?;
            let message = id(&message)?;
            if first {
                created_thread = thread(discord, inner, parent, message).await?;
            }
            if !first {
                mentions.retain(|m| m != &author);
            }
            reply_author = Some(author);
            Id::new(message.get())
        }
        Target::Reply {
            place,
            message,
            author,
            first,
            ..
        } => {
            continuing = !first;
            used_thread = place.thread.is_some();
            if continuing {
                mentions.retain(|m| m != &author);
            }
            reference = Some(id(&message)?);
            channel(&place)?
        }
        Target::To {
            place,
            create_thread,
            fallback_parent,
        } => {
            if let Some(thread_id) = &place.thread {
                used_thread = true;
                let parent = id(&place.channel)?;
                if create_thread {
                    missing_thread = Some(MissingThreadAction::CreateThread(parent, id(thread_id)?));
                } else if fallback_parent {
                    missing_thread = Some(MissingThreadAction::FallbackParent(parent));
                }
            }
            channel(&place)?
        }
    };
    let place = messenger::Place {
        channel: destination.to_string(),
        thread: None,
        dm: false,
    };
    let ping = !continuing
        && (reply_author.is_some() || reference.is_some())
        && inner
            .ping_state(&place)
            .await
            .map(|(_, _, ping)| ping)
            .unwrap_or(true);
    if let Some(author) = reply_author.filter(|_| ping) {
        mentions.retain(|m| m != &author);
        mentions.insert(0, author);
    }
    let replied_user = reference.is_some() && ping;
    let users: Vec<Id<UserMarker>> = mentions.iter().map(|m| id(m)).collect::<Result<_, _>>()?;
    let allowed = AllowedMentions {
        parse: vec![],
        roles: vec![],
        users,
        replied_user,
    };
    let prefix: String = mentions.iter().map(|m| format!("<@{m}> ")).collect();
    let text = format!("{prefix}{}", outbound.text);
    let post = |dest: Id<ChannelMarker>| {
        let request = inner
            .http
            .create_message(dest)
            .content(&text)
            .allowed_mentions(Some(&allowed));
        if let Some(message) = reference {
            request.reply(message).fail_if_not_exists(false)
        } else {
            request
        }
    };
    match post(destination).await {
        Ok(_) => Ok(Sent {
            created_thread,
            used_thread,
        }),
        Err(error) if has_code(&error, 10003) && missing_thread.is_some() => {
            match missing_thread.expect("missing_thread checked") {
                MissingThreadAction::CreateThread(parent, message) => {
                    created_thread = thread(discord, inner, parent, message).await?;
                    post(destination).await.map_err(backend)?;
                    Ok(Sent {
                        created_thread,
                        used_thread: true,
                    })
                }
                MissingThreadAction::FallbackParent(parent) => {
                    post(parent).await.map_err(backend)?;
                    Ok(Sent {
                        created_thread: false,
                        used_thread: false,
                    })
                }
            }
        }
        Err(error) => Err(backend(error)),
    }
}
