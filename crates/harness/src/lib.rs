use environment::{Environment, ToolCall, ToolInput, ToolSpec};
use std::{collections::HashMap, sync::Mutex};

use storage::{Message, Session, SessionId, SessionStore, StoreError};

#[derive(Debug, Clone)]
pub struct CompletionRequest<'a> {
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCallRequest>,
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
}

#[async_trait::async_trait]
pub trait Llm {
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<CompletionResponse, LlmError>;
    async fn summarize(&self, messages: &[Message]) -> Result<String, LlmError>;
}

#[derive(Debug, Clone, Copy)]
pub struct HarnessConfig {
    pub max_history_messages: usize,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            max_history_messages: 100,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Llm(#[from] LlmError),
}

pub struct Harness<L, E, S> {
    llm: L,
    environment: E,
    store: S,
    config: HarnessConfig,
    steering: Mutex<HashMap<SessionId, Vec<String>>>,
}

impl<L, E, S> Harness<L, E, S>
where
    L: Llm,
    E: Environment,
    S: SessionStore,
{
    pub fn new(llm: L, environment: E, store: S) -> Self {
        Self {
            llm,
            environment,
            store,
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

        session.push(Message::User(user_message));
        self.store.save(session.clone())?;

        loop {
            self.apply_steering(&mut session, id);

            if session.messages.len() > self.config.max_history_messages {
                self.compact(&mut session).await?;
            }

            let response = self
                .llm
                .complete(CompletionRequest {
                    messages: &session.messages,
                    tools: environment::CATALOG,
                })
                .await?;

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

    async fn compact(&self, session: &mut Session) -> Result<(), HarnessError> {
        let is_user = |message: &Message| matches!(message, Message::User(_));
        let Some(last_user) = session.messages.iter().rposition(is_user) else {
            return Ok(());
        };
        let budget_split = session
            .messages
            .len()
            .saturating_sub(self.config.max_history_messages);
        let split = session.messages[budget_split..]
            .iter()
            .position(is_user)
            .map_or(last_user, |offset| budget_split + offset);

        if session.messages[..split]
            .iter()
            .all(|message| matches!(message, Message::System(_)))
        {
            return Ok(());
        }

        let summary = self.llm.summarize(&session.messages[..split]).await?;
        let mut compacted = Vec::with_capacity(self.config.max_history_messages + 1);
        compacted.push(Message::System(summary));
        compacted.extend(session.messages[split..].to_vec());
        session.messages = compacted;
        Ok(())
    }
}
