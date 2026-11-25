//! The built-in web chat: one conversation per bot, shown in the web UI. It is
//! a messenger like Discord: the owner's messages reach the bot as
//! `<message messenger="web">` and the bot answers with `send`. Messages live
//! in the bot's `web_messages` table; typing and task status are pushed to
//! open pages as `chat` changes.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use messenger::{
    Fetched, Lookup, Mark, Messenger, MessengerError, Outbound, Person, Place, Scope, Sent, Target,
};
use storage::WebChatStore;
use storage_sqlite::SqliteStorage;
use tokio::sync::mpsc;

/// The messenger name, in `<message messenger="web">` and destinations.
pub const NAME: &str = "web";
/// The web home's stored target: the owner's one conversation.
pub const HOME_TARGET: &str = "dm:owner";
/// A web chat message's size cap.
pub const MAX_TEXT: usize = 20_000;
/// How long one typing signal shows; the bot refreshes it while it works.
const TYPING_SECS: i64 = 12;

/// The owner, the web chat's only author besides the bot.
pub fn owner() -> Person {
    Person {
        id: "owner".into(),
        name: "Owner".into(),
        bot: false,
    }
}

/// The web chat's one place. It's a DM, so replies never open threads.
pub fn place() -> Place {
    Place {
        channel: NAME.into(),
        thread: None,
        dm: true,
    }
}

/// A message the owner posted, on its way to the bot's chat loop.
pub struct Posted {
    pub id: String,
    pub text: String,
}

#[derive(Default)]
struct Status {
    /// Owner messages with a background task running (the `Working` mark).
    working: HashSet<String>,
    /// Unix seconds until which the bot shows as typing.
    typing_until: i64,
}

pub struct WebChat {
    store: Arc<SqliteStorage>,
    status: Mutex<Status>,
    /// Set while the bot's chat loop runs; `None` while it is stopped or paused.
    inbound: Mutex<Option<mpsc::Sender<Posted>>>,
    /// Tells open pages the status changed (messages notify on their own).
    notify: Box<dyn Fn() + Send + Sync>,
}

impl WebChat {
    pub fn new(store: Arc<SqliteStorage>, notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            store,
            status: Mutex::default(),
            inbound: Mutex::default(),
            notify: Box::new(notify),
        }
    }

    /// Connects the running chat loop; the returned receiver yields posts.
    pub fn attach(&self) -> mpsc::Receiver<Posted> {
        let (tx, rx) = mpsc::channel(64);
        *self.inbound.lock().expect("inbound mutex poisoned") = Some(tx);
        rx
    }

    /// Disconnects the chat loop and clears its status.
    pub fn detach(&self) {
        *self.inbound.lock().expect("inbound mutex poisoned") = None;
        *self.status.lock().expect("status mutex poisoned") = Status::default();
        (self.notify)();
    }

    /// Stores an owner message and hands it to the bot. Fails, storing
    /// nothing, while the bot isn't running.
    pub fn post(&self, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("Message is empty".into());
        }
        if text.chars().count() > MAX_TEXT {
            return Err(format!("Message is over {MAX_TEXT} characters"));
        }
        let sender = self
            .inbound
            .lock()
            .expect("inbound mutex poisoned")
            .clone()
            .ok_or("The bot isn't running")?;
        let permit = sender.try_reserve().map_err(|_| "The bot is busy; try again")?;
        let id = self
            .store
            .add_web_message(false, text, now())
            .map_err(|error| error.to_string())?;
        permit.send(Posted {
            id: id.to_string(),
            text: text.to_owned(),
        });
        Ok(())
    }

    /// Whether any background task is working on the owner's messages, and
    /// until when the bot shows as typing (Unix seconds, `None` if not).
    pub fn status(&self) -> (bool, Option<i64>) {
        let status = self.status.lock().expect("status mutex poisoned");
        let typing = (status.typing_until > now()).then_some(status.typing_until);
        (!status.working.is_empty(), typing)
    }

    fn update(&self, change: impl FnOnce(&mut Status)) {
        change(&mut self.status.lock().expect("status mutex poisoned"));
        (self.notify)();
    }
}

#[async_trait::async_trait]
impl Lookup for WebChat {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn message(&self, _place: &Place, id: &str) -> Result<Fetched, MessengerError> {
        let parsed = id
            .parse::<i64>()
            .map_err(|_| MessengerError::Unsupported(format!("no web message {id}")))?;
        let message = self
            .store
            .web_message(parsed)
            .map_err(|error| MessengerError::Backend(error.into()))?
            .ok_or_else(|| MessengerError::Unsupported(format!("no web message {id}")))?;
        Ok(Fetched {
            message: message.id.to_string(),
            author: if message.bot {
                Person {
                    id: "bot".into(),
                    name: "Bot".into(),
                    bot: true,
                }
            } else {
                owner()
            },
            text: message.text,
            date: message.created_at.to_string(),
            attachments: Vec::new(),
        })
    }

    fn parse_link(&self, _link: &str) -> Option<(Place, String)> {
        None
    }

    // Every web message is already in the bot's memory, so there is nothing
    // for history or search to add.
    async fn history(
        &self,
        _place: &Place,
        _before: Option<&str>,
        _limit: u32,
    ) -> Result<Vec<Fetched>, MessengerError> {
        Err(MessengerError::Unsupported(
            "the web chat is already in memory".into(),
        ))
    }

    async fn search(
        &self,
        _scope: &Scope,
        _query: &str,
        _limit: u32,
    ) -> Result<Vec<Fetched>, MessengerError> {
        Err(MessengerError::Unsupported(
            "the web chat is already in memory".into(),
        ))
    }

    async fn participants(
        &self,
        _scope: &Scope,
        _query: Option<&str>,
    ) -> Result<Vec<Person>, MessengerError> {
        Ok(vec![owner()])
    }
}

#[async_trait::async_trait]
impl Messenger for WebChat {
    async fn direct(&self, _user: &str) -> Result<Place, MessengerError> {
        Ok(place())
    }

    /// Web posts reach the chat loop through `attach`, not the shared
    /// inbound channel, since they skip Discord's admission rules.
    async fn listen(&self, _inbound: mpsc::Sender<messenger::Inbound>) -> Result<(), MessengerError> {
        std::future::pending().await
    }

    async fn react(&self, _place: &Place, message: &str, mark: Mark) -> Result<(), MessengerError> {
        if mark == Mark::Working {
            self.update(|status| {
                status.working.insert(message.to_owned());
            });
        }
        Ok(())
    }

    async fn unreact(
        &self,
        _place: &Place,
        message: &str,
        mark: Mark,
    ) -> Result<(), MessengerError> {
        if mark == Mark::Working {
            self.update(|status| {
                status.working.remove(message);
            });
        }
        Ok(())
    }

    async fn typing(&self, _place: &Place) -> Result<(), MessengerError> {
        self.update(|status| status.typing_until = now() + TYPING_SECS);
        Ok(())
    }

    async fn send(&self, outbound: Outbound) -> Result<Sent, MessengerError> {
        if !outbound.files.is_empty() {
            return Err(MessengerError::Unsupported(
                "the web chat takes text only, not files".into(),
            ));
        }
        let place = match &outbound.target {
            Target::Reply { place, .. } | Target::To { place, .. } => place,
        };
        if place.channel != NAME {
            return Err(MessengerError::Unsupported("unknown web chat place".into()));
        }
        // Stop showing typing first, so the reply lands with no stale dots.
        self.status
            .lock()
            .expect("status mutex poisoned")
            .typing_until = 0;
        self.store
            .add_web_message(true, &outbound.text, now())
            .map_err(|error| MessengerError::Backend(error.into()))?;
        Ok(Sent {
            created_thread: false,
            used_thread: false,
        })
    }

    async fn rename(&self, _place: &Place, _name: &str) -> Result<(), MessengerError> {
        Ok(())
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
