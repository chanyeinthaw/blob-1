use std::sync::Arc;

use environment::Environment;
use storage::{MemoryStore, Message, SessionId, SessionStore, WorkspaceStore};

use crate::{chat::Inner, HarnessError, Llm};

type Error = Box<dyn std::error::Error + Send + Sync>;

pub(crate) async fn spawn<L, E, S, M>(
    inner: &Arc<Inner<L, E, S, M>>,
    tasks: Vec<String>,
) -> Result<Vec<String>, Error>
where
    L: Llm + Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
{
    inner.memory.settle(inner.memory.count()?).await?;
    let view = inner.memory.view()?;
    let mut names = Vec::new();
    for task in tasks {
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
        start(inner, name.clone(), text);
        names.push(name);
    }
    Ok(names)
}

pub(crate) fn tell<L, E, S, M>(
    inner: &Arc<Inner<L, E, S, M>>,
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
    if state.running.contains(name) {
        inner.session.steer(&id, text)?;
    } else if inner.store.load(&id)?.is_some() {
        state.running.insert(name.to_owned());
        drop(state);
        launch(inner, name.to_owned(), text);
    } else {
        return Err(format!("unknown agent: {name}").into());
    }
    Ok(())
}

fn start<L, E, S, M>(inner: &Arc<Inner<L, E, S, M>>, name: String, text: String)
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

fn launch<L, E, S, M>(inner: &Arc<Inner<L, E, S, M>>, name: String, text: String)
where
    L: Llm + Send + Sync + 'static,
    E: Environment + 'static,
    S: SessionStore + WorkspaceStore + MemoryStore + Clone + 'static,
    M: Llm + Send + Sync + 'static,
{
    let inner = Arc::clone(inner);
    let task_inner = Arc::clone(&inner);
    let task = tokio::spawn(async move {
        let inner = task_inner;
        let id = SessionId::new(format!("agent:{name}"));
        let mut text = text;
        loop {
            let result = inner
                .session
                .run(&id, text)
                .await
                .unwrap_or_else(|error| format!("error: {error}"));
            let mut state = inner.state.lock().expect("chat state mutex poisoned");
            let mut pending = inner.session.take_steering(&id).into_iter();
            if let Some(next) = pending.next() {
                for message in pending {
                    inner.session.steer(&id, message).expect("steering failed");
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
    let mut state = inner.state.lock().expect("chat state mutex poisoned");
    state.tasks.retain(|task| !task.is_finished());
    state.tasks.push(task.abort_handle());
}

pub(crate) fn zoom<L, E, S, M>(
    inner: &Inner<L, E, S, M>,
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
