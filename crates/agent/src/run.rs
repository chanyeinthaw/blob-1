use std::path::Path;
use std::sync::Arc;

use environment_local::LocalEnvironment;
use harness::{CompactionConfig, EnvironmentPaths, Harness, HarnessConfig};
use storage::{ConfigStore, ReasoningLevel, SessionId, Tree};
use storage_sqlite::SqliteStorage;
use tokio::signal::unix::{signal, SignalKind};

use crate::{api_key, Result};

pub async fn run(
    store: Arc<SqliteStorage>,
    mount_dir: &Path,
    prompt: String,
    session: String,
    reasoning: Option<ReasoningLevel>,
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

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let workspace = mount_dir.join("workspace");
    let skills = mount_dir.join("skills");
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&skills)?;
    let workspace_mount = vfs::mount(store.clone(), Tree::Workspace, &workspace, false)?;
    let skills_mount = vfs::mount(store.clone(), Tree::Skills, &skills, true)?;

    let environment = LocalEnvironment::new(&workspace)?.with_readable(&skills)?;
    let harness = Harness::new(
        llm,
        environment,
        store,
        EnvironmentPaths { workspace, skills },
    )
    .with_config(HarnessConfig {
        compaction: CompactionConfig {
            context_window: stored_model.effective_context(),
            ..CompactionConfig::default()
        },
    });
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
