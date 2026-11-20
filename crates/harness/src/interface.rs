use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use environment::ToolSpec;
use interface::{Fetched, Interface, Outbound, Place, Scope, Target};
use serde::Deserialize;
use storage::{InterfaceStore, MemoryStore, Meta};

type Error = Box<dyn std::error::Error + Send + Sync>;

pub const SEND: ToolSpec = ToolSpec {
    name: "send",
    description: "Send a message to people on an interface. Give exactly one of reply (an m-token from a <message> tag: answers that message in its thread, starting one if needed) or to (a d-token: posts plainly in that conversation). At most 250 characters; split longer text into several sends. mention: user ids to mention; the system adds the mentions.",
    input_schema: r#"{"type":"object","properties":{"text":{"type":"string"},"reply":{"type":"string","description":"m-token of the message to reply to"},"to":{"type":"string","description":"d-token of the conversation"},"mention":{"type":"array","items":{"type":"string"},"description":"User ids to mention"}},"required":["text"]}"#,
};
pub const LOOKUP: &[ToolSpec] = &[
    ToolSpec {
        name: "history",
        description: "Read earlier messages of a conversation (to = d-token), oldest first; channel: true reads the thread's parent channel instead. before: a message id to page further back. limit 1-50, default 20.",
        input_schema: r#"{"type":"object","properties":{"to":{"type":"string","description":"d-token of the conversation"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread"},"before":{"type":"string","description":"Message id to page before"},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["to"]}"#,
    },
    ToolSpec {
        name: "search",
        description: "Search messages containing query, in one conversation (to = d-token, channel: true for its parent channel) or across a whole interface (interface = name, if supported). limit 1-50, default 20.",
        input_schema: r#"{"type":"object","properties":{"query":{"type":"string"},"to":{"type":"string","description":"d-token of the conversation"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread"},"interface":{"type":"string","description":"Interface name for an interface-wide lookup"},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["query"]}"#,
    },
    ToolSpec {
        name: "participants",
        description: "Find people and their user ids, in one conversation (to, channel) or across a whole interface (interface + query). Use the ids with send's mention.",
        input_schema: r#"{"type":"object","properties":{"to":{"type":"string","description":"d-token of the conversation"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread"},"interface":{"type":"string","description":"Interface name for an interface-wide lookup"},"query":{"type":"string"}}}"#,
    },
];

trait Store: InterfaceStore + MemoryStore {}
impl<T: InterfaceStore + MemoryStore> Store for T {}

#[derive(Clone)]
pub struct Interfaces {
    interfaces: Vec<Arc<dyn Interface>>,
    store: Arc<dyn Store>,
    live: Arc<LiveState>,
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
    to: Option<String>,
    interface: Option<String>,
    query: Option<String>,
    #[serde(default)]
    channel: bool,
    before: Option<String>,
    limit: Option<u32>,
}

fn token(value: &str, prefix: char) -> Result<u64, Error> {
    Ok(value.strip_prefix(prefix).ok_or("invalid token")?.parse()?)
}
fn render(messages: Vec<Fetched>) -> String {
    if messages.is_empty() {
        return "no messages".into();
    }
    messages
        .into_iter()
        .map(|m| {
            format!(
                "{} {} ({}) [{}]: {}",
                m.date, m.author.name, m.author.id, m.message, m.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
impl Interfaces {
    pub fn new<S: InterfaceStore + MemoryStore + 'static>(
        interfaces: Vec<Arc<dyn Interface>>,
        store: S,
    ) -> Self {
        Self {
            interfaces,
            store: Arc::new(store),
            live: Arc::default(),
        }
    }
    pub fn live(&self, interface: &str, to: u64, place: Place) {
        let interface = self
            .interfaces
            .iter()
            .find(|i| i.name() == interface)
            .expect("unknown interface")
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
                if let Err(error) = interface.typing(&place).await {
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
    }
    pub fn quiet(&self) {
        for (_, pulse) in self.live.0.lock().expect("live mutex poisoned").drain() {
            pulse.abort();
        }
    }
    fn find(&self, name: &str) -> Result<&dyn Interface, Error> {
        self.interfaces
            .iter()
            .find(|i| i.name() == name)
            .map(Arc::as_ref)
            .ok_or_else(|| format!("unknown interface: {name}").into())
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
        match (&args.to, &args.interface) {
            (Some(to), None) => {
                let (name, place) = self.place(to, args.channel)?;
                Ok((name, Scope::Place(place)))
            }
            (None, Some(name)) => Ok((name.clone(), Scope::Interface)),
            _ => Err("exactly one of to or interface is required".into()),
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
                        interface,
                        place,
                        message,
                        author,
                        to,
                        ..
                    }) => (
                        interface,
                        Target::Reply {
                            place,
                            message,
                            author: author.id,
                        },
                        to,
                    ),
                    _ => return Err("message has no interface metadata".into()),
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
            "history" => {
                let to = args.to.as_deref().ok_or("to is required")?;
                let (name, place) = self.place(to, args.channel)?;
                Ok(render(
                    self.find(&name)?
                        .history(&place, args.before.as_deref(), limit)
                        .await?,
                ))
            }
            "search" => {
                let (name, scope) = self.scope(&args)?;
                let query = args.query.as_deref().ok_or("query is required")?;
                Ok(render(
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
                    .map(|p| format!("{} ({})", p.name, p.id))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            _ => Err(format!("unknown tool: {name}").into()),
        }
    }
}
