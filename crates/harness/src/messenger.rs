use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{borrow::Cow, collections::HashMap};

use environment::ToolSpec;
use messenger::{Fetched, Mark, Messenger, Outbound, OutboundFile, Place, Scope, Target};
use serde::Deserialize;
use storage::{MemoryStore, MessengerStore, Meta};

type Error = Box<dyn std::error::Error + Send + Sync>;

const SEND_LIMIT: usize = 300;
const MAX_SEND_FILES: usize = 10;
const MAX_SEND_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_SEND_TOTAL_BYTES: u64 = 25 * 1024 * 1024;

pub const SEND: ToolSpec = ToolSpec {
    name: Cow::Borrowed("send"),
    description: Cow::Borrowed("Send a message to people on a messenger. Give exactly one of reply (an m-token from a <message> tag: answers that message) or to (a d-token or messenger URL such as a Discord channel link: posts plainly in that conversation), and text and/or files. At most 300 characters of text; split longer text into several sends. files: relative workspace file paths to attach (max 10 files, 10 MiB each, 25 MiB total). mention: user ids to mention; the system adds the mentions."),
    input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"text":{"type":"string","description":"Message text (at most 300 characters; optional when files are attached)"},"reply":{"type":"string","description":"m-token of the message to reply to"},"to":{"type":"string","description":"d-token of the conversation, or a messenger URL such as a Discord channel or message link"},"mention":{"type":"array","items":{"type":"string"},"description":"User ids to mention"},"thread":{"type":"boolean","description":"thread: true moves the conversation into a thread started from the message you reply to; use it when an inline exchange turns into real work"},"files":{"type":"array","items":{"type":"string"},"maxItems":10,"description":"Relative paths to workspace files to attach (max 10 files, 10 MiB per file, 25 MiB total)"}}}"#),
};
pub const LOOKUP: &[ToolSpec] = &[
    ToolSpec {
        name: Cow::Borrowed("message"),
        description: Cow::Borrowed("Read one message: by link (a message link someone pasted), or by to (d-token or messenger URL) plus message id (ids appear in history/search results); channel: true means the thread's parent channel (d-token to only)."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"link":{"type":"string"},"to":{"type":"string"},"message":{"type":"string"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread (d-token to only)"}}}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("history"),
        description: Cow::Borrowed("Read earlier messages of a conversation (to = d-token or messenger URL), oldest first; channel: true reads the thread's parent channel instead (d-token to only). before: a message id to page further back. limit 1-50, default 20."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"to":{"type":"string","description":"d-token of the conversation, or a messenger URL such as a Discord channel or message link"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread (d-token to only)"},"before":{"type":"string","description":"Message id to page before"},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["to"]}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("search"),
        description: Cow::Borrowed("Search messages containing query, in one conversation (to = d-token or messenger URL, channel: true for its parent channel, d-token only) or across a whole messenger (messenger = name, if supported). limit 1-50, default 20."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"query":{"type":"string"},"to":{"type":"string","description":"d-token of the conversation, or a messenger URL such as a Discord channel or message link"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread (d-token to only)"},"messenger":{"type":"string","description":"Messenger name for a messenger-wide lookup"},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["query"]}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("participants"),
        description: Cow::Borrowed("Find people and their user ids, in one conversation (to; channel for the parent, d-token only) or across a whole messenger (messenger + query). Use the ids with send's mention."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"to":{"type":"string","description":"d-token of the conversation, or a messenger URL such as a Discord channel or message link"},"channel":{"type":"boolean","description":"Use the parent channel instead of the thread (d-token to only)"},"messenger":{"type":"string","description":"Messenger name for a messenger-wide lookup"},"query":{"type":"string"}}}"#),
    },
];

trait Store: MessengerStore + MemoryStore {}
impl<T: MessengerStore + MemoryStore> Store for T {}

#[derive(Clone)]
pub struct Messengers {
    messengers: Vec<Arc<dyn Messenger>>,
    store: Arc<dyn Store>,
    utility: Option<Arc<dyn Fn() -> Option<crate::Utility> + Send + Sync>>,
    workspace: Option<crate::PinnedWorkspace>,
    live: Arc<LiveState>,
    answered: Arc<Mutex<std::collections::HashSet<u64>>>,
    moved: Arc<Mutex<std::collections::HashSet<u64>>>,
    threaded: Arc<Mutex<std::collections::HashSet<u64>>>,
    marks: tokio::sync::mpsc::UnboundedSender<MarkJob>,
}

struct MarkJob {
    messenger: Arc<dyn Messenger>,
    place: Place,
    message: String,
    add: Vec<Mark>,
    remove: Vec<Mark>,
}

#[derive(Default)]
struct LiveState(Mutex<HashMap<u64, tokio::task::AbortHandle>>);

impl Drop for LiveState {
    fn drop(&mut self) {
        for (_, pulse) in self.0.get_mut().expect("live mutex poisoned").drain() {
            pulse.abort();
        }
    }
}

#[derive(Deserialize)]
struct SendArgs {
    #[serde(default)]
    text: String,
    reply: Option<String>,
    to: Option<String>,
    #[serde(default)]
    mention: Vec<String>,
    #[serde(default)]
    thread: bool,
    #[serde(default)]
    files: Vec<String>,
}
#[derive(Deserialize)]
struct LookupArgs {
    link: Option<String>,
    message: Option<String>,
    to: Option<String>,
    messenger: Option<String>,
    query: Option<String>,
    #[serde(default)]
    channel: bool,
    before: Option<String>,
    limit: Option<u32>,
}

fn token(value: &str, prefix: char) -> Result<u64, Error> {
    Ok(value.strip_prefix(prefix).ok_or("invalid token")?.parse()?)
}
fn element(m: &Fetched) -> String {
    format!(
        "<message author=\"{}\" author_name=\"{}\" message=\"{}\" date=\"{}\">\n{}{}\n</message>",
        storage::attribute(&m.author.id),
        storage::attribute(&m.author.name),
        storage::attribute(&m.message),
        storage::attribute(&m.date),
        storage::render_attachments(&m.attachments),
        storage::escape(&m.text)
    )
}
fn fit<'a>(messages: impl Iterator<Item = &'a Fetched>) -> Vec<(&'a Fetched, String)> {
    let mut kept = Vec::new();
    let mut size = 0;
    for m in messages {
        let line = element(m);
        size += line.chars().count() + 1;
        if size + 100 > memory::TOOL_OUTPUT_LIMIT {
            break;
        }
        kept.push((m, line));
    }
    kept
}
fn render_history(messages: Vec<Fetched>) -> String {
    if messages.is_empty() {
        return "no messages".into();
    }
    let mut kept = fit(messages.iter().rev());
    kept.reverse();
    let lines: Vec<_> = kept.iter().map(|(_, line)| line.as_str()).collect();
    match (messages.len() - kept.len(), kept.first()) {
        (0, _) => lines.join("\n"),
        (older, None) => format!(
            "[{older} messages not shown: the newest is too large to show; page with before=\"{}\"]",
            messages.last().expect("non-empty").message
        ),
        (older, Some((oldest, _))) => format!(
            "[{older} older messages not shown; page with before=\"{}\"]\n{}",
            oldest.message,
            lines.join("\n")
        ),
    }
}
fn render_search(messages: Vec<Fetched>) -> String {
    if messages.is_empty() {
        return "no messages".into();
    }
    let kept = fit(messages.iter());
    let lines: Vec<_> = kept.iter().map(|(_, line)| line.as_str()).collect();
    match messages.len() - kept.len() {
        0 => lines.join("\n"),
        more => format!("{}\n[{more} more matches not shown]", lines.join("\n")),
    }
}
/// Reads and validates workspace files to attach on an outbound send, running
/// blocking file I/O off the async executor and bounding both per-file and
/// total attachment sizes.
async fn read_outbound_files(
    workspace: crate::PinnedWorkspace,
    paths: Vec<String>,
) -> Result<Vec<OutboundFile>, Error> {
    tokio::task::spawn_blocking(move || -> Result<Vec<OutboundFile>, Error> {
        let mut out = Vec::with_capacity(paths.len());
        let mut total_bytes: u64 = 0;
        for raw_path in paths {
            let file = workspace
                .open_file_nofollow(&raw_path)
                .map_err(|err| format!("cannot attach '{raw_path}': {err}"))?;
            let meta_len = file
                .metadata()
                .map_err(|err| format!("cannot stat '{raw_path}': {err}"))?
                .len();
            if meta_len > MAX_SEND_FILE_BYTES {
                return Err(format!(
                    "file '{raw_path}' exceeds {} MiB limit",
                    MAX_SEND_FILE_BYTES / (1024 * 1024)
                )
                .into());
            }
            if total_bytes.saturating_add(meta_len) > MAX_SEND_TOTAL_BYTES {
                return Err(format!(
                    "total attachment size exceeds {} MiB limit",
                    MAX_SEND_TOTAL_BYTES / (1024 * 1024)
                )
                .into());
            }
            let read_limit = MAX_SEND_FILE_BYTES.min(MAX_SEND_TOTAL_BYTES - total_bytes);
            let mut data = Vec::with_capacity(meta_len.min(read_limit) as usize);
            file.take(read_limit + 1)
                .read_to_end(&mut data)
                .map_err(|err| format!("cannot read '{raw_path}': {err}"))?;
            let actual_len = data.len() as u64;
            if actual_len > MAX_SEND_FILE_BYTES {
                return Err(format!(
                    "file '{raw_path}' exceeds {} MiB limit",
                    MAX_SEND_FILE_BYTES / (1024 * 1024)
                )
                .into());
            }
            if total_bytes.saturating_add(actual_len) > MAX_SEND_TOTAL_BYTES {
                return Err(format!(
                    "total attachment size exceeds {} MiB limit",
                    MAX_SEND_TOTAL_BYTES / (1024 * 1024)
                )
                .into());
            }
            total_bytes += actual_len;
            let name = Path::new(&raw_path)
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| format!("invalid file name in '{raw_path}'"))?
                .to_owned();
            out.push(OutboundFile { name, data });
        }
        Ok(out)
    })
    .await
    .map_err(|err| -> Error { format!("failed to read attachments: {err}").into() })?
}
impl Messengers {
    pub fn new<S: MessengerStore + MemoryStore + 'static>(
        messengers: Vec<Arc<dyn Messenger>>,
        store: S,
    ) -> Self {
        Self {
            messengers,
            store: Arc::new(store),
            utility: None,
            workspace: None,
            live: Arc::default(),
            answered: Arc::default(),
            moved: Arc::default(),
            threaded: Arc::default(),
            marks: {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MarkJob>();
                tokio::spawn(async move {
                    while let Some(job) = rx.recv().await {
                        for mark in job.remove {
                            if let Err(error) =
                                job.messenger.unreact(&job.place, &job.message, mark).await
                            {
                                eprintln!("{error}");
                            }
                        }
                        for mark in job.add {
                            if let Err(error) =
                                job.messenger.react(&job.place, &job.message, mark).await
                            {
                                eprintln!("{error}");
                            }
                        }
                    }
                });
                tx
            },
        }
    }
    /// Resolves the utility model each time it is needed, so changes apply
    /// without rebuilding the messengers.
    pub fn with_utility(
        mut self,
        utility: impl Fn() -> Option<crate::Utility> + Send + Sync + 'static,
    ) -> Self {
        self.utility = Some(Arc::new(utility));
        self
    }
    /// Pins the workspace root so `send` can attach workspace files safely.
    pub fn with_workspace(mut self, workspace: crate::PinnedWorkspace) -> Self {
        self.workspace = Some(workspace);
        self
    }
    pub fn mark(&self, reply: &str, add: &[Mark], remove: &[Mark]) {
        let resolved = (|| -> Result<_, Error> {
            let row = self
                .store
                .message(token(reply, 'm')?)?
                .ok_or("unknown message")?;
            let Some(Meta::Message {
                messenger,
                place,
                message,
                ..
            }) = row.meta
            else {
                return Err("message has no messenger metadata".into());
            };
            let messenger = self
                .messengers
                .iter()
                .find(|i| i.name() == messenger)
                .ok_or("unknown messenger")?
                .clone();
            Ok((messenger, place, message))
        })();
        let (messenger, place, message) = match resolved {
            Ok(value) => value,
            Err(error) => {
                eprintln!("{reply}: {error}");
                return;
            }
        };
        if self
            .marks
            .send(MarkJob {
                messenger,
                place,
                message,
                add: add.to_vec(),
                remove: remove.to_vec(),
            })
            .is_err()
        {
            eprintln!("mark worker stopped");
        }
    }
    pub fn live(&self, messenger: &str, to: u64, place: Place) -> tokio::task::Id {
        let messenger = self
            .messengers
            .iter()
            .find(|i| i.name() == messenger)
            .expect("unknown messenger")
            .clone();
        let pulse = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
            let mut ticks = tokio::time::interval(Duration::from_secs(8));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                if let Err(error) = messenger.typing(&place).await {
                    eprintln!("{error}");
                }
            }
        });
        if let Some(old) = self
            .live
            .0
            .lock()
            .expect("live mutex poisoned")
            .insert(to, pulse.abort_handle())
        {
            old.abort();
        }
        pulse.id()
    }
    pub fn live_for(&self, reply: &str) -> Result<Option<(u64, tokio::task::Id)>, Error> {
        let row = self
            .store
            .message(token(reply, 'm')?)?
            .ok_or("unknown message")?;
        if let Some(Meta::Message {
            messenger,
            to,
            message,
            ..
        }) = row.meta
        {
            let (_, mut place) = self.store.place(to)?.ok_or("unknown destination")?;
            if self
                .moved
                .lock()
                .expect("moved mutex poisoned")
                .contains(&token(reply, 'm')?)
            {
                place.thread = Some(message);
            }
            return Ok(Some((to, self.live(&messenger, to, place))));
        }
        Ok(None)
    }
    pub fn settle(&self, to: u64, pulse: tokio::task::Id) {
        let mut live = self.live.0.lock().expect("live mutex poisoned");
        if live.get(&to).is_some_and(|handle| handle.id() == pulse) {
            live.remove(&to).expect("checked").abort();
        }
    }
    pub fn end_turn(&self) {
        self.answered
            .lock()
            .expect("answered mutex poisoned")
            .clear();
    }
    pub fn quiet(&self) {
        for (_, pulse) in self.live.0.lock().expect("live mutex poisoned").drain() {
            pulse.abort();
        }
    }
    pub(crate) fn find(&self, name: &str) -> Result<&dyn Messenger, Error> {
        self.messengers
            .iter()
            .find(|i| i.name() == name)
            .map(Arc::as_ref)
            .ok_or_else(|| format!("unknown messenger: {name}").into())
    }
    /// Resolves `to` — a `d` destination token or a URL the messenger parses
    /// (e.g. a Discord channel or message link) — to (messenger, place).
    /// `discord://` URIs also start with 'd', so the token path is chosen on a
    /// successful `d`-token parse, not on the prefix alone.
    fn place(&self, value: &str, channel: bool) -> Result<(String, Place), Error> {
        let (name, mut place) = match token(value, 'd') {
            Ok(id) => self.store.place(id)?.ok_or("unknown destination")?,
            Err(_) => {
                // A URL place's channel is already the concrete channel or
                // thread id from the link; there is no stored parent to hoist.
                if channel {
                    return Err("channel only applies to d destination tokens".into());
                }
                self.messengers
                    .iter()
                    .find_map(|m| {
                        m.parse_place(value)
                            .map(|place| (m.name().to_owned(), place))
                    })
                    .ok_or("unknown destination")?
            }
        };
        if channel {
            place.thread = None;
        }
        Ok((name, place))
    }
    fn scope(&self, args: &LookupArgs) -> Result<(String, Scope), Error> {
        match (&args.to, &args.messenger) {
            (Some(to), None) => {
                let (name, place) = self.place(to, args.channel)?;
                Ok((name, Scope::Place(place)))
            }
            (None, Some(name)) => Ok((name.clone(), Scope::Messenger)),
            _ => Err("exactly one of to or messenger is required".into()),
        }
    }
    /// Whether the place resolves to `direct` reply mode: channel override,
    /// then the channel's server override (guild resolved over REST only when a
    /// `server` override exists), then the messenger default.
    async fn is_direct_channel(&self, messenger: &str, channel_id: &str) -> Result<bool, Error> {
        let overrides = self.store.reply_modes(messenger)?;
        let default = self.store.messenger_defaults(messenger)?.reply_mode;
        // A channel override decides alone; the guild is looked up only when a
        // server override could apply, and a failed lookup fails the send
        // rather than silently ignoring a `direct` server.
        let channel_override = overrides
            .iter()
            .any(|o| o.scope == storage::ReplyScope::Channel && o.target_id == channel_id);
        let guild = if !channel_override
            && overrides
                .iter()
                .any(|o| o.scope == storage::ReplyScope::Server)
        {
            self.find(messenger)?.channel_guild(channel_id).await?
        } else {
            None
        };
        Ok(
            storage::effective_reply_mode(&overrides, channel_id, guild.as_deref(), default)
                == storage::ReplyMode::Direct,
        )
    }

    pub async fn send(&self, arguments: &str) -> Result<String, Error> {
        let args: SendArgs = serde_json::from_str(arguments)?;
        if args.text.is_empty() && args.files.is_empty() {
            return Err("text or files must be non-empty".into());
        }
        if args.text.chars().count() > SEND_LIMIT {
            return Err(format!(
                "text exceeds {SEND_LIMIT} characters; shorten or split into several sends"
            )
            .into());
        }
        let attachment_workspace = if args.files.is_empty() {
            None
        } else {
            if args.files.len() > MAX_SEND_FILES {
                return Err(
                    format!("at most {MAX_SEND_FILES} files can be attached per send").into(),
                );
            }
            Some(
                self.workspace
                    .clone()
                    .ok_or("workspace is not configured; cannot attach files")?,
            )
        };
        let mut answering = None;
        let mut title_prompt = None;
        let mut threading = false;
        let (name, target, destination, url_thread) = match (args.reply, args.to) {
            (Some(reply), None) => {
                let id = token(&reply, 'm')?;
                answering = Some(id);
                let row = self.store.message(id)?.ok_or("unknown message")?;
                match row.meta {
                    Some(Meta::Message {
                        messenger,
                        place,
                        message,
                        author,
                        to,
                        quote,
                        inline,
                        ..
                    }) => {
                        let first = !self
                            .answered
                            .lock()
                            .expect("answered mutex poisoned")
                            .contains(&id);
                        let moved = self
                            .moved
                            .lock()
                            .expect("moved mutex poisoned")
                            .contains(&id);
                        let forced_direct = !place.dm
                            && place.thread.is_none()
                            && self.is_direct_channel(&messenger, &place.channel).await?;
                        let thread = if forced_direct {
                            false
                        } else {
                            !inline || args.thread || moved
                        };
                        threading = thread && !place.dm && place.thread.is_none();
                        let first = if threading {
                            !self
                                .threaded
                                .lock()
                                .expect("threaded mutex poisoned")
                                .contains(&id)
                        } else {
                            first
                        };
                        if (first || inline)
                            && thread
                            && !place.dm
                            && place.thread.is_none()
                            && self.utility.as_ref().is_some_and(|u| u().is_some())
                        {
                            let mut prompt = row.text;
                            if let Some(quote) = quote {
                                prompt.push('\n');
                                prompt.push_str(&quote.text);
                            }
                            title_prompt = Some((
                                Place {
                                    channel: place.channel.clone(),
                                    thread: Some(message.clone()),
                                    dm: false,
                                },
                                Some(prompt),
                            ));
                        }
                        (
                            messenger,
                            Target::Reply {
                                place,
                                message,
                                author: author.id,
                                first,
                                thread,
                            },
                            Some(to),
                            false,
                        )
                    }
                    _ => return Err("message has no messenger metadata".into()),
                }
            }
            (None, Some(to)) => {
                let (name, place) = self.place(&to, false)?;
                let (to_id, url_thread) = match token(&to, 'd') {
                    Ok(id) => (Some(id), false),
                    Err(_) => {
                        let id = self.store.place_id(&name, &place)?;
                        // A thread URL posts into the thread's channel but
                        // reports used_thread=false — the stored row knows it
                        // is a thread, so flag it for pending cleanup.
                        let thread = match id {
                            Some(id) => self
                                .store
                                .place(id)?
                                .is_some_and(|(_, p)| p.thread.is_some()),
                            None => false,
                        };
                        (id, thread)
                    }
                };
                // Thread handling only applies when the place carries a
                // thread; skip the reply-mode lookup (possibly a REST call)
                // for plain-channel places and URLs.
                let forced_direct = !place.dm
                    && place.thread.is_some()
                    && self.is_direct_channel(&name, &place.channel).await?;
                let is_pending = match to_id {
                    Some(id) if place.thread.is_some() => self.store.is_pending_thread(id)?,
                    _ => false,
                };
                let create_thread = !forced_direct;
                let fallback_parent = forced_direct && is_pending;
                if place.thread.is_some() && !place.dm && create_thread {
                    title_prompt = Some((place.clone(), None));
                }
                (
                    name,
                    Target::To {
                        place,
                        create_thread,
                        fallback_parent,
                    },
                    to_id,
                    url_thread,
                )
            }
            _ => return Err("exactly one of reply or to is required".into()),
        };
        let messenger = self.find(&name)?;
        let files = match attachment_workspace {
            Some(workspace) => read_outbound_files(workspace, args.files).await?,
            None => Vec::new(),
        };
        let sent = messenger
            .send(Outbound {
                target,
                text: args.text,
                mention: args.mention,
                files,
            })
            .await?;
        if let Some(destination) = destination {
            if sent.created_thread || sent.used_thread || url_thread {
                let _ = self.store.clear_pending_thread(destination);
            }
        }
        if sent.created_thread {
            if let (Some((place, prompt)), Some(utility)) = (
                title_prompt,
                self.utility
                    .as_ref()
                    .and_then(|u| u())
                    .filter(|u| u.has_llm()),
            ) {
                let messenger = self
                    .messengers
                    .iter()
                    .find(|m| m.name() == name)
                    .expect("messenger already found")
                    .clone();
                tokio::spawn(async move {
                    let prompt = match prompt {
                        Some(prompt) => prompt,
                        None => {
                            let source = Place {
                                channel: place.channel.clone(),
                                thread: None,
                                dm: false,
                            };
                            let id = place.thread.clone().expect("title place is a thread");
                            match messenger.message(&source, &id).await {
                                Ok(message) => message.text,
                                Err(error) => {
                                    eprintln!("{error}");
                                    return;
                                }
                            }
                        }
                    };
                    match utility.generate("Write a short title (max 6 words) for a chat thread that starts with this message. Reply with the title only.", &prompt).await {
                    Ok(title) => {
                        let title: String = title.trim().trim_matches(['"', '\'']).split_whitespace().take(6).collect::<Vec<_>>().join(" ").chars().take(100).collect();
                        if !title.is_empty() {
                            if let Err(error) = messenger.rename(&place, &title).await {
                                eprintln!("{error}");
                            }
                        }
                    }
                    Err(error) => eprintln!("{error}"),
                }
                });
            }
        }
        if let Some(id) = answering {
            if threading {
                self.threaded
                    .lock()
                    .expect("threaded mutex poisoned")
                    .insert(id);
            }
            if args.thread && threading {
                self.moved.lock().expect("moved mutex poisoned").insert(id);
            }
            self.answered
                .lock()
                .expect("answered mutex poisoned")
                .insert(id);
        }
        if let Some(destination) = destination {
            if let Some(pulse) = self
                .live
                .0
                .lock()
                .expect("live mutex poisoned")
                .remove(&destination)
            {
                pulse.abort();
            }
        }
        Ok("sent".into())
    }
    pub async fn lookup(&self, name: &str, arguments: &str) -> Result<String, Error> {
        let args: LookupArgs = serde_json::from_str(arguments)?;
        let limit = args.limit.unwrap_or(20);
        if !(1..=50).contains(&limit) {
            return Err("limit must be between 1 and 50".into());
        }
        match name {
            "message" => {
                let (messenger, place, message) = match (&args.link, &args.to, &args.message) {
                    (Some(link), None, None) => {
                        let (messenger, (place, message)) = self
                            .messengers
                            .iter()
                            .find_map(|messenger| {
                                messenger
                                    .parse_link(link)
                                    .map(|parsed| (messenger.as_ref(), parsed))
                            })
                            .ok_or("not a message link")?;
                        (messenger, place, message)
                    }
                    (None, Some(to), Some(message)) => {
                        let (name, place) = self.place(to, args.channel)?;
                        (self.find(&name)?, place, message.clone())
                    }
                    _ => return Err("exactly one of link or to plus message is required".into()),
                };
                Ok(element(&messenger.message(&place, &message).await?))
            }
            "history" => {
                let to = args.to.as_deref().ok_or("to is required")?;
                let (name, place) = self.place(to, args.channel)?;
                Ok(render_history(
                    self.find(&name)?
                        .history(&place, args.before.as_deref(), limit)
                        .await?,
                ))
            }
            "search" => {
                let (name, scope) = self.scope(&args)?;
                let query = args.query.as_deref().ok_or("query is required")?;
                Ok(render_search(
                    self.find(&name)?.search(&scope, query, limit).await?,
                ))
            }
            "participants" => {
                let (name, scope) = self.scope(&args)?;
                let people = self
                    .find(&name)?
                    .participants(&scope, args.query.as_deref())
                    .await?;
                if people.is_empty() {
                    return Ok("no people".into());
                }
                Ok(people
                    .into_iter()
                    .map(|p| format!("{} ({})", storage::escape(&p.name), storage::escape(&p.id)))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            _ => Err(format!("unknown tool: {name}").into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::token;

    /// `discord://` URIs start with 'd' but are not destination tokens —
    /// `Messengers::place` picks the URL path on this parse failing.
    #[test]
    fn token_classifies_only_d_ids_as_destinations() {
        assert_eq!(token("d42", 'd').unwrap(), 42);
        assert!(token("discord://-/channels/111/222", 'd').is_err());
        assert!(token("dabc", 'd').is_err());
        assert!(token("https://discord.com/channels/1/2", 'd').is_err());
    }
}
