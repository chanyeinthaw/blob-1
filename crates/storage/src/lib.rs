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
    fn home(&self) -> Result<Option<Home>, StoreError>;
    fn set_home(&self, home: Option<&Home>) -> Result<(), StoreError>;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Meta {
    Checkin {
        time: String,
        #[serde(default)]
        home: Option<u64>,
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
        #[serde(default)]
        home: Option<u64>,
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
                home.map_or_else(String::new, |v| format!(" home=\"d{v}\""))
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
                home.map_or_else(String::new, |v| format!(" home=\"d{v}\""))
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
                inline,
                unaddressed,
                ..
            } => format!(
                "<message messenger=\"{}\" reply=\"m{i}\" to=\"d{to}\" author=\"{}\" author_name=\"{}\" where=\"{}\"{}{}{}{}>\n{}{}{body}\n</message>",
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
                quote.as_ref().map_or_else(String::new, |quote| format!(
                    "<quote author=\"{}\" author_name=\"{}\" message=\"{}\">\n{}{}\n</quote>\n",
                    attribute(&quote.author.id), attribute(&quote.author.name), attribute(&quote.message), render_attachments(&quote.attachments), escape(&quote.text)
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

/// Redacts ephemeral callback credentials (`cbk_<43-char-token>` anywhere in
/// text, as well as `/hooks/cb/<token>` paths) before persisting transcripts or
/// memory rows so database snapshots never leak active callback tokens.
pub fn redact_callback_tokens(text: &str) -> std::borrow::Cow<'_, str> {
    const URL_PREFIX: &str = "/hooks/cb/";
    const TOKEN_PREFIX: &str = "cbk_";
    const TOKEN_LEN: usize = 43;
    let is_token_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if !text.contains(URL_PREFIX) && !text.contains(TOKEN_PREFIX) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut prev_is_token = false;
    let mut replaced = false;
    while !rest.is_empty() {
        let next_url = rest.find(URL_PREFIX).map(|pos| (pos, true));
        let next_tok = rest.find(TOKEN_PREFIX).map(|pos| (pos, false));
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
            let split = pos + URL_PREFIX.len();
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
            let after = &rest[pos + TOKEN_PREFIX.len()..];
            let token_len = after.bytes().take_while(|&b| is_token_byte(b)).count();
            if boundary_ok && token_len == TOKEN_LEN {
                out.push_str("<redacted>");
                rest = &after[token_len..];
                prev_is_token = false;
                replaced = true;
            } else {
                out.push_str(TOKEN_PREFIX);
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

/// Returns true if `s` starts immediately with a `cbk_<43-char-token>`
/// credential on a non-token end boundary.
fn starts_with_callback_credential(s: &str) -> bool {
    const TOKEN_PREFIX: &str = "cbk_";
    const TOKEN_LEN: usize = 43;
    let is_token_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if let Some(after) = s.strip_prefix(TOKEN_PREFIX) {
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
                        bytes[esc] == b'c' && {
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
}
