use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Summary {
        text: String,
        first_kept: usize,
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
        images: Vec<Image>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub messages: Vec<Message>,
    pub usage: Option<SessionUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionUsage {
    pub tokens: u64,
    pub view_boundary: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskChoice {
    pub provider: String,
    pub model: String,
    pub reasoning: String,
}

impl Session {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            messages: Vec::new(),
            usage: None,
        }
    }

    pub fn view_indices(&self) -> Vec<usize> {
        match self
            .messages
            .iter()
            .rposition(|m| matches!(m, Message::Summary { .. }))
        {
            Some(s) => {
                let Message::Summary { first_kept, .. } = &self.messages[s] else {
                    unreachable!()
                };
                std::iter::once(s)
                    .chain(
                        (*first_kept..s)
                            .filter(|&i| !matches!(self.messages[i], Message::Summary { .. })),
                    )
                    .chain(s + 1..self.messages.len())
                    .collect()
            }
            None => (0..self.messages.len()).collect(),
        }
    }

    pub fn view(&self) -> Vec<Message> {
        self.view_indices()
            .into_iter()
            .map(|i| self.messages[i].clone())
            .collect()
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("storage failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("{0}")]
    Invalid(String),
}

pub trait SessionStore: Send + Sync {
    fn create_task_session(
        &self,
        name: &str,
        text: &str,
        choice: &TaskChoice,
    ) -> Result<bool, StoreError>;
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError>;
    fn save(&self, session: Session) -> Result<(), StoreError>;
    fn session_ids(&self, prefix: &str) -> Result<Vec<SessionId>, StoreError>;
    fn task_choice(&self, name: &str) -> Result<Option<TaskChoice>, StoreError>;
}

impl<T: SessionStore + ?Sized> SessionStore for std::sync::Arc<T> {
    fn create_task_session(
        &self,
        name: &str,
        text: &str,
        choice: &TaskChoice,
    ) -> Result<bool, StoreError> {
        (**self).create_task_session(name, text, choice)
    }
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        (**self).load(id)
    }

    fn save(&self, session: Session) -> Result<(), StoreError> {
        (**self).save(session)
    }

    fn session_ids(&self, prefix: &str) -> Result<Vec<SessionId>, StoreError> {
        (**self).session_ids(prefix)
    }

    fn task_choice(&self, name: &str) -> Result<Option<TaskChoice>, StoreError> {
        (**self).task_choice(name)
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

/// Where a bot may use a skill. A skill without a stored mode is `Both`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillMode {
    #[default]
    Both,
    Chat,
    Task,
    Off,
}

impl SkillMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Chat => "chat",
            Self::Task => "task",
            Self::Off => "off",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "both" => Some(Self::Both),
            "chat" => Some(Self::Chat),
            "task" => Some(Self::Task),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn chat(self) -> bool {
        matches!(self, Self::Both | Self::Chat)
    }

    pub fn task(self) -> bool {
        matches!(self, Self::Both | Self::Task)
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderApi {
    OpenAiResponses,
    OpenAiCompletions,
    /// OpenCode Go (`https://opencode.ai/inference/go`): OpenAI Chat
    /// Completions plus a required per-conversation `x-opencode-session`
    /// header.
    OpenCodeGo,
    Decision,
}

impl ProviderApi {
    pub fn is_decision(self) -> bool {
        matches!(self, Self::Decision)
    }

    pub fn is_llm(self) -> bool {
        !self.is_decision()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Provider {
    pub id: ProviderId,
    pub api: ProviderApi,
    pub base_url: String,
    pub has_api_key: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Model {
    pub images: bool,
    /// Offered to spawn and role pickers. Models are disabled until explicitly
    /// enabled in the catalog; assigned role slots keep resolving regardless.
    pub enabled: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelRef {
    pub provider: ProviderId,
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Settings {
    pub default_model: Option<ModelRef>,
    pub reasoning: ReasoningLevel,
    pub compaction_model: Option<ModelRef>,
    pub compaction_reasoning: Option<ReasoningLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    pub messenger: String,
    pub target: String,
}

pub trait WebAuthStore: Send + Sync {
    fn web_password(&self) -> Result<Option<String>, StoreError>;
    fn set_web_password(&self, hash: &str) -> Result<(), StoreError>;
    fn create_web_session(
        &self,
        hash: &[u8],
        expires: i64,
        password: &str,
    ) -> Result<bool, StoreError>;
    fn authenticate_web_session(
        &self,
        hash: &[u8],
        now: i64,
        expires: i64,
    ) -> Result<bool, StoreError>;
    fn delete_web_session(&self, hash: &[u8]) -> Result<(), StoreError>;
}

pub trait ConfigStore: Send + Sync {
    fn timezone(&self) -> Result<String, StoreError>;
    fn set_timezone(&self, timezone: &str) -> Result<(), StoreError>;
    /// Home destinations, at most one per messenger; `send` to `home`
    /// delivers to all of them.
    fn homes(&self) -> Result<Vec<Home>, StoreError>;
    /// Sets (`Some`) or clears (`None`) the home for one messenger.
    fn set_home(&self, messenger: &str, target: Option<&str>) -> Result<(), StoreError>;
    fn instructions(&self) -> Result<Option<String>, StoreError>;
    fn set_instructions(&self, text: Option<&str>) -> Result<(), StoreError>;
    fn auto_respond(&self) -> Result<bool, StoreError>;
    fn set_auto_respond(&self, enabled: bool) -> Result<(), StoreError>;
    fn public_url(&self) -> Result<Option<String>, StoreError>;
    fn set_public_url(&self, url: Option<&str>) -> Result<(), StoreError>;
    fn providers(&self) -> Result<Vec<Provider>, StoreError>;
    fn save_provider(&self, provider: Provider, api_key: Option<&str>) -> Result<(), StoreError>;
    fn provider_api_key(&self, id: &ProviderId) -> Result<Option<String>, StoreError>;
    /// Non-secret fingerprint of the stored encrypted credential, used for cache invalidation.
    fn provider_key_fingerprint(&self, id: &ProviderId) -> Result<Option<u64>, StoreError>;
    fn set_provider_api_key(
        &self,
        id: &ProviderId,
        api_key: Option<&str>,
    ) -> Result<(), StoreError>;
    fn remove_provider(&self, id: &ProviderId) -> Result<(), StoreError>;
    fn models(&self, provider: &ProviderId) -> Result<Vec<Model>, StoreError>;
    fn set_images(
        &self,
        provider: &ProviderId,
        model: &str,
        images: bool,
    ) -> Result<(), StoreError>;
    fn set_enabled(
        &self,
        provider: &ProviderId,
        model: &str,
        enabled: bool,
    ) -> Result<(), StoreError>;
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
    fn utility_model(&self) -> Result<Option<(ModelRef, Option<ReasoningLevel>)>, StoreError>;
    fn set_utility_model(
        &self,
        model: Option<(ModelRef, Option<ReasoningLevel>)>,
    ) -> Result<(), StoreError>;
    fn decision_model(&self) -> Result<Option<ModelRef>, StoreError>;
    fn set_decision_model(&self, model: Option<&ModelRef>) -> Result<(), StoreError>;
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Place {
    pub channel: String,
    pub thread: Option<String>,
    pub dm: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    pub name: String,
    /// Bot-authored (other bots, webhooks); admission control is the access
    /// rules, the flag only labels the author for the model.
    #[serde(default)]
    pub bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub content_type: Option<String>,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub message: String,
    pub author: Person,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

/// A conversation message the bot never saw, inlined into the `<message>`
/// that follows the gap: arrivals while the bot was offline, filtered out,
/// or predating its first involvement in the place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Missed {
    pub message: String,
    pub author: Person,
    pub date: String,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Meta {
    Checkin {
        time: String,
        /// Whether any home destination is set: `send` with `to: "home"`.
        #[serde(default, deserialize_with = "home_flag")]
        home: bool,
    },
    Scheduled {
        id: String,
        due: String,
        #[serde(default)]
        late: Option<String>,
        timezone: String,
        #[serde(default)]
        to: Option<u64>,
        #[serde(rename = "for", default)]
        for_message: Option<String>,
        #[serde(default, deserialize_with = "home_flag")]
        home: bool,
    },
    Report {
        task: String,
        status: ReportStatus,
        #[serde(rename = "for", default)]
        for_message: Option<String>,
    },
    Reminder {
        task: String,
        running: String,
        last_activity: String,
        #[serde(rename = "for")]
        for_message: Option<String>,
    },
    Event {
        source: String,
        name: String,
        #[serde(rename = "for", default, skip_serializing_if = "Option::is_none")]
        for_message: Option<String>,
    },
    Message {
        #[serde(alias = "interface")]
        messenger: String,
        place: Place,
        message: String,
        author: Person,
        to: u64,
        #[serde(default)]
        inline: bool,
        #[serde(default)]
        unaddressed: bool,
        #[serde(rename = "where")]
        location: String,
        earlier: Option<u32>,
        #[serde(default)]
        quote: Option<Quote>,
        #[serde(default)]
        attachments: Vec<Attachment>,
        /// Conversation messages missed since the last one the bot saw in
        /// this place, oldest first; empty when there is no gap.
        #[serde(default)]
        missed: Vec<Missed>,
        /// The gap before this message could not be filled (history fetch
        /// failed): the durable high-water mark must not advance past it, so
        /// a later admission retries the range. Transient — never persisted.
        #[serde(skip)]
        gap_open: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Done,
    Failed,
    Stopped,
    Crashed,
}

impl ReportStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Crashed => "crashed",
        }
    }
}

fn home_attribute(home: bool) -> &'static str {
    if home {
        " home=\"home\""
    } else {
        ""
    }
}

/// Reads a stored `home`: a flag now, a destination id (any home) or null in
/// rows written before homes became a list.
fn home_flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Flag(bool),
        Destination(#[allow(dead_code)] u64),
    }
    Ok(match Option::<Stored>::deserialize(deserializer)? {
        Some(Stored::Flag(flag)) => flag,
        Some(Stored::Destination(_)) => true,
        None => false,
    })
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn attribute(value: &str) -> String {
    escape(value).replace('"', "&quot;")
}

pub fn render_attachments(attachments: &[Attachment]) -> String {
    attachments
        .iter()
        .map(|attachment| {
            let content_type = attachment
                .content_type
                .as_ref()
                .map_or_else(String::new, |value| {
                    format!(" type=\"{}\"", attribute(value))
                });
            format!(
                "<attachment name=\"{}\"{} size=\"{}\" url=\"{}\"/>\n",
                attribute(&attachment.name),
                content_type,
                attachment.size,
                attribute(&attachment.url)
            )
        })
        .collect()
}

/// Renders missed conversation messages as a `<missed>` block of compact
/// `<message>` elements, matching the shape the history lookup tool returns.
pub fn render_missed(missed: &[Missed]) -> String {
    if missed.is_empty() {
        return String::new();
    }
    let items: String = missed
        .iter()
        .map(|m| {
            format!(
                "<message author=\"{}\" author_name=\"{}\" message=\"{}\" date=\"{}\"{}>\n{}{}\n</message>\n",
                attribute(&m.author.id),
                attribute(&m.author.name),
                attribute(&m.message),
                attribute(&m.date),
                if m.author.bot { " bot=\"true\"" } else { "" },
                render_attachments(&m.attachments),
                escape(&m.text)
            )
        })
        .collect();
    format!("<missed>\n{items}</missed>\n")
}

pub fn render_input(meta: Option<&Meta>, i: u64, body: &str, time: Option<&str>) -> String {
    match meta {
        Some(meta) => meta.render_at(i, body, time),
        None => format!("<message{}>\n{}\n</message>", time_attribute(time), escape(body)),
    }
}

fn time_attribute(value: Option<&str>) -> String {
    value.map_or_else(String::new, |value| {
        format!(" time=\"{}\"", attribute(value))
    })
}

fn for_attribute(value: &Option<String>) -> String {
    value.as_ref().map_or_else(String::new, |value| {
        format!(" for=\"{}\"", attribute(value))
    })
}

impl Meta {
    pub fn render(&self, i: u64, body: &str) -> String {
        self.render_at(i, body, None)
    }

    pub fn render_at(&self, i: u64, body: &str, time: Option<&str>) -> String {
        if let Self::Checkin { time, home } = self {
            return format!(
                "<checkin time=\"{}\"{}>\n{body}\n</checkin>",
                attribute(time),
                home_attribute(*home)
            );
        }
        let body = escape(body);
        match self {
            Self::Checkin { .. } => unreachable!(),
            Self::Scheduled { id, due, late, timezone, to, for_message, home } => format!(
                "<scheduled id=\"{}\" due=\"{} {}\"{}{}{}{}>\n{body}\n</scheduled>",
                attribute(id), attribute(due), attribute(timezone),
                late.as_ref().map_or_else(String::new, |v| format!(" late=\"{}\"", attribute(v))),
                to.map_or_else(String::new, |v| format!(" to=\"d{v}\"")),
                for_attribute(for_message),
                home_attribute(*home)
            ),
            Self::Report { task, status, for_message } => format!(
                "<report task=\"{}\" status=\"{}\"{}>\n{body}\n</report>",
                attribute(task), status.as_str(), for_attribute(for_message)
            ),
            Self::Reminder { task, running, last_activity, for_message } => format!(
                "<reminder task=\"{}\" running=\"{}\" last_activity=\"{}\"{}>\n{body}\n</reminder>",
                attribute(task), attribute(running), attribute(last_activity), for_attribute(for_message)
            ),
            Self::Event {
                source,
                name,
                for_message,
            } => format!(
                "<event source=\"{}\" name=\"{}\"{}{}>\n{body}\n</event>",
                attribute(source),
                attribute(name),
                for_attribute(for_message),
                time_attribute(time)
            ),
            Self::Message {
                messenger,
                author,
                to,
                location,
                earlier,
                quote,
                attachments,
                missed,
                inline,
                unaddressed,
                ..
            } => format!(
                "<message messenger=\"{}\" reply=\"m{i}\" to=\"d{to}\" author=\"{}\" author_name=\"{}\" where=\"{}\"{}{}{}{}{}>\n{}{}{}{body}\n</message>",
                attribute(messenger),
                attribute(&author.id),
                attribute(&author.name),
                attribute(location),
                time_attribute(time),
                earlier.map_or_else(String::new, |n| format!(" earlier=\"{n}\"")),
                if *inline { " mode=\"inline\"" } else { "" },
                if *unaddressed {
                    " unaddressed=\"true\""
                } else {
                    ""
                },
                if author.bot { " bot=\"true\"" } else { "" },
                render_missed(missed),
                quote.as_ref().map_or_else(String::new, |quote| format!(
                    "<quote author=\"{}\" author_name=\"{}\" message=\"{}\"{}>\n{}{}\n</quote>\n",
                    attribute(&quote.author.id), attribute(&quote.author.name), attribute(&quote.message), if quote.author.bot { " bot=\"true\"" } else { "" }, render_attachments(&quote.attachments), escape(&quote.text)
                )),
                render_attachments(attachments)
            ),
        }
    }

    pub fn source(&self) -> String {
        match self {
            Self::Checkin { .. } => "checkin".into(),
            Self::Scheduled { id, .. } => format!("scheduled {id}"),
            Self::Report { task, status, .. } => format!("task {task}, {}", status.as_str()),
            Self::Reminder { task, .. } => format!("reminder for task {task}"),
            Self::Event { source, name, .. } => format!("{source} {name}"),
            Self::Message {
                messenger,
                author,
                location,
                ..
            } => format!("{messenger} @{} in {location}", author.name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryMessage {
    pub i: u64,
    pub kind: MemoryKind,
    pub meta: Option<Meta>,
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
    /// Stores built nodes (possibly none) and view changes in one transaction.
    fn put_nodes(&self, nodes: &[MemoryNode], changes: &[ViewChange]) -> Result<(), StoreError>;
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
    fn put_nodes(&self, nodes: &[MemoryNode], changes: &[ViewChange]) -> Result<(), StoreError> {
        (**self).put_nodes(nodes, changes)
    }
    fn view(&self, name: ViewName) -> Result<Option<MemoryView>, StoreError> {
        (**self).view(name)
    }
}

#[derive(Debug, Clone)]
pub struct Schedule {
    pub id: u64,
    pub text: String,
    pub at: Option<String>,
    pub cron: Option<String>,
    pub timezone: String,
    pub to_dest: Option<u64>,
    pub reply: Option<String>,
    pub next_due: i64,
}

pub trait ScheduleStore: Send + Sync {
    fn add_schedule(&self, schedule: &Schedule) -> Result<u64, StoreError>;
    fn schedules(&self) -> Result<Vec<Schedule>, StoreError>;
    fn due_schedules(&self, now: i64) -> Result<Vec<Schedule>, StoreError>;
    fn remove_schedule(&self, id: u64) -> Result<bool, StoreError>;
    fn claim_schedule(&self, id: u64, due: i64, next_due: Option<i64>) -> Result<bool, StoreError>;
}

impl<T: ScheduleStore + ?Sized> ScheduleStore for std::sync::Arc<T> {
    fn add_schedule(&self, schedule: &Schedule) -> Result<u64, StoreError> {
        (**self).add_schedule(schedule)
    }
    fn schedules(&self) -> Result<Vec<Schedule>, StoreError> {
        (**self).schedules()
    }
    fn due_schedules(&self, now: i64) -> Result<Vec<Schedule>, StoreError> {
        (**self).due_schedules(now)
    }
    fn remove_schedule(&self, id: u64) -> Result<bool, StoreError> {
        (**self).remove_schedule(id)
    }
    fn claim_schedule(&self, id: u64, due: i64, next_due: Option<i64>) -> Result<bool, StoreError> {
        (**self).claim_schedule(id, due, next_due)
    }
}

/// What an interaction override applies to: a Discord channel or thread id,
/// a user id, or a whole server (guild) id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionScope {
    Channel,
    User,
    Server,
}

impl InteractionScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::User => "user",
            Self::Server => "server",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.as_str() == s)
    }

    const ALL: [Self; 3] = [Self::Channel, Self::User, Self::Server];
}

/// How eagerly the bot engages: `MentionOnly` answers DMs, mentions and
/// replies; `Always` answers every message in scope; `Auto` routes
/// unaddressed messages through the Auto-respond check (acting like
/// `MentionOnly` when no utility model is configured or Auto-respond is off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionMode {
    MentionOnly,
    Always,
    Auto,
}

impl InteractionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MentionOnly => "mention-only",
            Self::Always => "always",
            Self::Auto => "auto",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == s)
    }

    const ALL: [Self; 3] = [Self::MentionOnly, Self::Always, Self::Auto];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionOverride {
    pub scope: InteractionScope,
    /// Discord guild id for `User` overrides; `None` for `Channel` and
    /// `Server` overrides (a `Server` override's `target_id` is the guild id).
    pub guild_id: Option<String>,
    pub target_id: String,
    pub mode: InteractionMode,
}

/// How the bot replies to plain-channel messages (outside DMs and threads):
/// `auto` asks the utility model (falling back to a thread), `thread`
/// always starts a thread, and `direct` always replies inline in the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMode {
    Auto,
    Thread,
    Direct,
}

impl ReplyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Thread => "thread",
            Self::Direct => "direct",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == s)
    }

    const ALL: [Self; 3] = [Self::Auto, Self::Thread, Self::Direct];
}

/// What a reply-mode override applies to: a Discord channel id or a whole
/// server (guild) id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyScope {
    Channel,
    Server,
}

impl ReplyScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Server => "server",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.as_str() == s)
    }

    const ALL: [Self; 2] = [Self::Channel, Self::Server];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyOverride {
    pub scope: ReplyScope,
    /// Channel id for `Channel` overrides, guild id for `Server` overrides.
    pub target_id: String,
    pub mode: ReplyMode,
}

/// The effective reply mode for a plain-channel message: a `channel`
/// override wins, then the `server` override for the message's guild,
/// then `default`.
pub fn effective_reply_mode(
    overrides: &[ReplyOverride],
    channel: &str,
    guild: Option<&str>,
    default: ReplyMode,
) -> ReplyMode {
    overrides
        .iter()
        .find(|o| o.scope == ReplyScope::Channel && o.target_id == channel)
        .or_else(|| {
            overrides
                .iter()
                .find(|o| o.scope == ReplyScope::Server && Some(o.target_id.as_str()) == guild)
        })
        .map_or(default, |o| o.mode)
}

/// Per-messenger fallbacks applied when no override matches: unaddressed
/// messages go to the Auto-respond check and replies are decided per message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessengerDefaults {
    pub interaction_mode: InteractionMode,
    pub reply_mode: ReplyMode,
}

impl Default for MessengerDefaults {
    fn default() -> Self {
        Self {
            interaction_mode: InteractionMode::Auto,
            reply_mode: ReplyMode::Auto,
        }
    }
}

/// Outcome of an access rule or the messenger-wide default (`block` or `allow`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessEffect {
    Allow,
    Block,
}

impl AccessEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Block => "block",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Self::Allow),
            "block" => Some(Self::Block),
            _ => None,
        }
    }
}

/// Where an access rule applies: globally (all servers + DMs; person-only),
/// in a specific Discord server (guild channels/threads, never DMs), or in a
/// specific channel/thread (channel + its threads, never DMs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessScope {
    Global,
    Server,
    Channel,
}

impl AccessScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Server => "server",
            Self::Channel => "channel",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "global" => Some(Self::Global),
            "server" => Some(Self::Server),
            "channel" => Some(Self::Channel),
            _ => None,
        }
    }
}

/// One unified access rule `(who, where, effect)` unique per `(user_id, scope, scope_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessRule {
    /// Specific Discord user id, or `None` for everyone (`server`/`channel` only).
    pub user_id: Option<String>,
    pub scope: AccessScope,
    /// Server (guild) id for `Server`, channel/thread id for `Channel`, `None` for `Global`.
    pub scope_id: Option<String>,
    pub effect: AccessEffect,
}

pub trait MessengerStore: Send + Sync {
    fn destination(&self, messenger: &str, place: &Place) -> Result<u64, StoreError>;
    /// The destination id for `place`, without creating a row.
    fn place_id(&self, messenger: &str, place: &Place) -> Result<Option<u64>, StoreError>;
    fn place(&self, id: u64) -> Result<Option<(String, Place)>, StoreError>;
    fn allowed_users(&self, messenger: &str) -> Result<Vec<String>, StoreError>;
    fn allow_user(&self, messenger: &str, user: &str) -> Result<(), StoreError>;
    fn disallow_user(&self, messenger: &str, user: &str) -> Result<bool, StoreError>;
    /// Default access decision for a messenger when no rule matches; defaults to `AccessEffect::Block`.
    fn default_access(&self, messenger: &str) -> Result<AccessEffect, StoreError>;
    fn set_default_access(&self, messenger: &str, effect: AccessEffect) -> Result<(), StoreError>;
    /// Unified access rules, ordered by user (people first or everyone), scope, then scope id.
    fn access_rules(&self, messenger: &str) -> Result<Vec<AccessRule>, StoreError>;
    fn set_access_rule(
        &self,
        messenger: &str,
        user_id: Option<&str>,
        scope: AccessScope,
        scope_id: Option<&str>,
        effect: AccessEffect,
    ) -> Result<(), StoreError>;
    fn remove_access_rule(
        &self,
        messenger: &str,
        user_id: Option<&str>,
        scope: AccessScope,
        scope_id: Option<&str>,
    ) -> Result<bool, StoreError>;
    /// The bot token for a chat platform (e.g. `discord`); stored encrypted.
    fn messenger_token(&self, messenger: &str) -> Result<Option<String>, StoreError>;
    fn set_messenger_token(&self, messenger: &str, token: Option<&str>) -> Result<(), StoreError>;
    /// Per-channel and per-guild-user interaction overrides, ordered by scope,
    /// guild id, then target id.
    fn interaction_modes(&self, messenger: &str) -> Result<Vec<InteractionOverride>, StoreError>;
    fn set_interaction_mode(
        &self,
        messenger: &str,
        scope: InteractionScope,
        guild_id: Option<&str>,
        target_id: &str,
        mode: InteractionMode,
    ) -> Result<(), StoreError>;
    fn clear_interaction_mode(
        &self,
        messenger: &str,
        scope: InteractionScope,
        guild_id: Option<&str>,
        target_id: &str,
    ) -> Result<bool, StoreError>;
    /// Per-channel and per-server reply mode overrides, ordered by scope,
    /// then target id.
    fn reply_modes(&self, messenger: &str) -> Result<Vec<ReplyOverride>, StoreError>;
    fn set_reply_mode(
        &self,
        messenger: &str,
        scope: ReplyScope,
        target_id: &str,
        mode: ReplyMode,
    ) -> Result<(), StoreError>;
    fn clear_reply_mode(
        &self,
        messenger: &str,
        scope: ReplyScope,
        target_id: &str,
    ) -> Result<bool, StoreError>;
    /// Messenger-wide fallbacks used when no override matches; an absent row
    /// resolves to the built-in defaults (`auto` interaction, `auto` reply).
    fn messenger_defaults(&self, messenger: &str) -> Result<MessengerDefaults, StoreError>;
    fn set_default_interaction_mode(
        &self,
        messenger: &str,
        mode: InteractionMode,
    ) -> Result<(), StoreError>;
    fn set_default_reply_mode(&self, messenger: &str, mode: ReplyMode) -> Result<(), StoreError>;
    /// Persisted markers for synthesized thread destinations that have not yet
    /// been created on the messenger.
    fn mark_pending_thread(&self, destination: u64) -> Result<(), StoreError>;
    fn clear_pending_thread(&self, destination: u64) -> Result<(), StoreError>;
    fn clear_pending_thread_by_place(
        &self,
        messenger: &str,
        channel: &str,
        thread: &str,
    ) -> Result<(), StoreError>;
    fn is_pending_thread(&self, destination: u64) -> Result<bool, StoreError>;
    /// The newest message id admitted into the bot's session in `place`
    /// — the high-water mark for missed-gap detection.
    fn last_seen_message(
        &self,
        messenger: &str,
        place: &Place,
    ) -> Result<Option<String>, StoreError>;
}

impl<T: MessengerStore + ?Sized> MessengerStore for std::sync::Arc<T> {
    fn destination(&self, messenger: &str, place: &Place) -> Result<u64, StoreError> {
        (**self).destination(messenger, place)
    }
    fn place_id(&self, messenger: &str, place: &Place) -> Result<Option<u64>, StoreError> {
        (**self).place_id(messenger, place)
    }
    fn place(&self, id: u64) -> Result<Option<(String, Place)>, StoreError> {
        (**self).place(id)
    }
    fn allowed_users(&self, messenger: &str) -> Result<Vec<String>, StoreError> {
        (**self).allowed_users(messenger)
    }
    fn allow_user(&self, messenger: &str, user: &str) -> Result<(), StoreError> {
        (**self).allow_user(messenger, user)
    }
    fn disallow_user(&self, messenger: &str, user: &str) -> Result<bool, StoreError> {
        (**self).disallow_user(messenger, user)
    }
    fn default_access(&self, messenger: &str) -> Result<AccessEffect, StoreError> {
        (**self).default_access(messenger)
    }
    fn set_default_access(&self, messenger: &str, effect: AccessEffect) -> Result<(), StoreError> {
        (**self).set_default_access(messenger, effect)
    }
    fn access_rules(&self, messenger: &str) -> Result<Vec<AccessRule>, StoreError> {
        (**self).access_rules(messenger)
    }
    fn set_access_rule(
        &self,
        messenger: &str,
        user_id: Option<&str>,
        scope: AccessScope,
        scope_id: Option<&str>,
        effect: AccessEffect,
    ) -> Result<(), StoreError> {
        (**self).set_access_rule(messenger, user_id, scope, scope_id, effect)
    }
    fn remove_access_rule(
        &self,
        messenger: &str,
        user_id: Option<&str>,
        scope: AccessScope,
        scope_id: Option<&str>,
    ) -> Result<bool, StoreError> {
        (**self).remove_access_rule(messenger, user_id, scope, scope_id)
    }
    fn messenger_token(&self, messenger: &str) -> Result<Option<String>, StoreError> {
        (**self).messenger_token(messenger)
    }
    fn set_messenger_token(&self, messenger: &str, token: Option<&str>) -> Result<(), StoreError> {
        (**self).set_messenger_token(messenger, token)
    }
    fn interaction_modes(&self, messenger: &str) -> Result<Vec<InteractionOverride>, StoreError> {
        (**self).interaction_modes(messenger)
    }
    fn set_interaction_mode(
        &self,
        messenger: &str,
        scope: InteractionScope,
        guild_id: Option<&str>,
        target_id: &str,
        mode: InteractionMode,
    ) -> Result<(), StoreError> {
        (**self).set_interaction_mode(messenger, scope, guild_id, target_id, mode)
    }
    fn clear_interaction_mode(
        &self,
        messenger: &str,
        scope: InteractionScope,
        guild_id: Option<&str>,
        target_id: &str,
    ) -> Result<bool, StoreError> {
        (**self).clear_interaction_mode(messenger, scope, guild_id, target_id)
    }
    fn reply_modes(&self, messenger: &str) -> Result<Vec<ReplyOverride>, StoreError> {
        (**self).reply_modes(messenger)
    }
    fn set_reply_mode(
        &self,
        messenger: &str,
        scope: ReplyScope,
        target_id: &str,
        mode: ReplyMode,
    ) -> Result<(), StoreError> {
        (**self).set_reply_mode(messenger, scope, target_id, mode)
    }
    fn clear_reply_mode(
        &self,
        messenger: &str,
        scope: ReplyScope,
        target_id: &str,
    ) -> Result<bool, StoreError> {
        (**self).clear_reply_mode(messenger, scope, target_id)
    }
    fn messenger_defaults(&self, messenger: &str) -> Result<MessengerDefaults, StoreError> {
        (**self).messenger_defaults(messenger)
    }
    fn set_default_interaction_mode(
        &self,
        messenger: &str,
        mode: InteractionMode,
    ) -> Result<(), StoreError> {
        (**self).set_default_interaction_mode(messenger, mode)
    }
    fn set_default_reply_mode(&self, messenger: &str, mode: ReplyMode) -> Result<(), StoreError> {
        (**self).set_default_reply_mode(messenger, mode)
    }
    fn mark_pending_thread(&self, destination: u64) -> Result<(), StoreError> {
        (**self).mark_pending_thread(destination)
    }
    fn clear_pending_thread(&self, destination: u64) -> Result<(), StoreError> {
        (**self).clear_pending_thread(destination)
    }
    fn clear_pending_thread_by_place(
        &self,
        messenger: &str,
        channel: &str,
        thread: &str,
    ) -> Result<(), StoreError> {
        (**self).clear_pending_thread_by_place(messenger, channel, thread)
    }
    fn is_pending_thread(&self, destination: u64) -> Result<bool, StoreError> {
        (**self).is_pending_thread(destination)
    }
    fn last_seen_message(
        &self,
        messenger: &str,
        place: &Place,
    ) -> Result<Option<String>, StoreError> {
        (**self).last_seen_message(messenger, place)
    }
}

#[derive(Debug, Clone)]
pub struct Watch {
    pub id: u64,
    pub text: String,
    pub to_dest: Option<u64>,
    pub reply: Option<String>,
    pub until: Option<i64>,
}

pub trait WatchStore: Send + Sync {
    fn add_watch(&self, watch: &Watch) -> Result<u64, StoreError>;
    fn watches(&self) -> Result<Vec<Watch>, StoreError>;
    fn remove_watch(&self, id: u64) -> Result<bool, StoreError>;
}

impl<T: WatchStore + ?Sized> WatchStore for std::sync::Arc<T> {
    fn add_watch(&self, watch: &Watch) -> Result<u64, StoreError> {
        (**self).add_watch(watch)
    }
    fn watches(&self) -> Result<Vec<Watch>, StoreError> {
        (**self).watches()
    }
    fn remove_watch(&self, id: u64) -> Result<bool, StoreError> {
        (**self).remove_watch(id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkin {
    pub every: i64,
    pub days: String,
    pub start_hour: u8,
    pub end_hour: u8,
}

pub trait CheckinStore: Send + Sync {
    fn checkin(&self) -> Result<Option<Checkin>, StoreError>;
    fn set_checkin(&self, checkin: &Checkin) -> Result<(), StoreError>;
    fn clear_checkin(&self) -> Result<(), StoreError>;
}

impl<T: CheckinStore + ?Sized> CheckinStore for std::sync::Arc<T> {
    fn checkin(&self) -> Result<Option<Checkin>, StoreError> {
        (**self).checkin()
    }
    fn set_checkin(&self, checkin: &Checkin) -> Result<(), StoreError> {
        (**self).set_checkin(checkin)
    }
    fn clear_checkin(&self) -> Result<(), StoreError> {
        (**self).clear_checkin()
    }
}

pub fn render_checkin_body(watches: &[Watch], local: impl Fn(i64) -> String) -> String {
    watches
        .iter()
        .map(|watch| {
            format!(
                "<watch id=\"w{}\"{}{}{}>\n{}\n</watch>",
                watch.id,
                watch.until.map_or_else(String::new, |until| {
                    format!(" until=\"{}\"", attribute(&local(until)))
                }),
                watch
                    .to_dest
                    .map_or_else(String::new, |to| format!(" to=\"d{to}\"")),
                for_attribute(&watch.reply),
                escape(&watch.text)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Configured MCP server. `env` carries literal values into the child
/// process; `headers` maps a header name to the environment variable
/// holding its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub id: String,
    pub transport: McpTransport,
    pub enabled: bool,
}

/// MCP server configuration plus OAuth state. Token `credentials` and
/// client `registration` are rmcp-serialized JSON; states live in
/// `mcp_oauth_states` keyed by CSRF.
pub trait McpStore: Send + Sync {
    fn mcp_servers(&self) -> Result<Vec<McpServer>, StoreError>;
    fn save_mcp_server(&self, server: &McpServer) -> Result<(), StoreError>;
    fn remove_mcp_server(&self, id: &str) -> Result<bool, StoreError>;
    fn mcp_oauth(
        &self,
        server: &str,
    ) -> Result<(Option<serde_json::Value>, Option<serde_json::Value>), StoreError>;
    fn save_mcp_credentials(
        &self,
        server: &str,
        credentials: Option<&serde_json::Value>,
    ) -> Result<(), StoreError>;
    fn save_mcp_registration(
        &self,
        server: &str,
        registration: &serde_json::Value,
    ) -> Result<(), StoreError>;
    fn mcp_oauth_state(
        &self,
        csrf: &str,
    ) -> Result<Option<(String, serde_json::Value)>, StoreError>;
    fn save_mcp_oauth_state(
        &self,
        server: &str,
        csrf: &str,
        state: &serde_json::Value,
        created: i64,
    ) -> Result<(), StoreError>;
    fn delete_mcp_oauth_state(&self, csrf: &str) -> Result<(), StoreError>;
}

impl<T: McpStore + ?Sized> McpStore for Arc<T> {
    fn mcp_servers(&self) -> Result<Vec<McpServer>, StoreError> {
        (**self).mcp_servers()
    }

    fn save_mcp_server(&self, server: &McpServer) -> Result<(), StoreError> {
        (**self).save_mcp_server(server)
    }

    fn remove_mcp_server(&self, id: &str) -> Result<bool, StoreError> {
        (**self).remove_mcp_server(id)
    }

    fn mcp_oauth(
        &self,
        server: &str,
    ) -> Result<(Option<serde_json::Value>, Option<serde_json::Value>), StoreError> {
        (**self).mcp_oauth(server)
    }

    fn save_mcp_credentials(
        &self,
        server: &str,
        credentials: Option<&serde_json::Value>,
    ) -> Result<(), StoreError> {
        (**self).save_mcp_credentials(server, credentials)
    }

    fn save_mcp_registration(
        &self,
        server: &str,
        registration: &serde_json::Value,
    ) -> Result<(), StoreError> {
        (**self).save_mcp_registration(server, registration)
    }

    fn mcp_oauth_state(
        &self,
        csrf: &str,
    ) -> Result<Option<(String, serde_json::Value)>, StoreError> {
        (**self).mcp_oauth_state(csrf)
    }

    fn save_mcp_oauth_state(
        &self,
        server: &str,
        csrf: &str,
        state: &serde_json::Value,
        created: i64,
    ) -> Result<(), StoreError> {
        (**self).save_mcp_oauth_state(server, csrf, state, created)
    }

    fn delete_mcp_oauth_state(&self, csrf: &str) -> Result<(), StoreError> {
        (**self).delete_mcp_oauth_state(csrf)
    }
}

/// A workspace file published under an unguessable public capability URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFile {
    pub id: String,
    pub workspace: String,
    pub path: String,
    pub content_type: String,
    pub created_at: i64,
}

pub trait PublicFileStore: Send + Sync {
    fn create_public_file(&self, file: &PublicFile) -> Result<(), StoreError>;
    fn public_file(&self, id: &str) -> Result<Option<PublicFile>, StoreError>;
}

impl<T: PublicFileStore + ?Sized> PublicFileStore for Arc<T> {
    fn create_public_file(&self, file: &PublicFile) -> Result<(), StoreError> {
        (**self).create_public_file(file)
    }

    fn public_file(&self, id: &str) -> Result<Option<PublicFile>, StoreError> {
        (**self).public_file(id)
    }
}

/// Role that issued an LLM completion, used for usage & cache telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageKind {
    Chat,
    Task,
    Utility,
    Compaction,
}

impl UsageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Task => "task",
            Self::Utility => "utility",
            Self::Compaction => "compaction",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "chat" => Some(Self::Chat),
            "task" => Some(Self::Task),
            "utility" => Some(Self::Utility),
            "compaction" => Some(Self::Compaction),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageEvent {
    pub ts: i64,
    pub kind: UsageKind,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTotals {
    pub calls: u64,
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageKindBucket {
    pub kind: UsageKind,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageModelBucket {
    pub provider: String,
    pub model: String,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageDayBucket {
    pub day: String,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageSummary {
    pub totals: UsageTotals,
    pub by_kind: Vec<UsageKindBucket>,
    pub by_model: Vec<UsageModelBucket>,
    pub daily: Vec<UsageDayBucket>,
}

pub trait UsageStore: Send + Sync {
    fn record_usage(&self, event: &UsageEvent) -> Result<(), StoreError>;
    fn usage_summary(&self, since_ts: i64) -> Result<UsageSummary, StoreError>;
}

impl<T: UsageStore + ?Sized> UsageStore for Arc<T> {
    fn record_usage(&self, event: &UsageEvent) -> Result<(), StoreError> {
        (**self).record_usage(event)
    }

    fn usage_summary(&self, since_ts: i64) -> Result<UsageSummary, StoreError> {
        (**self).usage_summary(since_ts)
    }
}

/// An inbound webhook endpoint whose capability token wakes the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Webhook {
    pub id: String,
    pub name: String,
    pub token: String,
    pub enabled: bool,
    pub created_at: i64,
    pub last_triggered_at: Option<i64>,
}

pub trait WebhookStore: Send + Sync {
    fn webhooks(&self) -> Result<Vec<Webhook>, StoreError>;
    fn create_webhook(
        &self,
        id: &str,
        name: &str,
        token: &str,
        token_hash: &[u8],
        created_at: i64,
    ) -> Result<Webhook, StoreError>;
    fn set_webhook_enabled(&self, id: &str, enabled: bool) -> Result<bool, StoreError>;
    fn remove_webhook(&self, id: &str) -> Result<bool, StoreError>;
    fn webhook_by_token_hash(&self, token_hash: &[u8]) -> Result<Option<Webhook>, StoreError>;
    fn touch_webhook(&self, id: &str, triggered_at: i64) -> Result<(), StoreError>;
    fn enqueue_webhook_event(
        &self,
        token_hash: &[u8],
        now: i64,
        payload: &str,
    ) -> Result<Option<String>, StoreError>;
}

impl<T: WebhookStore + ?Sized> WebhookStore for Arc<T> {
    fn webhooks(&self) -> Result<Vec<Webhook>, StoreError> {
        (**self).webhooks()
    }

    fn create_webhook(
        &self,
        id: &str,
        name: &str,
        token: &str,
        token_hash: &[u8],
        created_at: i64,
    ) -> Result<Webhook, StoreError> {
        (**self).create_webhook(id, name, token, token_hash, created_at)
    }

    fn set_webhook_enabled(&self, id: &str, enabled: bool) -> Result<bool, StoreError> {
        (**self).set_webhook_enabled(id, enabled)
    }

    fn remove_webhook(&self, id: &str) -> Result<bool, StoreError> {
        (**self).remove_webhook(id)
    }

    fn webhook_by_token_hash(&self, token_hash: &[u8]) -> Result<Option<Webhook>, StoreError> {
        (**self).webhook_by_token_hash(token_hash)
    }

    fn touch_webhook(&self, id: &str, triggered_at: i64) -> Result<(), StoreError> {
        (**self).touch_webhook(id, triggered_at)
    }

    fn enqueue_webhook_event(
        &self,
        token_hash: &[u8],
        now: i64,
        payload: &str,
    ) -> Result<Option<String>, StoreError> {
        (**self).enqueue_webhook_event(token_hash, now, payload)
    }
}

/// One message in a bot's web chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebMessage {
    pub id: i64,
    /// `true` for the bot's messages, `false` for the owner's.
    pub bot: bool,
    pub text: String,
    pub created_at: i64,
}

/// A bot's single web chat conversation.
pub trait WebChatStore: Send + Sync {
    fn add_web_message(&self, bot: bool, text: &str, now: i64) -> Result<i64, StoreError>;
    fn web_message(&self, id: i64) -> Result<Option<WebMessage>, StoreError>;
    /// Up to `limit` messages before id `before` (or the newest), oldest first.
    fn web_messages(&self, before: Option<i64>, limit: u32) -> Result<Vec<WebMessage>, StoreError>;
}

impl<T: WebChatStore + ?Sized> WebChatStore for Arc<T> {
    fn add_web_message(&self, bot: bool, text: &str, now: i64) -> Result<i64, StoreError> {
        (**self).add_web_message(bot, text, now)
    }
    fn web_message(&self, id: i64) -> Result<Option<WebMessage>, StoreError> {
        (**self).web_message(id)
    }
    fn web_messages(&self, before: Option<i64>, limit: u32) -> Result<Vec<WebMessage>, StoreError> {
        (**self).web_messages(before, limit)
    }
}

/// Whether `name` can name a secret: 1-64 of `[A-Za-z0-9_.-]`, not starting
/// with `.`. Secret names become file names, so `.` and `..` must never pass.
pub fn valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// A stored secret's metadata; the value leaves the store only through
/// `SecretStore::secret_value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretInfo {
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// An open request for the user to provide a secret through a one-time URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRequest {
    pub name: String,
    pub label: Option<String>,
    pub expires_at: i64,
}

/// Per-bot secrets, encrypted at rest, and the one-time requests that let a
/// user provide one.
pub trait SecretStore: Send + Sync {
    fn secrets(&self) -> Result<Vec<SecretInfo>, StoreError>;
    fn secret_value(&self, name: &str) -> Result<Option<String>, StoreError>;
    /// Creates or overwrites a secret.
    fn set_secret(&self, name: &str, value: &str, now: i64) -> Result<(), StoreError>;
    fn remove_secret(&self, name: &str) -> Result<bool, StoreError>;
    fn create_secret_request(
        &self,
        token_hash: &[u8],
        name: &str,
        label: Option<&str>,
        expires_at: i64,
        now: i64,
    ) -> Result<(), StoreError>;
    /// The open, unexpired request for a token hash.
    fn secret_request(
        &self,
        token_hash: &[u8],
        now: i64,
    ) -> Result<Option<SecretRequest>, StoreError>;
    /// Stores the value, consumes the request, and queues a `secret` event for
    /// the chat loop, in one transaction. Returns the secret's name, or `None`
    /// when the request is unknown, used, or expired.
    fn fulfill_secret_request(
        &self,
        token_hash: &[u8],
        value: &str,
        now: i64,
    ) -> Result<Option<String>, StoreError>;
}

impl<T: SecretStore + ?Sized> SecretStore for Arc<T> {
    fn secrets(&self) -> Result<Vec<SecretInfo>, StoreError> {
        (**self).secrets()
    }
    fn secret_value(&self, name: &str) -> Result<Option<String>, StoreError> {
        (**self).secret_value(name)
    }
    fn set_secret(&self, name: &str, value: &str, now: i64) -> Result<(), StoreError> {
        (**self).set_secret(name, value, now)
    }
    fn remove_secret(&self, name: &str) -> Result<bool, StoreError> {
        (**self).remove_secret(name)
    }
    fn create_secret_request(
        &self,
        token_hash: &[u8],
        name: &str,
        label: Option<&str>,
        expires_at: i64,
        now: i64,
    ) -> Result<(), StoreError> {
        (**self).create_secret_request(token_hash, name, label, expires_at, now)
    }
    fn secret_request(
        &self,
        token_hash: &[u8],
        now: i64,
    ) -> Result<Option<SecretRequest>, StoreError> {
        (**self).secret_request(token_hash, now)
    }
    fn fulfill_secret_request(
        &self,
        token_hash: &[u8],
        value: &str,
        now: i64,
    ) -> Result<Option<String>, StoreError> {
        (**self).fulfill_secret_request(token_hash, value, now)
    }
}

/// Redacts ephemeral capability credentials before persisting transcripts or
/// memory rows, so database snapshots never leak a live link: callback tokens
/// (`cbk_<43 chars>`, `/hooks/cb/<token>`) and secret-request tokens
/// (`sec_<43 chars>`, also inside `/secret/` links), which would let their
/// holder overwrite a secret. Secret tokens always carry their prefix, so
/// ordinary `/secret/` paths are left alone.
pub fn redact_callback_tokens(text: &str) -> std::borrow::Cow<'_, str> {
    let callbacks = redact_capability(text, Some("/hooks/cb/"), "cbk_");
    if let std::borrow::Cow::Owned(redacted) =
        redact_capability(callbacks.as_ref(), None, "sec_")
    {
        return std::borrow::Cow::Owned(redacted);
    }
    callbacks
}

/// Replaces `<url_prefix><token>` paths (when given) and standalone
/// `<token_prefix><43 token chars>` credentials with `<redacted>`.
fn redact_capability<'a>(
    text: &'a str,
    url_prefix: Option<&str>,
    token_prefix: &str,
) -> std::borrow::Cow<'a, str> {
    const TOKEN_LEN: usize = 43;
    let is_token_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if !url_prefix.is_some_and(|url| text.contains(url)) && !text.contains(token_prefix) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut prev_is_token = false;
    let mut replaced = false;
    while !rest.is_empty() {
        let next_url = url_prefix.and_then(|url| rest.find(url)).map(|pos| (pos, true));
        let next_tok = rest.find(token_prefix).map(|pos| (pos, false));
        let next = match (next_url, next_tok) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
            (a, b) => a.or(b),
        };
        let Some((pos, is_url)) = next else {
            out.push_str(rest);
            break;
        };
        let boundary_ok = if pos > 0 {
            !is_token_byte(rest.as_bytes()[pos - 1])
        } else {
            !prev_is_token
        };
        if is_url {
            let split = pos + url_prefix.map_or(0, str::len);
            out.push_str(&rest[..split]);
            let after = &rest[split..];
            let token_len = after.bytes().take_while(|&b| is_token_byte(b)).count();
            if token_len > 0 {
                out.push_str("<redacted>");
                rest = &after[token_len..];
                prev_is_token = false;
                replaced = true;
            } else {
                rest = after;
                prev_is_token = false;
            }
        } else {
            out.push_str(&rest[..pos]);
            let after = &rest[pos + token_prefix.len()..];
            let token_len = after.bytes().take_while(|&b| is_token_byte(b)).count();
            if boundary_ok && token_len == TOKEN_LEN {
                out.push_str("<redacted>");
                rest = &after[token_len..];
                prev_is_token = false;
                replaced = true;
            } else {
                out.push_str(token_prefix);
                rest = after;
                prev_is_token = true;
            }
        }
    }
    if replaced {
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// Recursively redacts callback credentials inside every decoded string value
/// and object key of a `serde_json::Value` before JSON escaping can mask token
/// boundaries (e.g. `\n` or `\u0063` escapes). Reserves unchanged keys first
/// so redacted keys never collide with plain keys, and gives distinct colliding
/// redacted keys `#2`, `#3`, ... suffixes so no field is overwritten.
pub fn redact_json_value(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => {
            if let std::borrow::Cow::Owned(redacted) = redact_callback_tokens(s) {
                *s = redacted;
                true
            } else {
                false
            }
        }
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= redact_json_value(item);
            }
            changed
        }
        serde_json::Value::Object(map) => {
            let mut changed = false;
            let entries: Vec<(String, serde_json::Value)> = std::mem::take(map).into_iter().collect();
            let mut reserved: std::collections::HashSet<String> = entries
                .iter()
                .filter_map(|(k, _)| {
                    matches!(redact_callback_tokens(k), std::borrow::Cow::Borrowed(_))
                        .then(|| k.clone())
                })
                .collect();
            for (k, mut v) in entries {
                let final_key = match redact_callback_tokens(&k) {
                    std::borrow::Cow::Owned(redacted) => {
                        changed = true;
                        let chosen = pick_unique_key(redacted, &reserved);
                        reserved.insert(chosen.clone());
                        chosen
                    }
                    std::borrow::Cow::Borrowed(_) => k,
                };
                changed |= redact_json_value(&mut v);
                map.insert(final_key, v);
            }
            changed
        }
        _ => false,
    }
}

fn pick_unique_key(base: String, reserved: &std::collections::HashSet<String>) -> String {
    if !reserved.contains(&base) {
        return base;
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("{base}#{n}");
        if !reserved.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Returns true if `s` starts immediately with a `cbk_` or `sec_`
/// `<43-char-token>` credential on a non-token end boundary.
fn starts_with_callback_credential(s: &str) -> bool {
    const TOKEN_LEN: usize = 43;
    let is_token_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if let Some(after) = s.strip_prefix("cbk_").or_else(|| s.strip_prefix("sec_")) {
        let len = after.bytes().take_while(|&b| is_token_byte(b)).count();
        return len == TOKEN_LEN;
    }
    false
}

/// UTF-8-safe lossy JSON string unescaper used when `serde_json::from_str::<String>`
/// rejects a raw quoted span (e.g. lone UTF-16 surrogates `\uD800` or malformed
/// escapes). Validates ASCII hex bytes before slicing and advances by whole
/// UTF-8 characters. On an unknown escape `\X`, peeks ahead (bounded O(1) window)
/// to decide per-escape whether `X` starts a credential (preserving `X` after `\`,
/// e.g. `\cbk_...` / `\c\u0062k_...`) or precedes one (replacing `\X` with `U+FFFD`,
/// e.g. `\xcbk_...` / `\ccbk_...`), so mixed modes in one string are all redacted.
fn lossy_unescape_json_inner(inner: &str, peek_credentials: bool) -> String {
    let bytes = inner.as_bytes();
    let mut out = String::with_capacity(inner.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let esc = i + 1;
            match bytes[esc] {
                b'"' => {
                    out.push('"');
                    i = esc + 1;
                }
                b'\\' => {
                    out.push('\\');
                    i = esc + 1;
                }
                b'/' => {
                    out.push('/');
                    i = esc + 1;
                }
                b'b' => {
                    out.push('\u{0008}');
                    i = esc + 1;
                }
                b'f' => {
                    out.push('\u{000C}');
                    i = esc + 1;
                }
                b'n' => {
                    out.push('\n');
                    i = esc + 1;
                }
                b'r' => {
                    out.push('\r');
                    i = esc + 1;
                }
                b't' => {
                    out.push('\t');
                    i = esc + 1;
                }
                b'u' if esc + 5 <= bytes.len()
                    && bytes[esc + 1..esc + 5].iter().all(u8::is_ascii_hexdigit) =>
                {
                    let code = u16::from_str_radix(&inner[esc + 1..esc + 5], 16).unwrap_or(0xFFFD);
                    i = esc + 5;
                    if (0xD800..=0xDBFF).contains(&code)
                        && i + 6 <= bytes.len()
                        && &bytes[i..i + 2] == b"\\u"
                        && bytes[i + 2..i + 6].iter().all(u8::is_ascii_hexdigit)
                    {
                        let low = u16::from_str_radix(&inner[i + 2..i + 6], 16).unwrap_or(0);
                        if (0xDC00..=0xDFFF).contains(&low) {
                            let cp = 0x10000
                                + (((u32::from(code) - 0xD800) << 10) | (u32::from(low) - 0xDC00));
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                            i += 6;
                            continue;
                        }
                    }
                    out.push(char::from_u32(u32::from(code)).unwrap_or('\u{FFFD}'));
                }
                _ => {
                    let starts_cred = if peek_credentials {
                        matches!(bytes[esc], b'c' | b's') && {
                            let mut end = (esc + 300).min(bytes.len());
                            while end < bytes.len() && !inner.is_char_boundary(end) {
                                end += 1;
                            }
                            starts_with_callback_credential(&lossy_unescape_json_inner(
                                &inner[esc..end],
                                false,
                            ))
                        }
                    } else {
                        true
                    };
                    if starts_cred {
                        out.push('\\');
                        i = esc;
                    } else {
                        out.push('\u{FFFD}');
                        let ch = inner[esc..].chars().next().unwrap();
                        i = esc + ch.len_utf8();
                    }
                }
            }
        } else {
            let ch = inner[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Decodes and redacts a malformed (or unterminated) JSON string body with
/// per-escape recovery so mixed malformed escapes in the same string are all
/// redacted.
fn redact_malformed_json_inner(inner: &str) -> (String, Option<String>) {
    let decoded = lossy_unescape_json_inner(inner, true);
    let redacted = match redact_callback_tokens(&decoded) {
        std::borrow::Cow::Owned(r) => Some(r),
        std::borrow::Cow::Borrowed(_) => None,
    };
    (decoded, redacted)
}

/// Redacts callback credentials inside a JSON string (such as tool call
/// arguments or callback payloads) by scanning every JSON string literal in
/// place:
/// - Decodes each literal individually (falling back to UTF-8-safe lossy escape
///   decoding for malformed or unterminated literals) so Unicode escapes,
///   escaped newlines, and duplicate keys are redacted without dropping members.
/// - Scans only non-literal gaps as plain text and copies unchanged valid
///   literals verbatim so escaped identifier boundaries never cause false hits.
/// - Reserves unchanged object keys per object before disambiguating colliding
///   redacted keys (`#2`, `#3`, ...), and reuses the same assigned name for
///   repeated copies of the same original credential-bearing key.
pub fn redact_json_str(raw: &str) -> std::borrow::Cow<'_, str> {
    struct LiteralSpan {
        start: usize,
        end: usize,
        decoded: String,
        redacted: Option<String>,
        obj_key: Option<usize>,
        terminated: bool,
    }

    let bytes = raw.as_bytes();
    let mut spans: Vec<LiteralSpan> = Vec::new();
    let mut obj_stack: Vec<usize> = Vec::new();
    let mut next_obj_id = 0usize;
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                obj_stack.push(next_obj_id);
                next_obj_id += 1;
                i += 1;
            }
            b'}' => {
                obj_stack.pop();
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                let mut escaped = false;
                while i < bytes.len() {
                    let b = bytes[i];
                    if escaped {
                        escaped = false;
                    } else if b == b'\\' {
                        escaped = true;
                    } else if b == b'"' {
                        break;
                    }
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'"' {
                    let end = i + 1;
                    let literal = &raw[start..end];
                    let (decoded, redacted) = match serde_json::from_str::<String>(literal) {
                        Ok(decoded) => {
                            let redacted = match redact_callback_tokens(&decoded) {
                                std::borrow::Cow::Owned(r) => Some(r),
                                std::borrow::Cow::Borrowed(_) => None,
                            };
                            (decoded, redacted)
                        }
                        Err(_) => redact_malformed_json_inner(&raw[start + 1..end - 1]),
                    };
                    let mut look = end;
                    while look < bytes.len() && bytes[look].is_ascii_whitespace() {
                        look += 1;
                    }
                    let obj_key = if look < bytes.len() && bytes[look] == b':' {
                        obj_stack.last().copied()
                    } else {
                        None
                    };
                    spans.push(LiteralSpan {
                        start,
                        end,
                        decoded,
                        redacted,
                        obj_key,
                        terminated: true,
                    });
                    i = end;
                } else {
                    // Unterminated quote: decode escapes across the remaining tail once.
                    let (decoded, redacted) = redact_malformed_json_inner(&raw[start + 1..]);
                    spans.push(LiteralSpan {
                        start,
                        end: bytes.len(),
                        decoded,
                        redacted,
                        obj_key: None,
                        terminated: false,
                    });
                    break;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    // Pass 1: reserve every unchanged key in each object so redacted keys never
    // collide with plain keys appearing earlier or later in the same object.
    let mut reserved_per_obj: Vec<std::collections::HashSet<String>> =
        vec![std::collections::HashSet::new(); next_obj_id];
    let mut assigned_per_obj: Vec<std::collections::HashMap<String, String>> =
        vec![std::collections::HashMap::new(); next_obj_id];
    for span in &spans {
        if span.redacted.is_none() {
            if let Some(obj_id) = span.obj_key {
                reserved_per_obj[obj_id].insert(span.decoded.clone());
            }
        }
    }

    // Pass 2: emit redacted spans and plain gaps without rescanning unchanged valid literals.
    let mut out = String::new();
    let mut changed = false;
    let mut copied_until = 0usize;
    let mut gap_start = 0usize;

    let flush_gap = |out: &mut String,
                     changed: &mut bool,
                     copied_until: &mut usize,
                     from: usize,
                     to: usize| {
        if from >= to {
            return;
        }
        if let std::borrow::Cow::Owned(redacted) = redact_callback_tokens(&raw[from..to]) {
            if !*changed {
                out.push_str(&raw[..from]);
                *changed = true;
            } else if *copied_until < from {
                out.push_str(&raw[*copied_until..from]);
            }
            out.push_str(&redacted);
            *copied_until = to;
        }
    };

    for span in spans {
        flush_gap(&mut out, &mut changed, &mut copied_until, gap_start, span.start);
        if let Some(mut redacted) = span.redacted {
            if let Some(obj_id) = span.obj_key {
                if let Some(existing) = assigned_per_obj[obj_id].get(&span.decoded) {
                    redacted = existing.clone();
                } else {
                    let chosen = pick_unique_key(redacted, &reserved_per_obj[obj_id]);
                    reserved_per_obj[obj_id].insert(chosen.clone());
                    assigned_per_obj[obj_id].insert(span.decoded, chosen.clone());
                    redacted = chosen;
                }
            }
            if let Ok(encoded) = serde_json::to_string(&redacted) {
                let replacement = if span.terminated {
                    encoded.as_str()
                } else {
                    // Keep the unterminated quote unterminated rather than inventing a closing quote.
                    &encoded[..encoded.len() - 1]
                };
                if !changed {
                    out.push_str(&raw[..span.start]);
                    changed = true;
                } else if copied_until < span.start {
                    out.push_str(&raw[copied_until..span.start]);
                }
                out.push_str(replacement);
                copied_until = span.end;
            }
        }
        gap_start = span.end;
    }
    flush_gap(&mut out, &mut changed, &mut copied_until, gap_start, bytes.len());
    if changed {
        if copied_until < bytes.len() {
            out.push_str(&raw[copied_until..]);
        }
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(raw)
    }
}

pub const DEFAULT_BOT_ID: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub created_at: i64,
    pub generation: i64,
    /// Execution environment name; "local" means the primary's machine.
    pub environment: String,
}

/// A remote execution environment — a `blob-1 node` accepted by an admin.
/// `credential` is sha256(node_key) and never leaves the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub name: String,
    pub node_id: String,
    pub credential: Vec<u8>,
    /// Fresh random ID minted at each accept — stable across redials and
    /// renames, invalidated by revoke. Consumers pin this (not the node
    /// key) so re-enrollment under the same name can't take over a binding.
    /// Empty for pending rows and environments accepted before the column
    /// existed (those are backfilled by migration).
    pub enrollment: Vec<u8>,
    /// Stashed node key hash while the row is pending (`None` after accept).
    pub pending_key: Option<Vec<u8>>,
    /// Hostname the node reported; display metadata for pending rows.
    pub hostname: Option<String>,
    pub version: Option<String>,
    pub last_seen: i64,
    pub created_at: i64,
}

pub const LOCAL_ENVIRONMENT: &str = "local";

/// Environment names must not collide with the built-in local executor or
/// the internal `pending:`/`revoked:` row namespaces.
pub fn environment_name_valid(name: &str) -> bool {
    !name.is_empty()
        && name != LOCAL_ENVIRONMENT
        && !name.starts_with("pending:")
        && !name.starts_with("revoked:")
}

/// A revoked environment leaves this tombstone binding on its bots so a
/// re-enrolled node under the same name cannot silently inherit them.
pub fn revoked_environment(name: &str) -> String {
    format!("revoked:{name}")
}

pub trait EnvironmentStore: Send + Sync {
    fn environments(&self) -> Result<Vec<Environment>, StoreError>;
    fn environment(&self, name: &str) -> Result<Option<Environment>, StoreError>;
    fn environment_for_node(&self, node_id: &str) -> Result<Option<Environment>, StoreError>;
    /// The environment whose stored credential matches `sha256(node_key)`.
    fn environment_for_key(&self, key_hash: &[u8]) -> Result<Option<Environment>, StoreError>;
    /// Renames `from` to `to`, returning the row actually renamed — the
    /// caller must not re-read by `to`, which a concurrent mutation could
    /// have recycled for another node. `expected_enrollment` pins the
    /// update to the row the caller read, so a name recycled for a
    /// different node's environment is left alone. Accepted rows only.
    fn rename_environment(
        &self,
        from: &str,
        to: &str,
        expected_enrollment: &[u8],
    ) -> Result<Option<Environment>, StoreError>;
    /// Deletes the environment named `name`, but only the row carrying
    /// `expected_enrollment`: a cross-process rename that recycled the name
    /// for another node leaves the replacement untouched and returns
    /// `false`. Pass an empty slice for rows without an enrollment
    /// (pending).
    fn remove_environment(
        &self,
        name: &str,
        expected_enrollment: &[u8],
    ) -> Result<bool, StoreError>;
    /// Refreshes version/last_seen when a node's session authenticates.
    fn touch_environment(
        &self,
        node_id: &str,
        version: &str,
        last_seen: i64,
    ) -> Result<(), StoreError>;
    /// Records a node awaiting accept (empty credential = pending); the
    /// presented key hash is stashed until accept promotes it. Refreshes
    /// `last_seen`/`version` on repeat connects; accepted rows are untouched.
    /// The first-presented key hash is pinned until the row expires: a
    /// reconnect presenting a different key for the same node_id updates
    /// nothing and returns `false`, so what the operator reviewed is what
    /// `accept_pending_node` enrolls.
    fn upsert_pending_node(
        &self,
        node_id: &str,
        hostname: &str,
        version: &str,
        key_hash: &[u8],
        seen_at: i64,
    ) -> Result<bool, StoreError>;
    /// Drops a pending (unaccepted) node row so a node_id pinned by a
    /// different key can re-enroll. `expected_pending_key` pins the delete
    /// to the row the caller reviewed — a re-registration in between is
    /// left alone. Returns `false` when no matching pending row.
    fn drop_pending_node(
        &self,
        node_id: &str,
        expected_pending_key: &[u8],
    ) -> Result<bool, StoreError>;
    /// Promotes a pending node into a named environment, adopting the key it
    /// presented. `expected_pending_key` pins the promotion to the exact row
    /// the caller reviewed: a deny+re-register in between leaves the new row
    /// untouched and returns `None`.
    fn accept_pending_node(
        &self,
        node_id: &str,
        name: &str,
        expected_pending_key: &[u8],
    ) -> Result<Option<Environment>, StoreError>;
    /// Drops pending nodes whose `last_seen` is older than `before`.
    fn expire_pending_nodes(&self, before: i64) -> Result<Vec<String>, StoreError>;
}

impl<T: EnvironmentStore + ?Sized> EnvironmentStore for Arc<T> {
    fn environments(&self) -> Result<Vec<Environment>, StoreError> {
        (**self).environments()
    }
    fn environment(&self, name: &str) -> Result<Option<Environment>, StoreError> {
        (**self).environment(name)
    }
    fn environment_for_node(&self, node_id: &str) -> Result<Option<Environment>, StoreError> {
        (**self).environment_for_node(node_id)
    }
    fn environment_for_key(&self, key_hash: &[u8]) -> Result<Option<Environment>, StoreError> {
        (**self).environment_for_key(key_hash)
    }
    fn rename_environment(
        &self,
        from: &str,
        to: &str,
        expected_enrollment: &[u8],
    ) -> Result<Option<Environment>, StoreError> {
        (**self).rename_environment(from, to, expected_enrollment)
    }
    fn remove_environment(
        &self,
        name: &str,
        expected_enrollment: &[u8],
    ) -> Result<bool, StoreError> {
        (**self).remove_environment(name, expected_enrollment)
    }
    fn touch_environment(
        &self,
        node_id: &str,
        version: &str,
        last_seen: i64,
    ) -> Result<(), StoreError> {
        (**self).touch_environment(node_id, version, last_seen)
    }
    fn upsert_pending_node(
        &self,
        node_id: &str,
        hostname: &str,
        version: &str,
        key_hash: &[u8],
        seen_at: i64,
    ) -> Result<bool, StoreError> {
        (**self).upsert_pending_node(node_id, hostname, version, key_hash, seen_at)
    }
    fn drop_pending_node(
        &self,
        node_id: &str,
        expected_pending_key: &[u8],
    ) -> Result<bool, StoreError> {
        (**self).drop_pending_node(node_id, expected_pending_key)
    }
    fn accept_pending_node(
        &self,
        node_id: &str,
        name: &str,
        expected_pending_key: &[u8],
    ) -> Result<Option<Environment>, StoreError> {
        (**self).accept_pending_node(node_id, name, expected_pending_key)
    }
    fn expire_pending_nodes(&self, before: i64) -> Result<Vec<String>, StoreError> {
        (**self).expire_pending_nodes(before)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BotModels {
    pub main: Option<ModelRef>,
    pub main_reasoning: ReasoningLevel,
    pub compaction: Option<ModelRef>,
    pub compaction_reasoning: Option<ReasoningLevel>,
    pub utility: Option<(ModelRef, Option<ReasoningLevel>)>,
    pub decision: Option<ModelRef>,
}

impl Default for BotModels {
    fn default() -> Self {
        Self {
            main: None,
            main_reasoning: ReasoningLevel::Medium,
            compaction: None,
            compaction_reasoning: None,
            utility: None,
            decision: None,
        }
    }
}

pub trait BotStore: Send + Sync {
    fn bots(&self) -> Result<Vec<Bot>, StoreError>;
    fn bot(&self, id: &str) -> Result<Option<Bot>, StoreError>;
    fn create_bot(&self, id: &str, name: &str, created_at: i64) -> Result<Bot, StoreError>;
    fn update_bot(
        &self,
        id: &str,
        name: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<Option<Bot>, StoreError>;
    fn remove_bot(&self, id: &str) -> Result<bool, StoreError>;
    /// Replaces the bot's custom avatar image; `updated_at` versions it for cache busting.
    fn set_bot_avatar(
        &self,
        bot_id: &str,
        data: &[u8],
        content_type: &str,
        updated_at: i64,
    ) -> Result<(), StoreError>;
    /// The bot's custom avatar bytes and content type, if one is set.
    fn bot_avatar(&self, bot_id: &str) -> Result<Option<(Vec<u8>, String)>, StoreError>;
    fn clear_bot_avatar(&self, bot_id: &str) -> Result<(), StoreError>;
    /// `(bot_id, updated_at)` for every custom avatar, for cache-busted URLs in bot listings.
    fn bot_avatar_versions(&self) -> Result<Vec<(String, i64)>, StoreError>;
    fn bot_models(&self, bot_id: &str) -> Result<BotModels, StoreError>;
    fn set_bot_main_model(
        &self,
        bot_id: &str,
        model: Option<&ModelRef>,
        reasoning: ReasoningLevel,
    ) -> Result<(), StoreError>;
    fn set_bot_compaction_model(
        &self,
        bot_id: &str,
        model: Option<&ModelRef>,
        reasoning: Option<ReasoningLevel>,
    ) -> Result<(), StoreError>;
    fn set_bot_utility_model(
        &self,
        bot_id: &str,
        model: Option<(&ModelRef, Option<ReasoningLevel>)>,
    ) -> Result<(), StoreError>;
    fn set_bot_decision_model(
        &self,
        bot_id: &str,
        model: Option<&ModelRef>,
    ) -> Result<(), StoreError>;
    fn provider_in_use_by_bot(&self, provider: &ProviderId) -> Result<Option<String>, StoreError>;
    fn bot_disabled_mcp_servers(&self, bot_id: &str) -> Result<Vec<String>, StoreError>;
    /// Skill modes the bot overrides; skills missing here are `SkillMode::Both`.
    fn bot_skill_modes(
        &self,
        bot_id: &str,
    ) -> Result<std::collections::BTreeMap<String, SkillMode>, StoreError>;
    fn set_bot_skill_mode(
        &self,
        bot_id: &str,
        name: &str,
        mode: SkillMode,
    ) -> Result<(), StoreError>;
    /// Forgets the skill's mode for `bot_id`, or for every bot when `None`.
    fn clear_skill_modes(&self, name: &str, bot_id: Option<&str>) -> Result<(), StoreError>;
    /// `None` binds the bot back to `local`.
    fn set_bot_environment(
        &self,
        bot_id: &str,
        environment: Option<&str>,
    ) -> Result<bool, StoreError>;
    /// The bot's environment name and the bound env's enrollment ID, read in
    /// one query so a revoke+re-accept between reads can't split them.
    /// `enrollment` is `None` for `local`, tombstones, and unaccepted rows.
    fn bot_env_binding(
        &self,
        bot_id: &str,
    ) -> Result<Option<(String, Option<Vec<u8>>)>, StoreError>;
    fn set_bot_mcp_server_enabled(
        &self,
        bot_id: &str,
        server_id: &str,
        enabled: bool,
    ) -> Result<(), StoreError>;
}

impl<T: BotStore + ?Sized> BotStore for Arc<T> {
    fn bots(&self) -> Result<Vec<Bot>, StoreError> {
        (**self).bots()
    }
    fn bot(&self, id: &str) -> Result<Option<Bot>, StoreError> {
        (**self).bot(id)
    }
    fn create_bot(&self, id: &str, name: &str, created_at: i64) -> Result<Bot, StoreError> {
        (**self).create_bot(id, name, created_at)
    }
    fn update_bot(
        &self,
        id: &str,
        name: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<Option<Bot>, StoreError> {
        (**self).update_bot(id, name, enabled)
    }
    fn remove_bot(&self, id: &str) -> Result<bool, StoreError> {
        (**self).remove_bot(id)
    }
    fn set_bot_avatar(
        &self,
        bot_id: &str,
        data: &[u8],
        content_type: &str,
        updated_at: i64,
    ) -> Result<(), StoreError> {
        (**self).set_bot_avatar(bot_id, data, content_type, updated_at)
    }
    fn bot_avatar(&self, bot_id: &str) -> Result<Option<(Vec<u8>, String)>, StoreError> {
        (**self).bot_avatar(bot_id)
    }
    fn clear_bot_avatar(&self, bot_id: &str) -> Result<(), StoreError> {
        (**self).clear_bot_avatar(bot_id)
    }
    fn bot_avatar_versions(&self) -> Result<Vec<(String, i64)>, StoreError> {
        (**self).bot_avatar_versions()
    }
    fn bot_models(&self, bot_id: &str) -> Result<BotModels, StoreError> {
        (**self).bot_models(bot_id)
    }
    fn set_bot_main_model(
        &self,
        bot_id: &str,
        model: Option<&ModelRef>,
        reasoning: ReasoningLevel,
    ) -> Result<(), StoreError> {
        (**self).set_bot_main_model(bot_id, model, reasoning)
    }
    fn set_bot_compaction_model(
        &self,
        bot_id: &str,
        model: Option<&ModelRef>,
        reasoning: Option<ReasoningLevel>,
    ) -> Result<(), StoreError> {
        (**self).set_bot_compaction_model(bot_id, model, reasoning)
    }
    fn set_bot_utility_model(
        &self,
        bot_id: &str,
        model: Option<(&ModelRef, Option<ReasoningLevel>)>,
    ) -> Result<(), StoreError> {
        (**self).set_bot_utility_model(bot_id, model)
    }
    fn set_bot_decision_model(
        &self,
        bot_id: &str,
        model: Option<&ModelRef>,
    ) -> Result<(), StoreError> {
        (**self).set_bot_decision_model(bot_id, model)
    }
    fn provider_in_use_by_bot(&self, provider: &ProviderId) -> Result<Option<String>, StoreError> {
        (**self).provider_in_use_by_bot(provider)
    }
    fn bot_disabled_mcp_servers(&self, bot_id: &str) -> Result<Vec<String>, StoreError> {
        (**self).bot_disabled_mcp_servers(bot_id)
    }
    fn bot_skill_modes(
        &self,
        bot_id: &str,
    ) -> Result<std::collections::BTreeMap<String, SkillMode>, StoreError> {
        (**self).bot_skill_modes(bot_id)
    }
    fn set_bot_skill_mode(
        &self,
        bot_id: &str,
        name: &str,
        mode: SkillMode,
    ) -> Result<(), StoreError> {
        (**self).set_bot_skill_mode(bot_id, name, mode)
    }
    fn clear_skill_modes(&self, name: &str, bot_id: Option<&str>) -> Result<(), StoreError> {
        (**self).clear_skill_modes(name, bot_id)
    }
    fn set_bot_environment(
        &self,
        bot_id: &str,
        environment: Option<&str>,
    ) -> Result<bool, StoreError> {
        (**self).set_bot_environment(bot_id, environment)
    }
    fn bot_env_binding(
        &self,
        bot_id: &str,
    ) -> Result<Option<(String, Option<Vec<u8>>)>, StoreError> {
        (**self).bot_env_binding(bot_id)
    }
    fn set_bot_mcp_server_enabled(
        &self,
        bot_id: &str,
        server_id: &str,
        enabled: bool,
    ) -> Result<(), StoreError> {
        (**self).set_bot_mcp_server_enabled(bot_id, server_id, enabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callback {
    pub id: String,
    pub name: String,
    pub for_token: Option<String>,
    pub remaining_uses: u32,
    pub expires_at: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEvent {
    pub id: i64,
    pub source: String,
    pub name: String,
    pub for_token: Option<String>,
    pub payload: String,
    pub created_at: i64,
}

pub trait CallbackStore: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn create_callback(
        &self,
        id: &str,
        name: &str,
        token_hash: &[u8],
        for_token: Option<&str>,
        max_uses: u32,
        expires_at: i64,
        created_at: i64,
    ) -> Result<Callback, StoreError>;
    fn has_active_callback(&self, token_hash: &[u8], now: i64) -> Result<bool, StoreError>;
    /// Atomically consumes one use of the callback AND persists the wake event
    /// into `pending_events` in the same SQLite transaction so a `202 Accepted`
    /// callback survives crashes, restarts, or transient turn failures.
    fn consume_callback(
        &self,
        token_hash: &[u8],
        now: i64,
        payload: &str,
    ) -> Result<Option<Callback>, StoreError>;
    fn next_pending_event(&self) -> Result<Option<PendingEvent>, StoreError>;
    fn delete_pending_event(&self, id: i64) -> Result<(), StoreError>;
    /// Enqueues a system-originated event (e.g. environment notices) that the
    /// bot loop consumes like a callback wake.
    fn add_pending_event(
        &self,
        source: &str,
        name: &str,
        for_token: Option<&str>,
        payload: &str,
        created_at: i64,
    ) -> Result<i64, StoreError>;
}

impl<T: CallbackStore + ?Sized> CallbackStore for Arc<T> {
    fn create_callback(
        &self,
        id: &str,
        name: &str,
        token_hash: &[u8],
        for_token: Option<&str>,
        max_uses: u32,
        expires_at: i64,
        created_at: i64,
    ) -> Result<Callback, StoreError> {
        (**self).create_callback(
            id, name, token_hash, for_token, max_uses, expires_at, created_at,
        )
    }
    fn has_active_callback(&self, token_hash: &[u8], now: i64) -> Result<bool, StoreError> {
        (**self).has_active_callback(token_hash, now)
    }
    fn consume_callback(
        &self,
        token_hash: &[u8],
        now: i64,
        payload: &str,
    ) -> Result<Option<Callback>, StoreError> {
        (**self).consume_callback(token_hash, now, payload)
    }
    fn next_pending_event(&self) -> Result<Option<PendingEvent>, StoreError> {
        (**self).next_pending_event()
    }
    fn delete_pending_event(&self, id: i64) -> Result<(), StoreError> {
        (**self).delete_pending_event(id)
    }
    fn add_pending_event(
        &self,
        source: &str,
        name: &str,
        for_token: Option<&str>,
        payload: &str,
        created_at: i64,
    ) -> Result<i64, StoreError> {
        (**self).add_pending_event(source, name, for_token, payload, created_at)
    }
}
