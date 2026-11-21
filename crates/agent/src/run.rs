use std::path::Path;
use std::sync::{Arc, Mutex};

use environment::{Environment, SkillBundle};
use environment_local::LocalEnvironment;
use harness::{ChatHarness, EnvironmentPaths, Models};
use storage::{ConfigStore, MessengerStore, Provider, ReasoningLevel, SkillStore};
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
    utility: Option<Option<llm_rig::RigLlm>>,
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

    /// Covers every input `resolve` reads, so a change to any of them
    /// re-resolves. Env vars referenced by key_env are fixed at process start.
    fn fingerprint(&self) -> std::result::Result<u64, harness::HarnessError> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.store.settings()?.hash(&mut hasher);
        self.store.utility_model()?.hash(&mut hasher);
        for provider in self.store.providers()? {
            provider.hash(&mut hasher);
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
                    return Ok((Model::Main(main.llm), main.context, main.selection));
                }
                let key = api_key(&main.provider).map_err(|_| {
                    harness::HarnessError::Compaction(format!(
                        "API key for provider {} is not set",
                        main.provider.id.0
                    ))
                })?;
                return Ok((
                    Model::Main(llm_rig::connect(
                        &main.provider,
                        &key,
                        &main.model,
                        reasoning,
                    )),
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
        let key = api_key(&provider).map_err(|_| {
            harness::HarnessError::Compaction(format!(
                "API key for provider {provider_id} is not set"
            ))
        })?;
        Ok((
            Model::Main(llm_rig::connect(&provider, &key, &model, reasoning)),
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
            main: Model::Main(main.llm.clone()),
            summarizer: cache.summarizer.clone().flatten(),
            compactor: cache
                .compactor
                .clone()
                .unwrap_or_else(|| Model::Main(main.llm.clone())),
            utility: cache.utility.clone().flatten().map(harness::Utility::new),
        })
    }
}

#[derive(Clone)]
enum Model {
    Main(llm_rig::RigLlm),
    Fallback(Arc<llm::Fallback<llm_rig::RigLlm, llm_rig::RigLlm>>),
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
    utility: Resolve<Option<llm_rig::RigLlm>>,
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
    let key = api_key(&provider).map_err(|error| error.to_string())?;
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
    store: &SqliteStorage,
    settings: &storage::Settings,
    main: &Resolve<Working>,
    cached: Option<&Working>,
) -> Resolve<Option<(Model, Option<u32>)>> {
    let Some(selected) = &settings.compaction_model else {
        return Ok(None);
    };
    let (provider, stored) = resolve_model(store, selected)?;
    let key = api_key(&provider).map_err(|error| error.to_string())?;
    let llm = llm_rig::connect(
        &provider,
        &key,
        &stored,
        settings.compaction_reasoning.unwrap_or(settings.reasoning),
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
        Some(main) => Model::Fallback(Arc::new(llm::Fallback::new(llm, main.llm.clone()))),
        None => Model::Main(llm),
    };
    Ok(Some((model, window)))
}

fn resolve_compactor(store: &SqliteStorage, settings: &storage::Settings) -> Resolve<Model> {
    let Some(selected) = &settings.default_model else {
        return Err("no default model; run `agent model set <provider>/<model>`".into());
    };
    let (provider, stored) = resolve_model(store, selected)?;
    let key = api_key(&provider).map_err(|error| error.to_string())?;
    Ok(Model::Main(llm_rig::connect(
        &provider,
        &key,
        &stored,
        settings.compaction_reasoning.unwrap_or(settings.reasoning),
    )))
}

fn resolve(store: &SqliteStorage, cached: Option<&Working>) -> Attempt {
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
    let utility = (|| -> Resolve<Option<llm_rig::RigLlm>> {
        let Some((selected, reasoning)) =
            store.utility_model().map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let (provider, model) = resolve_model(store, &selected)?;
        let key = api_key(&provider).map_err(|error| error.to_string())?;
        Ok(Some(llm_rig::connect(
            &provider,
            &key,
            &model,
            reasoning.unwrap_or(ReasoningLevel::Low),
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
    skills_dir: Option<&Path>,
    models: ModelFactory,
) -> Result<Chat> {
    models.current()?;
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
        store,
    ))
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
    let harness = chat_harness(store, workspace, skills_dir, models).await?;
    let mut failure = None;
    let answer = tokio::select! {
        answer = harness.run(&prompt) => answer,
        _ = interrupt.recv() => return Err("interrupted".into()),
        _ = terminate.recv() => return Err("terminated".into()),
    };
    match answer {
        Ok(answer) => println!("{answer}"),
        Err(error) => failure = Some(error),
    }
    while harness.tasks_running() || harness.has_work() {
        let answer = tokio::select! {
            answer = async {
                harness.work().await;
                harness.resume().await
            } => answer,
            _ = interrupt.recv() => return Err("interrupted".into()),
            _ = terminate.recv() => return Err("terminated".into()),
        };
        match answer {
            Ok(answer) if !answer.is_empty() => println!("{answer}"),
            Ok(_) => {}
            Err(error) => {
                eprintln!("{error}");
                failure.get_or_insert(error);
            }
        }
    }
    failure.map_or(Ok(()), |error| Err(error.into()))
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

async fn admit(
    store: &Arc<SqliteStorage>,
    discord: Arc<dyn messenger::Messenger>,
    messengers: &harness::Messengers,
    inbound: messenger::Inbound,
    utility: Option<&harness::Utility>,
) -> Result<Option<(String, storage::Meta)>> {
    if !store.allowed_users("discord")?.contains(&inbound.author.id) {
        return Ok(None);
    }
    let inline = if inbound.place.thread.is_none() && !inbound.place.dm {
        if let Some(utility) = utility {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let history = discord
                    .history(&inbound.place, Some(&inbound.message), 5)
                    .await
                    .ok()?;
                Some(decide_inline(utility, &history, &inbound).await)
            })
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    };
    let dest = if inline || inbound.place.thread.is_some() || inbound.place.dm {
        inbound.place.clone()
    } else {
        storage::Place {
            channel: inbound.place.channel.clone(),
            thread: Some(inbound.message.clone()),
            dm: false,
        }
    };
    let to = store.destination("discord", &dest)?;
    let (place, message) = (inbound.place.clone(), inbound.message.clone());
    tokio::spawn(async move {
        if let Err(error) = discord.react(&place, &message, messenger::Mark::Seen).await {
            eprintln!("{error}");
        }
    });
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
            location: inbound.location,
            earlier: inbound.earlier,
            quote: inbound.quote,
            attachments: inbound.attachments,
        },
    )))
}

pub async fn serve(
    store: Arc<SqliteStorage>,
    workspace: &Path,
    skills_dir: Option<&Path>,
    token: String,
    shared_discord: Arc<std::sync::OnceLock<Arc<messenger_discord::Discord>>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let models = ModelFactory::new(store.clone());
    if models.current().is_err() {
        eprintln!("Discord waiting for a main model; set one in the web settings");
        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                    if models.current().is_ok() {
                        break;
                    }
                }
                _ = shutdown.changed() => return Ok(()),
            }
        }
    }
    let harness = chat_harness(store.clone(), workspace, skills_dir, models.clone()).await?;
    let client = Arc::new(messenger_discord::Discord::new(token));
    let _ = shared_discord.set(client.clone());
    let warm = client.clone();
    tokio::spawn(async move {
        while !warm.is_ready() {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if let Err(error) = warm.directory().await {
            eprintln!("discord directory: {error}");
        }
    });
    let discord: Arc<dyn messenger::Messenger> = client;
    let messengers = harness::Messengers::new(vec![discord.clone()], store.clone())
        .with_utility({
            let models = models.clone();
            move || models.current().ok().and_then(|current| current.utility)
        });
    let harness = harness.with_messengers(messengers.clone());
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let listener_discord = discord.clone();
    let listener = tokio::spawn(async move { listener_discord.listen(tx).await });
    let (admitted_tx, mut admitted_rx) = tokio::sync::mpsc::channel(128);
    let admission_store = store.clone();
    let admission_discord = discord.clone();
    let admission_messengers = messengers.clone();
    let admission_models = models.clone();
    let admission = tokio::spawn(async move {
        while let Some(inbound) = rx.recv().await {
            let utility = admission_models
                .current()
                .ok()
                .and_then(|current| current.utility);
            match admit(
                &admission_store,
                admission_discord.clone(),
                &admission_messengers,
                inbound,
                utility.as_ref(),
            )
            .await
            {
                Ok(Some(next)) => {
                    if admitted_tx.send(Ok(next)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = admitted_tx.send(Err(error)).await;
                    return;
                }
            }
        }
    });
    let mut pending = std::collections::VecDeque::<(String, storage::Meta)>::new();
    let mut backoff = false;
    tokio::select! {
        result = async {
    loop {
        let next = if let Some(next) = pending.pop_front() {
            Some(next)
        } else {
            tokio::select! {
                value = admitted_rx.recv() => match value {
                    Some(next) => Some(next?),
                    None => return match listener.await? { Err(error) => Err(error.into()), Ok(()) => Err("discord listener stopped".into()) },
                },
                _ = async {
                    if backoff {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                    harness.work().await
                } => None,
                _ = shutdown.changed() => { listener.abort(); admission.abort(); return Ok(()); },
            }
        };
        let run = async {
            match next {
                Some((text, meta)) => harness.run_message(text, Some(meta)).await,
                None => harness.resume().await,
            }
        };
        tokio::pin!(run);
        while let Some((text, meta)) = pending.pop_front() {
            harness.steer_message(text, Some(meta));
        }
        loop {
            tokio::select! {
                result = &mut run => {
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
                _ = shutdown.changed() => { listener.abort(); admission.abort(); return Ok(()); },
            }
        }
        messengers.quiet();
        for (text, meta) in harness.take_steering().into_iter().rev() {
            if let Some(meta) = meta {
                pending.push_front((text, meta));
            }
        }
        for (_, meta) in &pending {
            if let storage::Meta::Message { messenger, to, .. } = meta {
                let (_, destination) = store.place(*to)?.ok_or("unknown destination")?;
                messengers.live(messenger, *to, destination);
            }
        }
    }
        } => result,
        _ = harness.run_schedules() => unreachable!(),
    }
}
