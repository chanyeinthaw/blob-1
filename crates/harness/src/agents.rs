use std::sync::Arc;
use std::time::Instant;

use environment::Environment;
use storage::{MemoryStore, Message, Session, SessionId, SessionStore, WorkspaceStore};

use crate::{
    chat::{Inner, Models, Shared, SpawnTask},
    CompactionConfig, HarnessConfig, HarnessError, Llm, SessionHarness, Step,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Report {
    pub(crate) text: String,
    name: String,
    run_id: u64,
}

pub(crate) struct Agent {
    provider: String,
    model: String,
    reasoning: String,
    started: Instant,
    activity: Instant,
    step_started: Instant,
    step: Step,
    status: Status,
    work: Option<tokio::task::AbortHandle>,
    supervisor: Option<tokio::task::JoinHandle<()>>,
    run_id: u64,
}

impl Agent {
    pub(crate) fn is_running(&self) -> bool {
        matches!(self.status, Status::Running)
    }
}

enum Status {
    Running,
    Idle { finished: Instant, delivered: bool },
    Crashed(String),
    Stopped(String),
}

fn duration(time: Instant) -> String {
    let seconds = time.elapsed().as_secs();
    if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

pub(crate) fn list<L, E, S, M, F: Models>(
    inner: &Inner<L, E, S, M, F>,
) -> Result<String, HarnessError>
where
    S: SessionStore,
{
    let stored = inner.store.session_ids("agent:")?;
    let state = inner.state.lock().expect("chat state mutex poisoned");
    let mut names: Vec<_> = state.agents.keys().collect();
    names.sort();
    let mut lines: Vec<_> = names.into_iter().map(|name| {
        let agent = &state.agents[name];
        let selection = format!("provider={} model={} reasoning={}", agent.provider, agent.model, agent.reasoning);
        let row = match &agent.status {
            Status::Running => {
                let step = match &agent.step { Step::Model => "model", Step::Tool { name } => name, Step::Idle => "idle" };
                format!("{name}  running  {}  provider={} model={} reasoning={}\n        now: {step} ({})    last activity {} ago", duration(agent.started), agent.provider, agent.model, agent.reasoning, duration(agent.step_started), duration(agent.activity))
            }
            Status::Idle { finished, delivered } => format!("{name}  idle  finished {} ago    report {}", duration(*finished), if *delivered { "delivered" } else { "pending" }),
            Status::Crashed(message) => format!("{name}  crashed  error: agent crashed: {message}"),
            Status::Stopped(reason) => format!("{name}  stopped  {reason}"),
        };
        if agent.is_running() { row } else { format!("{row}  {selection}") }
    }).collect();
    for id in stored {
        let name = id.0.strip_prefix("agent:").expect("matching prefix");
        if !state.agents.contains_key(name) {
            lines.push(format!("{name}  idle  (from an earlier run)"));
        }
    }
    Ok(if lines.is_empty() {
        "No agents.".into()
    } else {
        lines.join("\n")
    })
}

pub(crate) fn delivered<L, E, S, M, F: Models>(inner: &Inner<L, E, S, M, F>) {
    let mut state = inner.state.lock().expect("chat state mutex poisoned");
    if let Some(report) = state.reports.pop_front() {
        if let Some(agent) = state.agents.get_mut(&report.name) {
            if agent.run_id == report.run_id {
                if let Status::Idle { delivered, .. } = &mut agent.status {
                    *delivered = true;
                }
            }
        }
    }
}

pub(crate) async fn stop<L, E, S, M, F: Models>(
    inner: &Inner<L, E, S, M, F>,
    name: &str,
    reason: Option<String>,
) -> Result<String, HarnessError>
where
    S: SessionStore + WorkspaceStore,
    E: Environment,
{
    let supervisor = {
        let mut state = inner.state.lock().expect("chat state mutex poisoned");
        if !state.agents.get(name).is_some_and(Agent::is_running) {
            if state.agents.contains_key(name)
                || inner
                    .store
                    .load(&SessionId::new(format!("agent:{name}")))?
                    .is_some()
            {
                return Ok(format!("{name} is not running."));
            }
            return Ok(format!("unknown agent: {name}"));
        }
        let reason = reason.unwrap_or_else(|| "stopped by the main agent".into());
        let agent = state.agents.get_mut(name).expect("running agent tracked");
        agent.status = Status::Stopped(reason.clone());
        if let Some(work) = agent.work.take() {
            work.abort();
        }
        let supervisor = agent.supervisor.take();
        let run_id = agent.run_id;
        if let Some(session) = inner
            .sessions
            .lock()
            .expect("sessions mutex poisoned")
            .get(name)
        {
            session.take_steering(&SessionId::new(format!("agent:{name}")));
        }
        state.reports.push_back(Report {
            text: format!("[{name}]\nstopped: {reason}\n\nChat: zoom(\"{name}\")"),
            name: name.to_owned(),
            run_id,
        });
        supervisor
    };
    if let Some(supervisor) = supervisor {
        let _ = supervisor.await;
    }
    inner.notify.notify_one();
    Ok(format!("Stopped {name}."))
}

pub(crate) async fn spawn<L, E, S, M, F: Models>(
    inner: &Arc<Inner<L, E, S, M, F>>,
    tasks: Vec<SpawnTask>,
) -> Result<Vec<String>, Error>
where
    L: Llm + Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
{
    let mut prepared = Vec::new();
    for task in tasks {
        let choice = match (&task.provider, &task.model) {
            (Some(provider), Some(model)) => Some((provider.as_str(), model.as_str())),
            (None, None) => None,
            _ => return Err("provider and model must be given together".into()),
        };
        let reasoning = task.reasoning.as_deref().map(str::parse).transpose()?;
        let (llm, window, selection) = inner.models.connect(choice, reasoning)?;
        prepared.push((
            task.task,
            llm,
            window,
            selection.provider,
            selection.model,
            selection.reasoning.as_str().to_owned(),
        ));
    }
    inner.memory.settle(inner.memory.count()?).await?;
    let view = inner.memory.view()?;
    let mut names = Vec::new();
    for (task, llm, window, provider, model, reasoning) in prepared {
        let name = loop {
            let number = {
                let mut state = inner.state.lock().expect("chat state mutex poisoned");
                let n = state.next;
                state.next += 1;
                n
            };
            let name = format!("Agent{number}");
            if inner
                .store
                .load(&SessionId::new(format!("agent:{name}")))?
                .is_none()
                && !inner
                    .state
                    .lock()
                    .expect("chat state mutex poisoned")
                    .agents
                    .contains_key(&name)
            {
                break name;
            }
        };
        let text = format!("{view}\n\nThe <chat> above is the main chat's memory, as context. Your task:\n\n{task}");
        let mut initial = Session::new(SessionId::new(format!("agent:{name}")));
        initial.push(Message::User(text));
        inner.store.save(initial)?;
        let now = Instant::now();
        inner
            .state
            .lock()
            .expect("chat state mutex poisoned")
            .agents
            .insert(
                name.clone(),
                Agent {
                    provider,
                    model,
                    reasoning,
                    started: now,
                    activity: now,
                    step_started: now,
                    step: Step::Idle,
                    status: Status::Running,
                    work: None,
                    supervisor: None,
                    run_id: 1,
                },
            );
        let session = build_session(inner, llm, window, &name);
        inner
            .sessions
            .lock()
            .expect("sessions mutex poisoned")
            .insert(name.clone(), session);
        launch(inner, name.clone(), None);
        names.push(name);
    }
    Ok(names)
}

pub(crate) fn tell<L, E, S, M, F: Models>(
    inner: &Arc<Inner<L, E, S, M, F>>,
    name: &str,
    text: String,
) -> Result<(), Error>
where
    L: Llm + Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
{
    let id = SessionId::new(format!("agent:{name}"));
    let mut state = inner.state.lock().expect("chat state mutex poisoned");
    let session = inner
        .sessions
        .lock()
        .expect("sessions mutex poisoned")
        .get(name)
        .cloned();
    if state.agents.get(name).is_some_and(Agent::is_running) {
        session
            .expect("running agent has session")
            .steer(&id, text)?;
    } else if inner.store.load(&id)?.is_some() {
        let mut selection = None;
        if session.is_none() {
            let (llm, window, resolved) = inner.models.connect(None, None)?;
            selection = Some(resolved);
            inner
                .sessions
                .lock()
                .expect("sessions mutex poisoned")
                .insert(name.to_owned(), build_session(inner, llm, window, name));
        }
        let now = Instant::now();
        let agent = state
            .agents
            .entry(name.to_owned())
            .or_insert_with(|| Agent {
                provider: selection.as_ref().expect("selection").provider.clone(),
                model: selection.as_ref().expect("selection").model.clone(),
                reasoning: selection
                    .as_ref()
                    .expect("selection")
                    .reasoning
                    .as_str()
                    .to_owned(),
                started: now,
                activity: now,
                step_started: now,
                step: Step::Idle,
                status: Status::Running,
                work: None,
                supervisor: None,
                run_id: 0,
            });
        agent.started = now;
        agent.activity = now;
        agent.step_started = now;
        agent.step = Step::Idle;
        agent.status = Status::Running;
        agent.run_id += 1;
        drop(state);
        launch(inner, name.to_owned(), Some(text));
    } else {
        return Err(format!("unknown agent: {name}").into());
    }
    Ok(())
}

fn build_session<L, E, S, M, F: Models>(
    inner: &Arc<Inner<L, E, S, M, F>>,
    llm: F::Llm,
    window: Option<u32>,
    name: &str,
) -> Arc<SessionHarness<Shared<F::Llm>, Shared<E>, S>>
where
    L: Send + Sync + 'static,
    M: Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
{
    let weak = Arc::downgrade(inner);
    let name = name.to_owned();
    let mut session = SessionHarness::new(
        Shared(Arc::new(llm)),
        Shared(Arc::clone(&inner.environment.0)),
        inner.store.clone(),
        inner.paths.clone(),
    )
    .with_observer(Arc::new(move |step| {
        if let Some(inner) = weak.upgrade() {
            let mut state = inner.state.lock().expect("chat state mutex poisoned");
            if let Some(agent) = state.agents.get_mut(&name) {
                if matches!(agent.status, Status::Running) {
                    let now = Instant::now();
                    agent.step = step;
                    agent.activity = now;
                    agent.step_started = now;
                }
            }
        }
    }))
    .with_config(HarnessConfig {
        compaction: CompactionConfig {
            context_window: window,
            summarizer_window: if inner.summarizer.is_some() {
                inner.summarizer_window
            } else {
                window
            },
            ..CompactionConfig::default()
        },
    });
    if let Some(summarizer) = &inner.summarizer {
        session = session.with_summarizer(Shared(Arc::clone(&summarizer.0)));
    }
    Arc::new(session)
}

fn launch<L, E, S, M, F: Models>(
    inner: &Arc<Inner<L, E, S, M, F>>,
    name: String,
    text: Option<String>,
) where
    L: Llm + Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
{
    let inner = Arc::clone(inner);
    let mut state = inner.state.lock().expect("chat state mutex poisoned");
    let run_id = state.agents.get(&name).expect("agent tracked").run_id;
    let task_inner = Arc::clone(&inner);
    let session = inner
        .sessions
        .lock()
        .expect("sessions mutex poisoned")
        .get(&name)
        .cloned()
        .expect("agent has session");
    let crash_inner = Arc::clone(&inner);
    let crash_name = name.clone();
    let crash_name_for_handle = name.clone();
    let crash_session = Arc::clone(&session);
    let work = tokio::spawn(async move {
        let inner = task_inner;
        let id = SessionId::new(format!("agent:{name}"));
        let mut text = text;
        loop {
            let result = session
                .run_pending(&id, text)
                .await
                .unwrap_or_else(|error| format!("error: {error}"));
            let mut state = inner.state.lock().expect("chat state mutex poisoned");
            if !state
                .agents
                .get(&name)
                .is_some_and(|agent| agent.run_id == run_id && agent.is_running())
            {
                return;
            }
            let mut pending = session.take_steering(&id).into_iter();
            if let Some(next) = pending.next() {
                for message in pending {
                    session.steer(&id, message).expect("steering failed");
                }
                text = Some(next);
                continue;
            }
            let report = format!("[{name}]\n{result}\n\nChat: zoom(\"{name}\")");
            state.agents.get_mut(&name).expect("agent tracked").status = Status::Idle {
                finished: Instant::now(),
                delivered: false,
            };
            state.reports.push_back(Report {
                text: report,
                name: name.clone(),
                run_id,
            });
            break;
        }
        inner.notify.notify_one();
    });
    let work_abort = work.abort_handle();
    let task = tokio::spawn(async move {
        if let Err(error) = work.await {
            if error.is_cancelled() {
                return;
            }
            let panic = error.into_panic();
            let message = panic
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            let mut state = crash_inner.state.lock().expect("chat state mutex poisoned");
            if !state
                .agents
                .get(&crash_name)
                .is_some_and(|agent| agent.run_id == run_id && agent.is_running())
            {
                return;
            }
            let dropped = crash_session
                .take_steering(&SessionId::new(format!("agent:{crash_name}")))
                .len();
            let suffix = if dropped > 0 {
                format!(" (dropped {dropped} pending messages)")
            } else {
                String::new()
            };
            state
                .agents
                .get_mut(&crash_name)
                .expect("agent tracked")
                .status = Status::Crashed(message.to_owned());
            state.reports.push_back(Report {
                text: format!("[{crash_name}]\nerror: agent crashed: {message}{suffix}\n\nChat: zoom(\"{crash_name}\")"),
                name: crash_name.clone(),
                run_id,
            });
            drop(state);
            crash_inner.notify.notify_one();
        }
    });
    state.tasks.retain(|task| !task.is_finished());
    if let Some(agent) = state.agents.get_mut(&crash_name_for_handle) {
        agent.work = Some(work_abort.clone());
        agent.supervisor = Some(task);
    }
    state.tasks.push(work_abort);
}

pub(crate) fn zoom<L, E, S, M, F: Models>(
    inner: &Inner<L, E, S, M, F>,
    name: &str,
    at: usize,
) -> Result<String, HarnessError>
where
    S: SessionStore,
{
    let session = inner
        .store
        .load(&SessionId::new(format!("agent:{name}")))?
        .ok_or_else(|| HarnessError::Compaction(format!("unknown agent: {name}")))?;
    let text = session
        .messages
        .iter()
        .map(|message| match message {
            Message::User(text) => format!("user: {text}"),
            Message::Assistant(text) => format!("assistant: {text}"),
            Message::ToolCall {
                name, arguments, ..
            } => format!("tool: {name} {arguments}"),
            Message::ToolResult { content, .. } => format!("echo: {content}"),
            Message::Summary { text, .. } => format!("summary: {text}"),
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut chars = text.chars().skip(at);
    let mut page: String = chars.by_ref().take(30_000).collect();
    if chars.next().is_some() {
        page.push_str(&format!("\n[more from at={}]", at + 30_000));
    }
    Ok(page)
}
