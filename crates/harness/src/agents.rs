use std::sync::Arc;

use environment::Environment;
use storage::{MemoryStore, Message, SessionId, SessionStore, WorkspaceStore};

use crate::{
    chat::{Inner, Models, Shared, SpawnTask},
    CompactionConfig, HarnessConfig, HarnessError, Llm, SessionHarness,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

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
        let (llm, window) = inner.models.connect(choice, reasoning)?;
        prepared.push((task.task, llm, window));
    }
    inner.memory.settle(inner.memory.count()?).await?;
    let view = inner.memory.view()?;
    let mut names = Vec::new();
    for (task, llm, window) in prepared {
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
                    .running
                    .contains(&name)
            {
                break name;
            }
        };
        let text = format!("{view}\n\nThe <chat> above is the main chat's memory, as context. Your task:\n\n{task}");
        let session = build_session(inner, llm, window);
        inner
            .sessions
            .lock()
            .expect("sessions mutex poisoned")
            .insert(name.clone(), session);
        start(inner, name.clone(), text);
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
    if state.running.contains(name) {
        session
            .expect("running agent has session")
            .steer(&id, text)?;
    } else if inner.store.load(&id)?.is_some() {
        if session.is_none() {
            let (llm, window) = inner.models.connect(None, None)?;
            inner
                .sessions
                .lock()
                .expect("sessions mutex poisoned")
                .insert(name.to_owned(), build_session(inner, llm, window));
        }
        state.running.insert(name.to_owned());
        drop(state);
        launch(inner, name.to_owned(), text);
    } else {
        return Err(format!("unknown agent: {name}").into());
    }
    Ok(())
}

fn build_session<L, E, S, M, F: Models>(
    inner: &Inner<L, E, S, M, F>,
    llm: F::Llm,
    window: Option<u32>,
) -> Arc<SessionHarness<Shared<F::Llm>, Shared<E>, S>>
where
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
{
    let mut session = SessionHarness::new(
        Shared(Arc::new(llm)),
        Shared(Arc::clone(&inner.environment.0)),
        inner.store.clone(),
        inner.paths.clone(),
    )
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

fn start<L, E, S, M, F: Models>(inner: &Arc<Inner<L, E, S, M, F>>, name: String, text: String)
where
    L: Llm + Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
{
    inner
        .state
        .lock()
        .expect("chat state mutex poisoned")
        .running
        .insert(name.clone());
    launch(inner, name, text);
}

fn launch<L, E, S, M, F: Models>(inner: &Arc<Inner<L, E, S, M, F>>, name: String, text: String)
where
    L: Llm + Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
{
    let inner = Arc::clone(inner);
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
    let crash_session = Arc::clone(&session);
    let work = tokio::spawn(async move {
        let inner = task_inner;
        let id = SessionId::new(format!("agent:{name}"));
        let mut text = text;
        loop {
            let result = session
                .run(&id, text)
                .await
                .unwrap_or_else(|error| format!("error: {error}"));
            let mut state = inner.state.lock().expect("chat state mutex poisoned");
            let mut pending = session.take_steering(&id).into_iter();
            if let Some(next) = pending.next() {
                for message in pending {
                    session.steer(&id, message).expect("steering failed");
                }
                text = next;
                continue;
            }
            let report = format!("[{name}]\n{result}\n\nChat: zoom(\"{name}\")");
            state.reports.push_back(report);
            state.running.remove(&name);
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
            let dropped = crash_session
                .take_steering(&SessionId::new(format!("agent:{crash_name}")))
                .len();
            let suffix = if dropped > 0 {
                format!(" (dropped {dropped} pending messages)")
            } else {
                String::new()
            };
            state.reports.push_back(format!(
                "[{crash_name}]\nerror: agent crashed: {message}{suffix}\n\nChat: zoom(\"{crash_name}\")"
            ));
            state.running.remove(&crash_name);
            drop(state);
            crash_inner.notify.notify_one();
        }
    });
    let mut state = inner.state.lock().expect("chat state mutex poisoned");
    state.tasks.retain(|task| !task.is_finished());
    state.tasks.push(work_abort);
    state.tasks.push(task.abort_handle());
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
