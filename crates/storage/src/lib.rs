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
    fn identity(&self) -> Result<Option<String>, StoreError>;
    fn set_identity(&self, text: Option<&str>) -> Result<(), StoreError>;
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

pub fn render_input(meta: Option<&Meta>, i: u64, body: &str) -> String {
    match meta {
        Some(meta) => meta.render(i, body),
        None => format!("<message>\n{}\n</message>", escape(body)),
    }
}

fn for_attribute(value: &Option<String>) -> String {
    value.as_ref().map_or_else(String::new, |value| {
        format!(" for=\"{}\"", attribute(value))
    })
}

impl Meta {
    pub fn render(&self, i: u64, body: &str) -> String {
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
                "<message messenger=\"{}\" reply=\"m{i}\" to=\"d{to}\" author=\"{}\" author_name=\"{}\" where=\"{}\"{}{}{}>\n{}{}{body}\n</message>",
                attribute(messenger),
                attribute(&author.id),
                attribute(&author.name),
                attribute(location),
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
/// or a user id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionScope {
    Channel,
    User,
}

impl InteractionScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::User => "user",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|scope| scope.as_str() == s)
    }

    const ALL: [Self; 2] = [Self::Channel, Self::User];
}

/// How eagerly the bot engages: `MentionOnly` answers DMs, mentions and
/// replies; `Always` answers every message in scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionMode {
    MentionOnly,
    Always,
}

impl InteractionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MentionOnly => "mention-only",
            Self::Always => "always",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == s)
    }

    const ALL: [Self; 2] = [Self::MentionOnly, Self::Always];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionOverride {
    pub scope: InteractionScope,
    /// Discord guild id for `User` overrides; `None` for `Channel` overrides.
    pub guild_id: Option<String>,
    pub target_id: String,
    pub mode: InteractionMode,
}

/// How the bot replies to plain-channel messages (outside DMs and threads):
/// `decision` asks the utility model (falling back to a thread), `thread`
/// always starts a thread, and `direct` always replies inline in the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMode {
    Decision,
    Thread,
    Direct,
}

impl ReplyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Thread => "thread",
            Self::Direct => "direct",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == s)
    }

    const ALL: [Self; 3] = [Self::Decision, Self::Thread, Self::Direct];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyOverride {
    pub channel_id: String,
    pub mode: ReplyMode,
}

/// Who can talk to the bot on a messenger: everyone (`allow-all`), only listed
/// users (`whitelist`, the default), or everyone except listed users (`blacklist`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    AllowAll,
    Whitelist,
    Blacklist,
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllowAll => "allow-all",
            Self::Whitelist => "whitelist",
            Self::Blacklist => "blacklist",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == s)
    }

    const ALL: [Self; 3] = [Self::AllowAll, Self::Whitelist, Self::Blacklist];
}

/// Which access list an entry belongs to. Whitelist and blacklist entries are
/// stored separately so switching modes never flips trusted users into banned ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessList {
    Whitelist,
    Blacklist,
}

impl AccessList {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Whitelist => "whitelist",
            Self::Blacklist => "blacklist",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|list| list.as_str() == s)
    }

    const ALL: [Self; 2] = [Self::Whitelist, Self::Blacklist];
}

/// Where an access entry applies: globally (including DMs), in a specific
/// Discord server (guild), or in a specific channel/thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessScope {
    Global,
    Guild,
    Channel,
}

impl AccessScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Guild => "guild",
            Self::Channel => "channel",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.as_str() == s)
    }

    const ALL: [Self; 3] = [Self::Global, Self::Guild, Self::Channel];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessEntry {
    pub list: AccessList,
    pub scope: AccessScope,
    /// Server (guild) id for `Guild`, channel/thread id for `Channel`, `None` for `Global`.
    pub scope_id: Option<String>,
    pub user_id: String,
}

pub trait MessengerStore: Send + Sync {
    fn destination(&self, messenger: &str, place: &Place) -> Result<u64, StoreError>;
    fn place(&self, id: u64) -> Result<Option<(String, Place)>, StoreError>;
    fn allowed_users(&self, messenger: &str) -> Result<Vec<String>, StoreError>;
    fn allow_user(&self, messenger: &str, user: &str) -> Result<(), StoreError>;
    fn disallow_user(&self, messenger: &str, user: &str) -> Result<bool, StoreError>;
    /// Access policy for a messenger; defaults to `AccessMode::Whitelist`.
    fn access_mode(&self, messenger: &str) -> Result<AccessMode, StoreError>;
    fn set_access_mode(&self, messenger: &str, mode: AccessMode) -> Result<(), StoreError>;
    /// Whitelist and blacklist entries, ordered by list, scope, scope id, then user id.
    fn access_entries(&self, messenger: &str) -> Result<Vec<AccessEntry>, StoreError>;
    fn add_access_entry(
        &self,
        messenger: &str,
        list: AccessList,
        scope: AccessScope,
        scope_id: Option<&str>,
        user_id: &str,
    ) -> Result<(), StoreError>;
    fn remove_access_entry(
        &self,
        messenger: &str,
        list: AccessList,
        scope: AccessScope,
        scope_id: Option<&str>,
        user_id: &str,
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
    /// Per-channel reply mode overrides, ordered by channel id.
    fn reply_modes(&self, messenger: &str) -> Result<Vec<ReplyOverride>, StoreError>;
    fn set_reply_mode(
        &self,
        messenger: &str,
        channel_id: &str,
        mode: ReplyMode,
    ) -> Result<(), StoreError>;
    fn clear_reply_mode(&self, messenger: &str, channel_id: &str) -> Result<bool, StoreError>;
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
    fn access_mode(&self, messenger: &str) -> Result<AccessMode, StoreError> {
        (**self).access_mode(messenger)
    }
    fn set_access_mode(&self, messenger: &str, mode: AccessMode) -> Result<(), StoreError> {
        (**self).set_access_mode(messenger, mode)
    }
    fn access_entries(&self, messenger: &str) -> Result<Vec<AccessEntry>, StoreError> {
        (**self).access_entries(messenger)
    }
    fn add_access_entry(
        &self,
        messenger: &str,
        list: AccessList,
        scope: AccessScope,
        scope_id: Option<&str>,
        user_id: &str,
    ) -> Result<(), StoreError> {
        (**self).add_access_entry(messenger, list, scope, scope_id, user_id)
    }
    fn remove_access_entry(
        &self,
        messenger: &str,
        list: AccessList,
        scope: AccessScope,
        scope_id: Option<&str>,
        user_id: &str,
    ) -> Result<bool, StoreError> {
        (**self).remove_access_entry(messenger, list, scope, scope_id, user_id)
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
        channel_id: &str,
        mode: ReplyMode,
    ) -> Result<(), StoreError> {
        (**self).set_reply_mode(messenger, channel_id, mode)
    }
    fn clear_reply_mode(&self, messenger: &str, channel_id: &str) -> Result<bool, StoreError> {
        (**self).clear_reply_mode(messenger, channel_id)
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
