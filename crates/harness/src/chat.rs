use std::sync::{Arc, Mutex};

use environment::{Environment, ToolCall, ToolInput, ToolSpec};
use memory::{Kind, Memory};
use serde::Deserialize;
use storage::{MemoryStore, Message, SessionStore, WorkspaceStore};
use tokio::sync::Notify;

use crate::{
    agents, prompt, skills, CompletionRequest, EnvironmentPaths, HarnessError, Llm, SessionHarness,
};

pub const NAME: &str = "Agent";
pub const CHAT_PROMPT: &str = r#"You are Agent, an AI agent that works for one user in a single chat that never ends. Each call to you is a turn: the view below is followed by the user's new message or an agent's report.

# The view

Agent's memory: the whole chat between Agent and the user, oldest first, inside <chat> tags, as one-line summaries:

  id+n|text   the n messages from id on, summarized (newlines as spaces)

Each message has a kind:
- user: the user's words
- assistant: Agent's replies
- tool: Agent's tool calls
- echo: tool results
- work: an agent's report, starting "[Name]"
- note: memories from before this chat

The summaries form a binary tree: each message is compressed into a line (a short message is its own line), then adjacent lines are merged in pairs, again and again. So recent lines cover one message each, and older lines cover more. A message not summarized yet shows as "(not summarized yet: zoom it)". A text too long for one message is split over several in a row.

Tools:
- zoom(id, n) opens line id+n into the two lines it was made from;
- zoom(id, 1) gives message id whole
- zoom("Name") gives an agent's chat as stored, with older parts summarized if compacted
- date(id) gives the date and time of message id
- read, write, edit and bash work in the working directory

# Turns

Do the user's tasks yourself, with your tools, following the user's instructions at the end of this prompt: who they are, how their files are organized and how they want work done. Use subagents only when the user asks for them.

The view is your memory, and its latest word on a thing is the truth. Whenever you need any information, first find its latest mention in the view and zoom until you have it whole, before any other source, and before you act, guess or ask. Never grep or search memories manually; zoom is your only allowed mechanism to navigate the tree. Summaries keep little of tool output, so say in your reply what you learned that will matter later.

Messages the user sends while you work reach you between tool calls. Subagents run in the background; each one's report reaches you as a message starting "[Name]", between your tool calls or as a new turn. Never wait for one (no sleep, no polling): go on, or end your turn and tell the user what is running."#;

const EXTRA: &[ToolSpec] = &[
    ToolSpec { name: "zoom", description: "Open the line id+n of the view into the two lines of n/2 under it; n = 1 gives the message whole. With an agent's name as id, give that agent's chat as stored, with older parts summarized if compacted. A long text comes in pages from character at, saying where to go on.", input_schema: r#"{"type":"object","properties":{"id":{"oneOf":[{"type":"integer"},{"type":"string"}]},"n":{"type":"integer"},"at":{"type":"integer"}},"required":["id"]}"# },
    ToolSpec { name: "date", description: "The date and time of message id.", input_schema: r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"# },
    ToolSpec { name: "spawn", description: "Start one background agent per task. Each sees the chat view and its task, works with read, write, edit and bash, and reports back as a message starting [Name]. Never wait or poll for it.", input_schema: r#"{"type":"object","properties":{"tasks":{"type":"array","items":{"type":"string"}}},"required":["tasks"]}"# },
    ToolSpec { name: "tell", description: "Send a message to an agent by name: delivered between its tool calls if it is working, or restarts it from its chat.", input_schema: r#"{"type":"object","properties":{"name":{"type":"string"},"text":{"type":"string"}},"required":["name","text"]}"# },
];

#[derive(Clone)]
pub(crate) struct Shared<T>(pub Arc<T>);
#[async_trait::async_trait]
impl<T: Llm + Send + Sync> Llm for Shared<T> {
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<crate::CompletionResponse, crate::LlmError> {
        self.0.complete(request).await
    }
}
#[async_trait::async_trait]
impl<T: Environment> Environment for Shared<T> {
    async fn execute(
        &self,
        call: ToolCall,
    ) -> Result<environment::ToolResult, environment::ToolError> {
        self.0.execute(call).await
    }
}

pub(crate) struct State {
    pub running: std::collections::HashSet<String>,
    pub reports: std::collections::VecDeque<String>,
    pub steering: Vec<String>,
    pub next: u64,
    pub tasks: Vec<tokio::task::AbortHandle>,
}
pub(crate) struct Inner<L, E, S, M> {
    pub llm: Shared<L>,
    pub environment: Shared<E>,
    pub store: S,
    pub memory: Memory<S, M>,
    pub paths: EnvironmentPaths,
    pub session: SessionHarness<Shared<L>, Shared<E>, S>,
    pub state: Mutex<State>,
    pub notify: Notify,
}
pub struct ChatHarness<L, E, S, M> {
    pub(crate) inner: Arc<Inner<L, E, S, M>>,
    turn: tokio::sync::Mutex<()>,
}

impl<L, E, S, M> Drop for ChatHarness<L, E, S, M> {
    fn drop(&mut self) {
        let state = self.inner.state.lock().expect("chat state mutex poisoned");
        for task in &state.tasks {
            task.abort();
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ZoomId {
    Number(u64),
    Name(String),
}
#[derive(Deserialize)]
struct Zoom {
    id: ZoomId,
    #[serde(default = "one")]
    n: u64,
    #[serde(default)]
    at: usize,
}
fn one() -> u64 {
    1
}
#[derive(Deserialize)]
struct Date {
    id: u64,
}
#[derive(Deserialize)]
struct Spawn {
    tasks: Vec<String>,
}
#[derive(Deserialize)]
struct Tell {
    name: String,
    text: String,
}

impl<L, E, S, M> ChatHarness<L, E, S, M>
where
    L: Llm + Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
{
    pub fn new(
        llm: L,
        environment: E,
        store: S,
        memory: Memory<S, M>,
        paths: EnvironmentPaths,
    ) -> Self {
        let llm = Shared(Arc::new(llm));
        let environment = Shared(Arc::new(environment));
        let session = SessionHarness::new(
            Shared(Arc::clone(&llm.0)),
            Shared(Arc::clone(&environment.0)),
            store.clone(),
            paths.clone(),
        );
        Self {
            inner: Arc::new(Inner {
                llm,
                environment,
                store,
                memory,
                paths,
                session,
                state: Mutex::new(State {
                    running: Default::default(),
                    reports: Default::default(),
                    steering: Vec::new(),
                    next: 1,
                    tasks: Vec::new(),
                }),
                notify: Notify::new(),
            }),
            turn: tokio::sync::Mutex::new(()),
        }
    }

    pub fn steer(&self, text: String) {
        self.inner
            .state
            .lock()
            .expect("chat state mutex poisoned")
            .steering
            .push(text);
        self.inner.notify.notify_one();
    }

    pub async fn run(&self, text: &str) -> Result<String, HarnessError> {
        let _guard = self.turn.lock().await;
        self.inner.memory.settle(self.inner.memory.count()?).await?;
        let view = self.inner.memory.view()?;
        self.inner.memory.append(Kind::User, text)?;
        let mut answers = vec![self.turn(text, false, view).await?];
        loop {
            let notified = self.inner.notify.notified();
            let (report, steering, running) = {
                let mut state = self.inner.state.lock().expect("chat state mutex poisoned");
                let steering = std::mem::take(&mut state.steering);
                let report = if steering.is_empty() {
                    state.reports.front().cloned()
                } else {
                    None
                };
                (report, steering, !state.running.is_empty())
            };
            let had_steering = !steering.is_empty();
            for text in steering {
                self.inner.memory.settle(self.inner.memory.count()?).await?;
                let view = self.inner.memory.view()?;
                self.inner.memory.append(Kind::User, &text)?;
                answers.push(self.turn(&text, false, view).await?);
            }
            if let Some(report) = report {
                self.inner.memory.settle(self.inner.memory.count()?).await?;
                let view = self.inner.memory.view()?;
                self.inner.memory.append(Kind::Work, &report)?;
                self.inner
                    .state
                    .lock()
                    .expect("chat state mutex poisoned")
                    .reports
                    .pop_front();
                answers.push(self.turn(&report, true, view).await?);
            } else if had_steering {
                continue;
            } else if running {
                notified.await;
            } else {
                break;
            }
        }
        Ok(answers.join("\n\n"))
    }

    async fn turn(&self, text: &str, report: bool, view: String) -> Result<String, HarnessError> {
        let skills = skills::catalog(&self.inner.store, &self.inner.paths.skills)?;
        let system = format!(
            "{CHAT_PROMPT}\n\n{}",
            prompt::build(&self.inner.paths.workspace, &skills)
        );
        let mut messages = vec![
            Message::User(view),
            Message::User(format!(
                "Turn: {}\n\n{text}",
                if report {
                    "an agent's report:"
                } else {
                    "the user's new message:"
                }
            )),
        ];
        let tools: Vec<_> = environment::CATALOG.iter().chain(EXTRA).copied().collect();
        loop {
            let steering = {
                let mut state = self.inner.state.lock().expect("chat state mutex poisoned");
                std::mem::take(&mut state.steering)
            };
            for text in steering {
                self.inner.memory.append(Kind::User, &text)?;
                messages.push(Message::User(text));
            }
            loop {
                let report = self
                    .inner
                    .state
                    .lock()
                    .expect("chat state mutex poisoned")
                    .reports
                    .front()
                    .cloned();
                let Some(report) = report else { break };
                self.inner.memory.settle(self.inner.memory.count()?).await?;
                self.inner.memory.append(Kind::Work, &report)?;
                self.inner
                    .state
                    .lock()
                    .expect("chat state mutex poisoned")
                    .reports
                    .pop_front();
                messages.push(Message::User(report));
            }
            let response = self
                .inner
                .llm
                .complete(CompletionRequest {
                    system: &system,
                    messages: &messages,
                    tools: &tools,
                })
                .await?;
            if !response.text.is_empty() {
                self.inner.memory.append(Kind::Assistant, &response.text)?;
                messages.push(Message::Assistant(response.text.clone()));
            }
            if response.tool_calls.is_empty() {
                return Ok(response.text);
            }
            for call in &response.tool_calls {
                messages.push(Message::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                });
            }
            for call in response.tool_calls {
                self.inner
                    .memory
                    .append(Kind::Tool, &format!("{} {}", call.name, call.arguments))?;
                let content = match self.execute(&call).await {
                    Ok(content) => content,
                    Err(error) => format!("error: {error}"),
                };
                let content = memory::clip_tool_output(&content);
                self.inner.memory.append(Kind::Echo, &content)?;
                messages.push(Message::ToolResult {
                    id: call.id,
                    name: call.name,
                    content,
                });
            }
        }
    }

    async fn execute(
        &self,
        call: &crate::ToolCallRequest,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok(match call.name.as_str() {
            "zoom" => {
                let args: Zoom = serde_json::from_str(&call.arguments)?;
                match args.id {
                    ZoomId::Number(id) => self.inner.memory.zoom(id, args.n, args.at)?,
                    ZoomId::Name(name) => agents::zoom(&self.inner, &name, args.at)?,
                }
            }
            "date" => {
                let args: Date = serde_json::from_str(&call.arguments)?;
                self.inner.memory.date(args.id)?
            }
            "spawn" => {
                let args: Spawn = serde_json::from_str(&call.arguments)?;
                agents::spawn(&self.inner, args.tasks).await?.join(", ")
            }
            "tell" => {
                let args: Tell = serde_json::from_str(&call.arguments)?;
                agents::tell(&self.inner, &args.name, args.text)?;
                format!("Sent to {}", args.name)
            }
            _ => {
                let input = ToolInput::parse(&call.name, &call.arguments)?;
                self.inner
                    .environment
                    .execute(ToolCall {
                        id: call.id.clone(),
                        input,
                    })
                    .await?
                    .content
            }
        })
    }
}
