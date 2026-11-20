pub use storage::{Person, Place};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct Inbound {
    pub place: Place,
    pub message: String,
    pub author: Person,
    pub text: String,
    pub location: String,
    pub earlier: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum Target {
    Reply {
        place: Place,
        message: String,
        author: String,
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
pub enum Scope {
    Place(Place),
    Interface,
}

#[derive(Debug, Clone)]
pub struct Fetched {
    pub message: String,
    pub author: Person,
    pub text: String,
    pub date: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InterfaceError {
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
pub trait Lookup: Send + Sync {
    fn name(&self) -> &'static str;
    async fn history(
        &self,
        place: &Place,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Fetched>, InterfaceError>;
    async fn search(
        &self,
        scope: &Scope,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Fetched>, InterfaceError>;
    async fn participants(
        &self,
        scope: &Scope,
        query: Option<&str>,
    ) -> Result<Vec<Person>, InterfaceError>;
}

#[async_trait::async_trait]
pub trait Interface: Lookup {
    async fn listen(&self, inbound: mpsc::Sender<Inbound>) -> Result<(), InterfaceError>;
    async fn send(&self, outbound: Outbound) -> Result<(), InterfaceError>;
}
