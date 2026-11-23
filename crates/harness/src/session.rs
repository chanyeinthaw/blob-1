use std::path::PathBuf;
use std::sync::Arc;
use std::{collections::HashMap, sync::Mutex};

use environment::{Environment, ToolCall, ToolInput};
pub use llm::{CompletionRequest, CompletionResponse, Llm, LlmError, ToolCallRequest, Usage};
use mcp::SessionTools;
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
    pub images: bool,
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

type Summarizer<L> = dyn Fn() -> Option<(L, Option<u32>)> + Send + Sync;

pub struct SessionHarness<L, E, S> {
    llm: L,
    memory_tools: Option<Arc<dyn MemoryTools>>,
    lookup_tools: Option<crate::Messengers>,
    share_tools: Option<crate::ShareTools>,
    mcp_tools: Option<Arc<SessionTools>>,
    observer: Option<Arc<dyn Fn(Step) + Send + Sync>>,
    summarizer: Option<Arc<Summarizer<L>>>,
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
            lookup_tools: None,
            share_tools: None,
            mcp_tools: None,
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

    pub fn with_lookup_tools(mut self, tools: crate::Messengers) -> Self {
        self.lookup_tools = Some(tools);
        self
    }

    pub fn with_share_tools(mut self, tools: crate::ShareTools) -> Self {
        self.share_tools = Some(tools);
        self
    }

    /// Static `mcp_*` tools (`mcp_search`, `mcp_execute`, resources, prompts)
    /// exposed to this session when at least one MCP server is enabled.
    pub fn with_mcp_tools(mut self, tools: Arc<SessionTools>) -> Self {
        self.mcp_tools = Some(tools);
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

    /// Resolves the compaction model and its usable window at the start of
    /// each compaction pass; a pass keeps the model it resolved. When the
    /// resolver yields no model the session's own model summarizes.
    pub fn with_summarizer(
        mut self,
        resolve: Arc<dyn Fn() -> Option<(L, Option<u32>)> + Send + Sync>,
    ) -> Self {
        self.summarizer = Some(resolve);
        self
    }

    pub fn with_config(mut self, config: HarnessConfig) -> Self {
        self.config = config;
        self
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
        let mcp_servers = match &self.mcp_tools {
            Some(tools) => tools.mcp().describe().await,
            None => Vec::new(),
        };
        let system = prompt::build(&self.paths.workspace, &skills, &mcp_servers);
        let system = if self.memory_tools.is_some() {
            format!("{}\n\n{}", prompt::SUB_PROMPT, system)
        } else {
            system
        };
        let mut tools = environment::CATALOG.to_vec();
        if self.memory_tools.is_some() {
            tools.extend_from_slice(&crate::chat::EXTRA[..2]);
        }

        if self.lookup_tools.is_some() {
            tools.extend_from_slice(crate::messenger::LOOKUP);
        }
        if self.share_tools.is_some() {
            tools.push(crate::share::TOOL);
        }
        if let Some(mcp) = &self.mcp_tools {
            tools.extend(mcp.specs());
        }
        let cache_key = crate::prompt_cache_key("task", &self.paths.workspace, &id.0);

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
                    compaction::context_tokens(&system, &session.view(), session.usage)
                        > u64::from(window.saturating_sub(self.config.compaction.reserve_tokens))
                })
            {
                self.compact(&mut session, false).await?;
            }
            retrying = false;

            self.observe(Step::Model);
            let mut view = session.view();
            if !self.config.images {
                for message in &mut view {
                    if let Message::ToolResult {
                        content, images, ..
                    } = message
                    {
                        if !images.is_empty() {
                            images.clear();
                            content.push_str("\n[image omitted: this model does not accept images; use a model with image input]");
                        }
                    }
                }
            }
            let response = match self.llm.complete(CompletionRequest {
                system: &system,
                messages: &view,
                tools: &tools,
                cache_key: Some(&cache_key),
                kind: Some(storage::UsageKind::Task),
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
                        view_boundary: session.view().len(),
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
                    view_boundary: session.view().len(),
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
                let mut images = Vec::new();
                let executed = if let Some(result) = memory_result {
                    result.map(|content| {
                        if call.name == "zoom" {
                            content
                        } else {
                            memory::clip_tool_output(&content)
                        }
                    })
                } else if let Some(lookup) = self.lookup_tools.as_ref().filter(|_| {
                    matches!(
                        call.name.as_str(),
                        "message" | "history" | "search" | "participants"
                    )
                }) {
                    lookup
                        .lookup(&call.name, &call.arguments)
                        .await
                        .map(|content| {
                            if call.name == "participants" || call.name == "message" {
                                memory::clip_tool_output(&content)
                            } else {
                                content
                            }
                        })
                        .map_err(|e| e.to_string())
                } else if let Some(share) = self
                    .share_tools
                    .as_ref()
                    .filter(|_| call.name == "share")
                {
                    share.execute(&call.arguments).map_err(|e| e.to_string())
                } else if let Some(result) = match &self.mcp_tools {
                    Some(mcp) => mcp.execute(&call.name, &call.arguments).await,
                    None => None,
                } {
                    result.map(|outcome| {
                        images = outcome
                            .images
                            .into_iter()
                            .map(|image| storage::Image {
                                mime: image.mime,
                                data: image.data,
                            })
                            .collect();
                        outcome.content
                    })
                } else {
                    match ToolInput::parse(&call.name, &call.arguments) {
                        Ok(input) => self
                            .environment
                            .execute(ToolCall {
                                id: call.id.clone(),
                                input,
                            })
                            .await
                            .map(|result| {
                                images = result
                                    .images
                                    .into_iter()
                                    .map(|image| storage::Image {
                                        mime: image.mime,
                                        data: image.data,
                                    })
                                    .collect();
                                result.content
                            })
                            .map_err(|e| e.to_string()),
                        Err(error) => Err(error.to_string()),
                    }
                };
                self.observe(Step::Idle);
                let mut content = match executed {
                    Ok(result) => result,
                    Err(error) => format!("error: {error}"),
                };
                if !self.config.images && !images.is_empty() {
                    images.clear();
                    content.push_str("\n[image omitted: this model does not accept images; use a model with image input]");
                }
                session.push(Message::ToolResult {
                    id: call.id,
                    name: call.name,
                    content,
                    images,
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
                .min(compaction::context_tokens("", &session.view(), None) / 2)
                .max(1) as u32
        } else {
            keep
        };
        let resolved = self.summarizer.as_ref().and_then(|resolve| resolve());
        let (llm, window) = match &resolved {
            Some((llm, window)) => (llm, *window),
            None => (&self.llm, config.summarizer_window),
        };
        let observer = &self.observer;
        let indices = session.view_indices();
        let view = session.view();
        let Some((mut summary, cut)) =
            compaction::compact(llm, &view, keep, window, config.reserve_tokens, &|| {
                if let Some(observer) = observer {
                    observer(Step::Model);
                }
            })
            .await?
        else {
            return Ok(false);
        };
        if let Message::Summary { first_kept, .. } = &mut summary {
            *first_kept = indices[cut];
        }
        let mut projected = vec![summary.clone()];
        projected.extend_from_slice(&view[cut..]);
        if overflow
            && compaction::context_tokens("", &projected, None)
                >= compaction::context_tokens("", &view, None)
        {
            return Ok(false);
        }
        session.push(summary);
        session.usage = None;
        self.store.save(session.clone())?;
        Ok(true)
    }
}
