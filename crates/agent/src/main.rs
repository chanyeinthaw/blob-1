use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use storage::{
    ConfigStore, MessengerStore, ModelRef, Provider, ProviderApi, ProviderId, ReasoningLevel,
    SkillStore,
};
use storage_sqlite::SqliteStorage;

mod import;
mod run;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Parser)]
struct Cli {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,
    #[arg(long, global = true)]
    skills_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve,
    Messenger {
        #[command(subcommand)]
        command: MessengerCommand,
    },
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Reasoning {
        #[command(subcommand)]
        command: ReasoningCommand,
    },
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Chat {
        message: String,
    },
    Run {
        prompt: String,
        #[arg(long, default_value = "default")]
        session: String,
        #[arg(long)]
        reasoning: Option<ReasoningLevel>,
    },
}

#[derive(Subcommand)]
enum MessengerCommand {
    Allow { messenger: String, user_id: String },
    Disallow { messenger: String, user_id: String },
    List { messenger: String },
}

#[derive(Subcommand)]
enum ProviderCommand {
    Add {
        id: String,
        #[arg(long)]
        api: Api,
        #[arg(long)]
        base_url: String,
        #[arg(long)]
        key_env: String,
    },
    List,
    Remove {
        id: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Api {
    Responses,
    Completions,
}

#[derive(Subcommand)]
enum ModelsCommand {
    Refresh { provider: Option<String> },
    List,
}

#[derive(Subcommand)]
enum ModelCommand {
    Input {
        model: String,
        input: InputKind,
    },
    Set {
        model: String,
    },
    Compaction {
        model: String,
        #[arg(long)]
        reasoning: Option<ReasoningLevel>,
    },
    Map {
        model: String,
        pairs: Vec<String>,
    },
    Context {
        model: String,
        tokens: String,
    },
}

#[derive(Clone, clap::ValueEnum)]
enum InputKind {
    Text,
    Image,
}

#[derive(Subcommand)]
enum ReasoningCommand {
    Set { level: ReasoningLevel },
}

#[derive(Subcommand)]
enum SkillCommand {
    Add { dir: PathBuf },
    List,
    Remove { name: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => default_data_dir()?,
    };
    std::fs::create_dir_all(&data_dir)?;
    let data_dir = data_dir.canonicalize()?;
    let workspace = cli.workspace.unwrap_or_else(|| data_dir.join("workspace"));
    let skills_dir = match cli.skills_dir {
        Some(dir) if !dir.is_absolute() => Some(std::env::current_dir()?.join(dir)),
        dir => dir,
    };
    let path = data_dir.join("agent.db");
    let store = Arc::new(if matches!(cli.command, Command::Messenger { .. }) {
        SqliteStorage::open_shared(path)?
    } else {
        SqliteStorage::open(path)?
    });

    match cli.command {
        Command::Messenger { command } => messenger(&store, command),
        Command::Serve => {
            let token =
                std::env::var("DISCORD_BOT_TOKEN").map_err(|_| "DISCORD_BOT_TOKEN is not set")?;
            std::fs::create_dir_all(&workspace)?;
            run::serve(
                store,
                &workspace.canonicalize()?,
                skills_dir.as_deref(),
                token,
            )
            .await
        }
        Command::Provider { command } => provider(&store, command),
        Command::Models { command } => models(&store, command).await,
        Command::Model { command } => model_command(&store, command),
        Command::Reasoning {
            command: ReasoningCommand::Set { level },
        } => {
            let mut settings = store.settings()?;
            settings.reasoning = level;
            store.save_settings(settings)?;
            Ok(())
        }
        Command::Skill { command } => skill(&store, command),
        Command::Run {
            prompt,
            session,
            reasoning,
        } => {
            std::fs::create_dir_all(&workspace)?;
            let workspace = workspace.canonicalize()?;
            run::run(
                store,
                &workspace,
                skills_dir.as_deref(),
                prompt,
                session,
                reasoning,
                false,
            )
            .await
        }
        Command::Chat { message } => {
            std::fs::create_dir_all(&workspace)?;
            let workspace = workspace.canonicalize()?;
            run::run(
                store,
                &workspace,
                skills_dir.as_deref(),
                message,
                String::new(),
                None,
                true,
            )
            .await
        }
    }
}

fn provider(store: &SqliteStorage, command: ProviderCommand) -> Result<()> {
    match command {
        ProviderCommand::Add {
            id,
            api,
            base_url,
            key_env,
        } => {
            if id.is_empty() || id.contains('/') {
                return Err(format!(
                    "invalid provider id: {id:?}; it must be non-empty and contain no '/'"
                )
                .into());
            }
            store.save_provider(Provider {
                id: ProviderId(id),
                api: match api {
                    Api::Responses => ProviderApi::OpenAiResponses,
                    Api::Completions => ProviderApi::OpenAiCompletions,
                },
                base_url,
                api_key_env: key_env,
            })?
        }
        ProviderCommand::List => {
            for provider in store.providers()? {
                let api = match provider.api {
                    ProviderApi::OpenAiResponses => "responses",
                    ProviderApi::OpenAiCompletions => "completions",
                };
                println!(
                    "{}\t{api}\t{}\t${}",
                    provider.id.0, provider.base_url, provider.api_key_env
                );
            }
        }
        ProviderCommand::Remove { id } => store.remove_provider(&ProviderId(id))?,
    }
    Ok(())
}

async fn models(store: &SqliteStorage, command: ModelsCommand) -> Result<()> {
    match command {
        ModelsCommand::Refresh { provider } => {
            let providers = store.providers()?;
            let selected: Vec<&Provider> = match &provider {
                Some(id) => vec![providers
                    .iter()
                    .find(|candidate| &candidate.id.0 == id)
                    .ok_or_else(|| format!("unknown provider: {id}"))?],
                None => providers.iter().collect(),
            };
            for provider in selected {
                let key = api_key(provider)?;
                let models = llm_rig::discover(provider, &key).await?;
                println!("{}: {} models", provider.id.0, models.len());
                store.replace_models(&provider.id, models)?;
            }
        }
        ModelsCommand::List => {
            let settings = store.settings()?;
            println!("reasoning: {}", settings.reasoning.as_str());
            let default = settings.default_model;
            let compaction = settings.compaction_model;
            for provider in store.providers()? {
                for model in store.models(&provider.id)? {
                    let marker = match &default {
                        Some(default)
                            if default.provider == model.provider && default.model == model.id =>
                        {
                            "*"
                        }
                        _ => " ",
                    };
                    let compaction_marker = if compaction.as_ref().is_some_and(|selected| {
                        selected.provider == model.provider && selected.model == model.id
                    }) {
                        "c"
                    } else {
                        " "
                    };
                    let override_level = if compaction_marker == "c" {
                        settings
                            .compaction_reasoning
                            .map(|level| format!("\tcompaction reasoning: {}", level.as_str()))
                            .unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let context = match (model.effective_context(), model.context_window) {
                        (Some(limit), Some(window)) if limit < window => {
                            format!("\t{limit} (of {window})")
                        }
                        (Some(limit), _) => format!("\t{limit}"),
                        _ => String::new(),
                    };
                    let map = model
                        .reasoning_map
                        .iter()
                        .map(|(level, value)| {
                            format!("{}={}", level.as_str(), value.as_deref().unwrap_or("-"))
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let map = if map.is_empty() {
                        String::new()
                    } else {
                        format!("\t{map}")
                    };
                    let images = if model.images { "\timage" } else { "" };
                    println!(
                        "{marker}{compaction_marker} {}/{}{context}{map}{override_level}{images}",
                        provider.id.0, model.id
                    );
                }
            }
        }
    }
    Ok(())
}

fn known_model(store: &SqliteStorage, model: &str) -> Result<storage::Model> {
    let (provider, id) = model.split_once('/').ok_or("expected <provider>/<model>")?;
    let provider = ProviderId(provider.to_owned());
    store
        .models(&provider)?
        .into_iter()
        .find(|known| known.id == id)
        .ok_or_else(|| {
            format!(
                "unknown model {}/{id}; run `agent models refresh` first",
                provider.0
            )
            .into()
        })
}

fn model_command(store: &SqliteStorage, command: ModelCommand) -> Result<()> {
    match command {
        ModelCommand::Input { model, input } => {
            let images = matches!(input, InputKind::Image);
            if model.contains('/') {
                let model = known_model(store, &model)?;
                store.set_images(&model.provider, &model.id, images)?;
            } else {
                let provider = ProviderId(model);
                if !store.providers()?.iter().any(|item| item.id == provider) {
                    return Err(format!("unknown provider: {}", provider.0).into());
                }
                for model in store.models(&provider)? {
                    store.set_images(&provider, &model.id, images)?;
                }
            }
        }
        ModelCommand::Set { model } => {
            let model = known_model(store, &model)?;
            let mut settings = store.settings()?;
            settings.default_model = Some(ModelRef {
                provider: model.provider,
                model: model.id,
            });
            store.save_settings(settings)?;
        }
        ModelCommand::Compaction { model, reasoning } => {
            let mut settings = store.settings()?;
            if model == "-" {
                settings.compaction_model = None;
                settings.compaction_reasoning = None;
            } else {
                let model = known_model(store, &model)?;
                settings.compaction_model = Some(ModelRef {
                    provider: model.provider,
                    model: model.id,
                });
                settings.compaction_reasoning = reasoning;
            }
            store.save_settings(settings)?;
        }
        ModelCommand::Map { model, pairs } => {
            let model = known_model(store, &model)?;
            let mut map = BTreeMap::new();
            for pair in pairs {
                let (level, value) = pair.split_once('=').ok_or("expected level=value")?;
                if value.is_empty() {
                    return Err("reasoning value cannot be empty".into());
                }
                map.insert(level.parse()?, (value != "-").then(|| value.to_owned()));
            }
            store.set_reasoning_map(&model.provider, &model.id, map)?;
        }
        ModelCommand::Context { model, tokens } => {
            let model = known_model(store, &model)?;
            let limit = if tokens == "-" {
                None
            } else {
                let value = tokens
                    .parse::<u32>()
                    .map_err(|_| "expected a positive token count or -")?;
                if value == 0 {
                    return Err("context limit must be greater than zero".into());
                }
                Some(value)
            };
            store.set_context_limit(&model.provider, &model.id, limit)?;
        }
    }
    Ok(())
}

fn skill(store: &SqliteStorage, command: SkillCommand) -> Result<()> {
    match command {
        SkillCommand::Add { dir } => {
            let name = import::add(store, &dir)?;
            println!("added {name}");
        }
        SkillCommand::List => {
            for skill in store.skills()? {
                println!("{}\t{}", skill.name, skill.description);
            }
        }
        SkillCommand::Remove { name } => import::remove(store, &name)?,
    }
    Ok(())
}

fn messenger(store: &SqliteStorage, command: MessengerCommand) -> Result<()> {
    match command {
        MessengerCommand::Allow { messenger, user_id } => store.allow_user(&messenger, &user_id)?,
        MessengerCommand::Disallow { messenger, user_id } => {
            store.disallow_user(&messenger, &user_id)?;
        }
        MessengerCommand::List { messenger } => {
            for user in store.allowed_users(&messenger)? {
                println!("{user}");
            }
        }
    }
    Ok(())
}

fn api_key(provider: &Provider) -> Result<String> {
    std::env::var(&provider.api_key_env)
        .map_err(|_| format!("{} is not set", provider.api_key_env).into())
}

fn default_data_dir() -> Result<PathBuf> {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data).join("agent"));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/share/agent"))
}
