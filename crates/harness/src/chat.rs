use std::sync::{Arc, Mutex};

use environment::{Environment, ToolCall, ToolInput, ToolSpec};
use memory::{Kind, Memory};
use serde::Deserialize;
use storage::{MemoryStore, Message, ReasoningLevel, SessionStore};
use tokio::sync::Notify;

use crate::{
    agents, prompt, CompletionRequest, EnvironmentPaths, HarnessError, Llm, SessionHarness,
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

Do the user's tasks yourself, with your tools. Use subagents only when the user asks for them.

The view is your memory, and its latest word on a thing is the truth. Whenever you need any information, first find its latest mention in the view and zoom until you have it whole, before any other source, and before you act, guess or ask. Never grep or search memories manually; zoom is your only allowed mechanism to navigate the tree. Summaries keep little of tool output, so say in your reply what you learned that will matter later.

Messages the user sends while you work reach you between tool calls. Use agents to check on agents when the user asks or before deciding what to do next; never call it in a loop or to wait. Use stop to stop a running agent. Subagents run in the background; each one's report reaches you as a message starting "[Name]", between your tool calls or as a new turn. Never wait for one (no sleep, no polling): go on, or end your turn and tell the user what is running."#;

pub(crate) const EXTRA: &[ToolSpec] = &[
    ToolSpec { name: "zoom", description: "Open the line id+n of the view into the two lines of n/2 under it; n = 1 gives the message whole. With an agent's name as id, give that agent's chat as stored, with older parts summarized if compacted. A long text comes in pages from character at, saying where to go on.", input_schema: r#"{"type":"object","properties":{"id":{"oneOf":[{"type":"integer"},{"type":"string"}]},"n":{"type":"integer"},"at":{"type":"integer"}},"required":["id"]}"# },
    ToolSpec { name: "date", description: "The date and time of message id.", input_schema: r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"# },
    ToolSpec { name: "models", description: "List the configured providers and models you can give to spawn: provider, model id, context window, supported reasoning levels. Use search to filter.", input_schema: r#"{"type":"object","properties":{"search":{"type":"string"}}}"# },
    ToolSpec { name: "spawn", description: "Start one background agent per task. Optionally specify provider, model and reasoning per task; call models to see available choices. Give provider and model together. Never wait or poll for it.", input_schema: r#"{"type":"object","properties":{"tasks":{"type":"array","items":{"type":"object","properties":{"task":{"type":"string"},"provider":{"type":"string"},"model":{"type":"string"},"reasoning":{"type":"string","enum":["off","low","medium","high","xhigh","max"]}},"required":["task"]}}},"required":["tasks"]}"# },
    ToolSpec { name: "agents", description: "List agents with their status, model, how long they have been running, and what each is doing now.", input_schema: r#"{"type":"object","properties":{}}"# },
    ToolSpec { name: "stop", description: "Stop a running agent: cancels its work and kills its running commands. Its report says it was stopped.", input_schema: r#"{"type":"object","properties":{"name":{"type":"string"},"reason":{"type":"string"}},"required":["name"]}"# },
    ToolSpec { name: "tell", description: "Send a message to an agent by name: delivered between its tool calls if it is working, or restarts it from its chat.", input_schema: r#"{"type":"object","properties":{"name":{"type":"string"},"text":{"type":"string"}},"required":["name","text"]}"# },
];

pub trait Models: Send + Sync + 'static {
    type Llm: Llm + Send + Sync + 'static;
    fn list(&self, search: Option<&str>) -> Result<String, HarnessError>;
    fn connect(
        &self,
        choice: Option<(&str, &str)>,
        reasoning: Option<ReasoningLevel>,
    ) -> Result<(Self::Llm, Option<u32>, ModelSelection), HarnessError>;
}

#[derive(Clone)]
pub struct ModelSelection {
    pub provider: String,
    pub model: String,
    pub reasoning: ReasoningLevel,
}

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
    async fn managed_skills(
        &self,
    ) -> Result<std::collections::BTreeMap<String, (String, bool)>, environment::ToolError> {
        self.0.managed_skills().await
    }
    async fn apply_skills(
        &self,
        put: Vec<environment::SkillBundle>,
        remove: Vec<String>,
    ) -> Result<Vec<String>, environment::ToolError> {
        self.0.apply_skills(put, remove).await
    }
    async fn skill_catalog(&self) -> Result<Vec<environment::SkillEntry>, environment::ToolError> {
        self.0.skill_catalog().await
    }
    async fn execute(
        &self,
        call: ToolCall,
    ) -> Result<environment::ToolResult, environment::ToolError> {
        self.0.execute(call).await
    }
}

pub(crate) struct State {
    pub agents: std::collections::HashMap<String, agents::Agent>,
    pub reports: std::collections::VecDeque<agents::Report>,
    pub steering: Vec<String>,
    pub next: u64,
    pub tasks: Vec<tokio::task::AbortHandle>,
}
type AgentSessions<L, E, S> =
    Mutex<std::collections::HashMap<String, Arc<SessionHarness<Shared<L>, Shared<E>, S>>>>;

pub(crate) struct Inner<L, E, S, M, F: Models> {
    pub llm: Shared<L>,
    pub environment: Shared<E>,
    pub store: S,
    pub memory: Memory<S, M>,
    pub paths: EnvironmentPaths,
    pub models: F,
    pub summarizer: Option<Shared<F::Llm>>,
    pub summarizer_window: Option<u32>,
    pub sessions: AgentSessions<F::Llm, E, S>,
    pub state: Mutex<State>,
    pub notify: Notify,
}
pub struct ChatHarness<L, E, S, M, F: Models> {
    pub(crate) inner: Arc<Inner<L, E, S, M, F>>,
    turn: tokio::sync::Mutex<()>,
}

impl<L, E, S, M, F: Models> Drop for ChatHarness<L, E, S, M, F> {
    fn drop(&mut self) {
        let state = self.inner.state.lock().expect("chat state mutex poisoned");
        for task in &state.tasks {
            task.abort();
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum ZoomId {
    Number(u64),
    Name(String),
}
#[derive(Deserialize)]
pub(crate) struct Zoom {
    pub(crate) id: ZoomId,
    #[serde(default = "one")]
    pub(crate) n: u64,
    #[serde(default)]
    pub(crate) at: usize,
}
fn one() -> u64 {
    1
}
#[derive(Deserialize)]
pub(crate) struct Date {
    pub(crate) id: u64,
}
#[derive(Deserialize)]
struct Spawn {
    tasks: Vec<SpawnTask>,
}
#[derive(Deserialize)]
pub(crate) struct SpawnTask {
    pub task: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
}
#[derive(Deserialize)]
struct ModelsArgs {
    search: Option<String>,
}
#[derive(Deserialize)]
struct Stop {
    name: String,
    reason: Option<String>,
}
#[derive(Deserialize)]
struct Tell {
    name: String,
    text: String,
}

impl<L, E, S, M, F: Models> ChatHarness<L, E, S, M, F>
where
    L: Llm + Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
{
    pub fn new(
        llm: L,
        environment: E,
        store: S,
        memory: Memory<S, M>,
        paths: EnvironmentPaths,
        models: F,
        summarizer: Option<(F::Llm, Option<u32>)>,
    ) -> Self {
        let llm = Shared(Arc::new(llm));
        let environment = Shared(Arc::new(environment));
        let (summarizer, summarizer_window) = match summarizer {
            Some((llm, window)) => (Some(Shared(Arc::new(llm))), window),
            None => (None, None),
        };
        Self {
            inner: Arc::new(Inner {
                llm,
                environment,
                store,
                memory,
                paths,
                models,
                summarizer,
                summarizer_window,
                sessions: Mutex::new(Default::default()),
                state: Mutex::new(State {
                    agents: Default::default(),
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
                (
                    report,
                    steering,
                    state.agents.values().any(agents::Agent::is_running),
                )
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
                self.inner.memory.append(Kind::Work, &report.text)?;
                agents::delivered(&self.inner);
                answers.push(self.turn(&report.text, true, view).await?);
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
        let skills = self.inner.environment.skill_catalog().await?;
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
                self.inner.memory.append(Kind::Work, &report.text)?;
                agents::delivered(&self.inner);
                messages.push(Message::User(report.text));
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
                crate::MemoryTools::zoom(self.inner.as_ref(), args.id, args.n, args.at)?
            }
            "date" => {
                let args: Date = serde_json::from_str(&call.arguments)?;
                crate::MemoryTools::date(self.inner.as_ref(), args.id)?
            }
            "models" => {
                let args: ModelsArgs = serde_json::from_str(&call.arguments)?;
                self.inner.models.list(args.search.as_deref())?
            }
            "spawn" => {
                let args: Spawn = serde_json::from_str(&call.arguments)?;
                agents::spawn(&self.inner, args.tasks).await?.join(", ")
            }
            "agents" => agents::list(&self.inner)?,
            "stop" => {
                let args: Stop = serde_json::from_str(&call.arguments)?;
                agents::stop(&self.inner, &args.name, args.reason).await?
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

impl<L, E, S, M, F> crate::MemoryTools for Inner<L, E, S, M, F>
where
    L: Send + Sync + 'static,
    E: Send + Sync + 'static,
    S: SessionStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
    F: Models,
{
    fn zoom(&self, id: ZoomId, n: u64, at: usize) -> Result<String, HarnessError> {
        match id {
            ZoomId::Number(id) => Ok(self.memory.zoom(id, n, at)?),
            ZoomId::Name(name) => agents::zoom(self, &name, at),
        }
    }
    fn date(&self, id: u64) -> Result<String, HarnessError> {
        Ok(self.memory.date(id)?)
    }
}
