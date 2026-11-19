use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Summary {
        text: String,
        read_files: Vec<String>,
        modified_files: Vec<String>,
    },
    User(String),
    Assistant(String),
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        id: String,
        name: String,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub messages: Vec<Message>,
}

impl Session {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            messages: Vec::new(),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("storage failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

pub trait SessionStore: Send + Sync {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError>;
    fn save(&self, session: Session) -> Result<(), StoreError>;
}

impl<T: SessionStore + ?Sized> SessionStore for std::sync::Arc<T> {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        (**self).load(id)
    }

    fn save(&self, session: Session) -> Result<(), StoreError> {
        (**self).save(session)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tree {
    Workspace,
    Skills,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub mode: u32,
    pub size: u64,
    pub modified: std::time::SystemTime,
    pub linked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub node: Node,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("no such file or directory")]
    NotFound,
    #[error("file exists")]
    AlreadyExists,
    #[error("not a directory")]
    NotADirectory,
    #[error("is a directory")]
    IsADirectory,
    #[error("directory not empty")]
    NotEmpty,
    #[error("invalid argument")]
    Invalid,
    #[error("file too large")]
    TooLarge,
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub trait WorkspaceStore: Send + Sync {
    fn root(&self, tree: Tree) -> Result<NodeId, WorkspaceError>;
    fn node(&self, id: NodeId) -> Result<Node, WorkspaceError>;
    fn lookup(&self, parent: NodeId, name: &str) -> Result<Node, WorkspaceError>;
    fn children(&self, parent: NodeId) -> Result<Vec<DirEntry>, WorkspaceError>;
    fn create(
        &self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        mode: u32,
    ) -> Result<Node, WorkspaceError>;
    fn read(&self, id: NodeId, offset: u64, size: u32) -> Result<Vec<u8>, WorkspaceError>;
    fn write(&self, id: NodeId, offset: u64, data: &[u8]) -> Result<Node, WorkspaceError>;
    fn truncate(&self, id: NodeId, size: u64) -> Result<Node, WorkspaceError>;
    fn set_mode(&self, id: NodeId, mode: u32) -> Result<Node, WorkspaceError>;
    fn set_modified(
        &self,
        id: NodeId,
        modified: std::time::SystemTime,
    ) -> Result<Node, WorkspaceError>;
    fn remove(&self, parent: NodeId, name: &str, kind: NodeKind) -> Result<(), WorkspaceError>;
    fn rename(
        &self,
        parent: NodeId,
        name: &str,
        new_parent: NodeId,
        new_name: &str,
        replace: bool,
    ) -> Result<(), WorkspaceError>;
    fn forget(&self, id: NodeId) -> Result<(), WorkspaceError>;
    fn sync(&self) -> Result<(), WorkspaceError>;
}

impl<T: WorkspaceStore + ?Sized> WorkspaceStore for std::sync::Arc<T> {
    fn root(&self, tree: Tree) -> Result<NodeId, WorkspaceError> {
        (**self).root(tree)
    }

    fn node(&self, id: NodeId) -> Result<Node, WorkspaceError> {
        (**self).node(id)
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<Node, WorkspaceError> {
        (**self).lookup(parent, name)
    }

    fn children(&self, parent: NodeId) -> Result<Vec<DirEntry>, WorkspaceError> {
        (**self).children(parent)
    }

    fn create(
        &self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        mode: u32,
    ) -> Result<Node, WorkspaceError> {
        (**self).create(parent, name, kind, mode)
    }

    fn read(&self, id: NodeId, offset: u64, size: u32) -> Result<Vec<u8>, WorkspaceError> {
        (**self).read(id, offset, size)
    }

    fn write(&self, id: NodeId, offset: u64, data: &[u8]) -> Result<Node, WorkspaceError> {
        (**self).write(id, offset, data)
    }

    fn truncate(&self, id: NodeId, size: u64) -> Result<Node, WorkspaceError> {
        (**self).truncate(id, size)
    }

    fn set_mode(&self, id: NodeId, mode: u32) -> Result<Node, WorkspaceError> {
        (**self).set_mode(id, mode)
    }

    fn set_modified(
        &self,
        id: NodeId,
        modified: std::time::SystemTime,
    ) -> Result<Node, WorkspaceError> {
        (**self).set_modified(id, modified)
    }

    fn remove(&self, parent: NodeId, name: &str, kind: NodeKind) -> Result<(), WorkspaceError> {
        (**self).remove(parent, name, kind)
    }

    fn rename(
        &self,
        parent: NodeId,
        name: &str,
        new_parent: NodeId,
        new_name: &str,
        replace: bool,
    ) -> Result<(), WorkspaceError> {
        (**self).rename(parent, name, new_parent, new_name, replace)
    }

    fn forget(&self, id: NodeId) -> Result<(), WorkspaceError> {
        (**self).forget(id)
    }

    fn sync(&self) -> Result<(), WorkspaceError> {
        (**self).sync()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderApi {
    OpenAiResponses,
    OpenAiCompletions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub id: ProviderId,
    pub api: ProviderApi,
    pub base_url: String,
    pub api_key_env: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ReasoningLevel {
    Off,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningLevel {
    pub const ALL: [ReasoningLevel; 6] = [
        Self::Off,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown reasoning level: {0}")]
pub struct ParseReasoningLevelError(String);

impl std::str::FromStr for ReasoningLevel {
    type Err = ParseReasoningLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|level| level.as_str() == s)
            .ok_or_else(|| ParseReasoningLevelError(s.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredModel {
    pub id: String,
    pub context_window: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub provider: ProviderId,
    pub id: String,
    pub context_window: Option<u32>,
    pub context_limit: Option<u32>,
    pub reasoning_map: BTreeMap<ReasoningLevel, Option<String>>,
}

impl Model {
    pub fn effective_context(&self) -> Option<u32> {
        [self.context_limit, self.context_window]
            .into_iter()
            .flatten()
            .min()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: ProviderId,
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub default_model: Option<ModelRef>,
    pub reasoning: ReasoningLevel,
}

pub trait ConfigStore: Send + Sync {
    fn providers(&self) -> Result<Vec<Provider>, StoreError>;
    fn save_provider(&self, provider: Provider) -> Result<(), StoreError>;
    fn remove_provider(&self, id: &ProviderId) -> Result<(), StoreError>;
    fn models(&self, provider: &ProviderId) -> Result<Vec<Model>, StoreError>;
    fn replace_models(
        &self,
        provider: &ProviderId,
        models: Vec<DiscoveredModel>,
    ) -> Result<(), StoreError>;
    fn set_context_limit(
        &self,
        provider: &ProviderId,
        model: &str,
        limit: Option<u32>,
    ) -> Result<(), StoreError>;
    fn set_reasoning_map(
        &self,
        provider: &ProviderId,
        model: &str,
        map: BTreeMap<ReasoningLevel, Option<String>>,
    ) -> Result<(), StoreError>;
    fn settings(&self) -> Result<Settings, StoreError>;
    fn save_settings(&self, settings: Settings) -> Result<(), StoreError>;
}
