use std::path::Path;
use std::sync::Arc;

use environment_local::LocalEnvironment;
use harness::{ChatHarness, CompactionConfig, EnvironmentPaths, HarnessConfig, SessionHarness};
use storage::{ConfigStore, ReasoningLevel, SessionId, Tree};
use storage_sqlite::SqliteStorage;
use tokio::signal::unix::{signal, SignalKind};

use crate::{api_key, Result};

enum Model {
    Main(llm_rig::RigLlm),
    Fallback(llm::Fallback<llm_rig::RigLlm, llm_rig::RigLlm>),
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

pub async fn run(
    store: Arc<SqliteStorage>,
    mount_dir: &Path,
    prompt: String,
    session: String,
    reasoning: Option<ReasoningLevel>,
    chat: bool,
) -> Result<()> {
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
    let llm = llm_rig::connect(
        &provider,
        &api_key(&provider)?,
        &stored_model,
        reasoning.unwrap_or(settings.reasoning),
    );

    let llm_main = llm;
    let mut summarizer = if let Some(selected) = settings.compaction_model.clone() {
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
                Model::Fallback(llm::Fallback::new(llm, llm_main.clone()))
            },
            stored.effective_context(),
        ))
    } else {
        None
    };

    let compactor = if chat {
        Some(if let Some((llm, _)) = summarizer.take() {
            llm
        } else {
            Model::Main(llm_rig::connect(
                &provider,
                &api_key(&provider)?,
                &stored_model,
                settings
                    .compaction_reasoning
                    .unwrap_or(reasoning.unwrap_or(settings.reasoning)),
            ))
        })
    } else {
        None
    };
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let workspace = mount_dir.join("workspace");
    let skills = mount_dir.join("skills");
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&skills)?;
    let workspace_mount = vfs::mount(store.clone(), Tree::Workspace, &workspace, false)?;
    let skills_mount = vfs::mount(store.clone(), Tree::Skills, &skills, true)?;

    let environment = LocalEnvironment::new(&workspace)?.with_readable(&skills)?;
    if let Some(compactor) = compactor {
        let memory =
            memory::Memory::open(store.clone(), compactor, memory::MemoryConfig::default())?;
        let harness = ChatHarness::new(
            Model::Main(llm_main),
            environment,
            store,
            memory,
            EnvironmentPaths { workspace, skills },
        );
        let answer: Result<String> = tokio::select! {
            answer = harness.run(&prompt) => answer.map_err(Into::into),
            _ = interrupt.recv() => Err("interrupted".into()),
            _ = terminate.recv() => Err("terminated".into()),
        };
        drop(harness);
        skills_mount.umount_and_join()?;
        workspace_mount.umount_and_join()?;
        println!("{}", answer?);
        return Ok(());
    }
    let harness = SessionHarness::new(
        Model::Main(llm_main),
        environment,
        store,
        EnvironmentPaths { workspace, skills },
    )
    .with_config(HarnessConfig {
        compaction: CompactionConfig {
            context_window: stored_model.effective_context(),
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
    skills_mount.umount_and_join()?;
    workspace_mount.umount_and_join()?;
    println!("{}", answer?);
    Ok(())
}
