use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use environment::ToolSpec;
use messenger::{Fetched, Mark, Messenger, Outbound, Place, Scope, Target};
use serde::Deserialize;
use storage::{MemoryStore, MessengerStore, Meta};

type Error = Box<dyn std::error::Error + Send + Sync>;

pub const SEND: ToolSpec = ToolSpec {
    name: "send",
    description: "Send a message to people on a messenger. Give exactly one of reply (an m-token from a <message> tag: answers that message in its thread, starting one if needed) or to (a d-token: posts plainly in that conversation). At most 250 characters; split longer text into several sends. mention: user ids to mention; the system adds the mentions.",
    input_schema: r#"{"type":"object","properties":{"text":{"type":"string"},"reply":{"type":"string","description":"m-token of the message to reply to"},"to":{"type":"string","description":"d-token of the conversation"},"mention":{"type":"array","items":{"type":"string"},"description":"User ids to mention"}},"required":["text"]}"#,
};
pub const LOOKUP: &[ToolSpec] = &[
    ToolSpec {
        name: "message",
        description: "Read one message: by link (a message link someone pasted), or by to (d-token) plus message id (ids appear in history/search results); channel: true means the thread's parent channel.",
        input_schema: r#"{"type":"object","properties":{"link":{"type":"string"},"to":{"type":"string"},"message":{"type":"string"},"channel":{"type":"boolean"}}}"#,
    },
    ToolSpec {
        name: "history",
        description: "Read earlier messages of a conversation (to = d-token), oldest first; channel: true reads the thread's parent channel instead. before: a message id to page further back. limit 1-50, default 20.",
        input_schema: r#"{"type":"object","properties":{"to":{"type":"string","description":"d-token of the conversation"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread"},"before":{"type":"string","description":"Message id to page before"},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["to"]}"#,
    },
    ToolSpec {
        name: "search",
        description: "Search messages containing query, in one conversation (to = d-token, channel: true for its parent channel) or across a whole messenger (messenger = name, if supported). limit 1-50, default 20.",
        input_schema: r#"{"type":"object","properties":{"query":{"type":"string"},"to":{"type":"string","description":"d-token of the conversation"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread"},"messenger":{"type":"string","description":"Messenger name for a messenger-wide lookup"},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["query"]}"#,
    },
    ToolSpec {
        name: "participants",
        description: "Find people and their user ids, in one conversation (to, channel) or across a whole messenger (messenger + query). Use the ids with send's mention.",
        input_schema: r#"{"type":"object","properties":{"to":{"type":"string","description":"d-token of the conversation"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread"},"messenger":{"type":"string","description":"Messenger name for a messenger-wide lookup"},"query":{"type":"string"}}}"#,
    },
];

trait Store: MessengerStore + MemoryStore {}
impl<T: MessengerStore + MemoryStore> Store for T {}

#[derive(Clone)]
pub struct Messengers {
    messengers: Vec<Arc<dyn Messenger>>,
    store: Arc<dyn Store>,
    live: Arc<LiveState>,
    marks: tokio::sync::mpsc::UnboundedSender<MarkJob>,
}

struct MarkJob {
    messenger: Arc<dyn Messenger>,
    place: Place,
    message: String,
    add: Vec<Mark>,
    remove: Vec<Mark>,
}

#[derive(Default)]
struct LiveState(Mutex<HashMap<u64, tokio::task::AbortHandle>>);

impl Drop for LiveState {
    fn drop(&mut self) {
        for (_, pulse) in self.0.get_mut().expect("live mutex poisoned").drain() {
            pulse.abort();
        }
    }
}

#[derive(Deserialize)]
struct SendArgs {
    text: String,
    reply: Option<String>,
    to: Option<String>,
    #[serde(default)]
    mention: Vec<String>,
}
#[derive(Deserialize)]
struct LookupArgs {
    link: Option<String>,
    message: Option<String>,
    to: Option<String>,
    messenger: Option<String>,
    query: Option<String>,
    #[serde(default)]
    channel: bool,
    before: Option<String>,
    limit: Option<u32>,
}

fn token(value: &str, prefix: char) -> Result<u64, Error> {
    Ok(value.strip_prefix(prefix).ok_or("invalid token")?.parse()?)
}
fn element(m: &Fetched) -> String {
    format!(
        "<message author=\"{}\" author_name=\"{}\" message=\"{}\" date=\"{}\">\n{}{}\n</message>",
        storage::attribute(&m.author.id),
        storage::attribute(&m.author.name),
        storage::attribute(&m.message),
        storage::attribute(&m.date),
        storage::render_attachments(&m.attachments),
        storage::escape(&m.text)
    )
}
fn fit<'a>(messages: impl Iterator<Item = &'a Fetched>) -> Vec<(&'a Fetched, String)> {
    let mut kept = Vec::new();
    let mut size = 0;
    for m in messages {
        let line = element(m);
        size += line.chars().count() + 1;
        if size + 100 > memory::TOOL_OUTPUT_LIMIT {
            break;
        }
        kept.push((m, line));
    }
    kept
}
fn render_history(messages: Vec<Fetched>) -> String {
    if messages.is_empty() {
        return "no messages".into();
    }
    let mut kept = fit(messages.iter().rev());
    kept.reverse();
    let lines: Vec<_> = kept.iter().map(|(_, line)| line.as_str()).collect();
    match (messages.len() - kept.len(), kept.first()) {
        (0, _) => lines.join("\n"),
        (older, None) => format!(
            "[{older} messages not shown: the newest is too large to show; page with before=\"{}\"]",
            messages.last().expect("non-empty").message
        ),
        (older, Some((oldest, _))) => format!(
            "[{older} older messages not shown; page with before=\"{}\"]\n{}",
            oldest.message,
            lines.join("\n")
        ),
    }
}
fn render_search(messages: Vec<Fetched>) -> String {
    if messages.is_empty() {
        return "no messages".into();
    }
    let kept = fit(messages.iter());
    let lines: Vec<_> = kept.iter().map(|(_, line)| line.as_str()).collect();
    match messages.len() - kept.len() {
        0 => lines.join("\n"),
        more => format!("{}\n[{more} more matches not shown]", lines.join("\n")),
    }
}
impl Messengers {
    pub fn new<S: MessengerStore + MemoryStore + 'static>(
        messengers: Vec<Arc<dyn Messenger>>,
        store: S,
    ) -> Self {
        Self {
            messengers,
            store: Arc::new(store),
            live: Arc::default(),
            marks: {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MarkJob>();
                tokio::spawn(async move {
                    while let Some(job) = rx.recv().await {
                        for mark in job.remove {
                            if let Err(error) =
                                job.messenger.unreact(&job.place, &job.message, mark).await
                            {
                                eprintln!("{error}");
                            }
                        }
                        for mark in job.add {
                            if let Err(error) =
                                job.messenger.react(&job.place, &job.message, mark).await
                            {
                                eprintln!("{error}");
                            }
                        }
                    }
                });
                tx
            },
        }
    }
    pub fn mark(&self, reply: &str, add: &[Mark], remove: &[Mark]) {
        let resolved = (|| -> Result<_, Error> {
            let row = self
                .store
                .message(token(reply, 'm')?)?
                .ok_or("unknown message")?;
            let Some(Meta::Message {
                messenger,
                place,
                message,
                ..
            }) = row.meta
            else {
                return Err("message has no messenger metadata".into());
            };
            let messenger = self
                .messengers
                .iter()
                .find(|i| i.name() == messenger)
                .ok_or("unknown messenger")?
                .clone();
            Ok((messenger, place, message))
        })();
        let (messenger, place, message) = match resolved {
            Ok(value) => value,
            Err(error) => {
                eprintln!("{reply}: {error}");
                return;
            }
        };
        if self
            .marks
            .send(MarkJob {
                messenger,
                place,
                message,
                add: add.to_vec(),
                remove: remove.to_vec(),
            })
            .is_err()
        {
            eprintln!("mark worker stopped");
        }
    }
    pub fn live(&self, messenger: &str, to: u64, place: Place) -> tokio::task::Id {
        let messenger = self
            .messengers
            .iter()
            .find(|i| i.name() == messenger)
            .expect("unknown messenger")
            .clone();
        let pulse = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            let mut ticks = tokio::time::interval(Duration::from_secs(8));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                if let Err(error) = messenger.typing(&place).await {
                    eprintln!("{error}");
                }
            }
        });
        if let Some(old) = self
            .live
            .0
            .lock()
            .expect("live mutex poisoned")
            .insert(to, pulse.abort_handle())
        {
            old.abort();
        }
        pulse.id()
    }
    pub fn live_for(&self, reply: &str) -> Result<Option<(u64, tokio::task::Id)>, Error> {
        let row = self
            .store
            .message(token(reply, 'm')?)?
            .ok_or("unknown message")?;
        if let Some(Meta::Message {
            messenger,
            place,
            to,
            ..
        }) = row.meta
        {
            return Ok(Some((to, self.live(&messenger, to, place))));
        }
        Ok(None)
    }
    pub fn settle(&self, to: u64, pulse: tokio::task::Id) {
        let mut live = self.live.0.lock().expect("live mutex poisoned");
        if live.get(&to).is_some_and(|handle| handle.id() == pulse) {
            live.remove(&to).expect("checked").abort();
        }
    }
    pub fn quiet(&self) {
        for (_, pulse) in self.live.0.lock().expect("live mutex poisoned").drain() {
            pulse.abort();
        }
    }
    fn find(&self, name: &str) -> Result<&dyn Messenger, Error> {
        self.messengers
            .iter()
            .find(|i| i.name() == name)
            .map(Arc::as_ref)
            .ok_or_else(|| format!("unknown messenger: {name}").into())
    }
    fn place(&self, value: &str, channel: bool) -> Result<(String, Place), Error> {
        let (name, mut place) = self
            .store
            .place(token(value, 'd')?)?
            .ok_or("unknown destination")?;
        if channel {
            place.thread = None;
        }
        Ok((name, place))
    }
    fn scope(&self, args: &LookupArgs) -> Result<(String, Scope), Error> {
        match (&args.to, &args.messenger) {
            (Some(to), None) => {
                let (name, place) = self.place(to, args.channel)?;
                Ok((name, Scope::Place(place)))
            }
            (None, Some(name)) => Ok((name.clone(), Scope::Messenger)),
            _ => Err("exactly one of to or messenger is required".into()),
        }
    }
    pub async fn send(&self, arguments: &str) -> Result<String, Error> {
        let args: SendArgs = serde_json::from_str(arguments)?;
        if args.text.is_empty() {
            return Err("text must not be empty".into());
        }
        if args.text.chars().count() > 250 {
            return Err("text exceeds 250 characters; shorten or split into several sends".into());
        }
        let (name, target, destination) = match (args.reply, args.to) {
            (Some(reply), None) => {
                let row = self
                    .store
                    .message(token(&reply, 'm')?)?
                    .ok_or("unknown message")?;
                match row.meta {
                    Some(Meta::Message {
                        messenger,
                        place,
                        message,
                        author,
                        to,
                        ..
                    }) => (
                        messenger,
                        Target::Reply {
                            place,
                            message,
                            author: author.id,
                        },
                        to,
                    ),
                    _ => return Err("message has no messenger metadata".into()),
                }
            }
            (None, Some(to)) => {
                let (name, place) = self.place(&to, false)?;
                (name, Target::To(place), token(&to, 'd')?)
            }
            _ => return Err("exactly one of reply or to is required".into()),
        };
        self.find(&name)?
            .send(Outbound {
                target,
                text: args.text,
                mention: args.mention,
            })
            .await?;
        if let Some(pulse) = self
            .live
            .0
            .lock()
            .expect("live mutex poisoned")
            .remove(&destination)
        {
            pulse.abort();
        }
        Ok("sent".into())
    }
    pub async fn lookup(&self, name: &str, arguments: &str) -> Result<String, Error> {
        let args: LookupArgs = serde_json::from_str(arguments)?;
        let limit = args.limit.unwrap_or(20);
        if !(1..=50).contains(&limit) {
            return Err("limit must be between 1 and 50".into());
        }
        match name {
            "message" => {
                let (messenger, place, message) = match (&args.link, &args.to, &args.message) {
                    (Some(link), None, None) => {
                        let (messenger, (place, message)) = self
                            .messengers
                            .iter()
                            .find_map(|messenger| {
                                messenger
                                    .parse_link(link)
                                    .map(|parsed| (messenger.as_ref(), parsed))
                            })
                            .ok_or("not a message link")?;
                        (messenger, place, message)
                    }
                    (None, Some(to), Some(message)) => {
                        let (name, place) = self.place(to, args.channel)?;
                        (self.find(&name)?, place, message.clone())
                    }
                    _ => return Err("exactly one of link or to plus message is required".into()),
                };
                Ok(element(&messenger.message(&place, &message).await?))
            }
            "history" => {
                let to = args.to.as_deref().ok_or("to is required")?;
                let (name, place) = self.place(to, args.channel)?;
                Ok(render_history(
                    self.find(&name)?
                        .history(&place, args.before.as_deref(), limit)
                        .await?,
                ))
            }
            "search" => {
                let (name, scope) = self.scope(&args)?;
                let query = args.query.as_deref().ok_or("query is required")?;
                Ok(render_search(
                    self.find(&name)?.search(&scope, query, limit).await?,
                ))
            }
            "participants" => {
                let (name, scope) = self.scope(&args)?;
                let people = self
                    .find(&name)?
                    .participants(&scope, args.query.as_deref())
                    .await?;
                if people.is_empty() {
                    return Ok("no people".into());
                }
                Ok(people
                    .into_iter()
                    .map(|p| format!("{} ({})", storage::escape(&p.name), storage::escape(&p.id)))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            _ => Err(format!("unknown tool: {name}").into()),
        }
    }
}
