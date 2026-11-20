use crate::{backend, channel, id, Discord};
use interface::{InterfaceError, Outbound, Target};
use twilight_http::{api_error::ApiError, error::ErrorType};
use twilight_model::{
    channel::message::AllowedMentions,
    id::{
        marker::{ChannelMarker, MessageMarker, UserMarker},
        Id,
    },
};

fn has_code(error: &twilight_http::Error, code: u64) -> bool {
    matches!(error.kind(), ErrorType::Response { error: ApiError::General(error), .. } if error.code == code)
}

async fn thread(
    discord: &Discord,
    parent: Id<ChannelMarker>,
    message: Id<MessageMarker>,
) -> Result<(), InterfaceError> {
    let source = discord
        .http
        .message(parent, message)
        .await
        .map_err(backend)?
        .model()
        .await
        .map_err(backend)?;
    let content = source.mentions.iter().fold(source.content, |content, user| {
        let name = format!("@{}", user.name);
        content
            .replace(&format!("<@{}>", user.id), &name)
            .replace(&format!("<@!{}>", user.id), &name)
    });
    let name: String = content.trim().chars().take(100).collect();
    let name = if name.is_empty() { "thread" } else { &name };
    match discord
        .http
        .create_thread_from_message(parent, message, name)
        .await
    {
        Ok(_) => Ok(()),
        Err(error) if has_code(&error, 160004) => Ok(()),
        Err(error) => Err(backend(error)),
    }
}

pub async fn send(discord: &Discord, outbound: Outbound) -> Result<(), InterfaceError> {
    let mut mentions = outbound.mention;
    let mut reference = None;
    let mut retry = None;
    let destination = match outbound.target {
        Target::Reply {
            place,
            message,
            author,
        } if !place.dm && place.thread.is_none() => {
            let parent = channel(&place)?;
            let message = id(&message)?;
            thread(discord, parent, message).await?;
            mentions.retain(|m| m != &author);
            mentions.insert(0, author);
            Id::new(message.get())
        }
        Target::Reply { place, message, .. } => {
            reference = Some(id(&message)?);
            channel(&place)?
        }
        Target::To(place) => {
            if let Some(thread_id) = &place.thread {
                retry = Some((id(&place.channel)?, id(thread_id)?));
            }
            channel(&place)?
        }
    };
    let users: Vec<Id<UserMarker>> = mentions.iter().map(|m| id(m)).collect::<Result<_, _>>()?;
    let allowed = AllowedMentions {
        parse: vec![],
        roles: vec![],
        users,
        replied_user: reference.is_some(),
    };
    let prefix: String = mentions.iter().map(|m| format!("<@{m}> ")).collect();
    let text = format!("{prefix}{}", outbound.text);
    let post = || {
        let request = discord
            .http
            .create_message(destination)
            .content(&text)
            .allowed_mentions(Some(&allowed));
        if let Some(message) = reference {
            request.reply(message).fail_if_not_exists(false)
        } else {
            request
        }
    };
    match post().await {
        Ok(_) => Ok(()),
        Err(error) if has_code(&error, 10003) && retry.is_some() => {
            let (parent, message) = retry.expect("retry checked");
            thread(discord, parent, message).await?;
            post().await.map_err(backend)?;
            Ok(())
        }
        Err(error) => Err(backend(error)),
    }
}
