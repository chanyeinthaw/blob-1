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
    fn session_ids(&self, prefix: &str) -> Result<Vec<SessionId>, StoreError>;
}

impl<T: SessionStore + ?Sized> SessionStore for std::sync::Arc<T> {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        (**self).load(id)
    }

    fn save(&self, session: Session) -> Result<(), StoreError> {
        (**self).save(session)
    }

    fn session_ids(&self, prefix: &str) -> Result<Vec<SessionId>, StoreError> {
        (**self).session_ids(prefix)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFile {
    pub path: String,
    pub mode: u32,
    pub content: Vec<u8>,
}

pub trait SkillStore: Send + Sync {
    fn skills(&self) -> Result<Vec<SkillInfo>, StoreError>;
    fn skill_files(&self, name: &str) -> Result<Vec<SkillFile>, StoreError>;
    fn put_skill(&self, info: SkillInfo, files: Vec<SkillFile>) -> Result<(), StoreError>;
    fn remove_skill(&self, name: &str) -> Result<bool, StoreError>;
}

impl<T: SkillStore + ?Sized> SkillStore for std::sync::Arc<T> {
    fn skills(&self) -> Result<Vec<SkillInfo>, StoreError> {
        (**self).skills()
    }

    fn skill_files(&self, name: &str) -> Result<Vec<SkillFile>, StoreError> {
        (**self).skill_files(name)
    }

    fn put_skill(&self, info: SkillInfo, files: Vec<SkillFile>) -> Result<(), StoreError> {
        (**self).put_skill(info, files)
    }

    fn remove_skill(&self, name: &str) -> Result<bool, StoreError> {
        (**self).remove_skill(name)
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
    pub compaction_model: Option<ModelRef>,
    pub compaction_reasoning: Option<ReasoningLevel>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryKind {
    User,
    Assistant,
    Tool,
    Echo,
    Work,
}

impl MemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::Echo => "echo",
            Self::Work => "work",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryMessage {
    pub i: u64,
    pub kind: MemoryKind,
    pub text: String,
    pub size: u64,
    pub date: String,
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryNode {
    pub l: u32,
    pub i: u64,
    pub text: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewName {
    Chat,
    Seen,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryView {
    pub parts: Vec<(u32, u64)>,
    pub batch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewChange {
    pub view: ViewName,
    pub removed: Vec<(u32, u64)>,
    pub added: Vec<(u32, u64)>,
    pub batch: Option<bool>,
}

pub trait MemoryStore: Send + Sync {
    fn append_messages(
        &self,
        rows: &[MemoryMessage],
        changes: &[ViewChange],
    ) -> Result<(), StoreError>;
    fn message(&self, i: u64) -> Result<Option<MemoryMessage>, StoreError>;
    fn message_count(&self) -> Result<u64, StoreError>;
    fn node(&self, l: u32, i: u64) -> Result<Option<MemoryNode>, StoreError>;
    fn built_nodes(&self) -> Result<Vec<(u32, u64)>, StoreError>;
    fn put_node(&self, node: MemoryNode, changes: &[ViewChange]) -> Result<(), StoreError>;
    fn view(&self, name: ViewName) -> Result<Option<MemoryView>, StoreError>;
}

impl<T: MemoryStore + ?Sized> MemoryStore for std::sync::Arc<T> {
    fn append_messages(
        &self,
        rows: &[MemoryMessage],
        changes: &[ViewChange],
    ) -> Result<(), StoreError> {
        (**self).append_messages(rows, changes)
    }
    fn message(&self, i: u64) -> Result<Option<MemoryMessage>, StoreError> {
        (**self).message(i)
    }
    fn message_count(&self) -> Result<u64, StoreError> {
        (**self).message_count()
    }
    fn node(&self, l: u32, i: u64) -> Result<Option<MemoryNode>, StoreError> {
        (**self).node(l, i)
    }
    fn built_nodes(&self) -> Result<Vec<(u32, u64)>, StoreError> {
        (**self).built_nodes()
    }
    fn put_node(&self, node: MemoryNode, changes: &[ViewChange]) -> Result<(), StoreError> {
        (**self).put_node(node, changes)
    }
    fn view(&self, name: ViewName) -> Result<Option<MemoryView>, StoreError> {
        (**self).view(name)
    }
}
