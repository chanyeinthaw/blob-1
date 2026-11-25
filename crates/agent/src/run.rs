use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use environment::{Environment, SkillBundle};
use environment_local::LocalEnvironment;
use harness::{ChatHarness, EnvironmentPaths, Models};
use messenger::Messenger;
use storage::{
    Bot, BotStore, CallbackStore, ConfigStore, McpStore, MessengerStore, Provider, ReasoningLevel,
    SkillStore, UsageEvent, UsageKind, UsageStore, DEFAULT_BOT_ID,
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
    global_store: Arc<SqliteStorage>,
    usage_store: Arc<SqliteStorage>,
    bot_id: String,
    cache: Arc<Mutex<Cache>>,
}

#[derive(Default)]
struct Cache {
    fingerprint: Option<u64>,
    main: Option<Working>,
    summarizer: Option<Option<(Model, Option<u32>)>>,
    compactor: Option<Model>,
    utility: Option<Option<RecordedLlm>>,
    decision: Option<Option<RecordedDecision>>,
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
        Self::new_for_bot(store.clone(), store, DEFAULT_BOT_ID.to_owned())
    }

    fn new_for_bot(
        global_store: Arc<SqliteStorage>,
        usage_store: Arc<SqliteStorage>,
        bot_id: String,
    ) -> Self {
        Self {
            global_store,
            usage_store,
            bot_id,
            cache: Arc::new(Mutex::new(Cache::default())),
        }
    }

    /// Covers every input `resolve` reads, including a non-secret fingerprint of
    /// each provider's stored API key ciphertext, so key rotation invalidates cached clients.
    fn fingerprint(&self) -> std::result::Result<u64, harness::HarnessError> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.global_store
            .bot_models(&self.bot_id)?
            .hash(&mut hasher);
        for provider in self.global_store.providers()? {
            provider.hash(&mut hasher);
            self.global_store
                .provider_key_fingerprint(&provider.id)?
                .hash(&mut hasher);
            self.global_store.models(&provider.id)?.hash(&mut hasher);
        }
        Ok(hasher.finish())
    }
}

impl harness::Models for ModelFactory {
    type Llm = Model;

    fn list(&self, search: Option<&str>) -> std::result::Result<String, harness::HarnessError> {
        let bm = self.global_store.bot_models(&self.bot_id)?;
        let default = bm
            .main
            .ok_or_else(|| harness::HarnessError::Compaction("no default model".into()))?;
        // Role assignments resolve even when disabled; only the table below is
        // selectable for spawn, so flag disabled slots as unavailable.
        let enabled_of = |selected: &storage::ModelRef| -> bool {
            self.global_store
                .models(&selected.provider)
                .ok()
                .and_then(|models| models.into_iter().find(|model| model.id == selected.model))
                .is_some_and(|model| model.enabled)
        };
        let unavailable = |enabled: bool| {
            if enabled {
                ""
            } else {
                " (disabled for explicit selection)"
            }
        };
        let mut lines = vec![
            format!(
                "default: provider={} model={} reasoning={}{}",
                default.provider.0,
                default.model,
                bm.main_reasoning.as_str(),
                unavailable(enabled_of(&default)),
            ),
            match &bm.compaction {
                Some(model) => format!(
                    "compaction: provider={} model={}{}",
                    model.provider.0,
                    model.model,
                    unavailable(enabled_of(model)),
                ),
                None => "compaction: default".into(),
            },
            "provider  model  context  reasoning  input".into(),
        ];
        let search = search.unwrap_or("").to_lowercase();
        for provider in self.global_store.providers()? {
            if provider.api.is_decision() {
                continue;
            }
            for model in self.global_store.models(&provider.id)? {
                if !model.enabled {
                    continue;
                }
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
                    None => self.global_store.bot_models(&self.bot_id)?.main_reasoning,
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
                            self.usage_store.clone(),
                            UsageKind::Chat,
                            &main.selection.provider,
                            &main.selection.model,
                        ),
                        main.context,
                        main.selection,
                    ));
                }
                let key = api_key(&self.global_store, &main.provider)
                    .map_err(|error| harness::HarnessError::Compaction(error.to_string()))?;
                return Ok((
                    Model::main(
                        llm_rig::connect(&main.provider, &key, &main.model, reasoning),
                        self.usage_store.clone(),
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
            .global_store
            .providers()?
            .into_iter()
            .find(|p| p.id.0 == provider_id)
            .ok_or_else(unknown)?;
        if provider.api.is_decision() {
            return Err(harness::HarnessError::Compaction(format!(
                "decision provider {} cannot run chat or tasks",
                provider.id.0
            )));
        }
        check_base_url(&provider).map_err(harness::HarnessError::Compaction)?;
        let model = self
            .global_store
            .models(&provider.id)?
            .into_iter()
            .find(|m| m.id == model_id)
            .ok_or_else(unknown)?;
        if !model.enabled {
            return Err(harness::HarnessError::DisabledModel(format!(
                "{provider_id}/{model_id}"
            )));
        }
        let key = api_key(&self.global_store, &provider)
            .map_err(|error| harness::HarnessError::Compaction(error.to_string()))?;
        Ok((
            Model::main(
                llm_rig::connect(&provider, &key, &model, reasoning),
                self.usage_store.clone(),
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
            let attempt = resolve(
                &self.global_store,
                &self.usage_store,
                &self.bot_id,
                cache.main.as_ref(),
            );
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
            match attempt.decision {
                Ok(decision) => cache.decision = Some(decision),
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
        let utility_llm: Option<Arc<dyn llm::Llm + Send + Sync>> = cache
            .utility
            .clone()
            .flatten()
            .map(|u| Arc::new(u) as Arc<dyn llm::Llm + Send + Sync>);
        let decision_model: Option<Arc<dyn llm::DecisionModel>> = cache
            .decision
            .clone()
            .flatten()
            .map(|d| Arc::new(d) as Arc<dyn llm::DecisionModel>);
        Ok(harness::Current {
            main: Model::main(
                main.llm.clone(),
                self.usage_store.clone(),
                UsageKind::Chat,
                &main.selection.provider,
                &main.selection.model,
            ),
            summarizer: cache.summarizer.clone().flatten(),
            compactor: cache.compactor.clone().unwrap_or_else(|| {
                Model::main(
                    main.llm.clone(),
                    self.usage_store.clone(),
                    UsageKind::Compaction,
                    &main.selection.provider,
                    &main.selection.model,
                )
            }),
            utility: harness::Utility::from_parts(utility_llm, decision_model),
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

/// Wraps a `DecisionClient` to persist per-call token telemetry into SQLite.
#[derive(Clone)]
struct RecordedDecision {
    inner: llm_rig::DecisionClient,
    store: Arc<SqliteStorage>,
    provider: Arc<str>,
    model: Arc<str>,
}

impl RecordedDecision {
    fn new(
        inner: llm_rig::DecisionClient,
        store: Arc<SqliteStorage>,
        provider: &str,
        model: &str,
    ) -> Self {
        Self {
            inner,
            store,
            provider: Arc::from(provider),
            model: Arc::from(model),
        }
    }
}

#[async_trait::async_trait]
impl llm::DecisionModel for RecordedDecision {
    async fn choose(
        &self,
        state: &str,
        question: llm::ChoiceQuestion<'_>,
    ) -> std::result::Result<llm::ChoiceAnswer, llm::LlmError> {
        let answer = self.inner.choose(state, question).await?;
        let usage = answer.usage.unwrap_or_default();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Err(error) = self.store.record_usage(&UsageEvent {
            ts,
            kind: UsageKind::Utility,
            provider: self.provider.to_string(),
            model: self.model.to_string(),
            input_tokens: usage.input,
            cached_tokens: usage.cache_read,
            output_tokens: usage.output,
        }) {
            eprintln!("usage telemetry write failed: {error}");
        }
        Ok(answer)
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

/// Shared handle to the running `ChatHarness` used by inbound webhooks and
/// temporary callbacks. Uses an async `RwLock` so shutdown (`close()`) waits
/// for any in-flight `PreparedInject` before stopping delivery, guaranteeing
/// late callbacks receive 503 before consumption and accepted events drain.
#[derive(Clone, Default)]
pub struct LiveChat {
    inner: Arc<tokio::sync::RwLock<Option<(Arc<Chat>, ModelFactory)>>>,
    outbox: Arc<tokio::sync::Notify>,
}

/// Validated live chat handle holding a read lock across durable outbox commit
/// and notification so shutdown cannot race between `prepare` and `wake`.
pub struct PreparedInject {
    _guard: tokio::sync::OwnedRwLockReadGuard<Option<(Arc<Chat>, ModelFactory)>>,
    outbox: Arc<tokio::sync::Notify>,
}

impl PreparedInject {
    pub fn wake(self) {
        self.outbox.notify_one();
    }
}

impl LiveChat {
    async fn set(&self, harness: Arc<Chat>, models: ModelFactory) {
        *self.inner.write().await = Some((harness, models));
    }

    pub async fn close(&self) {
        *self.inner.write().await = None;
    }

    fn outbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.outbox.clone()
    }

    pub async fn prepare(&self) -> Result<PreparedInject> {
        let guard = self.inner.clone().read_owned().await;
        let Some((_, models)) = guard.as_ref() else {
            return Err("agent is not accepting events".into());
        };
        models.current()?;
        Ok(PreparedInject {
            _guard: guard,
            outbox: self.outbox.clone(),
        })
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

type Chat =
    ChatHarness<Arc<dyn environment::Environment>, Arc<SqliteStorage>, Compactor, ModelFactory>;

type Resolve<T> = std::result::Result<T, String>;

struct Attempt {
    main: Resolve<Working>,
    summarizer: Resolve<Option<(Model, Option<u32>)>>,
    compactor: Resolve<Model>,
    utility: Resolve<Option<RecordedLlm>>,
    decision: Resolve<Option<RecordedDecision>>,
}

pub(crate) fn valid_http_url(base_url: &str) -> bool {
    !base_url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
        && url::Url::parse(base_url)
            .map(|url| matches!(url.scheme(), "http" | "https"))
            .unwrap_or(false)
        && base_url
            .parse::<axum::http::Uri>()
            .is_ok_and(|uri| uri.host().is_some_and(|host| !host.is_empty()))
}

fn check_base_url(provider: &Provider) -> Resolve<()> {
    let base_url = &provider.base_url;
    if valid_http_url(base_url) {
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
    if provider.api.is_decision() {
        return Err(format!(
            "decision provider {} cannot be used as the main model",
            provider.id.0
        ));
    }
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
    global_store: &Arc<SqliteStorage>,
    usage_store: &Arc<SqliteStorage>,
    settings: &storage::Settings,
    main: &Resolve<Working>,
    cached: Option<&Working>,
) -> Resolve<Option<(Model, Option<u32>)>> {
    let Some(selected) = &settings.compaction_model else {
        return Ok(None);
    };
    let (provider, stored) = resolve_model(global_store, selected)?;
    if provider.api.is_decision() {
        return Err(format!(
            "decision provider {} cannot be used as the compaction model",
            provider.id.0
        ));
    }
    let key = api_key(global_store, &provider).map_err(|error| error.to_string())?;
    let llm = RecordedLlm::new(
        llm_rig::connect(
            &provider,
            &key,
            &stored,
            settings.compaction_reasoning.unwrap_or(settings.reasoning),
        ),
        usage_store.clone(),
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
                usage_store.clone(),
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

fn resolve_compactor(
    global_store: &Arc<SqliteStorage>,
    usage_store: &Arc<SqliteStorage>,
    settings: &storage::Settings,
) -> Resolve<Model> {
    let Some(selected) = &settings.default_model else {
        return Err("no default model; run `agent model set <provider>/<model>`".into());
    };
    let (provider, stored) = resolve_model(global_store, selected)?;
    if provider.api.is_decision() {
        return Err(format!(
            "decision provider {} cannot be used as the compaction model",
            provider.id.0
        ));
    }
    let key = api_key(global_store, &provider).map_err(|error| error.to_string())?;
    Ok(Model::main(
        llm_rig::connect(
            &provider,
            &key,
            &stored,
            settings.compaction_reasoning.unwrap_or(settings.reasoning),
        ),
        usage_store.clone(),
        UsageKind::Compaction,
        &provider.id.0,
        &stored.id,
    ))
}

fn resolve(
    global_store: &Arc<SqliteStorage>,
    usage_store: &Arc<SqliteStorage>,
    bot_id: &str,
    cached: Option<&Working>,
) -> Attempt {
    let bm = match global_store.bot_models(bot_id) {
        Ok(bm) => bm,
        Err(error) => {
            let error = error.to_string();
            return Attempt {
                main: Err(error.clone()),
                summarizer: Err(error.clone()),
                compactor: Err(error.clone()),
                utility: Err(error.clone()),
                decision: Err(error),
            };
        }
    };
    let settings = storage::Settings {
        default_model: bm.main,
        reasoning: bm.main_reasoning,
        compaction_model: bm.compaction,
        compaction_reasoning: bm.compaction_reasoning,
    };
    let main = resolve_main(global_store, &settings);
    let summarizer = resolve_summarizer(global_store, usage_store, &settings, &main, cached);
    let compactor = match &summarizer {
        Ok(Some((model, _))) => Ok(model.clone()),
        Ok(None) => resolve_compactor(global_store, usage_store, &settings),
        Err(error) => Err(error.clone()),
    };
    let utility = (|| -> Resolve<Option<RecordedLlm>> {
        let Some((selected, reasoning)) = bm.utility else {
            return Ok(None);
        };
        let (provider, model) = resolve_model(global_store, &selected)?;
        if provider.api.is_decision() {
            return Err(format!(
                "decision provider {} cannot be used as the utility LLM",
                provider.id.0
            ));
        }
        let key = api_key(global_store, &provider).map_err(|error| error.to_string())?;
        Ok(Some(RecordedLlm::new(
            llm_rig::connect(
                &provider,
                &key,
                &model,
                reasoning.unwrap_or(ReasoningLevel::Low),
            ),
            usage_store.clone(),
            UsageKind::Utility,
            &provider.id.0,
            &model.id,
        )))
    })();
    let decision = (|| -> Resolve<Option<RecordedDecision>> {
        let Some(selected) = bm.decision else {
            return Ok(None);
        };
        let (provider, model) = resolve_model(global_store, &selected)?;
        if !provider.api.is_decision() {
            return Err(format!(
                "provider {} is not a decision provider",
                provider.id.0
            ));
        }
        let key = api_key(global_store, &provider).map_err(|error| error.to_string())?;
        Ok(Some(RecordedDecision::new(
            llm_rig::connect_decision(&provider, &key, &model),
            usage_store.clone(),
            &provider.id.0,
            &model.id,
        )))
    })();
    Attempt {
        main,
        summarizer,
        compactor,
        utility,
        decision,
    }
}

/// Pushes DB-tracked skills onto the bot's execution environment — local or a
/// remote node, whichever the `Environment` impl targets.
async fn sync_shared_skills(
    store: &Arc<SqliteStorage>,
    environment: &Arc<dyn environment::Environment>,
) -> Result<()> {
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
    Ok(())
}

/// `pinned_workspace` is `None` for remote environments: `share` and file
/// attachments read the primary's disk, so they are withheld rather than
/// served from a workspace the bot's tools never see.
async fn chat_harness(
    global_store: Arc<SqliteStorage>,
    bot_store: Arc<SqliteStorage>,
    bot_id: String,
    workspace: &Path,
    pinned_workspace: Option<harness::PinnedWorkspace>,
    environment: Arc<dyn environment::Environment>,
    models: ModelFactory,
    mcp: mcp::Mcp,
) -> Result<Chat> {
    let memory = memory::Memory::open(
        bot_store.clone(),
        Compactor(models.clone()),
        memory::MemoryConfig::default(),
    )?;
    let filter_store = global_store.clone();
    let filter_bot = bot_id;
    let mcp_filter: Arc<dyn Fn(&str) -> bool + Send + Sync> =
        Arc::new(
            move |server_id: &str| match filter_store.bot_disabled_mcp_servers(&filter_bot) {
                Ok(disabled) => !disabled.iter().any(|id| id == server_id),
                Err(_) => false,
            },
        );
    let harness = ChatHarness::new(
        environment,
        bot_store.clone(),
        memory,
        EnvironmentPaths {
            workspace: workspace.to_path_buf(),
        },
        models,
        bot_store.clone(),
    );
    let harness = match pinned_workspace {
        Some(pinned) => harness.with_share(harness::ShareTools::new(
            bot_store.clone(),
            global_store.clone(),
            pinned,
        )),
        None => harness,
    };
    Ok(harness
        .with_secrets(harness::SecretTools::new(
            bot_store.clone(),
            global_store.clone(),
        ))
        .with_callback(harness::CallbackTools::new(bot_store, global_store))
        .with_mcp(mcp)
        .with_mcp_filter(mcp_filter))
}

/// Creates a fresh owner-only directory with a random name under the temp
/// dir. Creation is atomic and fails if the path exists, so another local user
/// can't plant it in a shared `/tmp`.
fn private_temp_dir(prefix: &str) -> Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let mut nonce = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", hex::encode(nonce)));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
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
    // Secret files for this run only: their deletion timers end with the
    // process, so the directory is removed on the way out.
    let secrets_dir = private_temp_dir("blob-1-secrets")?;
    let environment: Arc<dyn environment::Environment> = Arc::new(
        LocalEnvironment::new(workspace, skills_dir.map(Path::to_path_buf))?
            .with_secrets_dir(secrets_dir.clone()),
    );
    sync_shared_skills(&store, &environment).await?;
    let pinned_workspace = harness::PinnedWorkspace::open(workspace)?;
    let harness = chat_harness(
        store.clone(),
        store,
        DEFAULT_BOT_ID.to_owned(),
        workspace,
        Some(pinned_workspace),
        environment,
        models,
        mcp,
    )
    .await?;
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
    let _ = environment_local::clear_secrets(&secrets_dir);
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
    harness::decide_inline(utility, history, inbound).await
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

/// Evaluates the unified 8-step access precedence:
/// 1. `user` in `thread`
/// 2. `user` in `parent channel`
/// 3. `user` in `server`
/// 4. `user` `global`
/// 5. `everyone` in `thread`
/// 6. `everyone` in `parent channel`
/// 7. `everyone` in `server`
/// 8. `default` (`block` or `allow`)
///
/// DMs have no server/channel, so they check step 4 (`user` `global`), then step 8 (`default`).
fn evaluate_access(
    default: storage::AccessEffect,
    rules: &[storage::AccessRule],
    author: &str,
    guild: Option<&str>,
    channel: &str,
    thread: Option<&str>,
    dm: bool,
) -> (storage::AccessEffect, u8) {
    use storage::AccessScope;
    let find = |user: Option<&str>, scope: AccessScope, scope_id: Option<&str>| {
        rules
            .iter()
            .find(|r| {
                r.user_id.as_deref() == user
                    && r.scope == scope
                    && r.scope_id.as_deref() == scope_id
            })
            .map(|r| r.effect)
    };
    if dm {
        return match find(Some(author), AccessScope::Global, None) {
            Some(effect) => (effect, 4),
            None => (default, 8),
        };
    }
    if let Some(t) = thread {
        if let Some(effect) = find(Some(author), AccessScope::Channel, Some(t)) {
            return (effect, 1);
        }
    }
    if let Some(effect) = find(Some(author), AccessScope::Channel, Some(channel)) {
        return (effect, 2);
    }
    if let Some(g) = guild {
        if let Some(effect) = find(Some(author), AccessScope::Server, Some(g)) {
            return (effect, 3);
        }
    }
    if let Some(effect) = find(Some(author), AccessScope::Global, None) {
        return (effect, 4);
    }
    if let Some(t) = thread {
        if let Some(effect) = find(None, AccessScope::Channel, Some(t)) {
            return (effect, 5);
        }
    }
    if let Some(effect) = find(None, AccessScope::Channel, Some(channel)) {
        return (effect, 6);
    }
    if let Some(g) = guild {
        if let Some(effect) = find(None, AccessScope::Server, Some(g)) {
            return (effect, 7);
        }
    }
    (default, 8)
}

fn access_allowed(
    default: storage::AccessEffect,
    rules: &[storage::AccessRule],
    author: &str,
    guild: Option<&str>,
    channel: &str,
    thread: Option<&str>,
    dm: bool,
) -> bool {
    evaluate_access(default, rules, author, guild, channel, thread, dm).0
        == storage::AccessEffect::Allow
}

/// Evaluates the gateway pre-filter decision for a message given its unified
/// access policy (`default_access` + `access_rules`), interaction overrides,
/// Discord-wide `default_mode`, and known guild/channel/thread identity
/// (`thread == Some(t)` when `channel` is the resolved parent channel;
/// `thread == None` when `channel` is a known top-level channel or an uncached
/// channel/thread).
#[allow(clippy::too_many_arguments)]
fn pre_filter_decision(
    default_access: storage::AccessEffect,
    access_rules: &[storage::AccessRule],
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
    use storage::{AccessEffect, AccessScope, InteractionMode, InteractionScope};
    let dm = guild.is_none();
    let access_decision = if channel_known || dm {
        if access_allowed(
            default_access,
            access_rules,
            author,
            guild,
            channel,
            thread,
            dm,
        ) {
            PreFilterDecision::Allow
        } else {
            PreFilterDecision::Reject
        }
    } else {
        // `channel` may be an uncached thread id whose parent channel P is unknown.
        // Treat `channel` as the thread id (step 1 / step 5) when evaluating what is
        // already known, then check whether a parent-channel rule (step 2 or step 6)
        // of higher precedence could flip the outcome.
        let find = |user: Option<&str>, scope: AccessScope, scope_id: Option<&str>| {
            access_rules
                .iter()
                .find(|r| {
                    r.user_id.as_deref() == user
                        && r.scope == scope
                        && r.scope_id.as_deref() == scope_id
                })
                .map(|r| r.effect)
        };
        let (known_effect, matched_step) = if let Some(e) =
            find(Some(author), AccessScope::Channel, Some(channel))
        {
            (e, 1u8)
        } else if let Some(e) = guild.and_then(|g| find(Some(author), AccessScope::Server, Some(g)))
        {
            (e, 3)
        } else if let Some(e) = find(Some(author), AccessScope::Global, None) {
            (e, 4)
        } else if let Some(e) = find(None, AccessScope::Channel, Some(channel)) {
            (e, 5)
        } else if let Some(e) = guild.and_then(|g| find(None, AccessScope::Server, Some(g))) {
            (e, 7)
        } else {
            (default_access, 8)
        };
        // Step 2 (`user` in parent channel) beats steps 3..=8.
        let user_parent_can_flip = matched_step > 2
            && access_rules.iter().any(|r| {
                r.user_id.as_deref() == Some(author)
                    && r.scope == AccessScope::Channel
                    && r.effect != known_effect
            });
        // Step 6 (`everyone` in parent channel) beats only steps 7..=8.
        let everyone_parent_can_flip = matched_step > 6
            && access_rules.iter().any(|r| {
                r.user_id.is_none() && r.scope == AccessScope::Channel && r.effect != known_effect
            });
        if user_parent_can_flip || everyone_parent_can_flip {
            PreFilterDecision::NeedParent
        } else if known_effect == AccessEffect::Allow {
            PreFilterDecision::Allow
        } else {
            PreFilterDecision::Reject
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
    let default_access = store.default_access("discord")?;
    let rules = store.access_rules("discord")?;
    if !access_allowed(
        default_access,
        &rules,
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
    watermarks: &Watermarks,
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
                        // Re-verify access & policy after history fetch before calling utility model.
                        let still_allowed = match admission_policy(store, &inbound) {
                            Ok(AdmissionPolicy::AdmitDirect) => true,
                            Ok(AdmissionPolicy::AutoRespondCheck) => {
                                auto_responded && store.auto_respond().unwrap_or(false)
                            }
                            _ => false,
                        };
                        if !still_allowed {
                            return None;
                        }
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
    // Fill the conversation gap before the final policy recheck, so access
    // revoked during any of the async waits is still caught before admission.
    let gap = missed_history(store, watermarks, discord, &inbound).await;
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
    // Finalize the gap without awaiting: re-read the in-memory mark and drop
    // whatever an overlapping lane covered while the fetch was in flight.
    // The overlap check is best effort — seeing a published mark guarantees
    // only that its covering message was already queued ahead in the FIFO
    // (see `publish_watermark`), not that it has committed. The durable mark
    // follows when the memory append commits. On fetch failure (`None`)
    // neither mark moves, so the gap survives for the next admission until
    // the durable append covers this message.
    let gap_open = gap.is_none();
    let mut missed = Vec::new();
    if let Some(gap) = gap {
        missed = gap;
        // Drop whatever an overlapping lane already covered while the fetch
        // was in flight. The mark itself is only advanced after the item is
        // enqueued (see `publish_watermark`), so any lane reading it is
        // guaranteed the covering message is already ahead in the FIFO.
        if let Some(entry) = watermarks.get(&watermark_key(&inbound.place)) {
            if *entry > 0 {
                missed.retain(|m| {
                    m.message.parse::<u64>().ok().is_some_and(|id| id > *entry)
                });
            }
        }
        // Re-filter authors against current access rules: the fetch-time
        // filter read them before up to five REST requests, and access
        // revoked meanwhile must not enter the session through the gap.
        if !missed.is_empty() {
            let default_access = store.default_access("discord")?;
            let rules = store.access_rules("discord")?;
            missed.retain(|m| {
                access_allowed(
                    default_access,
                    &rules,
                    &m.author.id,
                    inbound.guild.as_deref(),
                    &inbound.place.channel,
                    inbound.place.thread.as_deref(),
                    inbound.place.dm,
                )
            });
        }
        if missed.len() > 10 {
            missed = missed.split_off(missed.len() - 10);
        }
    }
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
            missed,
            gap_open,
        },
    )))
}

/// The conversation messages missed since the last admission in this place,
/// oldest first and capped at the newest 10. The high-water mark is the later
/// of the durable `destinations.last_message` (written when the memory append
/// commits) and the in-memory admission mark; entries are access-filtered so a
/// blocked user's messages never enter the session through the gap, while
/// interaction-mode filtering deliberately does not apply — recovering the
/// messages it skipped is the point. `None` when the history fetch failed.
async fn missed_history(
    store: &Arc<SqliteStorage>,
    watermarks: &Watermarks,
    discord: &Arc<dyn messenger::Messenger>,
    inbound: &messenger::Inbound,
) -> Option<Vec<storage::Missed>> {
    let current = inbound.message.parse::<u64>().ok()?;
    let durable = store
        .last_seen_message("discord", &inbound.place)
        .ok()
        .flatten()
        .and_then(|id| id.parse::<u64>().ok());
    let memory = watermarks.get(&watermark_key(&inbound.place)).map(|v| *v);
    let last = durable.into_iter().chain(memory).max();
    // The mark at or past this message means no gap (replays, out-of-order).
    if last.is_some_and(|last| last >= current) {
        return Some(Vec::new());
    }
    let default_access = store.default_access("discord").ok()?;
    let rules = store.access_rules("discord").ok()?;
    // Page backwards: bot posts and blocked users are filtered after fetching,
    // so a single 20-message page can hide every eligible message behind a
    // run of ineligible ones. Capped at 5 pages — a 100-message scan bounds
    // the latency one cold admission can add.
    let mut pages: Vec<Vec<storage::Missed>> = Vec::new();
    let mut before = inbound.message.clone();
    for _ in 0..5 {
        let history = discord.history(&inbound.place, Some(&before), 20).await.ok()?;
        if history.is_empty() {
            break;
        }
        // Oldest-first, so the first element is the cursor for the next page.
        before = history[0].message.clone();
        let exhausted = history.len() < 20;
        let page: Vec<storage::Missed> = history
            .iter()
            .filter(|m| {
                m.message
                    .parse::<u64>()
                    .ok()
                    .is_some_and(|id| id < current && last.is_none_or(|last| id > last))
            })
            .filter(|m| !m.bot)
            .filter(|m| {
                access_allowed(
                    default_access,
                    &rules,
                    &m.author.id,
                    inbound.guild.as_deref(),
                    &inbound.place.channel,
                    inbound.place.thread.as_deref(),
                    inbound.place.dm,
                )
            })
            .map(|m| storage::Missed {
                message: m.message.clone(),
                author: m.author.clone(),
                date: m.date.clone(),
                text: m.text.clone(),
                attachments: m.attachments.clone(),
            })
            .collect();
        pages.push(page);
        // Once the page's oldest id sits at or before the mark, older pages
        // have nothing left to contribute.
        let oldest = history
            .first()
            .and_then(|m| m.message.parse::<u64>().ok());
        let at_mark = match (last, oldest) {
            (Some(last), Some(oldest)) => oldest <= last,
            _ => false,
        };
        if exhausted || at_mark || pages.iter().map(Vec::len).sum::<usize>() >= 10 {
            break;
        }
    }
    // Pages were fetched newest-first; flatten them back into oldest-first.
    // Not capped here: the cap applies after the final access re-filter, so a
    // mid-fetch revocation can backfill from spares the last page collected.
    Some(pages.into_iter().rev().flatten().collect())
}

fn watermark_key(place: &storage::Place) -> (String, String) {
    (
        place.channel.clone(),
        place.thread.clone().unwrap_or_default(),
    )
}

/// The watermark an admitted input will publish once enqueued — `None` for
/// `gap_open` rows, whose unfilled gap must stay open for a later retry.
fn watermark_of(meta: &storage::Meta) -> Option<((String, String), u64)> {
    let storage::Meta::Message {
        place,
        message,
        gap_open,
        ..
    } = meta
    else {
        return None;
    };
    if *gap_open {
        return None;
    }
    let id = message.parse::<u64>().ok()?;
    Some((watermark_key(place), id))
}

/// Advances the in-memory mark after the covering item is in the admission
/// FIFO. Post-send publication is the ordering guarantee: any lane that reads
/// the mark either misses it (and inlines the range itself — cosmetic
/// duplication) or sees it only after the covering message is already queued
/// ahead, so downstream commits reach the covering row first and a restart
/// can never leave the durable mark past a gap nothing persisted.
fn publish_watermark(watermarks: &Watermarks, publish: Option<((String, String), u64)>) {
    if let Some((key, id)) = publish {
        let mut entry = watermarks.entry(key).or_insert(0);
        *entry = (*entry).max(id);
    }
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
    admit_batch(
        store,
        discord,
        messengers,
        vec![inbound],
        utility,
        &Watermarks::default(),
        &tx,
    )
    .await?;
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
/// Per-place high-water marks for admitted messages, in-memory and
/// process-local: they cover the window between admission and the durable
/// mark that the memory append writes, so bursts and overlapping lanes do not
/// re-inline messages already admitted. Keyed by (channel, thread-or-empty).
type Watermarks = Arc<dashmap::DashMap<(String, String), u64>>;

async fn admit_batch(
    store: &Arc<SqliteStorage>,
    discord: Arc<dyn messenger::Messenger>,
    messengers: &harness::Messengers,
    batch: Vec<messenger::Inbound>,
    utility: Option<&harness::Utility>,
    watermarks: &Watermarks,
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

    let mut approved_triage_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    // Reuse triage's prefetched history ONLY for the exact message whose cutoff it
    // was fetched for; other items in the burst read the live cache (or REST fallback).
    let mut triage_prefetched: Option<(String, Vec<messenger::Fetched>)> = None;
    if !triage_snapshot.is_empty() {
        let place = triage_snapshot[0].place.clone();
        let first_msg_id = triage_snapshot[0].message.clone();
        let auto_respond = store.auto_respond()?;
        if let Some(utility) = utility.filter(|_| auto_respond) {
            let instructions = store
                .instructions()?
                .or_else(|| Some(harness::DEFAULT_INSTRUCTIONS.into()));
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
                // Re-verify access & Auto-respond after history fetch before calling utility model.
                if !store.auto_respond().unwrap_or(false) {
                    return None;
                }
                let live_snapshot: Vec<messenger::Inbound> = triage_snapshot
                    .iter()
                    .filter(|item| {
                        matches!(
                            admission_policy(store, item),
                            Ok(AdmissionPolicy::AutoRespondCheck)
                        )
                    })
                    .cloned()
                    .collect();
                if live_snapshot.is_empty() {
                    return None;
                }
                let respond = harness::decide_respond(
                    utility,
                    instructions.as_deref(),
                    bot_id.as_deref(),
                    &prompt_history,
                    &live_snapshot,
                )
                .await;
                let evaluated_ids: Vec<String> =
                    live_snapshot.iter().map(|m| m.message.clone()).collect();
                Some((
                    respond,
                    evaluated_ids,
                    prompt_history,
                    seed_snapshot,
                    complete_from_start,
                ))
            })
            .await;
            if let Ok(Some((
                true,
                evaluated_ids,
                prompt_history,
                seed_snapshot,
                complete_from_start,
            ))) = decision
            {
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
                approved_triage_ids.extend(evaluated_ids);
                triage_prefetched = Some((first_msg_id, prompt_history));
            }
            drop(warm_hold);
        }
    }

    for (inbound, _) in ordered {
        match admission_policy(store, &inbound)? {
            AdmissionPolicy::AdmitDirect => {
                if let Some(item) =
                    admit_approved(
                        store, &discord, messengers, inbound, utility, None, false, watermarks,
                    )
                        .await?
                {
                    let publish = watermark_of(&item.1);
                    let _ = tx.send(Ok(item)).await;
                    publish_watermark(watermarks, publish);
                }
            }
            AdmissionPolicy::AutoRespondCheck if approved_triage_ids.contains(&inbound.message) => {
                let prefetched = triage_prefetched
                    .as_ref()
                    .and_then(|(id, h)| (*id == inbound.message).then_some(h.as_slice()));
                if let Some(item) = admit_approved(
                    store, &discord, messengers, inbound, utility, prefetched, true, watermarks,
                )
                .await?
                {
                    let publish = watermark_of(&item.1);
                    let _ = tx.send(Ok(item)).await;
                    publish_watermark(watermarks, publish);
                }
            }
            AdmissionPolicy::AutoRespondCheck | AdmissionPolicy::Deny => {}
        }
    }
    Ok(())
}

/// Validates a bot slug (`^[a-z0-9][a-z0-9-]{0,31}$`).
pub fn valid_bot_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[derive(Clone)]
pub struct BotHandle {
    pub id: String,
    pub generation: i64,
    pub store: Arc<SqliteStorage>,
    pub workspace: harness::PinnedWorkspace,
    pub discord: Arc<messenger_discord::Discord>,
    pub webchat: Arc<crate::webchat::WebChat>,
    pub live_chat: LiveChat,
}

struct BotEntry {
    handle: BotHandle,
    workspace_path: PathBuf,
    running: Option<(
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    )>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Manages every bot running inside one `blob-1 serve` process. The `'default'`
/// bot uses the root `agent.db` and `workspace/` in place; additional bots live
/// under `<data_dir>/bots/<id>/{agent.db, workspace/}` while sharing global
/// providers, model catalog, and the MCP connection pool.
#[derive(Clone)]
pub struct BotRegistry {
    global_store: Arc<SqliteStorage>,
    data_dir: PathBuf,
    app_key: Option<storage_sqlite::AppKey>,
    root_workspace: PathBuf,
    root_pinned_workspace: harness::PinnedWorkspace,
    skills_dir: Option<PathBuf>,
    mcp: mcp::Mcp,
    nodes: Arc<crate::nodes::NodeRegistry>,
    /// Per-env serialization of remote skill pushes: bots sharing a node
    /// would otherwise race the same node-wide skills dir on reconnect.
    /// env name is unique per node, so one lock per env = one per node.
    skill_sync: Arc<dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    bots: Arc<tokio::sync::RwLock<HashMap<String, BotEntry>>>,
    /// Writes to web-visible bot data, fanned out to `/ws` clients.
    changes: tokio::sync::broadcast::Sender<Change>,
}

/// A bot's web-visible data changed; `/ws` clients refetch `topic`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Change {
    pub bot_id: String,
    pub topic: &'static str,
}

/// The web data topic a bot-store table feeds, or `None` when no page shows it.
fn change_topic(table: &str) -> Option<&'static str> {
    match table {
        "schedules" | "watches" | "checkin" | "clock" => Some("schedule"),
        "usage_events" => Some("usage"),
        "webhooks" => Some("webhooks"),
        "secrets" => Some("secrets"),
        "web_messages" => Some("chat"),
        _ => None,
    }
}

impl BotRegistry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        global_store: Arc<SqliteStorage>,
        data_dir: PathBuf,
        app_key: Option<storage_sqlite::AppKey>,
        root_workspace: PathBuf,
        root_pinned_workspace: harness::PinnedWorkspace,
        skills_dir: Option<PathBuf>,
        mcp: mcp::Mcp,
        nodes: Arc<crate::nodes::NodeRegistry>,
    ) -> Self {
        Self {
            global_store,
            data_dir,
            app_key,
            root_workspace,
            root_pinned_workspace,
            skills_dir,
            mcp,
            nodes,
            skill_sync: Arc::new(dashmap::DashMap::new()),
            bots: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            changes: tokio::sync::broadcast::channel(256).0,
        }
    }

    /// Subscribes to bot data changes. Model tools, tasks, and the web all
    /// write through the bot stores, so every source shows up here.
    pub fn changes(&self) -> tokio::sync::broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    fn bot_dir(&self, bot_id: &str) -> PathBuf {
        self.data_dir.join("bots").join(bot_id)
    }

    fn open_entry(&self, bot: &Bot) -> Result<BotEntry> {
        let (store, workspace_path, workspace) = if bot.id == DEFAULT_BOT_ID {
            (
                self.global_store.clone(),
                self.root_workspace.clone(),
                self.root_pinned_workspace.clone(),
            )
        } else {
            let dir = self.bot_dir(&bot.id);
            // Recovery: a delete rollback that could not move the directory back
            // leaves the real data in `.trash-<id>-<gen>-*`. Restore it before
            // creating anything so a restart never shadows it with a fresh DB.
            // The generation in the name keeps a recreated slug from picking up
            // a prior instance's archive.
            if !dir.exists() {
                let prefix = format!(".trash-{}-{}-", bot.id, bot.generation);
                if let Ok(entries) = std::fs::read_dir(self.data_dir.join("bots")) {
                    for entry in entries.flatten() {
                        if entry.file_name().to_string_lossy().starts_with(&prefix) {
                            match std::fs::rename(entry.path(), &dir) {
                                Ok(()) => eprintln!(
                                    "recovered bot '{}' directory from {}",
                                    bot.id,
                                    entry.path().display()
                                ),
                                // A matching archive exists but could not be moved
                                // back — opening a fresh DB would shadow it.
                                Err(error) => {
                                    return Err(format!(
                                        "failed to recover bot '{}' directory from {}: {error}",
                                        bot.id,
                                        entry.path().display()
                                    )
                                    .into())
                                }
                            }
                            break;
                        }
                    }
                }
            }
            let ws = dir.join("workspace");
            std::fs::create_dir_all(&ws)?;
            let ws = ws.canonicalize()?;
            let store = Arc::new(SqliteStorage::open(
                dir.join("agent.db"),
                self.app_key.clone(),
            )?);
            let pinned = harness::PinnedWorkspace::open(&ws)?;
            (store, ws, pinned)
        };
        let changes = self.changes.clone();
        let bot_id = bot.id.clone();
        store.on_change(move |table| {
            if let Some(topic) = change_topic(table) {
                // No subscribers is the normal case without an open web UI.
                let _ = changes.send(Change {
                    bot_id: bot_id.clone(),
                    topic,
                });
            }
        })?;
        let webchat = Arc::new(crate::webchat::WebChat::new(store.clone(), {
            let changes = self.changes.clone();
            let bot_id = bot.id.clone();
            move || {
                let _ = changes.send(Change {
                    bot_id: bot_id.clone(),
                    topic: "chat",
                });
            }
        }));
        Ok(BotEntry {
            handle: BotHandle {
                id: bot.id.clone(),
                generation: bot.generation,
                store,
                workspace,
                discord: Arc::new(messenger_discord::Discord::new()),
                webchat,
                live_chat: LiveChat::default(),
            },
            workspace_path,
            running: None,
        })
    }

    fn spawn_bot_loop(&self, entry: &mut BotEntry) {
        if let Some((_, task)) = &entry.running {
            if !task.is_finished() {
                return;
            }
        }
        entry.running = None;
        let (tx, rx) = tokio::sync::watch::channel(false);
        let global_store = self.global_store.clone();
        let bot_store = entry.handle.store.clone();
        let bot_id = entry.handle.id.clone();
        // Remote workspace dir on the node. The generation keeps a recreated
        // slug from inheriting a deleted bot's files, as the local trash does.
        let remote_workspace = format!("{}-{}", entry.handle.id, entry.handle.generation);
        // Local secret files, per bot generation like the remote workspace.
        let secrets_dir = self.data_dir.join("secrets").join(&remote_workspace);
        let ws_path = entry.workspace_path.clone();
        let pinned = entry.handle.workspace.clone();
        let skills_dir = self.skills_dir.clone();
        let discord = entry.handle.discord.clone();
        let webchat = entry.handle.webchat.clone();
        let mcp = self.mcp.clone();
        let live_chat = entry.handle.live_chat.clone();
        let nodes = self.nodes.clone();
        let skill_sync = self.skill_sync.clone();
        let task = tokio::spawn(async move {
            // A bot bound to a remote environment runs only while its node
            // session is authenticated; env drops pause the whole bot and a
            // reconnect resumes it with a generic notice. The binding is
            // re-read every iteration so env renames/rebinds propagate.
            let result: Result<()> = async {
                'supervised: loop {
                    // Binding name and the bound env's enrollment ID come
                    // from one query — a revoke+re-accept between reads
                    // can't split them and pin the replacement's identity.
                    let (env_name, pin) = global_store
                        .bot_env_binding(&bot_id)?
                        .unwrap_or_else(|| (storage::LOCAL_ENVIRONMENT.to_owned(), None));
                    let mut env_online = (env_name != storage::LOCAL_ENVIRONMENT)
                        .then(|| nodes.availability(&env_name));
                    let mut env_was_down = false;
                    if let Some(online) = &mut env_online {
                        let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
                        tick.tick().await; // skip the immediate first tick
                                           // Wait until the routed enrollment IS the bound one:
                                           // a re-enrolled name (even same node) isn't online
                                           // for this binding.
                        while pin.is_none() || online.borrow().as_deref() != pin.as_deref() {
                            env_was_down = true;
                            let mut shutdown = rx.clone();
                            tokio::select! {
                                _ = shutdown.changed() => break 'supervised Ok(()),
                                changed = online.changed() => {
                                    if changed.is_err() {
                                        break 'supervised Ok(());
                                    }
                                }
                                _ = tick.tick() => {
                                    let current = global_store
                                        .bot_env_binding(&bot_id)
                                        .ok()
                                        .flatten();
                                    if current.as_ref() != Some(&(env_name.clone(), pin.clone())) {
                                        continue 'supervised;
                                    }
                                }
                            }
                        }
                        if env_was_down {
                            let _ = bot_store.add_pending_event(
                                "system",
                                "environment-resumed",
                                None,
                                "The execution environment was unreachable and is back online now.",
                                now(),
                            );
                        }
                    }
                    let (environment, env_watch): (
                        Arc<dyn environment::Environment>,
                        Option<tokio::sync::watch::Receiver<bool>>,
                    ) = match &env_online {
                        Some(_) => {
                            // Pinned to the bound enrollment: a re-enrolled
                            // name flips its derived watch offline and fails
                            // its calls until the supervisor rebinds.
                            let remote = environment_remote::RemoteEnvironment::new(
                                env_name.clone(),
                                remote_workspace.clone(),
                                nodes.clone(),
                                pin.clone(),
                            );
                            let watch = remote.availability();
                            (Arc::new(remote), Some(watch))
                        }
                        None => (
                            Arc::new(
                                LocalEnvironment::new(ws_path.clone(), skills_dir.clone())?
                                    .with_secrets_dir(secrets_dir.clone()),
                            ),
                            None,
                        ),
                    };
                    // Remote skills dirs are node-wide; the global catalog is
                    // the single authoritative set pushed on each (re)connect,
                    // matching the local shared-dir model. The per-env lock
                    // stops bots sharing one node from racing the same dir.
                    if env_online.is_some() {
                        let lock = skill_sync
                            .entry(env_name.clone())
                            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                            .clone();
                        if *rx.borrow() {
                            break 'supervised Ok(());
                        }
                        let mut shutdown = rx.clone();
                        let _sync = tokio::select! {
                            _ = shutdown.changed() => break 'supervised Ok(()),
                            guard = lock.lock() => guard,
                        };
                        // Shutdown interrupts the sync too — a held lock
                        // shouldn't stall a stop (e.g. web rename) by two
                        // RPC timeouts.
                        let result = tokio::select! {
                            _ = shutdown.changed() => break 'supervised Ok(()),
                            result = sync_shared_skills(&global_store, &environment) => result,
                        };
                        if let Err(error) = result {
                            eprintln!("bot {bot_id} remote skill sync failed: {error}");
                        }
                    }
                    // Remote bots run in the node's `<workspaces>/<bot_id>-<generation>`
                    // (reported in `hello`); `share`/attachments stay off
                    // since they read the primary's disk.
                    let remote = env_online.is_some();
                    let run_workspace = match remote {
                        true => nodes.workspace_root(&env_name, &remote_workspace),
                        false => ws_path.clone(),
                    };
                    let result = serve_bot(
                        global_store.clone(),
                        bot_store.clone(),
                        bot_id.clone(),
                        &run_workspace,
                        (!remote).then(|| pinned.clone()),
                        environment,
                        env_watch,
                        discord.clone(),
                        webchat.clone(),
                        rx.clone(),
                        mcp.clone(),
                        live_chat.clone(),
                    )
                    .await;
                    discord.disconnect();
                    match result {
                        Err(error) => break 'supervised Err(error),
                        Ok(()) => {
                            if *rx.borrow() {
                                break 'supervised Ok(());
                            }
                            // Otherwise the env dropped — loop back to wait for it.
                        }
                    }
                }
            }
            .await;
            if let Err(error) = result {
                eprintln!("bot {bot_id} stopped with error: {error}");
            }
        });
        entry.running = Some((tx, task));
    }

    pub async fn start_all(&self) -> Result<()> {
        // Secret files outlive no restart: their deletion timers died with the
        // previous process.
        environment_local::clear_secrets(&self.data_dir.join("secrets"))?;
        let environment: Arc<dyn environment::Environment> = Arc::new(LocalEnvironment::new(
            &self.root_workspace,
            self.skills_dir.clone(),
        )?);
        sync_shared_skills(&self.global_store, &environment).await?;
        let bots = self.global_store.bots()?;
        let mut map = self.bots.write().await;
        for bot in bots {
            let mut entry = self.open_entry(&bot)?;
            if bot.enabled {
                self.spawn_bot_loop(&mut entry);
            }
            map.insert(bot.id, entry);
        }
        Ok(())
    }

    #[cfg(test)]
    pub async fn register_default_for_test(&self) -> Result<()> {
        let bots = self.global_store.bots()?;
        let mut map = self.bots.write().await;
        for bot in bots {
            let entry = self.open_entry(&bot)?;
            map.insert(bot.id, entry);
        }
        Ok(())
    }

    pub async fn get(&self, bot_id: &str) -> Result<BotHandle> {
        let map = self.bots.read().await;
        map.get(bot_id)
            .map(|e| e.handle.clone())
            .ok_or_else(|| format!("Unknown bot {bot_id}").into())
    }

    pub async fn all_handles(&self) -> Vec<BotHandle> {
        self.bots
            .read()
            .await
            .values()
            .map(|e| e.handle.clone())
            .collect()
    }

    pub async fn create(&self, id: &str, name: &str) -> Result<Bot> {
        if !valid_bot_id(id) {
            return Err(
                "Bot ID must be 1–32 lowercase letters, digits, or '-' (starting with a letter or digit)"
                    .into(),
            );
        }
        if id == DEFAULT_BOT_ID {
            return Err("Bot ID 'default' is reserved".into());
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err("Bot name must be 1–64 characters".into());
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(1);
        let mut map = self.bots.write().await;
        if map.contains_key(id) {
            return Err(format!("Bot {id} already exists").into());
        }
        let bot_dir = self.bot_dir(id);
        if bot_dir.exists() {
            return Err(format!(
                "Directory for bot '{id}' already exists on disk; remove it before reusing this ID"
            )
            .into());
        }
        let max_created = self
            .global_store
            .bots()?
            .into_iter()
            .map(|b| b.created_at)
            .max()
            .unwrap_or(0);
        let created_at = now_ms.max(max_created + 1);
        let bot = self.global_store.create_bot(id, name, created_at)?;
        let mut entry = match self.open_entry(&bot) {
            Ok(entry) => entry,
            Err(error) => {
                let _ = self.global_store.remove_bot(id);
                let _ = std::fs::remove_dir_all(&bot_dir);
                return Err(error);
            }
        };
        let _ = entry
            .handle
            .store
            .set_instructions(Some(&format!("# Identity\n\nYou are {name}")));
        self.spawn_bot_loop(&mut entry);
        map.insert(bot.id.clone(), entry);
        Ok(bot)
    }

    pub async fn update(&self, id: &str, name: Option<&str>, enabled: Option<bool>) -> Result<Bot> {
        if let Some(name) = name {
            let trimmed = name.trim();
            if trimmed.is_empty() || trimmed.chars().count() > 64 {
                return Err("Bot name must be 1–64 characters".into());
            }
        }
        let mut stop_task = None;
        let updated = {
            let mut map = self.bots.write().await;
            let Some(entry) = map.get_mut(id) else {
                return Err(format!("Unknown bot {id}").into());
            };
            let trimmed_name = name.map(str::trim);
            let Some(bot) = self.global_store.update_bot(id, trimmed_name, enabled)? else {
                return Err(format!("Unknown bot {id}").into());
            };
            if bot.enabled {
                self.spawn_bot_loop(entry);
            } else if let Some((tx, task)) = entry.running.take() {
                let _ = tx.send(true);
                stop_task = Some(task);
            }
            bot
        };
        if let Some(task) = stop_task {
            let _ = task.await;
        }
        Ok(updated)
    }

    /// Rebinds a bot to another execution environment. The running loop is
    /// restarted and the bot gets a generic workspace-reset notice — files do
    /// not follow the bot between machines.
    pub async fn set_environment(&self, id: &str, environment: Option<&str>) -> Result<Bot> {
        let mut stop_task = None;
        let bot = {
            let mut map = self.bots.write().await;
            let Some(entry) = map.get_mut(id) else {
                return Err(format!("Unknown bot {id}").into());
            };
            if !self.global_store.set_bot_environment(id, environment)? {
                return Err(format!("Unknown bot {id}").into());
            }
            let _ = entry.handle.store.add_pending_event(
                "system",
                "environment-changed",
                None,
                "The execution environment changed; the workspace was reset and previous files are not available.",
                now(),
            );
            let bot = self
                .global_store
                .bot(id)?
                .ok_or_else(|| format!("Unknown bot {id}"))?;
            if bot.enabled {
                if let Some((tx, task)) = entry.running.take() {
                    let _ = tx.send(true);
                    stop_task = Some(task);
                }
            }
            bot
        };
        if let Some(task) = stop_task {
            let _ = task.await;
        }
        if bot.enabled {
            let mut map = self.bots.write().await;
            if let Some(entry) = map.get_mut(id) {
                self.spawn_bot_loop(entry);
            }
        }
        Ok(bot)
    }

    /// Restarts enabled bots bound to `env` — e.g. after the env is renamed
    /// (storage cascades the binding, so each loop respawns on the new name).
    pub async fn restart_environment(&self, env: &str) -> Result<()> {
        let ids: Vec<String> = self
            .global_store
            .bots()?
            .into_iter()
            .filter(|b| b.environment == env && b.enabled)
            .map(|b| b.id)
            .collect();
        let mut stops = Vec::new();
        {
            let mut map = self.bots.write().await;
            for id in &ids {
                if let Some(entry) = map.get_mut(id) {
                    if let Some((tx, task)) = entry.running.take() {
                        let _ = tx.send(true);
                        stops.push(task);
                    }
                }
            }
        }
        for task in stops {
            let _ = task.await;
        }
        let mut map = self.bots.write().await;
        for id in &ids {
            if let Some(entry) = map.get_mut(id) {
                self.spawn_bot_loop(entry);
            }
        }
        Ok(())
    }

    pub async fn remove(&self, id: &str) -> Result<()> {
        if id == DEFAULT_BOT_ID {
            return Err("Cannot remove the default bot".into());
        }
        let removed = {
            let mut map = self.bots.write().await;
            map.remove(id)
        };
        let Some(mut entry) = removed else {
            return Err(format!("Unknown bot {id}").into());
        };
        if let Some((tx, task)) = entry.running.take() {
            let _ = tx.send(true);
            let _ = task.await;
        }
        entry.handle.discord.disconnect();
        entry.handle.live_chat.close().await;
        let generation = entry.handle.generation;
        drop(entry);

        // Atomically move the bot directory out of the live namespace BEFORE
        // deleting the roster row so a recreated slug can never inherit stale data.
        // The generation in the trash name lets a later open recover this exact
        // instance (and only this instance) after a failed rollback.
        let dir = self.bot_dir(id);
        let trash = if dir.exists() {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let target = self
                .data_dir
                .join("bots")
                .join(format!(".trash-{id}-{generation}-{nonce}"));
            if let Err(error) = std::fs::rename(&dir, &target) {
                self.restore_entry(id).await;
                return Err(format!("Failed to archive bot directory for '{id}': {error}").into());
            }
            Some(target)
        } else {
            None
        };
        if let Err(error) = self.global_store.remove_bot(id) {
            let mut restored = trash.is_none();
            if let Some(trash_dir) = &trash {
                match std::fs::rename(trash_dir, &dir) {
                    Ok(()) => restored = true,
                    Err(restore_err) => eprintln!(
                        "warning: failed to restore bot directory {} after DB error: {restore_err}; data remains at {}",
                        dir.display(),
                        trash_dir.display()
                    ),
                }
            }
            // Reopen only when the bot's data is actually back at its live path —
            // opening otherwise would shadow the archived database with a fresh one.
            if restored {
                self.restore_entry(id).await;
            }
            return Err(error.into());
        }
        if let Some(trash_dir) = trash {
            if let Err(error) = std::fs::remove_dir_all(&trash_dir) {
                eprintln!(
                    "warning: failed to clean up deleted bot archive {}: {error}",
                    trash_dir.display()
                );
            }
        }
        Ok(())
    }

    /// Reopens a bot whose removal rolled back, keeping the roster row live.
    /// Failures leave the row without a registry entry (logged; a restart reopens it).
    async fn restore_entry(&self, id: &str) {
        match self.global_store.bot(id) {
            Ok(Some(bot)) => match self.open_entry(&bot) {
                Ok(mut reopened) => {
                    if bot.enabled {
                        self.spawn_bot_loop(&mut reopened);
                    }
                    self.bots.write().await.insert(bot.id, reopened);
                }
                Err(error) => {
                    eprintln!("warning: failed to reopen bot '{id}' after rollback: {error}")
                }
            },
            Ok(None) => {}
            Err(error) => {
                eprintln!("warning: failed to reload bot '{id}' after rollback: {error}")
            }
        }
    }

    pub async fn shutdown_all(&self) {
        let tasks: Vec<tokio::task::JoinHandle<()>> = {
            let mut map = self.bots.write().await;
            map.values_mut()
                .filter_map(|entry| {
                    entry.running.take().map(|(tx, task)| {
                        let _ = tx.send(true);
                        task
                    })
                })
                .collect()
        };
        for task in tasks {
            let _ = task.await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn serve_bot(
    global_store: Arc<SqliteStorage>,
    store: Arc<SqliteStorage>,
    bot_id: String,
    workspace: &Path,
    pinned_workspace: Option<harness::PinnedWorkspace>,
    environment: Arc<dyn environment::Environment>,
    env_online: Option<tokio::sync::watch::Receiver<bool>>,
    discord: Arc<messenger_discord::Discord>,
    webchat: Arc<crate::webchat::WebChat>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    mcp: mcp::Mcp,
    live_chat: LiveChat,
) -> Result<()> {
    let models = ModelFactory::new_for_bot(global_store.clone(), store.clone(), bot_id.clone());
    let harness = chat_harness(
        global_store.clone(),
        store.clone(),
        bot_id.clone(),
        workspace,
        pinned_workspace.clone(),
        environment,
        models.clone(),
        mcp.clone(),
    )
    .await?;
    let outgoing: Arc<dyn messenger::Messenger> = discord.clone();
    let messengers = harness::Messengers::new(
        vec![outgoing.clone(), webchat.clone() as Arc<dyn messenger::Messenger>],
        store.clone(),
    );
    let messengers = match pinned_workspace {
        Some(pinned) => messengers.with_workspace(pinned),
        None => messengers,
    };
    let messengers = messengers.with_utility({
        let models = models.clone();
        move || models.current().ok().and_then(|current| current.utility)
    });
    let harness = Arc::new(
        harness
            .with_messengers(messengers.clone())
            .with_schedule_gate({
                let models = models.clone();
                let discord = discord.clone();
                let store = store.clone();
                // Due jobs wait for Discord only when it is configured and
                // the web chat isn't a home: with a web home, jobs can always
                // report somewhere.
                move || {
                    models.current().is_ok()
                        && (matches!(
                            discord.connection(),
                            messenger_discord::Connection::Online { .. }
                        ) || store.messenger_token("discord").ok().flatten().is_none()
                            || store.homes().is_ok_and(|homes| {
                                homes.iter().any(|h| h.messenger == crate::webchat::NAME)
                            }))
                }
            }),
    );
    live_chat.set(Arc::clone(&harness), models.clone()).await;
    // The cheap per-message gate the gateway runs before any Discord REST
    // call: evaluate the access policy and exact channel/thread/user
    // interaction overrides, asking for parent resolution only when an
    // uncached ID could inherit a parent channel rule.
    {
        let filter_store = store.clone();
        let filter_global = global_store.clone();
        let filter_bot = bot_id.clone();
        discord.set_pre_filter(Arc::new(
            move |author: &str,
                  guild: Option<&str>,
                  channel: &str,
                  thread: Option<&str>,
                  addressed: bool,
                  channel_known: bool| {
                use messenger_discord::PreFilterDecision;
                let default_access = match filter_store.default_access("discord") {
                    Ok(effect) => effect,
                    Err(error) => {
                        eprintln!("{error}");
                        return PreFilterDecision::Reject;
                    }
                };
                let access_rules = match filter_store.access_rules("discord") {
                    Ok(rules) => rules,
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
                let has_utility = match filter_global.bot_models(&filter_bot) {
                    Ok(bm) => bm.utility.is_some() || bm.decision.is_some(),
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
                    default_access,
                    &access_rules,
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
    let mut listener = Some(tokio::spawn(
        async move { listener_discord.listen(tx).await },
    ));
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
    // Web chat posts skip Discord's admission rules: the owner is always
    // addressing the bot, so each goes straight to the chat loop.
    let web_posts = webchat.attach();
    let web_admission = {
        let store = store.clone();
        let messengers = messengers.clone();
        let admitted = admitted_tx.clone();
        let mut posts = web_posts;
        tokio::spawn(async move {
            while let Some(post) = posts.recv().await {
                let place = crate::webchat::place();
                let admitted_post = store
                    .destination(crate::webchat::NAME, &place)
                    .map(|to| {
                        messengers.live(crate::webchat::NAME, to, place.clone());
                        (
                            post.text,
                            storage::Meta::Message {
                                messenger: crate::webchat::NAME.into(),
                                place,
                                message: post.id,
                                author: crate::webchat::owner(),
                                to,
                                inline: true,
                                unaddressed: false,
                                location: "web chat".into(),
                                earlier: None,
                                quote: None,
                                attachments: Vec::new(),
                                missed: Vec::new(),
                                gap_open: false,
                            },
                        )
                    })
                    .map_err(Into::into);
                if admitted.send(admitted_post).await.is_err() {
                    break;
                }
            }
        })
    };
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
        let watermarks: Watermarks = Watermarks::default();
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
            let watermarks = watermarks.clone();
            let tx = admitted_tx.clone();
            inflight.spawn(async move {
                let Ok(_permit) = limit.acquire_owned().await else {
                    return place;
                };
                if let Err(error) = admit_batch(
                    &store,
                    discord,
                    &messengers,
                    batch,
                    utility.as_ref(),
                    &watermarks,
                    &tx,
                )
                .await
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
    let outbox_notify = live_chat.outbox_notify();
    // A remote environment dropping pauses the whole bot — the loop exits and
    // the spawner re-runs it once the env session comes back.
    let env_offline = async {
        match env_online {
            Some(mut online) => {
                while *online.borrow() {
                    if online.changed().await.is_err() {
                        break;
                    }
                }
            }
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(env_offline);
    let loop_result: Result<()> = tokio::select! {
        result = async {
    'serve: loop {
        // Register outbox listener BEFORE querying SQLite so a callback commit +
        // `notify_one()` racing with an empty query can never miss a wakeup.
        let outbox_wait = outbox_notify.notified();
        tokio::pin!(outbox_wait);
        outbox_wait.as_mut().enable();
        let mut active_pending_event_id: Option<i64> = None;
        let next = if let Some(next) = pending.pop_front() {
            Some(next)
        } else if let Some(ev) = store.next_pending_event()? {
            if backoff {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            active_pending_event_id = Some(ev.id);
            Some((
                ev.payload,
                Some(storage::Meta::Event {
                    source: ev.source,
                    name: ev.name,
                    for_message: ev.for_token,
                }),
            ))
        } else {
            tokio::select! {
                value = admitted_rx.recv() => match value {
                    Some(next) => { let (text, meta) = next?; Some((text, Some(meta))) },
                    None => break match listener.take().expect("listener running").await? { Err(error) => Err(error.into()), Ok(()) => Err("discord listener stopped".into()) },
                },
                _ = &mut outbox_wait => continue,
                _ = async {
                    if backoff {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                    harness.work().await
                } => None,
                _ = shutdown.changed() => break Ok(()),
                _ = &mut env_offline => break Ok(()),
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
        let mut shutting_down = false;
        loop {
            tokio::select! {
                result = &mut run => {
                    turn_active.store(false, std::sync::atomic::Ordering::Relaxed);
                    backoff = result.is_err();
                    match result {
                        Ok(answer) => {
                            if let Some(ev_id) = active_pending_event_id {
                                store.delete_pending_event(ev_id)?;
                            }
                            if !answer.is_empty() {
                                println!("{answer}");
                            }
                        }
                        Err(error) => eprintln!("{error}"),
                    }
                    break;
                },
                value = admitted_rx.recv() => match value {
                    Some(next) => { let (text, meta) = next?; harness.steer_message(text, Some(meta)); },
                    None => {
                        let listener_err = match listener.take().expect("listener running").await? {
                            Err(error) => Err(error.into()),
                            Ok(()) => Err("discord listener stopped".into()),
                        };
                        break 'serve listener_err;
                    },
                },
                _ = shutdown.changed() => {
                    shutting_down = true;
                    break;
                },
                _ = &mut env_offline => {
                    shutting_down = true;
                    break;
                },
            }
        }
        if shutting_down {
            break Ok(());
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
    };
    live_chat.close().await;
    webchat.detach();
    web_admission.abort();
    discord.disconnect();
    if let Some(listener) = listener.take() {
        listener.abort();
        let _ = listener.await;
    }
    admission.abort();
    supervisor.abort();
    let _ = admission.await;
    let _ = supervisor.await;
    harness.shutdown().await;
    loop_result
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
        use storage::{AccessEffect, AccessRule, AccessScope};
        let rule = |user: Option<&str>,
                    scope: AccessScope,
                    scope_id: Option<&str>,
                    effect: AccessEffect| AccessRule {
            user_id: user.map(Into::into),
            scope,
            scope_id: scope_id.map(Into::into),
            effect,
        };
        let access = vec![
            rule(Some("u1"), AccessScope::Global, None, AccessEffect::Allow),
            rule(
                Some("u2"),
                AccessScope::Server,
                Some("g1"),
                AccessEffect::Allow,
            ),
            rule(
                Some("u3"),
                AccessScope::Channel,
                Some("c1"),
                AccessEffect::Allow,
            ),
        ];
        let overrides = vec![
            channel_override("c1", InteractionMode::Always),
            channel_override("t1", InteractionMode::MentionOnly),
            user_override("g1", "u1", InteractionMode::Always),
        ];
        // User override applies in g1, not in g2.
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Block,
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
                AccessEffect::Block,
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
                AccessEffect::Block,
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
                AccessEffect::Block,
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
                AccessEffect::Block,
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
        // Known thread under c1 allows directly for server-allowed u2.
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Block,
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
        // Channel-allowed u3 in an uncached thread asks for parent even when addressed.
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Block,
                &access,
                &overrides,
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
        // Unlisted u9 rejects immediately with no REST.
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Block,
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
        // Default Allow + channel-blocked user in uncached thread resolves parent once.
        let bl = vec![rule(
            Some("u4"),
            AccessScope::Channel,
            Some("c1"),
            AccessEffect::Block,
        )];
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Allow,
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
                AccessEffect::Allow,
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
        // Server interaction override covers known channels without REST.
        let with_server = vec![server_override("g1", InteractionMode::Always)];
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Block,
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
        let with_server = vec![server_override("g1", InteractionMode::MentionOnly)];
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Block,
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
                AccessEffect::Block,
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
                AccessEffect::Block,
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
                AccessEffect::Block,
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
    fn unified_access_precedence_and_need_parent() {
        use messenger_discord::PreFilterDecision;
        use storage::{AccessEffect, AccessRule, AccessScope};
        let rule = |user: Option<&str>,
                    scope: AccessScope,
                    scope_id: Option<&str>,
                    effect: AccessEffect| AccessRule {
            user_id: user.map(Into::into),
            scope,
            scope_id: scope_id.map(Into::into),
            effect,
        };

        // Full 8-step precedence ladder:
        // 1. user in thread (Allow) > 2. user in parent channel (Block) > 3. user in server (Allow)
        // > 4. user global (Block) > 5. everyone in thread (Allow) > 6. everyone in parent channel (Block)
        // > 7. everyone in server (Allow) > 8. default (Block).
        let rules = vec![
            rule(
                Some("u1"),
                AccessScope::Channel,
                Some("t1"),
                AccessEffect::Allow,
            ),
            rule(
                Some("u1"),
                AccessScope::Channel,
                Some("c1"),
                AccessEffect::Block,
            ),
            rule(
                Some("u1"),
                AccessScope::Server,
                Some("g1"),
                AccessEffect::Allow,
            ),
            rule(Some("u1"), AccessScope::Global, None, AccessEffect::Block),
            rule(None, AccessScope::Channel, Some("t1"), AccessEffect::Allow),
            rule(
                None,
                AccessScope::Channel,
                Some("t_all"),
                AccessEffect::Allow,
            ),
            rule(None, AccessScope::Channel, Some("c1"), AccessEffect::Block),
            rule(None, AccessScope::Server, Some("g1"), AccessEffect::Allow),
        ];

        // Step 1: user in thread beats step 2 (user in parent channel).
        assert!(access_allowed(
            AccessEffect::Block,
            &rules,
            "u1",
            Some("g1"),
            "c1",
            Some("t1"),
            false
        ));
        // Step 2: user in parent channel beats step 3 (user in server).
        assert!(!access_allowed(
            AccessEffect::Block,
            &rules,
            "u1",
            Some("g1"),
            "c1",
            Some("t2"),
            false
        ));
        // Step 3: user in server beats step 4 (user global).
        assert!(access_allowed(
            AccessEffect::Block,
            &rules,
            "u1",
            Some("g1"),
            "c2",
            None,
            false
        ));
        // Step 4: user global beats steps 5-7 (everyone rules).
        assert!(!access_allowed(
            AccessEffect::Block,
            &rules,
            "u1",
            Some("g2"),
            "c2",
            Some("t_all"),
            false
        ));
        // Step 5: everyone in thread beats step 6 (everyone in parent channel).
        assert!(access_allowed(
            AccessEffect::Block,
            &rules,
            "u_other",
            Some("g1"),
            "c1",
            Some("t1"),
            false
        ));
        // Step 6: everyone in parent channel beats step 7 (everyone in server).
        assert!(!access_allowed(
            AccessEffect::Block,
            &rules,
            "u_other",
            Some("g1"),
            "c1",
            Some("t2"),
            false
        ));
        // Step 7: everyone in server beats step 8 (default Block).
        assert!(access_allowed(
            AccessEffect::Block,
            &rules,
            "u_other",
            Some("g1"),
            "c2",
            None,
            false
        ));
        // Step 8: falls back to default.
        assert!(!access_allowed(
            AccessEffect::Block,
            &rules,
            "u_other",
            Some("g2"),
            "c2",
            None,
            false
        ));
        assert!(access_allowed(
            AccessEffect::Allow,
            &rules,
            "u_other",
            Some("g2"),
            "c2",
            None,
            false
        ));

        // DMs only check step 4 (user global) then step 8 (default), ignoring server/channel rules.
        let dm_rules = vec![
            rule(
                Some("u_global_allow"),
                AccessScope::Global,
                None,
                AccessEffect::Allow,
            ),
            rule(
                Some("u_global_block"),
                AccessScope::Global,
                None,
                AccessEffect::Block,
            ),
            rule(
                Some("u_server_only"),
                AccessScope::Server,
                Some("g1"),
                AccessEffect::Allow,
            ),
            rule(None, AccessScope::Channel, Some("dm1"), AccessEffect::Allow),
        ];
        assert!(access_allowed(
            AccessEffect::Block,
            &dm_rules,
            "u_global_allow",
            None,
            "dm1",
            None,
            true
        ));
        assert!(!access_allowed(
            AccessEffect::Allow,
            &dm_rules,
            "u_global_block",
            None,
            "dm1",
            None,
            true
        ));
        assert!(!access_allowed(
            AccessEffect::Block,
            &dm_rules,
            "u_server_only",
            None,
            "dm1",
            None,
            true
        ));
        assert!(!access_allowed(
            AccessEffect::Block,
            &dm_rules,
            "anyone",
            None,
            "dm1",
            None,
            true
        ));
        assert!(access_allowed(
            AccessEffect::Allow,
            &dm_rules,
            "anyone",
            None,
            "dm1",
            None,
            true
        ));

        // Pre-filter NeedParent precision:
        // 1) Step 4 (user global Allow) beats Step 6 (everyone channel Block) -> NO NeedParent!
        let global_vs_chan = vec![
            rule(Some("u1"), AccessScope::Global, None, AccessEffect::Allow),
            rule(
                None,
                AccessScope::Channel,
                Some("c_bad"),
                AccessEffect::Block,
            ),
        ];
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Block,
                &global_vs_chan,
                &[],
                "u1",
                Some("g1"),
                "t_uncached",
                None,
                true,
                false,
                InteractionMode::MentionOnly,
                false,
            ),
            PreFilterDecision::Allow
        );
        // 2) For an unlisted user under Default Allow, everyone channel Block CAN flip -> NeedParent!
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Allow,
                &global_vs_chan,
                &[],
                "u_other",
                Some("g1"),
                "t_uncached",
                None,
                true,
                false,
                InteractionMode::MentionOnly,
                false,
            ),
            PreFilterDecision::NeedParent
        );
        // 3) Step 1 (direct user rule on thread id) beats Step 2 (user parent channel) -> NO NeedParent!
        let thread_vs_parent = vec![
            rule(
                Some("u1"),
                AccessScope::Channel,
                Some("t1"),
                AccessEffect::Allow,
            ),
            rule(
                Some("u1"),
                AccessScope::Channel,
                Some("c1"),
                AccessEffect::Block,
            ),
        ];
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Block,
                &thread_vs_parent,
                &[],
                "u1",
                Some("g1"),
                "t1",
                None,
                true,
                false,
                InteractionMode::MentionOnly,
                false,
            ),
            PreFilterDecision::Allow
        );
        // 4) Step 5 (direct everyone rule on thread id) cannot be beaten by Step 6 (everyone parent channel) -> NO NeedParent!
        let everyone_thread_vs_parent = vec![
            rule(None, AccessScope::Channel, Some("t1"), AccessEffect::Allow),
            rule(None, AccessScope::Channel, Some("c1"), AccessEffect::Block),
        ];
        assert_eq!(
            pre_filter_decision(
                AccessEffect::Block,
                &everyone_thread_vs_parent,
                &[],
                "u_other",
                Some("g1"),
                "t1",
                None,
                true,
                false,
                InteractionMode::MentionOnly,
                false,
            ),
            PreFilterDecision::Allow
        );
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
                    bot: false,
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
