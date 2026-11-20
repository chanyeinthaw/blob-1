use std::path::PathBuf;
use std::sync::Arc;
use std::{collections::HashMap, sync::Mutex};

use environment::{Environment, ToolCall, ToolInput};
pub use llm::{CompletionRequest, CompletionResponse, Llm, LlmError, ToolCallRequest, Usage};
use storage::{Message, Session, SessionId, SessionStore, SessionUsage, StoreError};

use crate::{compaction, prompt};

fn usage_tokens(usage: Usage) -> u64 {
    if usage.total > 0 {
        usage.total
    } else {
        usage.input + usage.output + usage.cache_read + usage.cache_write
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    pub context_window: Option<u32>,
    pub summarizer_window: Option<u32>,
    pub reserve_tokens: u32,
    pub keep_recent_tokens: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            context_window: None,
            summarizer_window: None,
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
    Memory(#[from] memory::MemoryError),
    #[error(transparent)]
    Environment(#[from] environment::ToolError),
    #[error("{0}")]
    Compaction(String),
    #[error("Unknown model {0}; call models to see what is available.")]
    UnknownModel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentPaths {
    pub workspace: PathBuf,
}

#[derive(Debug, Clone)]
pub enum Step {
    Model,
    Tool { name: String },
    Idle,
}

pub trait MemoryTools: Send + Sync {
    fn zoom(&self, id: crate::chat::ZoomId, n: u64, at: usize) -> Result<String, HarnessError>;
    fn date(&self, id: u64) -> Result<String, HarnessError>;
}

pub struct SessionHarness<L, E, S> {
    llm: L,
    memory_tools: Option<Arc<dyn MemoryTools>>,
    observer: Option<Arc<dyn Fn(Step) + Send + Sync>>,
    summarizer: Option<L>,
    environment: E,
    store: S,
    paths: EnvironmentPaths,
    config: HarnessConfig,
    steering: Mutex<HashMap<SessionId, Vec<String>>>,
}

impl<L, E, S> SessionHarness<L, E, S>
where
    L: Llm,
    E: Environment,
    S: SessionStore,
{
    pub fn new(llm: L, environment: E, store: S, paths: EnvironmentPaths) -> Self {
        Self {
            llm,
            memory_tools: None,
            observer: None,
            summarizer: None,
            environment,
            store,
            paths,
            config: HarnessConfig::default(),
            steering: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_memory_tools(mut self, tools: Arc<dyn MemoryTools>) -> Self {
        self.memory_tools = Some(tools);
        self
    }

    pub fn with_observer(mut self, observer: Arc<dyn Fn(Step) + Send + Sync>) -> Self {
        self.observer = Some(observer);
        self
    }

    fn observe(&self, step: Step) {
        if let Some(observer) = &self.observer {
            observer(step);
        }
    }

    pub fn with_summarizer(mut self, llm: L) -> Self {
        self.summarizer = Some(llm);
        self
    }

    pub fn with_config(mut self, config: HarnessConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn run(&self, id: &SessionId, user_message: String) -> Result<String, HarnessError> {
        self.run_pending(id, Some(user_message)).await
    }

    pub(crate) async fn run_pending(
        &self,
        id: &SessionId,
        user_message: Option<String>,
    ) -> Result<String, HarnessError> {
        let result = self.run_inner(id, user_message).await;
        self.observe(Step::Idle);
        result
    }

    async fn run_inner(
        &self,
        id: &SessionId,
        user_message: Option<String>,
    ) -> Result<String, HarnessError> {
        let mut session = match self.store.load(id)? {
            Some(session) => session,
            None => Session::new(id.clone()),
        };

        let skills = self.environment.skill_catalog().await?;
        let system = prompt::build(&self.paths.workspace, &skills);
        let system = if self.memory_tools.is_some() {
            format!("{}\n\n{}", prompt::SUB_PROMPT, system)
        } else {
            system
        };
        let mut tools = environment::CATALOG.to_vec();
        if self.memory_tools.is_some() {
            tools.extend_from_slice(&crate::chat::EXTRA[..2]);
        }

        if let Some(user_message) = user_message {
            session.push(Message::User(user_message));
            self.store.save(session.clone())?;
        }

        let mut overflow_retried = false;
        let mut retrying = false;
        loop {
            self.apply_steering(&mut session, id);

            if !retrying
                && self.config.compaction.context_window.is_some_and(|window| {
                    compaction::context_tokens(&system, &session.messages, session.usage)
                        > u64::from(window.saturating_sub(self.config.compaction.reserve_tokens))
                })
            {
                self.compact(&mut session, false).await?;
            }
            retrying = false;

            self.observe(Step::Model);
            let response = match self.llm.complete(CompletionRequest {
                system: &system,
                messages: &session.messages,
                tools: &tools,
            }).await {
                Ok(response) => response,
                Err(LlmError::ContextOverflow(_)) if !overflow_retried => {
                    if self.compact(&mut session, true).await? {
                        overflow_retried = true;
                        retrying = true;
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
                if let Some(value) = response.usage {
                    session.usage = Some(SessionUsage {
                        tokens: usage_tokens(value),
                        boundary: session.messages.len(),
                    });
                }
                if self.apply_steering(&mut session, id) {
                    self.store.save(session.clone())?;
                    continue;
                }
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
                session.usage = Some(SessionUsage {
                    tokens: usage_tokens(value),
                    boundary: session.messages.len(),
                });
            }
            for call in response.tool_calls {
                self.observe(Step::Tool {
                    name: call.name.clone(),
                });
                let memory_result =
                    self.memory_tools
                        .as_ref()
                        .and_then(|tools| match call.name.as_str() {
                            "zoom" => Some(
                                serde_json::from_str::<crate::chat::Zoom>(&call.arguments)
                                    .map_err(|e| e.to_string())
                                    .and_then(|args| {
                                        tools
                                            .zoom(args.id, args.n, args.at)
                                            .map_err(|e| e.to_string())
                                    }),
                            ),
                            "date" => Some(
                                serde_json::from_str::<crate::chat::Date>(&call.arguments)
                                    .map_err(|e| e.to_string())
                                    .and_then(|args| {
                                        tools.date(args.id).map_err(|e| e.to_string())
                                    }),
                            ),
                            _ => None,
                        });
                let executed = if let Some(result) = memory_result {
                    result.map(|content| memory::clip_tool_output(&content))
                } else {
                    match ToolInput::parse(&call.name, &call.arguments) {
                        Ok(input) => self
                            .environment
                            .execute(ToolCall {
                                id: call.id.clone(),
                                input,
                            })
                            .await
                            .map(|result| result.content)
                            .map_err(|e| e.to_string()),
                        Err(error) => Err(error.to_string()),
                    }
                };
                self.observe(Step::Idle);
                let content = match executed {
                    Ok(result) => result,
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

    pub(crate) fn take_steering(&self, id: &SessionId) -> Vec<String> {
        self.steering
            .lock()
            .expect("steering queue mutex poisoned")
            .remove(id)
            .unwrap_or_default()
    }

    fn apply_steering(&self, session: &mut Session, id: &SessionId) -> bool {
        let queued = self
            .steering
            .lock()
            .expect("steering queue mutex poisoned")
            .remove(id)
            .unwrap_or_default();

        let pending = !queued.is_empty();
        for message in queued {
            session.push(Message::User(message));
        }
        pending
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
        let observer = &self.observer;
        let Some(messages) = compaction::compact(
            self.summarizer.as_ref().unwrap_or(&self.llm),
            &session.messages,
            keep,
            config.summarizer_window,
            config.reserve_tokens,
            &|| {
                if let Some(observer) = observer {
                    observer(Step::Model);
                }
            },
        )
        .await?
        else {
            return Ok(false);
        };
        if overflow
            && compaction::context_tokens("", &messages, None)
                >= compaction::context_tokens("", &session.messages, None)
        {
            return Ok(false);
        }
        session.messages = messages;
        session.usage = None;
        self.store.save(session.clone())?;
        Ok(true)
    }
}
