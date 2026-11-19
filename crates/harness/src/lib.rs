use executor::{ToolCall, ToolExecutor, ToolSpec};
use std::{collections::HashMap, sync::Mutex};

use storage::{Message, Session, SessionId, SessionStore, StoreError};

#[derive(Debug, Clone)]
pub struct CompletionRequest<'a> {
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompletionResponse {
    Final(String),
    ToolCalls(Vec<ToolCall>),
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
    executor: E,
    store: S,
    config: HarnessConfig,
    steering: Mutex<HashMap<SessionId, Vec<String>>>,
}

impl<L, E, S> Harness<L, E, S>
where
    L: Llm,
    E: ToolExecutor,
    S: SessionStore,
{
    pub fn new(llm: L, executor: E, store: S) -> Self {
        Self {
            llm,
            executor,
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
                    tools: executor::CATALOG,
                })
                .await?;

            match response {
                CompletionResponse::Final(answer) => {
                    session.push(Message::Assistant(answer.clone()));
                    self.store.save(session)?;
                    return Ok(answer);
                }
                CompletionResponse::ToolCalls(calls) => {
                    for call in calls {
                        let name = call.input.name().to_owned();
                        let call_id = call.id.clone();
                        let content = match self.executor.execute(call).await {
                            Ok(result) => result.content,
                            Err(error) => format!("error: {error}"),
                        };
                        session.push(Message::Tool {
                            id: call_id,
                            name,
                            content,
                        });
                    }
                    self.store.save(session.clone())?;
                }
            }
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
        let split = session
            .messages
            .len()
            .saturating_sub(self.config.max_history_messages);

        if split == 0 {
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
