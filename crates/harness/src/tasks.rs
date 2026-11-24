use std::sync::Arc;
use std::time::Instant;

use environment::Environment;
use memory::{Meta, ReportStatus};
use messenger::Mark;
use storage::{MemoryStore, Message, SessionId, SessionStore, TaskChoice};

use crate::{
    chat::{Inner, Models, Shared, SpawnTask},
    CompactionConfig, HarnessConfig, HarnessError, SessionHarness, Step,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

#[derive(Clone)]
pub(crate) struct Report {
    pub(crate) meta: Meta,
    pub(crate) text: String,
    pub(crate) name: String,
    pub(crate) run_id: u64,
}

pub(crate) struct Task {
    provider: String,
    model: String,
    reasoning: String,
    started: Instant,
    activity: Instant,
    step_started: Instant,
    step: Step,
    status: Status,
    for_message: Option<String>,
    watch: Option<tokio::task::AbortHandle>,
    work: Option<tokio::task::AbortHandle>,
    supervisor: Option<tokio::task::JoinHandle<()>>,
    run_id: u64,
}

impl Task {
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

pub(crate) fn list<E, S, M, F: Models>(inner: &Inner<E, S, M, F>) -> Result<String, HarnessError>
where
    S: SessionStore,
{
    let stored = inner.store.session_ids("task:")?;
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
            Status::Crashed(message) => format!("{name}  crashed  error: task crashed: {message}"),
            Status::Stopped(reason) => format!("{name}  stopped  {reason}"),
        };
        if agent.is_running() { row } else { format!("{row}  {selection}") }
    }).collect();
    for id in stored {
        let name = id.0.strip_prefix("task:").expect("matching prefix");
        if !state.agents.contains_key(name) {
            let choice = inner.store.task_choice(name)?;
            let suffix = choice
                .map(|c| {
                    format!(
                        "  provider={} model={} reasoning={}",
                        c.provider, c.model, c.reasoning
                    )
                })
                .unwrap_or_default();
            lines.push(format!("{name}  idle  (from an earlier run){suffix}"));
        }
    }
    Ok(if lines.is_empty() {
        "No tasks.".into()
    } else {
        lines.join("\n")
    })
}

pub(crate) fn delivered<E, S, M, F: Models>(inner: &Inner<E, S, M, F>) {
    let mut state = inner.state.lock().expect("chat state mutex poisoned");
    if let Some(report) = state.reports.pop_front() {
        if matches!(report.meta, Meta::Report { .. }) {
            if let Some(agent) = state.agents.get_mut(&report.name) {
                if agent.run_id == report.run_id {
                    if let Status::Idle { delivered, .. } = &mut agent.status {
                        *delivered = true;
                    }
                }
            }
        }
    }
}

pub(crate) async fn stop<E, S, M, F: Models>(
    inner: &Inner<E, S, M, F>,
    name: &str,
    reason: Option<String>,
) -> Result<String, HarnessError>
where
    S: SessionStore,
    E: Environment,
{
    let supervisor = {
        let mut state = inner.state.lock().expect("chat state mutex poisoned");
        if !state.agents.get(name).is_some_and(Task::is_running) {
            if state.agents.contains_key(name)
                || inner
                    .store
                    .load(&SessionId::new(format!("task:{name}")))?
                    .is_some()
            {
                return Ok(format!("{name} is not running."));
            }
            return Ok(format!("unknown task: {name}"));
        }
        let reason = reason.unwrap_or_else(|| "stopped by the main agent".into());
        let agent = state.agents.get_mut(name).expect("running agent tracked");
        agent.status = Status::Stopped(reason.clone());
        if let Some(watch) = agent.watch.take() {
            watch.abort();
        }
        let for_message = agent.for_message.clone();
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
            session.take_steering(&SessionId::new(format!("task:{name}")));
        }
        if let (Some(messengers), Some(message)) = (&inner.messengers, &for_message) {
            if !state
                .agents
                .values()
                .any(|agent| agent.is_running() && agent.for_message.as_ref() == Some(message))
            {
                messengers.mark(message, &[Mark::Failed], &[Mark::Working]);
            }
        }
        state.reports.push_back(Report {
            meta: Meta::Report {
                task: name.to_owned(),
                status: ReportStatus::Stopped,
                for_message,
            },
            text: format!("stopped: {reason}"),
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

pub(crate) async fn spawn<E, S, M, F: Models>(
    inner: &Arc<Inner<E, S, M, F>>,
    tasks: Vec<SpawnTask>,
) -> Result<Vec<String>, Error>
where
    E: Environment + 'static,
    S: SessionStore + MemoryStore + Clone + 'static,
    M: llm::Source + 'static,
{
    let mut prepared = Vec::new();
    for task in tasks {
        let for_message = task
            .for_message
            .as_deref()
            .map(|value| {
                value
                    .strip_prefix('m')
                    .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|digits| digits.parse::<u64>().ok())
                    .map(|row| format!("m{row}"))
                    .ok_or("invalid for token")
            })
            .transpose()?;
        let choice = match (&task.provider, &task.model) {
            (Some(provider), Some(model)) => Some((provider.as_str(), model.as_str())),
            (None, None) => None,
            _ => return Err("provider and model must be given together".into()),
        };
        let reasoning = task.reasoning.as_deref().map(str::parse).transpose()?;
        let (llm, window, selection) = inner.models.connect(choice, reasoning)?;
        prepared.push((
            task.task,
            for_message,
            llm,
            window,
            selection.provider,
            selection.model,
            selection.reasoning.as_str().to_owned(),
            selection.images,
        ));
    }
    inner.memory.settle(inner.memory.count()?).await?;
    let view = inner.memory.view()?;
    let mut names = Vec::new();
    for (task, for_message, llm, window, provider, model, reasoning, images) in prepared {
        let text = format!("{view}\n\nYour task:\n\n{task}");
        let choice = TaskChoice {
            provider: provider.clone(),
            model: model.clone(),
            reasoning: reasoning.clone(),
        };
        let name = loop {
            let name = nanoid::nanoid!(
                10,
                &[
                    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
                    'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v',
                    'w', 'x', 'y', 'z'
                ]
            );
            if inner.store.create_task_session(&name, &text, &choice)? {
                break name;
            }
        };
        let now = Instant::now();
        inner
            .state
            .lock()
            .expect("chat state mutex poisoned")
            .agents
            .insert(
                name.clone(),
                Task {
                    provider,
                    model,
                    reasoning,
                    started: now,
                    activity: now,
                    step_started: now,
                    step: Step::Idle,
                    status: Status::Running,
                    for_message,
                    watch: None,
                    work: None,
                    supervisor: None,
                    run_id: 1,
                },
            );
        let session = build_session(inner, llm, window, &name, images);
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

pub(crate) fn tell<E, S, M, F: Models>(
    inner: &Arc<Inner<E, S, M, F>>,
    name: &str,
    text: String,
) -> Result<String, Error>
where
    E: Environment + 'static,
    S: SessionStore + MemoryStore + Clone + 'static,
    M: llm::Source + 'static,
{
    let id = SessionId::new(format!("task:{name}"));
    let mut fallback = false;
    let mut state = inner.state.lock().expect("chat state mutex poisoned");
    let session = inner
        .sessions
        .lock()
        .expect("sessions mutex poisoned")
        .get(name)
        .cloned();
    if state.agents.get(name).is_some_and(Task::is_running) {
        session
            .expect("running agent has session")
            .steer(&id, text)?;
    } else if inner.store.load(&id)?.is_some() {
        let mut selection = None;
        if session.is_none() {
            let choice = inner.store.task_choice(name)?;
            let result = match &choice {
                Some(choice) => inner.models.connect(
                    Some((&choice.provider, &choice.model)),
                    Some(choice.reasoning.parse()?),
                ),
                None => inner.models.connect(None, None),
            };
            let (llm, window, resolved) = match result {
                Err(HarnessError::UnknownModel(_)) if choice.is_some() => {
                    fallback = true;
                    inner.models.connect(None, None)?
                }
                other => other?,
            };
            let images = resolved.images;
            selection = Some(resolved);
            inner
                .sessions
                .lock()
                .expect("sessions mutex poisoned")
                .insert(
                    name.to_owned(),
                    build_session(inner, llm, window, name, images),
                );
        }
        let now = Instant::now();
        let agent = state.agents.entry(name.to_owned()).or_insert_with(|| Task {
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
            for_message: None,
            watch: None,
            work: None,
            supervisor: None,
            run_id: 0,
        });
        if let Some(watch) = agent.watch.take() {
            watch.abort();
        }
        agent.started = now;
        agent.activity = now;
        agent.step_started = now;
        agent.step = Step::Idle;
        agent.status = Status::Running;
        agent.run_id += 1;
        drop(state);
        launch(inner, name.to_owned(), Some(text));
    } else {
        return Err(format!("unknown task: {name}").into());
    }
    Ok(if fallback {
        format!("Sent to {name}; stored model was unavailable, using the default model.")
    } else {
        format!("Sent to {name}")
    })
}

fn build_session<E, S, M, F: Models>(
    inner: &Arc<Inner<E, S, M, F>>,
    llm: F::Llm,
    window: Option<u32>,
    name: &str,
    images: bool,
) -> Arc<SessionHarness<Shared<F::Llm>, Shared<E>, S>>
where
    M: llm::Source + 'static,
    E: Environment + 'static,
    S: SessionStore + MemoryStore + Clone + 'static,
{
    let weak = Arc::downgrade(inner);
    let summarizer_weak = Arc::downgrade(inner);
    let name = name.to_owned();
    let mut session = SessionHarness::new(
        Shared(Arc::new(llm)),
        Shared(Arc::clone(&inner.environment.0)),
        inner.store.clone(),
        inner.paths.clone(),
    )
    .with_memory_tools(Arc::new(TaskMemory(Arc::downgrade(inner))))
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
    .with_summarizer(Arc::new(move || {
        summarizer_weak
            .upgrade()?
            .models
            .current()
            .ok()?
            .summarizer
            .map(|(llm, window)| (Shared(Arc::new(llm)), window))
    }))
    .with_config(HarnessConfig {
        images,
        compaction: CompactionConfig {
            context_window: window,
            summarizer_window: window,
            ..CompactionConfig::default()
        },
    });
    if let Some(messengers) = &inner.messengers {
        session = session.with_lookup_tools(messengers.clone());
    }
    if let Some(share) = &inner.share {
        session = session.with_share_tools(share.clone());
    }
    if let Some(callback) = &inner.callback {
        session = session.with_callback_tools(callback.clone());
    }
    if let Some(mcp) = &inner.mcp {
        let tools = match &inner.mcp_filter {
            Some(filter) => mcp::SessionTools::with_filter(mcp.clone(), Arc::clone(filter)),
            None => mcp::SessionTools::new(mcp.clone()),
        };
        session = session.with_mcp_tools(Arc::new(tools));
    }
    Arc::new(session)
}

fn launch<E, S, M, F: Models>(inner: &Arc<Inner<E, S, M, F>>, name: String, text: Option<String>)
where
    E: Environment + 'static,
    S: SessionStore + MemoryStore + Clone + 'static,
    M: llm::Source + 'static,
{
    let inner = Arc::clone(inner);
    let mut state = inner.state.lock().expect("chat state mutex poisoned");
    let agent = state.agents.get(&name).expect("agent tracked");
    let run_id = agent.run_id;
    if let (Some(messengers), Some(message)) = (&inner.messengers, &agent.for_message) {
        messengers.mark(message, &[Mark::Working], &[Mark::Done, Mark::Failed]);
    }
    let watch_inner = Arc::downgrade(&inner);
    let watch_name = name.clone();
    let watch = tokio::spawn(async move {
        loop {
            tokio::time::sleep(WATCH_INTERVAL).await;
            let Some(inner) = watch_inner.upgrade() else {
                break;
            };
            let mut state = inner.state.lock().expect("chat state mutex poisoned");
            let Some(agent) = state
                .agents
                .get(&watch_name)
                .filter(|agent| agent.run_id == run_id && agent.is_running())
            else {
                break;
            };
            let step = match &agent.step {
                Step::Model => "model",
                Step::Tool { name } => name,
                Step::Idle => "idle",
            };
            let report = Report {
                meta: Meta::Reminder {
                    task: watch_name.clone(),
                    running: duration(agent.started),
                    last_activity: duration(agent.activity),
                    for_message: agent.for_message.clone(),
                },
                text: format!("now: {step} ({})", duration(agent.step_started)),
                name: watch_name.clone(),
                run_id,
            };
            state.reports.push_back(report);
            drop(state);
            inner.notify.notify_one();
        }
    });
    state.agents.get_mut(&name).expect("agent tracked").watch = Some(watch.abort_handle());
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
    let work =
        tokio::spawn(async move {
            let inner = task_inner;
            let id = SessionId::new(format!("task:{name}"));
            let mut text = text;
            loop {
                let (result, status) = match session.run_pending(&id, text).await {
                    Ok(result) => (result, ReportStatus::Done),
                    Err(error) => (format!("error: {error}"), ReportStatus::Failed),
                };
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
                let agent = state.agents.get_mut(&name).expect("agent tracked");
                agent.status = Status::Idle {
                    finished: Instant::now(),
                    delivered: false,
                };
                if let Some(watch) = agent.watch.take() {
                    watch.abort();
                }
                let for_message = agent.for_message.clone();
                if let (Some(messengers), Some(message)) = (&inner.messengers, &for_message) {
                    if !state.agents.values().any(|agent| {
                        agent.is_running() && agent.for_message.as_ref() == Some(message)
                    }) {
                        let mark = if matches!(status, ReportStatus::Done) {
                            Mark::Done
                        } else {
                            Mark::Failed
                        };
                        messengers.mark(message, &[mark], &[Mark::Working]);
                    }
                }
                state.reports.push_back(Report {
                    meta: Meta::Report {
                        task: name.clone(),
                        status,
                        for_message,
                    },
                    text: result,
                    name: name.clone(),
                    run_id,
                });
                break;
            }
            inner.notify.notify_one();
        });
    let work_abort = work.abort_handle();
    let task =
        tokio::spawn(async move {
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
                    .take_steering(&SessionId::new(format!("task:{crash_name}")))
                    .len();
                let suffix = if dropped > 0 {
                    format!(" (dropped {dropped} pending messages)")
                } else {
                    String::new()
                };
                let agent = state.agents.get_mut(&crash_name).expect("agent tracked");
                agent.status = Status::Crashed(message.to_owned());
                if let Some(watch) = agent.watch.take() {
                    watch.abort();
                }
                let for_message = agent.for_message.clone();
                if let (Some(messengers), Some(message)) = (&crash_inner.messengers, &for_message) {
                    if !state.agents.values().any(|agent| {
                        agent.is_running() && agent.for_message.as_ref() == Some(message)
                    }) {
                        messengers.mark(message, &[Mark::Failed], &[Mark::Working]);
                    }
                }
                state.reports.push_back(Report {
                    meta: Meta::Report {
                        task: crash_name.clone(),
                        status: ReportStatus::Crashed,
                        for_message,
                    },
                    text: format!("error: task crashed: {message}{suffix}"),
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

pub(crate) fn zoom<E, S, M, F: Models>(
    inner: &Inner<E, S, M, F>,
    name: &str,
    at: usize,
) -> Result<String, HarnessError>
where
    S: SessionStore,
{
    let session = inner
        .store
        .load(&SessionId::new(format!("task:{name}")))?
        .ok_or_else(|| HarnessError::Compaction(format!("unknown task: {name}")))?;
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

struct TaskMemory<E, S, M, F: Models>(std::sync::Weak<Inner<E, S, M, F>>);

impl<E, S, M, F> crate::MemoryTools for TaskMemory<E, S, M, F>
where
    E: Send + Sync + 'static,
    S: SessionStore + MemoryStore + Clone + 'static,
    M: llm::Source + 'static,
    F: Models,
{
    fn zoom(&self, id: crate::chat::ZoomId, n: u64, at: usize) -> Result<String, HarnessError> {
        self.0.upgrade().expect("chat is alive").zoom(id, n, at)
    }
    fn date(&self, id: u64) -> Result<String, HarnessError> {
        self.0.upgrade().expect("chat is alive").date(id)
    }
}
