use std::path::Path;
use std::sync::{Arc, Mutex};

use environment::{Environment, SkillBundle};
use environment_local::LocalEnvironment;
use harness::{ChatHarness, EnvironmentPaths, Models};
use messenger::Messenger;
use storage::{
    ConfigStore, McpStore, MessengerStore, Provider, ReasoningLevel, SkillStore, UsageEvent,
    UsageKind, UsageStore,
};
use storage_sqlite::SqliteStorage;
use tokio::signal::unix::{signal, SignalKind};

use crate::{api_key, Result};

/// Resolves the models each role uses from the settings stored in SQLite.
/// `current` re-resolves only when the relevant settings changed; a failed
/// resolution keeps the last working model for that role and logs the error
/// once per change.
#[derive(Clone)]
struct ModelFactory {
    store: Arc<SqliteStorage>,
    cache: Arc<Mutex<Cache>>,
}

#[derive(Default)]
struct Cache {
    fingerprint: Option<u64>,
    main: Option<Working>,
    summarizer: Option<Option<(Model, Option<u32>)>>,
    compactor: Option<Model>,
    utility: Option<Option<RecordedLlm>>,
    main_error: Option<String>,
}

#[derive(Clone)]
struct Working {
    llm: llm_rig::RigLlm,
    context: Option<u32>,
    selection: harness::ModelSelection,
    provider: Provider,
    model: storage::Model,
}

impl ModelFactory {
    fn new(store: Arc<SqliteStorage>) -> Self {
        Self {
            store,
            cache: Arc::new(Mutex::new(Cache::default())),
        }
    }

    /// Covers every input `resolve` reads, including a non-secret fingerprint of
    /// each provider's stored API key ciphertext, so key rotation invalidates cached clients.
    fn fingerprint(&self) -> std::result::Result<u64, harness::HarnessError> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.store.settings()?.hash(&mut hasher);
        self.store.utility_model()?.hash(&mut hasher);
        for provider in self.store.providers()? {
            provider.hash(&mut hasher);
            self.store
                .provider_key_fingerprint(&provider.id)?
                .hash(&mut hasher);
            self.store.models(&provider.id)?.hash(&mut hasher);
        }
        Ok(hasher.finish())
    }
}

impl harness::Models for ModelFactory {
    type Llm = Model;

    fn list(&self, search: Option<&str>) -> std::result::Result<String, harness::HarnessError> {
        let settings = self.store.settings()?;
        let default = settings
            .default_model
            .ok_or_else(|| harness::HarnessError::Compaction("no default model".into()))?;
        let mut lines = vec![
            format!(
                "default: provider={} model={} reasoning={}",
                default.provider.0,
                default.model,
                settings.reasoning.as_str()
            ),
            match settings.compaction_model {
                Some(model) => format!(
                    "compaction: provider={} model={}",
                    model.provider.0, model.model
                ),
                None => "compaction: default".into(),
            },
            "provider  model  context  reasoning  input".into(),
        ];
        let search = search.unwrap_or("").to_lowercase();
        for provider in self.store.providers()? {
            for model in self.store.models(&provider.id)? {
                if !provider.id.0.to_lowercase().contains(&search)
                    && !model.id.to_lowercase().contains(&search)
                {
                    continue;
                }
                let context = match (model.effective_context(), model.context_window) {
                    (Some(limit), Some(window)) if limit < window => {
                        format!("{limit} (of {window})")
                    }
                    (Some(limit), _) => limit.to_string(),
                    _ => "-".into(),
                };
                let levels = match llm_rig::reasoning_levels(&provider, &model) {
                    Some(levels) => levels
                        .into_iter()
                        .map(|level| level.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                    None => {
                        let mapped = model
                            .reasoning_map
                            .iter()
                            .map(|(level, value)| {
                                format!("{}={}", level.as_str(), value.as_deref().unwrap_or("-"))
                            })
                            .collect::<Vec<_>>()
                            .join(" ");
                        if mapped.is_empty() {
                            "unknown".into()
                        } else {
                            format!("unknown (mapped: {mapped})")
                        }
                    }
                };
                lines.push(format!(
                    "{}  {}  {}  {}  {}",
                    provider.id.0,
                    model.id,
                    context,
                    if levels.is_empty() { "-" } else { &levels },
                    if model.images { "image" } else { "text" }
                ));
            }
        }
        Ok(lines.join("\n"))
    }

    fn connect(
        &self,
        choice: Option<(&str, &str)>,
        reasoning: Option<ReasoningLevel>,
    ) -> std::result::Result<(Model, Option<u32>, harness::ModelSelection), harness::HarnessError>
    {
        let (provider_id, model_id, reasoning) = match choice {
            Some((provider, model)) => (
                provider.to_owned(),
                model.to_owned(),
                match reasoning {
                    Some(reasoning) => reasoning,
                    None => self.store.settings()?.reasoning,
                },
            ),
            None => {
                self.current()?;
                let main = {
                    let cache = self.cache.lock().expect("model cache poisoned");
                    cache.main.clone().ok_or_else(|| {
                        harness::HarnessError::Compaction(
                            cache
                                .main_error
                                .clone()
                                .unwrap_or_else(|| "no main model".into()),
                        )
                    })?
                };
                let reasoning = reasoning.unwrap_or(main.selection.reasoning);
                if reasoning == main.selection.reasoning {
                    return Ok((
                        Model::main(
                            main.llm,
                            self.store.clone(),
                            UsageKind::Chat,
                            &main.selection.provider,
                            &main.selection.model,
                        ),
                        main.context,
                        main.selection,
                    ));
                }
                let key = api_key(&self.store, &main.provider)
                    .map_err(|error| harness::HarnessError::Compaction(error.to_string()))?;
                return Ok((
                    Model::main(
                        llm_rig::connect(&main.provider, &key, &main.model, reasoning),
                        self.store.clone(),
                        UsageKind::Chat,
                        &main.selection.provider,
                        &main.selection.model,
                    ),
                    main.context,
                    harness::ModelSelection {
                        reasoning,
                        ..main.selection
                    },
                ));
            }
        };
        let unknown = || harness::HarnessError::UnknownModel(format!("{provider_id} {model_id}"));
        let provider = self
            .store
            .providers()?
            .into_iter()
            .find(|p| p.id.0 == provider_id)
            .ok_or_else(unknown)?;
        check_base_url(&provider).map_err(harness::HarnessError::Compaction)?;
        let model = self
            .store
            .models(&provider.id)?
            .into_iter()
            .find(|m| m.id == model_id)
            .ok_or_else(unknown)?;
        let key = api_key(&self.store, &provider)
            .map_err(|error| harness::HarnessError::Compaction(error.to_string()))?;
        Ok((
            Model::main(
                llm_rig::connect(&provider, &key, &model, reasoning),
                self.store.clone(),
                UsageKind::Chat,
                &provider_id,
                &model_id,
            ),
            model.effective_context(),
            harness::ModelSelection {
                images: model.images,
                provider: provider_id,
                model: model_id,
                reasoning,
            },
        ))
    }

    fn current(&self) -> std::result::Result<harness::Current<Model>, harness::HarnessError> {
        let fingerprint = self.fingerprint()?;
        let mut cache = self.cache.lock().expect("model cache poisoned");
        if cache.fingerprint != Some(fingerprint) {
            cache.fingerprint = Some(fingerprint);
            let attempt = resolve(&self.store, cache.main.as_ref());
            // A role that fails to resolve keeps its last working model; each
            // distinct error is logged once per settings change.
            let mut logged: Vec<String> = Vec::new();
            let mut log = |error: &String| {
                if !logged.contains(error) {
                    eprintln!("{error}");
                    logged.push(error.clone());
                }
            };
            match attempt.main {
                Ok(working) => {
                    cache.main = Some(working);
                    cache.main_error = None;
                }
                Err(error) => {
                    log(&error);
                    cache.main_error = Some(error);
                }
            }
            match attempt.summarizer {
                Ok(summarizer) => cache.summarizer = Some(summarizer),
                Err(error) => log(&error),
            }
            match attempt.compactor {
                Ok(compactor) => cache.compactor = Some(compactor),
                Err(error) => log(&error),
            }
            match attempt.utility {
                Ok(utility) => cache.utility = Some(utility),
                Err(error) => log(&error),
            }
        }
        let main = cache.main.clone().ok_or_else(|| {
            harness::HarnessError::Compaction(
                cache
                    .main_error
                    .clone()
                    .unwrap_or_else(|| "no main model".into()),
            )
        })?;
        Ok(harness::Current {
            main: Model::main(
                main.llm.clone(),
                self.store.clone(),
                UsageKind::Chat,
                &main.selection.provider,
                &main.selection.model,
            ),
            summarizer: cache.summarizer.clone().flatten(),
            compactor: cache.compactor.clone().unwrap_or_else(|| {
                Model::main(
                    main.llm.clone(),
                    self.store.clone(),
                    UsageKind::Compaction,
                    &main.selection.provider,
                    &main.selection.model,
                )
            }),
            utility: cache.utility.clone().flatten().map(harness::Utility::new),
        })
    }
}

/// Wraps a `RigLlm` to persist per-call token and cache telemetry into SQLite.
#[derive(Clone)]
struct RecordedLlm {
    inner: llm_rig::RigLlm,
    store: Arc<SqliteStorage>,
    kind: UsageKind,
    provider: Arc<str>,
    model: Arc<str>,
}

impl RecordedLlm {
    fn new(
        inner: llm_rig::RigLlm,
        store: Arc<SqliteStorage>,
        kind: UsageKind,
        provider: &str,
        model: &str,
    ) -> Self {
        Self {
            inner,
            store,
            kind,
            provider: Arc::from(provider),
            model: Arc::from(model),
        }
    }
}

#[async_trait::async_trait]
impl llm::Llm for RecordedLlm {
    async fn complete(
        &self,
        request: llm::CompletionRequest<'_>,
    ) -> std::result::Result<llm::CompletionResponse, llm::LlmError> {
        let kind = request.kind.unwrap_or(self.kind);
        let response = self.inner.complete(request).await?;
        let usage = response.usage.unwrap_or_default();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Err(error) = self.store.record_usage(&UsageEvent {
            ts,
            kind,
            provider: self.provider.to_string(),
            model: self.model.to_string(),
            input_tokens: usage.input,
            cached_tokens: usage.cache_read,
            output_tokens: usage.output,
        }) {
            eprintln!("usage telemetry write failed: {error}");
        }
        Ok(response)
    }
}

#[derive(Clone)]
enum Model {
    Main(RecordedLlm),
    Fallback(Arc<llm::Fallback<RecordedLlm, RecordedLlm>>),
}

impl Model {
    fn main(
        inner: llm_rig::RigLlm,
        store: Arc<SqliteStorage>,
        kind: UsageKind,
        provider: &str,
        model: &str,
    ) -> Self {
        Self::Main(RecordedLlm::new(inner, store, kind, provider, model))
    }
}

#[async_trait::async_trait]
impl llm::Llm for Model {
    async fn complete(
        &self,
        request: llm::CompletionRequest<'_>,
    ) -> std::result::Result<llm::CompletionResponse, llm::LlmError> {
        match self {
            Self::Main(model) => model.complete(request).await,
            Self::Fallback(model) => model.complete(request).await,
        }
    }
}

/// Shared handle to the running `ChatHarness` used by inbound webhooks.
#[derive(Clone, Default)]
pub struct LiveChat {
    inner: Arc<std::sync::RwLock<Option<(Arc<Chat>, ModelFactory)>>>,
}

impl LiveChat {
    fn set(&self, harness: Arc<Chat>, models: ModelFactory) {
        *self.inner.write().expect("live chat lock poisoned") = Some((harness, models));
    }

    fn get(&self) -> Option<(Arc<Chat>, ModelFactory)> {
        self.inner.read().expect("live chat lock poisoned").clone()
    }

    pub fn inject(&self, text: String, meta: Option<storage::Meta>) -> Result<()> {
        let Some((chat, models)) = self.get() else {
            return Err("agent is still starting".into());
        };
        models.current()?;
        chat.steer_message(text, meta);
        Ok(())
    }
}

/// The memory compactor role: resolves the current compaction model once at
/// the start of each compaction pass.
#[derive(Clone)]
struct Compactor(ModelFactory);

impl llm::Source for Compactor {
    type Llm = Model;

    fn current(&self) -> std::result::Result<Model, llm::LlmError> {
        self.0
            .current()
            .map(|current| current.compactor)
            .map_err(|error| llm::LlmError::Request(error.to_string()))
    }
}

type Chat = ChatHarness<LocalEnvironment, Arc<SqliteStorage>, Compactor, ModelFactory>;

type Resolve<T> = std::result::Result<T, String>;

struct Attempt {
    main: Resolve<Working>,
    summarizer: Resolve<Option<(Model, Option<u32>)>>,
    compactor: Resolve<Model>,
    utility: Resolve<Option<RecordedLlm>>,
}

fn check_base_url(provider: &Provider) -> Resolve<()> {
    let base_url = &provider.base_url;
    let valid = !base_url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
        && url::Url::parse(base_url)
            .map(|url| matches!(url.scheme(), "http" | "https"))
            .unwrap_or(false)
        && base_url
            .parse::<axum::http::Uri>()
            .is_ok_and(|uri| uri.host().is_some_and(|host| !host.is_empty()));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "provider {} has an invalid base URL: expected an absolute http(s) URL",
            provider.id.0
        ))
    }
}

fn resolve_model(
    store: &SqliteStorage,
    selected: &storage::ModelRef,
) -> Resolve<(Provider, storage::Model)> {
    let provider = store
        .providers()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|provider| provider.id == selected.provider)
        .ok_or_else(|| format!("unknown provider: {}", selected.provider.0))?;
    check_base_url(&provider)?;
    let model = store
        .models(&selected.provider)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|model| model.id == selected.model)
        .ok_or_else(|| {
            format!(
                "unknown model {}/{}; run `agent models refresh` first",
                selected.provider.0, selected.model
            )
        })?;
    Ok((provider, model))
}

fn resolve_main(store: &SqliteStorage, settings: &storage::Settings) -> Resolve<Working> {
    let Some(selected) = &settings.default_model else {
        return Err("no default model; run `agent model set <provider>/<model>`".into());
    };
    let (provider, model) = resolve_model(store, selected)?;
    let key = api_key(store, &provider).map_err(|error| error.to_string())?;
    Ok(Working {
        llm: llm_rig::connect(&provider, &key, &model, settings.reasoning),
        context: model.effective_context(),
        selection: harness::ModelSelection {
            images: model.images,
            provider: provider.id.0.clone(),
            model: model.id.clone(),
            reasoning: settings.reasoning,
        },
        provider,
        model,
    })
}

fn resolve_summarizer(
    store: &Arc<SqliteStorage>,
    settings: &storage::Settings,
    main: &Resolve<Working>,
    cached: Option<&Working>,
) -> Resolve<Option<(Model, Option<u32>)>> {
    let Some(selected) = &settings.compaction_model else {
        return Ok(None);
    };
    let (provider, stored) = resolve_model(store, selected)?;
    let key = api_key(store, &provider).map_err(|error| error.to_string())?;
    let llm = RecordedLlm::new(
        llm_rig::connect(
            &provider,
            &key,
            &stored,
            settings.compaction_reasoning.unwrap_or(settings.reasoning),
        ),
        store.clone(),
        UsageKind::Compaction,
        &provider.id.0,
        &stored.id,
    );
    if settings.default_model.as_ref() == Some(selected) {
        return Ok(Some((Model::Main(llm), stored.effective_context())));
    }
    let fallback = main.as_ref().ok().or(cached);
    let main_window = fallback.and_then(|main| main.context);
    let window = match (stored.effective_context(), main_window) {
        (Some(compaction), Some(main)) => Some(compaction.min(main)),
        (compaction, main) => compaction.or(main),
    };
    let model = match fallback {
        Some(main) => {
            let secondary = RecordedLlm::new(
                main.llm.clone(),
                store.clone(),
                UsageKind::Compaction,
                &main.selection.provider,
                &main.selection.model,
            );
            Model::Fallback(Arc::new(llm::Fallback::new(llm, secondary)))
        }
        None => Model::Main(llm),
    };
    Ok(Some((model, window)))
}

fn resolve_compactor(store: &Arc<SqliteStorage>, settings: &storage::Settings) -> Resolve<Model> {
    let Some(selected) = &settings.default_model else {
        return Err("no default model; run `agent model set <provider>/<model>`".into());
    };
    let (provider, stored) = resolve_model(store, selected)?;
    let key = api_key(store, &provider).map_err(|error| error.to_string())?;
    Ok(Model::main(
        llm_rig::connect(
            &provider,
            &key,
            &stored,
            settings.compaction_reasoning.unwrap_or(settings.reasoning),
        ),
        store.clone(),
        UsageKind::Compaction,
        &provider.id.0,
        &stored.id,
    ))
}

fn resolve(store: &Arc<SqliteStorage>, cached: Option<&Working>) -> Attempt {
    let settings = match store.settings() {
        Ok(settings) => settings,
        Err(error) => {
            let error = error.to_string();
            return Attempt {
                main: Err(error.clone()),
                summarizer: Err(error.clone()),
                compactor: Err(error.clone()),
                utility: Err(error),
            };
        }
    };
    let main = resolve_main(store, &settings);
    let summarizer = resolve_summarizer(store, &settings, &main, cached);
    let compactor = match &summarizer {
        Ok(Some((model, _))) => Ok(model.clone()),
        Ok(None) => resolve_compactor(store, &settings),
        Err(error) => Err(error.clone()),
    };
    let utility = (|| -> Resolve<Option<RecordedLlm>> {
        let Some((selected, reasoning)) =
            store.utility_model().map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let (provider, model) = resolve_model(store, &selected)?;
        let key = api_key(store, &provider).map_err(|error| error.to_string())?;
        Ok(Some(RecordedLlm::new(
            llm_rig::connect(
                &provider,
                &key,
                &model,
                reasoning.unwrap_or(ReasoningLevel::Low),
            ),
            store.clone(),
            UsageKind::Utility,
            &provider.id.0,
            &model.id,
        )))
    })();
    Attempt {
        main,
        summarizer,
        compactor,
        utility,
    }
}

async fn environment(
    store: &Arc<SqliteStorage>,
    workspace: &Path,
    skills_dir: Option<&Path>,
) -> Result<LocalEnvironment> {
    let environment = LocalEnvironment::new(workspace, skills_dir.map(Path::to_path_buf))?;
    let manifest = environment.managed_skills().await?;
    let skills = store.skills()?;
    let mut put = Vec::new();
    for skill in &skills {
        if !matches!(manifest.get(&skill.name), Some((hash, true)) if hash == &skill.hash) {
            let files = store
                .skill_files(&skill.name)?
                .into_iter()
                .map(|file| environment::SkillFile {
                    path: file.path,
                    mode: file.mode,
                    content: file.content,
                })
                .collect();
            put.push(SkillBundle {
                name: skill.name.clone(),
                hash: skill.hash.clone(),
                files,
            });
        }
    }
    let remove = manifest
        .keys()
        .filter(|name| !skills.iter().any(|skill| &skill.name == *name))
        .cloned()
        .collect();
    for name in environment.apply_skills(put, remove).await? {
        eprintln!("skill {name} skipped: a user-owned skill with this name exists");
    }
    Ok(environment)
}

async fn chat_harness(
    store: Arc<SqliteStorage>,
    workspace: &Path,
    pinned_workspace: harness::PinnedWorkspace,
    skills_dir: Option<&Path>,
    models: ModelFactory,
    mcp: mcp::Mcp,
) -> Result<Chat> {
    let environment = environment(&store, workspace, skills_dir).await?;
    let memory = memory::Memory::open(
        store.clone(),
        Compactor(models.clone()),
        memory::MemoryConfig::default(),
    )?;
    Ok(ChatHarness::new(
        environment,
        store.clone(),
        memory,
        EnvironmentPaths {
            workspace: workspace.to_path_buf(),
        },
        models,
        store.clone(),
    )
    .with_share(harness::ShareTools::new(
        store.clone(),
        store,
        pinned_workspace,
    ))
    .with_mcp(mcp))
}

pub async fn chat(
    store: Arc<SqliteStorage>,
    workspace: &Path,
    skills_dir: Option<&Path>,
    prompt: String,
) -> Result<()> {
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let models = ModelFactory::new(store.clone());
    models.current()?;
    // Surface "set BLOB_1_APP_KEY" instead of silently dropping MCP servers
    // whose secrets can't be decrypted.
    store.mcp_servers()?;
    let mcp = mcp::Mcp::new(store.clone());
    let mcp_shutdown = mcp.clone();
    std::fs::create_dir_all(workspace)?;
    let pinned_workspace = harness::PinnedWorkspace::open(workspace)?;
    let harness = chat_harness(store, workspace, pinned_workspace, skills_dir, models, mcp).await?;
    let mut failure = None;
    let signal_text = tokio::select! {
        answer = harness.run(&prompt) => {
            match answer {
                Ok(answer) => println!("{answer}"),
                Err(error) => failure = Some(error),
            }
            None
        },
        _ = interrupt.recv() => Some("interrupted"),
        _ = terminate.recv() => Some("terminated"),
    };
    let mut signal_text = signal_text;
    while signal_text.is_none() && (harness.tasks_running() || harness.has_work()) {
        signal_text = tokio::select! {
            answer = async {
                harness.work().await;
                harness.resume().await
            } => {
                match answer {
                    Ok(answer) if !answer.is_empty() => println!("{answer}"),
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("{error}");
                        failure.get_or_insert(error);
                    }
                }
                None
            },
            _ = interrupt.recv() => Some("interrupted"),
            _ = terminate.recv() => Some("terminated"),
        };
    }
    mcp_shutdown.shutdown().await;
    match signal_text {
        Some(signal) => Err(signal.into()),
        None => failure.map_or(Ok(()), |error| Err(error.into())),
    }
}

async fn decide_inline(
    utility: &harness::Utility,
    history: &[messenger::Fetched],
    inbound: &messenger::Inbound,
) -> bool {
    let history = history
        .iter()
        .map(|m| format!("{}: {}", m.author.name, m.text))
        .collect::<Vec<_>>()
        .join("\n");
    let quote = inbound.quote.as_ref().map_or("", |q| q.text.as_str());
    let attachments = inbound
        .attachments
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let prompt = format!(
        "Recent messages:\n{history}\nMessage: {}\nQuote: {quote}\nAttachments: {attachments}",
        inbound.text
    );
    utility.generate("Decide how a team chat bot should answer this message. Reply 'inline' for a quick question, greeting or short answer; reply 'thread' if it will need work, several messages, files or a longer discussion. Reply with one word.", &prompt).await.is_ok_and(|answer| harness::parse_inline(&answer))
}

/// Returns any explicit interaction override matching the message location and
/// author: thread override -> parent channel override -> guild-scoped author
/// override -> server override. `None` means the messenger default applies.
/// Shared by `admission_policy` and `pre_filter_decision` so they cannot drift.
fn explicit_interaction_mode(
    overrides: &[storage::InteractionOverride],
    channel: &str,
    thread: Option<&str>,
    guild: Option<&str>,
    author: &str,
) -> Option<storage::InteractionMode> {
    use storage::InteractionScope;
    let find = |scope: InteractionScope, guild_id: Option<&str>, target: &str| {
        overrides
            .iter()
            .find(|o| {
                o.scope == scope && o.guild_id.as_deref() == guild_id && o.target_id == target
            })
            .map(|o| o.mode)
    };
    if let Some(thread) = thread {
        if let Some(mode) = find(InteractionScope::Channel, None, thread) {
            return Some(mode);
        }
    }
    find(InteractionScope::Channel, None, channel)
        .or_else(|| guild.and_then(|g| find(InteractionScope::User, Some(g), author)))
        .or_else(|| guild.and_then(|g| find(InteractionScope::Server, None, g)))
}

/// The effective interaction mode for an inbound message: the matching
/// override, else the Discord-wide `default`.
#[cfg(test)]
fn effective_mode(
    overrides: &[storage::InteractionOverride],
    default: storage::InteractionMode,
    inbound: &messenger::Inbound,
) -> storage::InteractionMode {
    explicit_interaction_mode(
        overrides,
        &inbound.place.channel,
        inbound.place.thread.as_deref(),
        inbound.guild.as_deref(),
        &inbound.author.id,
    )
    .unwrap_or(default)
}

/// Evaluates whether `author` is permitted to talk to the bot under the active
/// access mode (`allow-all`, `whitelist`, or `blacklist`) and scoped entries
/// (`global`, `guild`, or `channel`/thread). DMs only match `global` entries.
fn access_allowed(
    mode: storage::AccessMode,
    entries: &[storage::AccessEntry],
    author: &str,
    guild: Option<&str>,
    channel: &str,
    thread: Option<&str>,
    dm: bool,
) -> bool {
    use storage::{AccessList, AccessMode, AccessScope};
    let matches_list = |list: AccessList| {
        entries.iter().any(|e| {
            e.list == list
                && e.user_id == author
                && match e.scope {
                    AccessScope::Global => true,
                    AccessScope::Guild => !dm && guild.is_some() && e.scope_id.as_deref() == guild,
                    AccessScope::Channel => {
                        !dm && (e.scope_id.as_deref() == Some(channel)
                            || (thread.is_some() && e.scope_id.as_deref() == thread))
                    }
                }
        })
    };
    match mode {
        AccessMode::AllowAll => true,
        AccessMode::Whitelist => matches_list(AccessList::Whitelist),
        AccessMode::Blacklist => !matches_list(AccessList::Blacklist),
    }
}

/// Evaluates the gateway pre-filter decision for a message given its access
/// policy, interaction overrides, Discord-wide `default_mode`, and known
/// guild/channel/thread identity (`thread == Some(t)` when `channel` is the
/// resolved parent channel; `thread == None` when `channel` is a known
/// top-level channel or an uncached channel/thread).
#[allow(clippy::too_many_arguments)]
fn pre_filter_decision(
    access_mode: storage::AccessMode,
    access_entries: &[storage::AccessEntry],
    overrides: &[storage::InteractionOverride],
    author: &str,
    guild: Option<&str>,
    channel: &str,
    thread: Option<&str>,
    addressed: bool,
    channel_known: bool,
    default_mode: storage::InteractionMode,
    auto_respond: bool,
) -> messenger_discord::PreFilterDecision {
    use messenger_discord::PreFilterDecision;
    use storage::{AccessList, AccessMode, AccessScope, InteractionMode, InteractionScope};
    let dm = guild.is_none();
    let access_decision = match access_mode {
        AccessMode::AllowAll => PreFilterDecision::Allow,
        AccessMode::Whitelist => {
            if access_allowed(
                access_mode,
                access_entries,
                author,
                guild,
                channel,
                thread,
                dm,
            ) {
                PreFilterDecision::Allow
            } else if !channel_known
                && !dm
                && access_entries.iter().any(|e| {
                    e.list == AccessList::Whitelist
                        && e.scope == AccessScope::Channel
                        && e.user_id == author
                })
            {
                PreFilterDecision::NeedParent
            } else {
                PreFilterDecision::Reject
            }
        }
        AccessMode::Blacklist => {
            if !access_allowed(
                access_mode,
                access_entries,
                author,
                guild,
                channel,
                thread,
                dm,
            ) {
                PreFilterDecision::Reject
            } else if !channel_known
                && !dm
                && access_entries.iter().any(|e| {
                    e.list == AccessList::Blacklist
                        && e.scope == AccessScope::Channel
                        && e.user_id == author
                })
            {
                PreFilterDecision::NeedParent
            } else {
                PreFilterDecision::Allow
            }
        }
    };
    if access_decision == PreFilterDecision::Reject {
        return PreFilterDecision::Reject;
    }
    let interaction_decision = if addressed {
        PreFilterDecision::Allow
    } else {
        let mode_allows = |mode: InteractionMode| match mode {
            // `auto_respond` is only true when a utility model exists and
            // Auto-respond is enabled; inert `auto` behaves like mention-only.
            InteractionMode::Always => true,
            InteractionMode::MentionOnly => false,
            InteractionMode::Auto => auto_respond,
        };
        let mode = explicit_interaction_mode(overrides, channel, thread, guild, author)
            .unwrap_or(default_mode);
        let allows = mode_allows(mode);
        let decided = if allows {
            PreFilterDecision::Allow
        } else {
            PreFilterDecision::Reject
        };
        if channel_known
            || overrides
                .iter()
                .any(|o| o.scope == InteractionScope::Channel && o.target_id == channel)
        {
            decided
        } else if overrides
            .iter()
            .any(|o| o.scope == InteractionScope::Channel && mode_allows(o.mode) != allows)
        {
            // An uncached id may be a thread inheriting a parent channel
            // override that would change the outcome; ask for the parent.
            PreFilterDecision::NeedParent
        } else {
            decided
        }
    };
    if interaction_decision == PreFilterDecision::Reject {
        return PreFilterDecision::Reject;
    }
    if access_decision == PreFilterDecision::NeedParent
        || interaction_decision == PreFilterDecision::NeedParent
    {
        PreFilterDecision::NeedParent
    } else {
        PreFilterDecision::Allow
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmissionPolicy {
    Deny,
    AdmitDirect,
    AutoRespondCheck,
}

/// Evaluates deterministic access + interaction admission policy for `inbound`.
/// Shared by `admit` and tests so policy checks cannot drift.
fn admission_policy(
    store: &SqliteStorage,
    inbound: &messenger::Inbound,
) -> Result<AdmissionPolicy> {
    let mode = store.access_mode("discord")?;
    let entries = store.access_entries("discord")?;
    if !access_allowed(
        mode,
        &entries,
        &inbound.author.id,
        inbound.guild.as_deref(),
        &inbound.place.channel,
        inbound.place.thread.as_deref(),
        inbound.place.dm,
    ) {
        return Ok(AdmissionPolicy::Deny);
    }
    if inbound.addressed {
        return Ok(AdmissionPolicy::AdmitDirect);
    }
    let defaults = store.messenger_defaults("discord")?;
    match explicit_interaction_mode(
        &store.interaction_modes("discord")?,
        &inbound.place.channel,
        inbound.place.thread.as_deref(),
        inbound.guild.as_deref(),
        &inbound.author.id,
    )
    .unwrap_or(defaults.interaction_mode)
    {
        storage::InteractionMode::Always => Ok(AdmissionPolicy::AdmitDirect),
        storage::InteractionMode::MentionOnly => Ok(AdmissionPolicy::Deny),
        storage::InteractionMode::Auto => Ok(AdmissionPolicy::AutoRespondCheck),
    }
}

/// RAII guard that releases a per-caller warming hold on drop (covering timeouts,
/// errors, and `IGNORE` exits) without disrupting overlapping callers on the same place.
struct WarmingHold {
    discord: Arc<dyn messenger::Messenger>,
    place: storage::Place,
    token: u64,
}

impl WarmingHold {
    fn acquire(discord: Arc<dyn messenger::Messenger>, place: storage::Place) -> Self {
        let token = discord.begin_warming(&place);
        Self {
            discord,
            place,
            token,
        }
    }
}

impl Drop for WarmingHold {
    fn drop(&mut self) {
        self.discord.cancel_warming(&self.place, self.token);
    }
}

fn filter_history_before(
    history: &[messenger::Fetched],
    message_id: &str,
    limit: usize,
) -> Vec<messenger::Fetched> {
    let cutoff = message_id.parse::<u64>().ok();
    let filtered: Vec<messenger::Fetched> = match cutoff {
        Some(cutoff_num) => history
            .iter()
            .filter(|m| m.message.parse::<u64>().ok().is_none_or(|n| n < cutoff_num))
            .cloned()
            .collect(),
        None => history
            .iter()
            .take_while(|m| m.message != message_id)
            .cloned()
            .collect(),
    };
    let start = filtered.len().saturating_sub(limit);
    filtered[start..].to_vec()
}

/// Admits one already-authorized message, resolving reply mode, re-verifying
/// policy after async awaits (before any side effects), then recording destination,
/// pending-thread state, seen reaction, and live typing independently.
async fn admit_approved(
    store: &Arc<SqliteStorage>,
    discord: &Arc<dyn messenger::Messenger>,
    messengers: &harness::Messengers,
    inbound: messenger::Inbound,
    utility: Option<&harness::Utility>,
    prefetched_history: Option<&[messenger::Fetched]>,
    auto_responded: bool,
) -> Result<Option<(String, storage::Meta)>> {
    let inline = if inbound.place.thread.is_none() && !inbound.place.dm {
        let default = store.messenger_defaults("discord")?.reply_mode;
        match storage::effective_reply_mode(
            &store.reply_modes("discord")?,
            &inbound.place.channel,
            inbound.guild.as_deref(),
            default,
        ) {
            storage::ReplyMode::Direct => true,
            storage::ReplyMode::Thread => false,
            storage::ReplyMode::Auto => {
                if let Some(utility) = utility {
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        let fetched;
                        let slice: &[messenger::Fetched] = match prefetched_history {
                            Some(h) => &h[h.len().saturating_sub(5)..],
                            None => {
                                if let Some(cached) = discord.recent_history(
                                    &inbound.place,
                                    Some(&inbound.message),
                                    5,
                                ) {
                                    fetched = cached;
                                } else if inbound.addressed {
                                    // Start warming before a live latest-20 REST fetch (no `before`
                                    // cursor) so all messages up to now—including edits, deletes,
                                    // and intervening bot messages—are covered with zero gap.
                                    let hold = WarmingHold::acquire(
                                        discord.clone(),
                                        inbound.place.clone(),
                                    );
                                    let latest =
                                        discord.history(&inbound.place, None, 20).await.ok()?;
                                    let complete_from_start = latest.len() < 20;
                                    discord.seed_history(
                                        &inbound.place,
                                        &latest,
                                        complete_from_start,
                                        hold.token,
                                    );
                                    let before =
                                        filter_history_before(&latest, &inbound.message, 5);
                                    fetched = if !complete_from_start && before.len() < 5 {
                                        discord
                                            .history(&inbound.place, Some(&inbound.message), 5)
                                            .await
                                            .ok()?
                                    } else {
                                        before
                                    };
                                } else {
                                    fetched = discord
                                        .history(&inbound.place, Some(&inbound.message), 5)
                                        .await
                                        .ok()?;
                                }
                                &fetched
                            }
                        };
                        Some(decide_inline(utility, slice, &inbound).await)
                    })
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(false)
                } else {
                    false
                }
            }
        }
    } else {
        false
    };
    // Recheck policy after async triage / reply-mode awaits before any side effects.
    match admission_policy(store, &inbound)? {
        AdmissionPolicy::Deny => return Ok(None),
        AdmissionPolicy::AutoRespondCheck if auto_responded => {
            if !store.auto_respond()? {
                return Ok(None);
            }
        }
        AdmissionPolicy::AutoRespondCheck => return Ok(None),
        AdmissionPolicy::AdmitDirect => {}
    }
    let pending_thread = !inline && inbound.place.thread.is_none() && !inbound.place.dm;
    let dest = if !pending_thread {
        inbound.place.clone()
    } else {
        storage::Place {
            channel: inbound.place.channel.clone(),
            thread: Some(inbound.message.clone()),
            dm: false,
        }
    };
    let to = store.destination("discord", &dest)?;
    if pending_thread {
        store.mark_pending_thread(to)?;
    } else if inbound.place.thread.is_some() {
        store.clear_pending_thread(to)?;
    }
    if inbound.addressed {
        let (discord_react, place, message) = (
            discord.clone(),
            inbound.place.clone(),
            inbound.message.clone(),
        );
        tokio::spawn(async move {
            if let Err(error) = discord_react
                .react(&place, &message, messenger::Mark::Seen)
                .await
            {
                eprintln!("{error}");
            }
        });
    }
    messengers.live("discord", to, dest);
    Ok(Some((
        inbound.text,
        storage::Meta::Message {
            messenger: "discord".into(),
            place: inbound.place,
            message: inbound.message,
            author: inbound.author,
            to,
            inline,
            // Auto-respond approvals already passed the utility-model gate, so
            // only explicit `always` overrides ask the main model to re-judge.
            unaddressed: !inbound.addressed && !auto_responded,
            location: inbound.location,
            earlier: inbound.earlier,
            quote: inbound.quote,
            attachments: inbound.attachments,
        },
    )))
}

#[cfg(test)]
async fn admit(
    store: &Arc<SqliteStorage>,
    discord: Arc<dyn messenger::Messenger>,
    messengers: &harness::Messengers,
    inbound: messenger::Inbound,
    utility: Option<&harness::Utility>,
) -> Result<Option<(String, storage::Meta)>> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    admit_batch(store, discord, messengers, vec![inbound], utility, &tx).await?;
    drop(tx);
    match rx.recv().await {
        Some(Ok(item)) => Ok(Some(item)),
        Some(Err(err)) => Err(err),
        None => Ok(None),
    }
}

/// Evaluates a burst of messages from one `Place`: re-checks policy on every
/// message, runs a single utility-model gate over unaddressed `AutoRespondCheck`
/// messages as a burst, and emits each approved message immediately in strict
/// arrival order.
async fn admit_batch(
    store: &Arc<SqliteStorage>,
    discord: Arc<dyn messenger::Messenger>,
    messengers: &harness::Messengers,
    batch: Vec<messenger::Inbound>,
    utility: Option<&harness::Utility>,
    tx: &tokio::sync::mpsc::Sender<Result<(String, storage::Meta)>>,
) -> Result<()> {
    let mut ordered: Vec<(messenger::Inbound, AdmissionPolicy)> = Vec::with_capacity(batch.len());
    let mut triage_snapshot: Vec<messenger::Inbound> = Vec::new();
    for inbound in batch {
        match admission_policy(store, &inbound)? {
            AdmissionPolicy::Deny => {}
            AdmissionPolicy::AdmitDirect => ordered.push((inbound, AdmissionPolicy::AdmitDirect)),
            AdmissionPolicy::AutoRespondCheck => {
                triage_snapshot.push(inbound.clone());
                ordered.push((inbound, AdmissionPolicy::AutoRespondCheck));
            }
        }
    }

    let mut triage_approved = false;
    // Reuse triage's prefetched history ONLY for the exact message whose cutoff it
    // was fetched for; other items in the burst read the live cache (or REST fallback).
    let mut triage_prefetched: Option<(String, Vec<messenger::Fetched>)> = None;
    if !triage_snapshot.is_empty() {
        let place = triage_snapshot[0].place.clone();
        let first_msg_id = triage_snapshot[0].message.clone();
        let auto_respond = store.auto_respond()?;
        if let Some(utility) = utility.filter(|_| auto_respond) {
            let identity = store
                .identity()?
                .or_else(|| Some(harness::DEFAULT_IDENTITY.into()));
            let bot_id = discord.self_id();
            let cached_history = discord.recent_history(&place, Some(&first_msg_id), 10);
            let warm_hold = cached_history
                .is_none()
                .then(|| WarmingHold::acquire(discord.clone(), place.clone()));
            let decision = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let (prompt_history, seed_snapshot, complete_from_start) = match cached_history {
                    Some(history) => (history, None, false),
                    None => {
                        // Fetch the latest 20 live messages (no `before` cursor) after
                        // starting `warm_hold` so edits, deletes, and intervening bot
                        // messages are captured accurately with zero gap.
                        let latest = discord.history(&place, None, 20).await.ok()?;
                        let complete = latest.len() < 20;
                        let before = filter_history_before(&latest, &first_msg_id, 10);
                        let before_first = if !complete && before.len() < 10 {
                            discord
                                .history(&place, Some(&first_msg_id), 10)
                                .await
                                .ok()?
                        } else {
                            before
                        };
                        (before_first, Some(latest), complete)
                    }
                };
                let respond = harness::decide_respond(
                    utility,
                    identity.as_deref(),
                    bot_id.as_deref(),
                    &prompt_history,
                    &triage_snapshot,
                )
                .await;
                Some((respond, prompt_history, seed_snapshot, complete_from_start))
            })
            .await;
            if let Ok(Some((true, prompt_history, seed_snapshot, complete_from_start))) = decision {
                match (seed_snapshot, &warm_hold) {
                    (Some(latest), Some(hold)) => {
                        // Seed solely from the fresh REST snapshot bound to `hold.token`
                        // (never re-inserting stale queued inbound payloads).
                        discord.seed_history(&place, &latest, complete_from_start, hold.token);
                    }
                    _ => {
                        discord.touch_history(&place);
                    }
                }
                triage_approved = true;
                triage_prefetched = Some((first_msg_id, prompt_history));
            }
            drop(warm_hold);
        }
    }

    for (inbound, policy) in ordered {
        match policy {
            AdmissionPolicy::AdmitDirect => {
                if let Some(item) =
                    admit_approved(store, &discord, messengers, inbound, utility, None, false)
                        .await?
                {
                    let _ = tx.send(Ok(item)).await;
                }
            }
            AdmissionPolicy::AutoRespondCheck if triage_approved => {
                let prefetched = triage_prefetched
                    .as_ref()
                    .and_then(|(id, h)| (*id == inbound.message).then_some(h.as_slice()));
                if let Some(item) = admit_approved(
                    store, &discord, messengers, inbound, utility, prefetched, true,
                )
                .await?
                {
                    let _ = tx.send(Ok(item)).await;
                }
            }
            AdmissionPolicy::AutoRespondCheck | AdmissionPolicy::Deny => {}
        }
    }
    Ok(())
}

pub async fn serve(
    store: Arc<SqliteStorage>,
    workspace: &Path,
    pinned_workspace: harness::PinnedWorkspace,
    skills_dir: Option<&Path>,
    discord: Arc<messenger_discord::Discord>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    mcp: mcp::Mcp,
    live_chat: LiveChat,
) -> Result<()> {
    let models = ModelFactory::new(store.clone());
    let harness = chat_harness(
        store.clone(),
        workspace,
        pinned_workspace.clone(),
        skills_dir,
        models.clone(),
        mcp.clone(),
    )
    .await?;
    let outgoing: Arc<dyn messenger::Messenger> = discord.clone();
    let messengers = harness::Messengers::new(vec![outgoing.clone()], store.clone())
        .with_workspace(pinned_workspace)
        .with_utility({
            let models = models.clone();
            move || models.current().ok().and_then(|current| current.utility)
        });
    let harness = Arc::new(
        harness
            .with_messengers(messengers.clone())
            .with_schedule_gate({
                let models = models.clone();
                let discord = discord.clone();
                move || {
                    models.current().is_ok()
                        && matches!(
                            discord.connection(),
                            messenger_discord::Connection::Online { .. }
                        )
                }
            }),
    );
    live_chat.set(Arc::clone(&harness), models.clone());
    // The cheap per-message gate the gateway runs before any Discord REST
    // call: evaluate the access policy and exact channel/thread/user
    // interaction overrides, asking for parent resolution only when an
    // uncached ID could inherit a parent channel rule.
    {
        let filter_store = store.clone();
        discord.set_pre_filter(Arc::new(
            move |author: &str,
                  guild: Option<&str>,
                  channel: &str,
                  thread: Option<&str>,
                  addressed: bool,
                  channel_known: bool| {
                use messenger_discord::PreFilterDecision;
                let access_mode = match filter_store.access_mode("discord") {
                    Ok(mode) => mode,
                    Err(error) => {
                        eprintln!("{error}");
                        return PreFilterDecision::Reject;
                    }
                };
                let access_entries = match filter_store.access_entries("discord") {
                    Ok(entries) => entries,
                    Err(error) => {
                        eprintln!("{error}");
                        return PreFilterDecision::Reject;
                    }
                };
                let overrides = match filter_store.interaction_modes("discord") {
                    Ok(overrides) => overrides,
                    Err(error) => {
                        eprintln!("{error}");
                        return PreFilterDecision::Reject;
                    }
                };
                let auto_respond_enabled = match filter_store.auto_respond() {
                    Ok(enabled) => enabled,
                    Err(error) => {
                        eprintln!("{error}");
                        false
                    }
                };
                let has_utility = match filter_store.utility_model() {
                    Ok(model) => model.is_some(),
                    Err(error) => {
                        eprintln!("{error}");
                        false
                    }
                };
                let default_mode = match filter_store.messenger_defaults("discord") {
                    Ok(defaults) => defaults.interaction_mode,
                    Err(error) => {
                        eprintln!("{error}");
                        return PreFilterDecision::Reject;
                    }
                };
                let auto_respond = auto_respond_enabled && has_utility;
                pre_filter_decision(
                    access_mode,
                    &access_entries,
                    &overrides,
                    author,
                    guild,
                    channel,
                    thread,
                    addressed,
                    channel_known,
                    default_mode,
                    auto_respond,
                )
            },
        ));
        let thread_store = store.clone();
        discord.set_on_thread_seen(Arc::new(move |parent: &str, thread: &str| {
            if let Err(error) =
                thread_store.clear_pending_thread_by_place("discord", parent, thread)
            {
                eprintln!("{error}");
            }
        }));
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let listener_discord = discord.clone();
    let listener = tokio::spawn(async move { listener_discord.listen(tx).await });
    crate::runtime::mark_ready();
    let supervisor = {
        let store = store.clone();
        let models = models.clone();
        let discord = discord.clone();
        let mut shutdown = shutdown.clone();
        tokio::spawn(async move {
            // Connects Discord whenever a bot token and a main model resolve,
            // and swaps or drops the connection live on token changes. A
            // gateway failure stays reported until the token changes.
            let mut connected: Option<String> = None;
            loop {
                let token = match store.messenger_token("discord") {
                    Ok(token) => token,
                    Err(error) => {
                        eprintln!("{error}");
                        None
                    }
                };
                match token {
                    Some(token) => {
                        if models.current().is_ok() {
                            let resaved = !discord.uses_token(&token)
                                && matches!(
                                    discord.connection(),
                                    messenger_discord::Connection::Connecting
                                );
                            if connected.as_deref() != Some(token.as_str()) || resaved {
                                discord.connect(token.clone());
                                connected = Some(token);
                            }
                        } else {
                            connected = None;
                            discord.error("Waiting for a main model");
                        }
                    }
                    None => {
                        connected = None;
                        discord.disconnect();
                    }
                }
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                    _ = shutdown.changed() => break,
                }
            }
        })
    };
    let (admitted_tx, mut admitted_rx) = tokio::sync::mpsc::channel(128);
    let turn_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let admission_store = store.clone();
    let admission_discord = discord.clone();
    let admission_messengers = messengers.clone();
    let admission_models = models.clone();
    let admission_turn_active = turn_active.clone();
    let admission = tokio::spawn(async move {
        const MAX_DIRECT_QUEUED: usize = 64;
        const MAX_TRIAGE_QUEUED: usize = 128;
        const MAX_BURST_MESSAGES: usize = 10;
        const DEBOUNCE_QUIET: std::time::Duration = std::time::Duration::from_millis(1500);
        const DEBOUNCE_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(6);

        struct DebounceBatch {
            messages: Vec<messenger::Inbound>,
            flush_at: tokio::time::Instant,
            deadline: tokio::time::Instant,
        }

        // Buffers a message in `debounce`; when a batch reaches `MAX_BURST_MESSAGES`,
        // seals it into `ready` (never discarding messages) so the next arrival starts
        // a new batch.
        fn push_debounced(
            debounce: &mut std::collections::HashMap<storage::Place, DebounceBatch>,
            ready: &mut std::collections::HashMap<
                storage::Place,
                std::collections::VecDeque<Vec<messenger::Inbound>>,
            >,
            inbound: messenger::Inbound,
            now: tokio::time::Instant,
        ) {
            let place = inbound.place.clone();
            // Seal an already-expired batch before appending a new arrival so
            // simultaneous inbound + timer readiness never stretches past `flush_at`.
            if debounce.get(&place).is_some_and(|b| now >= b.flush_at) {
                if let Some(expired) = debounce.remove(&place) {
                    ready
                        .entry(place.clone())
                        .or_default()
                        .push_back(expired.messages);
                }
            }
            let sealed = if let Some(batch) = debounce.get_mut(&place) {
                batch.messages.push(inbound);
                batch.flush_at = (now + DEBOUNCE_QUIET).min(batch.deadline);
                batch.messages.len() >= MAX_BURST_MESSAGES
            } else {
                let deadline = now + DEBOUNCE_MAX_WAIT;
                debounce.insert(
                    place.clone(),
                    DebounceBatch {
                        messages: vec![inbound],
                        flush_at: (now + DEBOUNCE_QUIET).min(deadline),
                        deadline,
                    },
                );
                false
            };
            if sealed {
                if let Some(batch) = debounce.remove(&place) {
                    ready.entry(place).or_default().push_back(batch.messages);
                }
            }
        }

        // Separate fast-lane for DMs, @mentions, replies, and explicit `always`
        // overrides: never stalled by unaddressed triage traffic.
        let direct_limit = Arc::new(tokio::sync::Semaphore::new(8));
        let mut direct_inflight: tokio::task::JoinSet<storage::Place> = tokio::task::JoinSet::new();
        let mut direct_active: std::collections::HashSet<storage::Place> =
            std::collections::HashSet::new();
        let mut direct_debounce: std::collections::HashMap<storage::Place, DebounceBatch> =
            std::collections::HashMap::new();
        let mut direct_ready: std::collections::HashMap<
            storage::Place,
            std::collections::VecDeque<Vec<messenger::Inbound>>,
        > = std::collections::HashMap::new();

        // Separate per-place coalescing lane for unaddressed Auto-respond triage:
        // runs 1 utility decision per sealed burst and admits each approved message
        // individually with its own destination and reply/to tokens.
        let triage_limit = Arc::new(tokio::sync::Semaphore::new(8));
        let mut triage_inflight: tokio::task::JoinSet<storage::Place> = tokio::task::JoinSet::new();
        let mut triage_active: std::collections::HashSet<storage::Place> =
            std::collections::HashSet::new();
        let mut triage_debounce: std::collections::HashMap<storage::Place, DebounceBatch> =
            std::collections::HashMap::new();
        let mut triage_ready: std::collections::HashMap<
            storage::Place,
            std::collections::VecDeque<Vec<messenger::Inbound>>,
        > = std::collections::HashMap::new();

        let spawn_lane = |inflight: &mut tokio::task::JoinSet<storage::Place>,
                          active: &mut std::collections::HashSet<storage::Place>,
                          limit: &Arc<tokio::sync::Semaphore>,
                          batch: Vec<messenger::Inbound>| {
            let place = batch[0].place.clone();
            active.insert(place.clone());
            let limit = limit.clone();
            let store = admission_store.clone();
            let discord = admission_discord.clone();
            let messengers = admission_messengers.clone();
            let utility = admission_models
                .current()
                .ok()
                .and_then(|current| current.utility);
            let tx = admitted_tx.clone();
            inflight.spawn(async move {
                let Ok(_permit) = limit.acquire_owned().await else {
                    return place;
                };
                if let Err(error) =
                    admit_batch(&store, discord, &messengers, batch, utility.as_ref(), &tx).await
                {
                    let _ = tx.send(Err(error)).await;
                }
                place
            });
        };

        let dispatch_ready = |inflight: &mut tokio::task::JoinSet<storage::Place>,
                              active: &mut std::collections::HashSet<storage::Place>,
                              ready: &mut std::collections::HashMap<
            storage::Place,
            std::collections::VecDeque<Vec<messenger::Inbound>>,
        >,
                              limit: &Arc<tokio::sync::Semaphore>| {
            let places: Vec<storage::Place> = ready
                .keys()
                .filter(|p| !active.contains(*p))
                .cloned()
                .collect();
            for place in places {
                if let Some(queue) = ready.get_mut(&place) {
                    if let Some(batch) = queue.pop_front() {
                        spawn_lane(inflight, active, limit, batch);
                    }
                    if queue.is_empty() {
                        ready.remove(&place);
                    }
                }
            }
        };

        let queued_count = |inflight: &tokio::task::JoinSet<storage::Place>,
                            debounce: &std::collections::HashMap<storage::Place, DebounceBatch>,
                            ready: &std::collections::HashMap<
            storage::Place,
            std::collections::VecDeque<Vec<messenger::Inbound>>,
        >| {
            inflight.len()
                + debounce.len()
                + ready
                    .values()
                    .map(std::collections::VecDeque::len)
                    .sum::<usize>()
        };

        let next_flush_at = |direct: &std::collections::HashMap<storage::Place, DebounceBatch>,
                             triage: &std::collections::HashMap<storage::Place, DebounceBatch>|
         -> Option<tokio::time::Instant> {
            let d = direct.values().map(|b| b.flush_at).min();
            let t = triage.values().map(|b| b.flush_at).min();
            match (d, t) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        };

        loop {
            let wake_at = next_flush_at(&direct_debounce, &triage_debounce);
            // Only direct-lane saturation pauses `rx.recv()`; a flooded triage
            // queue never stalls DMs, @mentions, replies, or `always` traffic.
            let can_recv =
                queued_count(&direct_inflight, &direct_debounce, &direct_ready) < MAX_DIRECT_QUEUED;
            tokio::select! {
                inbound = rx.recv(), if can_recv => match inbound {
                    Some(inbound) => {
                        let now = tokio::time::Instant::now();
                        let turn_busy = admission_turn_active
                            .load(std::sync::atomic::Ordering::Relaxed);
                        match admission_policy(&admission_store, &inbound) {
                            Ok(AdmissionPolicy::Deny) => {}
                            Ok(AdmissionPolicy::AdmitDirect) => {
                                let should_debounce = turn_busy
                                    || direct_active.contains(&inbound.place)
                                    || direct_debounce.contains_key(&inbound.place)
                                    || direct_ready.contains_key(&inbound.place);
                                if should_debounce {
                                    push_debounced(
                                        &mut direct_debounce,
                                        &mut direct_ready,
                                        inbound,
                                        now,
                                    );
                                    dispatch_ready(
                                        &mut direct_inflight,
                                        &mut direct_active,
                                        &mut direct_ready,
                                        &direct_limit,
                                    );
                                } else {
                                    spawn_lane(
                                        &mut direct_inflight,
                                        &mut direct_active,
                                        &direct_limit,
                                        vec![inbound],
                                    );
                                }
                            }
                            Ok(AdmissionPolicy::AutoRespondCheck) => {
                                let triage_full = queued_count(
                                    &triage_inflight,
                                    &triage_debounce,
                                    &triage_ready,
                                ) >= MAX_TRIAGE_QUEUED;
                                // Allow coalescing into an existing debouncing batch when
                                // triage is saturated, but shed excess new unaddressed work
                                // rather than stalling the direct lane.
                                if triage_full && !triage_debounce.contains_key(&inbound.place) {
                                    continue;
                                }
                                let should_debounce = turn_busy
                                    || triage_active.contains(&inbound.place)
                                    || triage_debounce.contains_key(&inbound.place)
                                    || triage_ready.contains_key(&inbound.place);
                                if should_debounce {
                                    push_debounced(
                                        &mut triage_debounce,
                                        &mut triage_ready,
                                        inbound,
                                        now,
                                    );
                                    dispatch_ready(
                                        &mut triage_inflight,
                                        &mut triage_active,
                                        &mut triage_ready,
                                        &triage_limit,
                                    );
                                } else {
                                    spawn_lane(
                                        &mut triage_inflight,
                                        &mut triage_active,
                                        &triage_limit,
                                        vec![inbound],
                                    );
                                }
                            }
                            Err(error) => {
                                let _ = admitted_tx.send(Err(error)).await;
                            }
                        }
                    }
                    None => {
                        for (place, batch) in direct_debounce.drain() {
                            direct_ready.entry(place).or_default().push_back(batch.messages);
                        }
                        for (place, batch) in triage_debounce.drain() {
                            triage_ready.entry(place).or_default().push_back(batch.messages);
                        }
                        dispatch_ready(
                            &mut direct_inflight,
                            &mut direct_active,
                            &mut direct_ready,
                            &direct_limit,
                        );
                        dispatch_ready(
                            &mut triage_inflight,
                            &mut triage_active,
                            &mut triage_ready,
                            &triage_limit,
                        );
                        while !direct_inflight.is_empty() || !triage_inflight.is_empty() {
                            tokio::select! {
                                Some(done) = direct_inflight.join_next(), if !direct_inflight.is_empty() => {
                                    if let Ok(place) = done {
                                        direct_active.remove(&place);
                                        dispatch_ready(
                                            &mut direct_inflight,
                                            &mut direct_active,
                                            &mut direct_ready,
                                            &direct_limit,
                                        );
                                    }
                                }
                                Some(done) = triage_inflight.join_next(), if !triage_inflight.is_empty() => {
                                    if let Ok(place) = done {
                                        triage_active.remove(&place);
                                        dispatch_ready(
                                            &mut triage_inflight,
                                            &mut triage_active,
                                            &mut triage_ready,
                                            &triage_limit,
                                        );
                                    }
                                }
                            }
                        }
                        return;
                    }
                },
                _ = async {
                    if let Some(at) = wake_at {
                        tokio::time::sleep_until(at).await;
                    }
                }, if wake_at.is_some() => {
                    let now = tokio::time::Instant::now();
                    let expired_direct: Vec<storage::Place> = direct_debounce
                        .iter()
                        .filter(|(_, b)| b.flush_at <= now)
                        .map(|(p, _)| p.clone())
                        .collect();
                    for place in expired_direct {
                        if let Some(batch) = direct_debounce.remove(&place) {
                            direct_ready.entry(place).or_default().push_back(batch.messages);
                        }
                    }
                    dispatch_ready(
                        &mut direct_inflight,
                        &mut direct_active,
                        &mut direct_ready,
                        &direct_limit,
                    );
                    let expired_triage: Vec<storage::Place> = triage_debounce
                        .iter()
                        .filter(|(_, b)| b.flush_at <= now)
                        .map(|(p, _)| p.clone())
                        .collect();
                    for place in expired_triage {
                        if let Some(batch) = triage_debounce.remove(&place) {
                            triage_ready.entry(place).or_default().push_back(batch.messages);
                        }
                    }
                    dispatch_ready(
                        &mut triage_inflight,
                        &mut triage_active,
                        &mut triage_ready,
                        &triage_limit,
                    );
                }
                Some(done) = direct_inflight.join_next(), if !direct_inflight.is_empty() => {
                    if let Ok(place) = done {
                        direct_active.remove(&place);
                        dispatch_ready(
                            &mut direct_inflight,
                            &mut direct_active,
                            &mut direct_ready,
                            &direct_limit,
                        );
                    }
                }
                Some(done) = triage_inflight.join_next(), if !triage_inflight.is_empty() => {
                    if let Ok(place) = done {
                        triage_active.remove(&place);
                        dispatch_ready(
                            &mut triage_inflight,
                            &mut triage_active,
                            &mut triage_ready,
                            &triage_limit,
                        );
                    }
                }
            }
        }
    });
    let mut pending = std::collections::VecDeque::<(String, Option<storage::Meta>)>::new();
    let mut backoff = false;
    tokio::select! {
        result = async {
    loop {
        let next = if let Some(next) = pending.pop_front() {
            Some(next)
        } else {
            tokio::select! {
                value = admitted_rx.recv() => match value {
                    Some(next) => { let (text, meta) = next?; Some((text, Some(meta))) },
                    None => return match listener.await? { Err(error) => Err(error.into()), Ok(()) => Err("discord listener stopped".into()) },
                },
                _ = async {
                    if backoff {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                    harness.work().await
                } => None,
                _ = shutdown.changed() => { listener.abort(); admission.abort(); supervisor.abort(); return Ok(()); },
            }
        };
        turn_active.store(true, std::sync::atomic::Ordering::Relaxed);
        let run = async {
            match next {
                Some((text, meta)) => harness.run_message(text, meta).await,
                None => harness.resume().await,
            }
        };
        tokio::pin!(run);
        while let Some((text, meta)) = pending.pop_front() {
            harness.steer_message(text, meta);
        }
        loop {
            tokio::select! {
                result = &mut run => {
                    turn_active.store(false, std::sync::atomic::Ordering::Relaxed);
                    backoff = result.is_err();
                    match result {
                        Ok(answer) if !answer.is_empty() => println!("{answer}"),
                        Ok(_) => {}
                        Err(error) => eprintln!("{error}"),
                    }
                    break;
                },
                value = admitted_rx.recv() => match value {
                    Some(next) => { let (text, meta) = next?; harness.steer_message(text, Some(meta)); },
                    None => return match listener.await? { Err(error) => Err(error.into()), Ok(()) => Err("discord listener stopped".into()) },
                },
                _ = shutdown.changed() => { listener.abort(); admission.abort(); supervisor.abort(); return Ok(()); },
            }
        }
        messengers.quiet();
        for (text, meta) in harness.take_steering().into_iter().rev() {
            pending.push_front((text, meta));
        }
        for (_, meta) in &pending {
            if let Some(storage::Meta::Message { messenger, to, .. }) = meta {
                let (_, destination) = store.place(*to)?.ok_or("unknown destination")?;
                messengers.live(messenger, *to, destination);
            }
        }
    }
        } => result,
        _ = harness.run_schedules() => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage::{InteractionMode, InteractionOverride, InteractionScope, Person, Place};

    fn channel_override(target: &str, mode: InteractionMode) -> InteractionOverride {
        InteractionOverride {
            scope: InteractionScope::Channel,
            guild_id: None,
            target_id: target.into(),
            mode,
        }
    }

    fn user_override(guild: &str, target: &str, mode: InteractionMode) -> InteractionOverride {
        InteractionOverride {
            scope: InteractionScope::User,
            guild_id: Some(guild.into()),
            target_id: target.into(),
            mode,
        }
    }

    fn server_override(guild: &str, mode: InteractionMode) -> InteractionOverride {
        InteractionOverride {
            scope: InteractionScope::Server,
            guild_id: None,
            target_id: guild.into(),
            mode,
        }
    }

    fn inbound(
        place: Place,
        guild: Option<&str>,
        author: &str,
        addressed: bool,
    ) -> messenger::Inbound {
        messenger::Inbound {
            place,
            guild: guild.map(Into::into),
            message: "m1".into(),
            author: Person {
                id: author.into(),
                name: author.into(),
            },
            text: "hi".into(),
            quote: None,
            attachments: Vec::new(),
            location: "#c".into(),
            earlier: None,
            addressed,
        }
    }

    fn channel(channel: &str, thread: Option<&str>) -> Place {
        Place {
            channel: channel.into(),
            thread: thread.map(Into::into),
            dm: false,
        }
    }

    #[test]
    fn mode_precedence() {
        // Channel overrides beat guild-scoped user overrides, which beat
        // server overrides; with no override the messenger default applies.
        let overrides = vec![
            channel_override("c1", InteractionMode::Always),
            user_override("g1", "u1", InteractionMode::MentionOnly),
            user_override("g2", "u1", InteractionMode::Always),
            server_override("g3", InteractionMode::Always),
        ];
        assert_eq!(
            effective_mode(
                &overrides,
                InteractionMode::Auto,
                &inbound(channel("c1", None), Some("g1"), "u1", false)
            ),
            InteractionMode::Always
        );
        assert_eq!(
            effective_mode(
                &overrides,
                InteractionMode::Auto,
                &inbound(channel("c2", None), Some("g1"), "u1", false)
            ),
            InteractionMode::MentionOnly
        );
        // Same user in guild g2 gets g2's always override, and in g3 the server
        // override kicks in below the missing user override.
        assert_eq!(
            effective_mode(
                &overrides,
                InteractionMode::Auto,
                &inbound(channel("c2", None), Some("g2"), "u1", false)
            ),
            InteractionMode::Always
        );
        assert_eq!(
            effective_mode(
                &overrides,
                InteractionMode::MentionOnly,
                &inbound(channel("c2", None), Some("g3"), "u1", false)
            ),
            InteractionMode::Always
        );
        assert_eq!(
            effective_mode(
                &overrides,
                InteractionMode::MentionOnly,
                &inbound(channel("c2", None), Some("g3"), "u2", false)
            ),
            InteractionMode::Always
        );
        // A user override still wins over the server override in g3.
        let overrides = vec![
            user_override("g3", "u1", InteractionMode::MentionOnly),
            server_override("g3", InteractionMode::Always),
        ];
        assert_eq!(
            effective_mode(
                &overrides,
                InteractionMode::Always,
                &inbound(channel("c2", None), Some("g3"), "u1", false)
            ),
            InteractionMode::MentionOnly
        );
        // No override anywhere falls back to the messenger default, whatever it is.
        assert_eq!(
            effective_mode(
                &[],
                InteractionMode::MentionOnly,
                &inbound(channel("c2", None), Some("g1"), "u2", false)
            ),
            InteractionMode::MentionOnly
        );
        assert_eq!(
            effective_mode(
                &[],
                InteractionMode::Always,
                &inbound(channel("c2", None), Some("g1"), "u2", false)
            ),
            InteractionMode::Always
        );
        assert_eq!(
            effective_mode(
                &[],
                InteractionMode::Auto,
                &inbound(channel("c2", None), Some("g1"), "u2", false)
            ),
            InteractionMode::Auto
        );

        // A thread-level override wins over its parent channel and the guild user;
        // a thread without one inherits the parent channel's.
        let overrides = vec![
            channel_override("c1", InteractionMode::Always),
            channel_override("t1", InteractionMode::MentionOnly),
            user_override("g1", "u1", InteractionMode::Always),
        ];
        assert_eq!(
            effective_mode(
                &overrides,
                InteractionMode::Auto,
                &inbound(channel("c1", Some("t1")), Some("g1"), "u1", false)
            ),
            InteractionMode::MentionOnly
        );
        assert_eq!(
            effective_mode(
                &overrides,
                InteractionMode::Auto,
                &inbound(channel("c1", Some("t2")), Some("g1"), "u2", false)
            ),
            InteractionMode::Always
        );
    }

    #[test]
    fn pre_filter_avoids_rest_for_unrelated_channels() {
        use messenger_discord::PreFilterDecision;
        use storage::{AccessEntry, AccessList, AccessMode, AccessScope};
        let access = vec![
            AccessEntry {
                list: AccessList::Whitelist,
                scope: AccessScope::Global,
                scope_id: None,
                user_id: "u1".into(),
            },
            AccessEntry {
                list: AccessList::Whitelist,
                scope: AccessScope::Guild,
                scope_id: Some("g1".into()),
                user_id: "u2".into(),
            },
            AccessEntry {
                list: AccessList::Whitelist,
                scope: AccessScope::Channel,
                scope_id: Some("c1".into()),
                user_id: "u3".into(),
            },
        ];
        let overrides = vec![
            channel_override("c1", InteractionMode::Always),
            channel_override("t1", InteractionMode::MentionOnly),
            user_override("g1", "u1", InteractionMode::Always),
        ];
        // User override applies in g1, not in g2.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &overrides,
                "u1",
                Some("g1"),
                "c2",
                None,
                false,
                true,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::Allow
        );
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &overrides,
                "u1",
                Some("g2"),
                "c2",
                None,
                false,
                true,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::Reject
        );
        // With auto_respond enabled, unaddressed messages without overrides pass to the shared gate.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &overrides,
                "u1",
                Some("g2"),
                "c2",
                None,
                false,
                true,
                InteractionMode::Auto,
                true
            ),
            PreFilterDecision::Allow
        );
        // Explicit mention-only thread still rejects even when auto_respond is on.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &overrides,
                "u1",
                Some("g2"),
                "t1",
                None,
                false,
                false,
                InteractionMode::Auto,
                true
            ),
            PreFilterDecision::Reject
        );
        // With auto_respond on and an uncached thread, a mention-only channel override triggers NeedParent.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &overrides,
                "u1",
                Some("g2"),
                "t_unknown",
                None,
                false,
                false,
                InteractionMode::Auto,
                true
            ),
            PreFilterDecision::NeedParent
        );
        // Thread explicitly set to mention-only rejects even before parent lookup.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &overrides,
                "u1",
                Some("g1"),
                "t1",
                None,
                false,
                false,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::Reject
        );
        // Known thread under c1 allows directly for guild-whitelisted u2.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &overrides,
                "u2",
                Some("g1"),
                "c1",
                Some("t2"),
                false,
                true,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::Allow
        );
        // Unknown channel id asks to resolve parent once because c1 is always.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &overrides,
                "u2",
                Some("g1"),
                "t2",
                None,
                false,
                false,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::NeedParent
        );
        // Channel-whitelisted u3 in an uncached thread asks for parent even when addressed.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &[],
                "u3",
                Some("g1"),
                "t2",
                None,
                true,
                false,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::NeedParent
        );
        // Non-whitelisted u9 rejects immediately with no REST.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &overrides,
                "u9",
                Some("g1"),
                "t2",
                None,
                true,
                false,
                InteractionMode::Auto,
                true
            ),
            PreFilterDecision::Reject
        );
        // Blacklist mode: channel-blacklisted user in uncached thread resolves parent once.
        let bl = vec![AccessEntry {
            list: AccessList::Blacklist,
            scope: AccessScope::Channel,
            scope_id: Some("c1".into()),
            user_id: "u4".into(),
        }];
        assert_eq!(
            pre_filter_decision(
                AccessMode::Blacklist,
                &bl,
                &[],
                "u4",
                Some("g1"),
                "t2",
                None,
                true,
                false,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::NeedParent
        );
        assert_eq!(
            pre_filter_decision(
                AccessMode::Blacklist,
                &bl,
                &[],
                "u4",
                Some("g1"),
                "c1",
                Some("t2"),
                true,
                true,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::Reject
        );

        // A server override covers known channels in its guild without REST.
        let with_server = vec![server_override("g1", InteractionMode::Always)];
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &with_server,
                "u1",
                Some("g1"),
                "c9",
                None,
                false,
                true,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::Allow
        );
        // Server mention-only rejects even when Auto-respond is available.
        let with_server = vec![server_override("g1", InteractionMode::MentionOnly)];
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &with_server,
                "u1",
                Some("g1"),
                "c9",
                None,
                false,
                true,
                InteractionMode::Auto,
                true
            ),
            PreFilterDecision::Reject
        );
        // An `always` default admits unaddressed messages with no overrides.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &[],
                "u1",
                Some("g2"),
                "c9",
                None,
                false,
                true,
                InteractionMode::Always,
                false
            ),
            PreFilterDecision::Allow
        );
        // An `auto` default without a utility gate behaves like mention-only.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &[],
                "u1",
                Some("g2"),
                "c9",
                None,
                false,
                true,
                InteractionMode::Auto,
                false
            ),
            PreFilterDecision::Reject
        );
        // A mention-only default rejects even when Auto-respond is available.
        assert_eq!(
            pre_filter_decision(
                AccessMode::Whitelist,
                &access,
                &[],
                "u1",
                Some("g2"),
                "c9",
                None,
                false,
                true,
                InteractionMode::MentionOnly,
                true
            ),
            PreFilterDecision::Reject
        );
    }

    #[test]
    fn access_modes_and_scopes() {
        use storage::{AccessEntry, AccessList, AccessMode, AccessScope};
        let entries = vec![
            AccessEntry {
                list: AccessList::Whitelist,
                scope: AccessScope::Global,
                scope_id: None,
                user_id: "u_global".into(),
            },
            AccessEntry {
                list: AccessList::Whitelist,
                scope: AccessScope::Guild,
                scope_id: Some("g1".into()),
                user_id: "u_guild".into(),
            },
            AccessEntry {
                list: AccessList::Whitelist,
                scope: AccessScope::Channel,
                scope_id: Some("c1".into()),
                user_id: "u_chan".into(),
            },
            AccessEntry {
                list: AccessList::Blacklist,
                scope: AccessScope::Global,
                scope_id: None,
                user_id: "b_global".into(),
            },
            AccessEntry {
                list: AccessList::Blacklist,
                scope: AccessScope::Guild,
                scope_id: Some("g1".into()),
                user_id: "b_guild".into(),
            },
            AccessEntry {
                list: AccessList::Blacklist,
                scope: AccessScope::Channel,
                scope_id: Some("c1".into()),
                user_id: "b_chan".into(),
            },
        ];
        // AllowAll admits anyone everywhere.
        assert!(access_allowed(
            AccessMode::AllowAll,
            &entries,
            "anyone",
            Some("g9"),
            "c9",
            None,
            false
        ));
        assert!(access_allowed(
            AccessMode::AllowAll,
            &entries,
            "b_global",
            None,
            "dm1",
            None,
            true
        ));

        // Whitelist: global works everywhere + DM; guild only in g1 (not DM); channel only in c1/threads (not DM).
        assert!(access_allowed(
            AccessMode::Whitelist,
            &entries,
            "u_global",
            None,
            "dm1",
            None,
            true
        ));
        assert!(access_allowed(
            AccessMode::Whitelist,
            &entries,
            "u_guild",
            Some("g1"),
            "c9",
            None,
            false
        ));
        assert!(!access_allowed(
            AccessMode::Whitelist,
            &entries,
            "u_guild",
            Some("g2"),
            "c9",
            None,
            false
        ));
        assert!(!access_allowed(
            AccessMode::Whitelist,
            &entries,
            "u_guild",
            None,
            "dm1",
            None,
            true
        ));
        assert!(access_allowed(
            AccessMode::Whitelist,
            &entries,
            "u_chan",
            Some("g1"),
            "c1",
            Some("t1"),
            false
        ));
        assert!(!access_allowed(
            AccessMode::Whitelist,
            &entries,
            "u_chan",
            Some("g1"),
            "c2",
            None,
            false
        ));

        // Blacklist: everyone allowed except matched blacklist entries.
        assert!(access_allowed(
            AccessMode::Blacklist,
            &entries,
            "anyone",
            Some("g1"),
            "c1",
            None,
            false
        ));
        assert!(!access_allowed(
            AccessMode::Blacklist,
            &entries,
            "b_global",
            None,
            "dm1",
            None,
            true
        ));
        assert!(!access_allowed(
            AccessMode::Blacklist,
            &entries,
            "b_guild",
            Some("g1"),
            "c2",
            None,
            false
        ));
        assert!(access_allowed(
            AccessMode::Blacklist,
            &entries,
            "b_guild",
            Some("g2"),
            "c2",
            None,
            false
        ));
        assert!(access_allowed(
            AccessMode::Blacklist,
            &entries,
            "b_guild",
            None,
            "dm1",
            None,
            true
        ));
        assert!(!access_allowed(
            AccessMode::Blacklist,
            &entries,
            "b_chan",
            Some("g1"),
            "c1",
            Some("t1"),
            false
        ));
        assert!(access_allowed(
            AccessMode::Blacklist,
            &entries,
            "b_chan",
            Some("g1"),
            "c2",
            None,
            false
        ));
    }

    #[test]
    fn admission() {
        let dir = std::env::temp_dir().join(format!("blob-1-run-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();
        store.allow_user("discord", "u1").unwrap();

        // With no overrides, addressed messages admit directly and unaddressed go to AutoRespondCheck.
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c1", None), Some("g1"), "u1", true)
            )
            .unwrap(),
            AdmissionPolicy::AdmitDirect
        );
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c1", None), Some("g1"), "u1", false)
            )
            .unwrap(),
            AdmissionPolicy::AutoRespondCheck
        );
        // DMs count as addressed.
        let dm = Place {
            channel: "dm1".into(),
            thread: None,
            dm: true,
        };
        assert_eq!(
            admission_policy(&store, &inbound(dm, None, "u1", true)).unwrap(),
            AdmissionPolicy::AdmitDirect
        );

        // An `always` channel override admits unaddressed messages directly, while the
        // allowlist still denies unallowlisted authors.
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Channel,
                None,
                "c1",
                InteractionMode::Always,
            )
            .unwrap();
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c1", None), Some("g1"), "u1", false)
            )
            .unwrap(),
            AdmissionPolicy::AdmitDirect
        );
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c1", None), Some("g1"), "u9", false)
            )
            .unwrap(),
            AdmissionPolicy::Deny
        );

        // A mention-only thread override wins over the channel's `always`.
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Channel,
                None,
                "t1",
                InteractionMode::MentionOnly,
            )
            .unwrap();
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c1", Some("t1")), Some("g1"), "u1", false)
            )
            .unwrap(),
            AdmissionPolicy::Deny
        );

        // A guild-scoped `always` user override admits the user directly in that guild
        // only, not in other guilds.
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::User,
                Some("g1"),
                "u1",
                InteractionMode::Always,
            )
            .unwrap();
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c9", None), Some("g1"), "u1", false)
            )
            .unwrap(),
            AdmissionPolicy::AdmitDirect
        );
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c9", None), Some("g2"), "u1", false)
            )
            .unwrap(),
            AdmissionPolicy::AutoRespondCheck
        );

        // A server override covers every channel and user in that guild, below
        // channel/user overrides but above the Discord-wide default.
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Server,
                None,
                "g2",
                InteractionMode::Always,
            )
            .unwrap();
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Server,
                None,
                "g3",
                InteractionMode::MentionOnly,
            )
            .unwrap();
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c9", None), Some("g2"), "u1", false)
            )
            .unwrap(),
            AdmissionPolicy::AdmitDirect
        );
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c9", None), Some("g3"), "u1", false)
            )
            .unwrap(),
            AdmissionPolicy::Deny
        );

        // With no override matching, the Discord-wide default decides: auto
        // runs triage, mention-only denies, always admits directly.
        store
            .set_default_interaction_mode("discord", InteractionMode::MentionOnly)
            .unwrap();
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c2", None), Some("g9"), "u1", false)
            )
            .unwrap(),
            AdmissionPolicy::Deny
        );
        store
            .set_default_interaction_mode("discord", InteractionMode::Always)
            .unwrap();
        assert_eq!(
            admission_policy(
                &store,
                &inbound(channel("c2", None), Some("g9"), "u1", false)
            )
            .unwrap(),
            AdmissionPolicy::AdmitDirect
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reply_mode_resolution() {
        use storage::{effective_reply_mode, ReplyMode, ReplyOverride, ReplyScope};
        let overrides = vec![
            ReplyOverride {
                scope: ReplyScope::Channel,
                target_id: "c1".into(),
                mode: ReplyMode::Direct,
            },
            ReplyOverride {
                scope: ReplyScope::Channel,
                target_id: "c2".into(),
                mode: ReplyMode::Thread,
            },
            ReplyOverride {
                scope: ReplyScope::Channel,
                target_id: "c3".into(),
                mode: ReplyMode::Auto,
            },
            ReplyOverride {
                scope: ReplyScope::Server,
                target_id: "g1".into(),
                mode: ReplyMode::Direct,
            },
        ];
        // Channel overrides win over server overrides and the default; a
        // server override applies only to messages in that guild, and
        // everything else falls back to the messenger default.
        assert_eq!(
            effective_reply_mode(&overrides, "c1", Some("g1"), ReplyMode::Auto),
            ReplyMode::Direct
        );
        assert_eq!(
            effective_reply_mode(&overrides, "c2", Some("g1"), ReplyMode::Auto),
            ReplyMode::Thread
        );
        assert_eq!(
            effective_reply_mode(&overrides, "c3", Some("g9"), ReplyMode::Thread),
            ReplyMode::Auto
        );
        assert_eq!(
            effective_reply_mode(&overrides, "c9", Some("g1"), ReplyMode::Thread),
            ReplyMode::Direct
        );
        assert_eq!(
            effective_reply_mode(&overrides, "c9", Some("g9"), ReplyMode::Thread),
            ReplyMode::Thread
        );
        assert_eq!(
            effective_reply_mode(&overrides, "c9", None, ReplyMode::Direct),
            ReplyMode::Direct
        );
        assert_eq!(
            effective_reply_mode(&overrides, "c9", None, ReplyMode::Auto),
            ReplyMode::Auto
        );
    }

    #[tokio::test]
    async fn admit_auto_respond_gate() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use storage::{ConfigStore, InteractionMode, InteractionScope, MessengerStore, Place};

        struct StubLlm {
            reply: &'static str,
            calls: Arc<AtomicUsize>,
            last_prompt: Arc<Mutex<String>>,
        }
        #[async_trait::async_trait]
        impl harness::Llm for StubLlm {
            async fn complete(
                &self,
                request: harness::CompletionRequest<'_>,
            ) -> std::result::Result<harness::CompletionResponse, harness::LlmError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if let Some(storage::Message::User(text)) = request.messages.first() {
                    *self.last_prompt.lock().unwrap() = text.clone();
                }
                Ok(harness::CompletionResponse {
                    text: self.reply.into(),
                    tool_calls: Vec::new(),
                    usage: None,
                })
            }
        }

        struct StubMessenger {
            fail_history: bool,
        }
        #[async_trait::async_trait]
        impl messenger::Lookup for StubMessenger {
            fn name(&self) -> &'static str {
                "discord"
            }
            fn self_id(&self) -> Option<String> {
                Some("bot42".into())
            }
            async fn message(
                &self,
                _place: &Place,
                _id: &str,
            ) -> std::result::Result<messenger::Fetched, messenger::MessengerError> {
                Err(messenger::MessengerError::Unsupported("stub".into()))
            }
            fn parse_link(&self, _link: &str) -> Option<(Place, String)> {
                None
            }
            async fn history(
                &self,
                _place: &Place,
                _before: Option<&str>,
                _limit: u32,
            ) -> std::result::Result<Vec<messenger::Fetched>, messenger::MessengerError>
            {
                if self.fail_history {
                    return Err(messenger::MessengerError::Unsupported(
                        "history failed".into(),
                    ));
                }
                Ok(vec![messenger::Fetched {
                    message: "m0".into(),
                    author: storage::Person {
                        id: "bot42".into(),
                        name: "SupportBot".into(),
                    },
                    text: "Try restarting the service.".into(),
                    date: "2026-11-22".into(),
                    attachments: Vec::new(),
                }])
            }
            async fn search(
                &self,
                _scope: &messenger::Scope,
                _query: &str,
                _limit: u32,
            ) -> std::result::Result<Vec<messenger::Fetched>, messenger::MessengerError>
            {
                Ok(Vec::new())
            }
            async fn participants(
                &self,
                _scope: &messenger::Scope,
                _query: Option<&str>,
            ) -> std::result::Result<Vec<storage::Person>, messenger::MessengerError> {
                Ok(Vec::new())
            }
        }
        #[async_trait::async_trait]
        impl messenger::Messenger for StubMessenger {
            async fn direct(
                &self,
                _user: &str,
            ) -> std::result::Result<Place, messenger::MessengerError> {
                Err(messenger::MessengerError::Unsupported("stub".into()))
            }
            async fn listen(
                &self,
                _inbound: tokio::sync::mpsc::Sender<messenger::Inbound>,
            ) -> std::result::Result<(), messenger::MessengerError> {
                Ok(())
            }
            async fn react(
                &self,
                _place: &Place,
                _message: &str,
                _mark: messenger::Mark,
            ) -> std::result::Result<(), messenger::MessengerError> {
                Ok(())
            }
            async fn unreact(
                &self,
                _place: &Place,
                _message: &str,
                _mark: messenger::Mark,
            ) -> std::result::Result<(), messenger::MessengerError> {
                Ok(())
            }
            async fn typing(
                &self,
                _place: &Place,
            ) -> std::result::Result<(), messenger::MessengerError> {
                Ok(())
            }
            async fn send(
                &self,
                _outbound: messenger::Outbound,
            ) -> std::result::Result<messenger::Sent, messenger::MessengerError> {
                Ok(messenger::Sent {
                    created_thread: false,
                    used_thread: false,
                })
            }
            async fn rename(
                &self,
                _place: &Place,
                _name: &str,
            ) -> std::result::Result<(), messenger::MessengerError> {
                Ok(())
            }
        }

        let dir =
            std::env::temp_dir().join(format!("blob-1-admit-auto-respond-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(SqliteStorage::open(dir.join("agent.db"), None).unwrap());
        store.allow_user("discord", "u1").unwrap();
        let stub: Arc<dyn messenger::Messenger> = Arc::new(StubMessenger {
            fail_history: false,
        });
        let failing_stub: Arc<dyn messenger::Messenger> =
            Arc::new(StubMessenger { fail_history: true });
        let messengers = harness::Messengers::new(vec![stub.clone()], store.clone());

        let msg = |channel: &str, addressed: bool| messenger::Inbound {
            place: Place {
                channel: channel.into(),
                thread: Some("t1".into()),
                dm: false,
            },
            guild: Some("g1".into()),
            message: "m1".into(),
            author: storage::Person {
                id: "u1".into(),
                name: "Chan".into(),
            },
            text: "that worked, what next?".into(),
            quote: None,
            attachments: Vec::new(),
            location: "#general".into(),
            earlier: None,
            addressed,
        };

        // Utility returns RESPOND -> admitted with unaddressed=false and [assistant] history label.
        let calls = Arc::new(AtomicUsize::new(0));
        let last_prompt = Arc::new(Mutex::new(String::new()));
        let respond_utility = harness::Utility::new(StubLlm {
            reply: "RESPOND",
            calls: calls.clone(),
            last_prompt: last_prompt.clone(),
        });
        let (_, meta) = admit(
            &store,
            stub.clone(),
            &messengers,
            msg("c1", false),
            Some(&respond_utility),
        )
        .await
        .unwrap()
        .expect("admitted by auto-respond");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(last_prompt
            .lock()
            .unwrap()
            .contains("[assistant] SupportBot: Try restarting the service."));
        assert!(matches!(
            meta,
            storage::Meta::Message {
                unaddressed: false,
                ..
            }
        ));

        // History failure drops the message cleanly without waking the utility model.
        assert!(admit(
            &store,
            failing_stub,
            &messengers,
            msg("c1", false),
            Some(&respond_utility)
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Explicit Always channel override admits with unaddressed=true and skips utility.
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Channel,
                None,
                "c3",
                InteractionMode::Always,
            )
            .unwrap();
        let (_, always_meta) = admit(
            &store,
            stub.clone(),
            &messengers,
            msg("c3", false),
            Some(&respond_utility),
        )
        .await
        .unwrap()
        .expect("admitted by always override");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            always_meta,
            storage::Meta::Message {
                unaddressed: true,
                ..
            }
        ));

        // Utility returns IGNORE -> silently dropped.
        let ignore_calls = Arc::new(AtomicUsize::new(0));
        let ignore_utility = harness::Utility::new(StubLlm {
            reply: "IGNORE",
            calls: ignore_calls.clone(),
            last_prompt: Arc::new(Mutex::new(String::new())),
        });
        assert!(admit(
            &store,
            stub.clone(),
            &messengers,
            msg("c1", false),
            Some(&ignore_utility)
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(ignore_calls.load(Ordering::SeqCst), 1);

        // Explicit MentionOnly channel override skips utility completely.
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Channel,
                None,
                "c2",
                InteractionMode::MentionOnly,
            )
            .unwrap();
        assert!(admit(
            &store,
            stub.clone(),
            &messengers,
            msg("c2", false),
            Some(&respond_utility)
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Disabling auto_respond skips utility completely.
        store.set_auto_respond(false).unwrap();
        assert!(admit(
            &store,
            stub.clone(),
            &messengers,
            msg("c1", false),
            Some(&respond_utility)
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        drop(messengers);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
