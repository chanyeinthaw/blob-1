use std::path::Path;
use std::sync::Arc;

use environment::{Environment, SkillBundle};
use environment_local::LocalEnvironment;
use harness::{ChatHarness, EnvironmentPaths};
use storage::{ConfigStore, MessengerStore, ReasoningLevel, SkillStore};
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
                images: model.images,
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
    summarizer: Option<(Model, Option<u32>)>,
    utility: Option<harness::Utility>,
    compactor: Model,
    reasoning: ReasoningLevel,
}

async fn setup(
    store: &Arc<SqliteStorage>,
    workspace: &Path,
    skills_dir: Option<&Path>,
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
    let llm = llm_rig::connect(&provider, &key, &stored_model, settings.reasoning);

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
            settings.compaction_reasoning.unwrap_or(settings.reasoning),
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

    let utility = if let Some((selected, reasoning)) = store.utility_model()? {
        let provider = store
            .providers()?
            .into_iter()
            .find(|provider| provider.id == selected.provider)
            .ok_or_else(|| format!("unknown provider: {}", selected.provider.0))?;
        let model = store
            .models(&selected.provider)?
            .into_iter()
            .find(|model| model.id == selected.model)
            .ok_or_else(|| {
                format!(
                    "unknown model {}/{}; run `agent models refresh` first",
                    selected.provider.0, selected.model
                )
            })?;
        Some(harness::Utility::new(llm_rig::connect(
            &provider,
            &api_key(&provider)?,
            &model,
            reasoning.unwrap_or(ReasoningLevel::Low),
        )))
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
                settings.compaction_reasoning.unwrap_or(settings.reasoning),
            ))
        });
    Ok(Setup {
        llm: llm_main,
        environment,
        summarizer,
        utility,
        compactor,
        reasoning: settings.reasoning,
    })
}

async fn chat_harness(
    store: Arc<SqliteStorage>,
    workspace: &Path,
    skills_dir: Option<&Path>,
) -> Result<Chat> {
    let setup = setup(&store, workspace, skills_dir).await?;
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
            store: store.clone(),
            default_reasoning: setup.reasoning,
        },
        (setup.summarizer, setup.utility, store),
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
    let harness = chat_harness(store, workspace, skills_dir).await?;
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
) -> Result<()> {
    let harness = chat_harness(store.clone(), workspace, skills_dir).await?;
    let discord: Arc<dyn messenger::Messenger> = Arc::new(messenger_discord::Discord::new(token));
    let messengers = harness::Messengers::new(vec![discord.clone()], store.clone())
        .with_utility(harness.utility().cloned());
    let harness = harness.with_messengers(messengers.clone());
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let listener_discord = discord.clone();
    let listener = tokio::spawn(async move { listener_discord.listen(tx).await });
    let (admitted_tx, mut admitted_rx) = tokio::sync::mpsc::channel(128);
    let admission_store = store.clone();
    let admission_discord = discord.clone();
    let admission_messengers = messengers.clone();
    let utility = harness.utility().cloned();
    let admission = tokio::spawn(async move {
        while let Some(inbound) = rx.recv().await {
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
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
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
                _ = interrupt.recv() => { listener.abort(); admission.abort(); return Ok(()); },
                _ = terminate.recv() => { listener.abort(); admission.abort(); return Ok(()); },
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
                _ = interrupt.recv() => { listener.abort(); admission.abort(); return Ok(()); },
                _ = terminate.recv() => { listener.abort(); admission.abort(); return Ok(()); },
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
