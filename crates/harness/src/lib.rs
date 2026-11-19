use std::path::PathBuf;
use std::{collections::HashMap, sync::Mutex};

use environment::{Environment, ToolCall, ToolInput, ToolSpec};
use storage::{
    Message, Session, SessionId, SessionStore, StoreError, WorkspaceError, WorkspaceStore,
};

mod compaction;
pub mod prompt;
pub mod skills;

#[derive(Debug, Clone)]
pub struct CompletionRequest<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCallRequest>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("LLM request failed: {0}")]
    Request(String),
    #[error("context overflow: {0}")]
    ContextOverflow(String),
}

#[async_trait::async_trait]
pub trait Llm {
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<CompletionResponse, LlmError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    pub context_window: Option<u32>,
    pub reserve_tokens: u32,
    pub keep_recent_tokens: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            context_window: None,
            reserve_tokens: 16384,
            keep_recent_tokens: 20000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HarnessConfig {
    pub compaction: CompactionConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("{0}")]
    Compaction(String),
    #[error("invalid skill: {0}")]
    Skill(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentPaths {
    pub workspace: PathBuf,
    pub skills: PathBuf,
}

pub struct Harness<L, E, S> {
    llm: L,
    environment: E,
    store: S,
    paths: EnvironmentPaths,
    config: HarnessConfig,
    steering: Mutex<HashMap<SessionId, Vec<String>>>,
}

impl<L, E, S> Harness<L, E, S>
where
    L: Llm,
    E: Environment,
    S: SessionStore + WorkspaceStore,
{
    pub fn new(llm: L, environment: E, store: S, paths: EnvironmentPaths) -> Self {
        Self {
            llm,
            environment,
            store,
            paths,
            config: HarnessConfig::default(),
            steering: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_config(mut self, config: HarnessConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn run(&self, id: &SessionId, user_message: String) -> Result<String, HarnessError> {
        let mut session = match self.store.load(id)? {
            Some(session) => session,
            None => Session::new(id.clone()),
        };

        let skills = skills::catalog(&self.store, &self.paths.skills)?;
        let system = prompt::build(&self.paths.workspace, &skills);

        session.push(Message::User(user_message));
        self.store.save(session.clone())?;

        let mut usage = None;
        let mut overflow_retried = false;
        let mut retrying = false;
        loop {
            self.apply_steering(&mut session, id);

            if !retrying
                && self.config.compaction.context_window.is_some_and(|window| {
                    compaction::context_tokens(&system, &session.messages, usage)
                        > u64::from(window.saturating_sub(self.config.compaction.reserve_tokens))
                })
                && self.compact(&mut session, false).await?
            {
                usage = None;
            }
            retrying = false;

            let response = match self.llm.complete(CompletionRequest {
                system: &system,
                messages: &session.messages,
                tools: environment::CATALOG,
            }).await {
                Ok(response) => response,
                Err(LlmError::ContextOverflow(_)) if !overflow_retried => {
                    if self.compact(&mut session, true).await? {
                        overflow_retried = true;
                        retrying = true;
                        usage = None;
                        continue;
                    }
                    return Err(HarnessError::Compaction(
                        "Context overflow recovery failed after one compact-and-retry attempt. Try reducing context or switching to a larger-context model.".into(),
                    ));
                }
                Err(LlmError::ContextOverflow(_)) => return Err(HarnessError::Compaction(
                    "Context overflow recovery failed after one compact-and-retry attempt. Try reducing context or switching to a larger-context model.".into(),
                )),
                Err(error) => return Err(error.into()),
            };
            if !response.text.is_empty() {
                session.push(Message::Assistant(response.text.clone()));
            }
            if response.tool_calls.is_empty() {
                self.store.save(session)?;
                return Ok(response.text);
            }

            for call in &response.tool_calls {
                session.push(Message::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                });
            }
            if let Some(value) = response.usage {
                usage = Some((value, session.messages.len()));
            }
            for call in response.tool_calls {
                let executed = match ToolInput::parse(&call.name, &call.arguments) {
                    Ok(input) => {
                        self.environment
                            .execute(ToolCall {
                                id: call.id.clone(),
                                input,
                            })
                            .await
                    }
                    Err(error) => Err(error),
                };
                let content = match executed {
                    Ok(result) => result.content,
                    Err(error) => format!("error: {error}"),
                };
                session.push(Message::ToolResult {
                    id: call.id,
                    name: call.name,
                    content,
                });
            }
            self.store.save(session.clone())?;
        }
    }

    pub fn steer(&self, id: &SessionId, message: String) -> Result<(), HarnessError> {
        self.steering
            .lock()
            .expect("steering queue mutex poisoned")
            .entry(id.clone())
            .or_default()
            .push(message);
        Ok(())
    }

    fn apply_steering(&self, session: &mut Session, id: &SessionId) {
        let queued = self
            .steering
            .lock()
            .expect("steering queue mutex poisoned")
            .remove(id)
            .unwrap_or_default();

        for message in queued {
            session.push(Message::User(message));
        }
    }

    async fn compact(&self, session: &mut Session, overflow: bool) -> Result<bool, HarnessError> {
        let config = self.config.compaction;
        let keep = config
            .context_window
            .map_or(config.keep_recent_tokens, |window| {
                config
                    .keep_recent_tokens
                    .min(window.saturating_sub(config.reserve_tokens) / 2)
                    .max(1)
            });
        let keep = if overflow {
            u64::from(keep)
                .min(compaction::context_tokens("", &session.messages, None) / 2)
                .max(1) as u32
        } else {
            keep
        };
        let Some(messages) = compaction::compact(&self.llm, &session.messages, keep).await? else {
            return Ok(false);
        };
        if overflow
            && compaction::context_tokens("", &messages, None)
                >= compaction::context_tokens("", &session.messages, None)
        {
            return Ok(false);
        }
        session.messages = messages;
        self.store.save(session.clone())?;
        Ok(true)
    }
}
