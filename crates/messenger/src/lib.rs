pub use storage::{Attachment, Person, Place, Quote};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct Inbound {
    pub place: Place,
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
    To(Place),
}

#[derive(Debug, Clone)]
pub struct Outbound {
    pub target: Target,
    pub text: String,
    pub mention: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Sent {
    pub created_thread: bool,
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
