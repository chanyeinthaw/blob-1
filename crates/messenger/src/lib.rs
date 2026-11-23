pub use storage::{Attachment, Person, Place, Quote};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct Inbound {
    pub place: Place,
    /// Discord guild id when the message arrived in a server; `None` for DMs.
    pub guild: Option<String>,
    pub message: String,
    pub author: Person,
    pub text: String,
    pub quote: Option<Quote>,
    pub attachments: Vec<Attachment>,
    pub location: String,
    pub earlier: Option<u32>,
    /// `true` for DMs, mentions of the bot and replies to the bot; `false`
    /// for plain channel or thread messages the bot was not addressed by.
    pub addressed: bool,
}

#[derive(Debug, Clone)]
pub enum Target {
    Reply {
        place: Place,
        message: String,
        author: String,
        first: bool,
        thread: bool,
    },
    To {
        place: Place,
        create_thread: bool,
        fallback_parent: bool,
    },
}

#[derive(Debug, Clone)]
pub struct OutboundFile {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Outbound {
    pub target: Target,
    pub text: String,
    pub mention: Vec<String>,
    pub files: Vec<OutboundFile>,
}

#[derive(Debug, Clone)]
pub struct Sent {
    pub created_thread: bool,
    pub used_thread: bool,
}

#[derive(Debug, Clone)]
pub enum Scope {
    Place(Place),
    Messenger,
}

#[derive(Debug, Clone)]
pub struct Fetched {
    pub message: String,
    pub author: Person,
    pub text: String,
    pub date: String,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, thiserror::Error)]
pub enum MessengerError {
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
pub trait Lookup: Send + Sync {
    fn name(&self) -> &'static str;
    /// Connected bot user ID when known, so shared triage can distinguish assistant turns.
    fn self_id(&self) -> Option<String> {
        None
    }
    /// Returns cached recent history before `before` (oldest -> newest, up to `limit`)
    /// for a channel/thread the bot is actively participating in, avoiding a REST call.
    fn recent_history(
        &self,
        _place: &Place,
        _before: Option<&str>,
        _limit: usize,
    ) -> Option<Vec<Fetched>> {
        None
    }
    /// Begins transiently collecting live gateway arrivals for a cold `place`
    /// while triage/reply-mode lookup is in flight, returning a globally unique
    /// cache token for this warming hold.
    fn begin_warming(&self, _place: &Place) -> u64 {
        0
    }
    /// Releases one warming hold for `token`, removing the transient warming buffer
    /// once no holders remain (no-op if already promoted or invalidated).
    fn cancel_warming(&self, _place: &Place, _token: u64) {}
    /// Promotes and seeds a warming buffer matching `token` into the participating
    /// cache. Rejects seeds if the entry was invalidated, evicted, or belongs to
    /// an earlier connection/session.
    fn seed_history(
        &self,
        _place: &Place,
        _messages: &[Fetched],
        _complete_from_start: bool,
        _token: u64,
    ) {
    }
    /// Refreshes TTL on an already-participating channel without overwriting entries.
    fn touch_history(&self, _place: &Place) {}
    /// Guild (server) id `channel_id` belongs to; `Ok(None)` for DMs and
    /// guild-less messengers, `Err` when the lookup itself fails.
    async fn channel_guild(&self, _channel_id: &str) -> Result<Option<String>, MessengerError> {
        Ok(None)
    }
    async fn message(&self, place: &Place, id: &str) -> Result<Fetched, MessengerError>;
    fn parse_link(&self, link: &str) -> Option<(Place, String)>;
    async fn history(
        &self,
        place: &Place,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Fetched>, MessengerError>;
    async fn search(
        &self,
        scope: &Scope,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Fetched>, MessengerError>;
    async fn participants(
        &self,
        scope: &Scope,
        query: Option<&str>,
    ) -> Result<Vec<Person>, MessengerError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Seen,
    Working,
    Done,
    Failed,
}

#[async_trait::async_trait]
pub trait Messenger: Lookup {
    async fn direct(&self, user: &str) -> Result<Place, MessengerError>;
    async fn listen(&self, inbound: mpsc::Sender<Inbound>) -> Result<(), MessengerError>;
    async fn react(&self, place: &Place, message: &str, mark: Mark) -> Result<(), MessengerError>;
    async fn unreact(&self, place: &Place, message: &str, mark: Mark)
        -> Result<(), MessengerError>;
    async fn typing(&self, place: &Place) -> Result<(), MessengerError>;
    async fn send(&self, outbound: Outbound) -> Result<Sent, MessengerError>;
    async fn rename(&self, place: &Place, name: &str) -> Result<(), MessengerError>;
}
