use std::path::Path;
use std::sync::Arc;

use environment::{Environment, SkillBundle};
use environment_local::LocalEnvironment;
use harness::{ChatHarness, CompactionConfig, EnvironmentPaths, HarnessConfig, SessionHarness};
use storage::{ConfigStore, ReasoningLevel, SessionId, SkillStore};
use storage_sqlite::SqliteStorage;
use tokio::signal::unix::{signal, SignalKind};

use crate::{api_key, Result};

struct ModelFactory {
    store: Arc<SqliteStorage>,
    default_reasoning: ReasoningLevel,
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
                self.default_reasoning.as_str()
            ),
            match settings.compaction_model {
                Some(model) => format!(
                    "compaction: provider={} model={}",
                    model.provider.0, model.model
                ),
                None => "compaction: default".into(),
            },
            "provider  model  context  reasoning".into(),
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
                    "{}  {}  {}  {}",
                    provider.id.0,
                    model.id,
                    context,
                    if levels.is_empty() { "-" } else { &levels }
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
        let settings = self.store.settings()?;
        let default = settings
            .default_model
            .ok_or_else(|| harness::HarnessError::Compaction("no default model".into()))?;
        let (provider_id, model_id) = choice.unwrap_or((&default.provider.0, &default.model));
        let unknown = || harness::HarnessError::UnknownModel(format!("{provider_id} {model_id}"));
        let provider = self
            .store
            .providers()?
            .into_iter()
            .find(|p| p.id.0 == provider_id)
            .ok_or_else(unknown)?;
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
            Model::Main(llm_rig::connect(
                &provider,
                &key,
                &model,
                reasoning.unwrap_or(self.default_reasoning),
            )),
            model.effective_context(),
            harness::ModelSelection {
                provider: provider_id.to_owned(),
                model: model_id.to_owned(),
                reasoning: reasoning.unwrap_or(self.default_reasoning),
            },
        ))
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

type Chat = ChatHarness<Model, LocalEnvironment, Arc<SqliteStorage>, Model, ModelFactory>;

struct Setup {
    llm: llm_rig::RigLlm,
    environment: LocalEnvironment,
    context_window: Option<u32>,
    summarizer: Option<(Model, Option<u32>)>,
    compactor: Model,
    reasoning: ReasoningLevel,
}

async fn setup(
    store: &Arc<SqliteStorage>,
    workspace: &Path,
    skills_dir: Option<&Path>,
    reasoning: Option<ReasoningLevel>,
) -> Result<Setup> {
    let settings = store.settings()?;
    let model = settings
        .default_model
        .ok_or("no default model; run `agent model set <provider>/<model>`")?;
    let provider = store
        .providers()?
        .into_iter()
        .find(|provider| provider.id == model.provider)
        .ok_or_else(|| format!("unknown provider: {}", model.provider.0))?;
    let stored_model = store
        .models(&model.provider)?
        .into_iter()
        .find(|candidate| candidate.id == model.model)
        .ok_or_else(|| {
            format!(
                "unknown model {}/{}; run `agent models refresh` first",
                model.provider.0, model.model
            )
        })?;
    let key = api_key(&provider)?;
    let llm = llm_rig::connect(
        &provider,
        &key,
        &stored_model,
        reasoning.unwrap_or(settings.reasoning),
    );

    let llm_main = llm;
    let summarizer = if let Some(selected) = settings.compaction_model.clone() {
        let is_main = selected == model;
        let model = selected;
        let provider = store
            .providers()?
            .into_iter()
            .find(|provider| provider.id == model.provider)
            .ok_or_else(|| format!("unknown provider: {}", model.provider.0))?;
        let stored = store
            .models(&model.provider)?
            .into_iter()
            .find(|candidate| candidate.id == model.model)
            .ok_or_else(|| {
                format!(
                    "unknown model {}/{}; run `agent models refresh` first",
                    model.provider.0, model.model
                )
            })?;
        let llm = llm_rig::connect(
            &provider,
            &api_key(&provider)?,
            &stored,
            settings
                .compaction_reasoning
                .unwrap_or(reasoning.unwrap_or(settings.reasoning)),
        );
        Some((
            if is_main {
                Model::Main(llm)
            } else {
                Model::Fallback(Arc::new(llm::Fallback::new(llm, llm_main.clone())))
            },
            if is_main {
                stored.effective_context()
            } else {
                match (stored.effective_context(), stored_model.effective_context()) {
                    (Some(compaction), Some(main)) => Some(compaction.min(main)),
                    (compaction, main) => compaction.or(main),
                }
            },
        ))
    } else {
        None
    };

    let workspace = workspace.to_path_buf();
    let environment = LocalEnvironment::new(&workspace, skills_dir.map(Path::to_path_buf))?;
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
    let compactor = summarizer
        .as_ref()
        .map(|(model, _)| model.clone())
        .unwrap_or_else(|| {
            Model::Main(llm_rig::connect(
                &provider,
                &key,
                &stored_model,
                settings
                    .compaction_reasoning
                    .unwrap_or(reasoning.unwrap_or(settings.reasoning)),
            ))
        });
    Ok(Setup {
        llm: llm_main,
        environment,
        context_window: stored_model.effective_context(),
        summarizer,
        compactor,
        reasoning: reasoning.unwrap_or(settings.reasoning),
    })
}

async fn chat_harness(
    store: Arc<SqliteStorage>,
    workspace: &Path,
    skills_dir: Option<&Path>,
    reasoning: Option<ReasoningLevel>,
) -> Result<Chat> {
    let setup = setup(&store, workspace, skills_dir, reasoning).await?;
    let memory = memory::Memory::open(
        store.clone(),
        setup.compactor,
        memory::MemoryConfig::default(),
    )?;
    Ok(ChatHarness::new(
        Model::Main(setup.llm),
        setup.environment,
        store.clone(),
        memory,
        EnvironmentPaths {
            workspace: workspace.to_path_buf(),
        },
        ModelFactory {
            store,
            default_reasoning: setup.reasoning,
        },
        setup.summarizer,
    ))
}

pub async fn run(
    store: Arc<SqliteStorage>,
    workspace: &Path,
    skills_dir: Option<&Path>,
    prompt: String,
    session: String,
    reasoning: Option<ReasoningLevel>,
    chat: bool,
) -> Result<()> {
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    if chat {
        let harness = chat_harness(store, workspace, skills_dir, reasoning).await?;
        let answer: Result<String> = tokio::select! {
            answer = harness.run(&prompt) => answer.map_err(Into::into),
            _ = interrupt.recv() => Err("interrupted".into()),
            _ = terminate.recv() => Err("terminated".into()),
        };
        println!("{}", answer?);
        return Ok(());
    }
    let setup = setup(&store, workspace, skills_dir, reasoning).await?;
    let summarizer = setup.summarizer;
    let harness = SessionHarness::new(
        Model::Main(setup.llm),
        setup.environment,
        store,
        EnvironmentPaths {
            workspace: workspace.to_path_buf(),
        },
    )
    .with_config(HarnessConfig {
        compaction: CompactionConfig {
            context_window: setup.context_window,
            summarizer_window: summarizer.as_ref().and_then(|(_, window)| *window),
            ..CompactionConfig::default()
        },
    });
    let harness = if let Some((llm, _)) = summarizer {
        harness.with_summarizer(llm)
    } else {
        harness
    };
    let session = SessionId::new(session);
    let answer: Result<String> = tokio::select! {
        answer = harness.run(&session, prompt) => answer.map_err(Into::into),
        _ = interrupt.recv() => Err("interrupted".into()),
        _ = terminate.recv() => Err("terminated".into()),
    };

    drop(harness);
    println!("{}", answer?);
    Ok(())
}

fn admit(
    store: &Arc<SqliteStorage>,
    discord: Arc<dyn interface::Interface>,
    interfaces: &harness::Interfaces,
    inbound: interface::Inbound,
) -> Result<Option<(String, storage::Meta)>> {
    use storage::InterfaceStore;
    if !store.allowed_users("discord")?.contains(&inbound.author.id) {
        return Ok(None);
    }
    let dest = if inbound.place.thread.is_some() || inbound.place.dm {
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
        if let Err(error) = discord.react(&place, &message, interface::Mark::Seen).await {
            eprintln!("{error}");
        }
    });
    interfaces.live("discord", to, inbound.place.clone());
    Ok(Some((
        inbound.text,
        storage::Meta::Message {
            interface: "discord".into(),
            place: inbound.place,
            message: inbound.message,
            author: inbound.author,
            to,
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
) -> Result<()> {
    let harness = chat_harness(store.clone(), workspace, skills_dir, None).await?;
    let discord: Arc<dyn interface::Interface> = Arc::new(interface_discord::Discord::new(token));
    let interfaces = harness::Interfaces::new(vec![discord.clone()], store.clone());
    let harness = harness.with_interfaces(interfaces.clone());
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let listener_discord = discord.clone();
    let listener = tokio::spawn(async move { listener_discord.listen(tx).await });
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut pending = std::collections::VecDeque::<(String, storage::Meta)>::new();
    loop {
        let (text, meta) = if let Some(next) = pending.pop_front() {
            next
        } else {
            let inbound = tokio::select! {
                value = rx.recv() => match value {
                    Some(value) => value,
                    None => return match listener.await? { Err(error) => Err(error.into()), Ok(()) => Err("discord listener stopped".into()) },
                },
                _ = interrupt.recv() => { listener.abort(); return Ok(()); },
                _ = terminate.recv() => { listener.abort(); return Ok(()); },
            };
            let Some(next) = admit(&store, discord.clone(), &interfaces, inbound)? else {
                continue;
            };
            next
        };
        let run = harness.run_message(text, Some(meta));
        tokio::pin!(run);
        while let Some((text, meta)) = pending.pop_front() {
            harness.steer_message(text, Some(meta));
        }
        loop {
            tokio::select! {
                result = &mut run => { if let Err(error) = result { eprintln!("{error}"); } break; },
                value = rx.recv() => match value {
                    Some(inbound) => if let Some((text, meta)) = admit(&store, discord.clone(), &interfaces, inbound)? { harness.steer_message(text, Some(meta)); },
                    None => return match listener.await? { Err(error) => Err(error.into()), Ok(()) => Err("discord listener stopped".into()) },
                },
                _ = interrupt.recv() => { listener.abort(); return Ok(()); },
                _ = terminate.recv() => { listener.abort(); return Ok(()); },
            }
        }
        interfaces.quiet();
        for (text, meta) in harness.take_steering().into_iter().rev() {
            if let Some(meta) = meta {
                pending.push_front((text, meta));
            }
        }
        for (_, meta) in &pending {
            if let storage::Meta::Message {
                interface,
                place,
                to,
                ..
            } = meta
            {
                interfaces.live(interface, *to, place.clone());
            }
        }
    }
}
