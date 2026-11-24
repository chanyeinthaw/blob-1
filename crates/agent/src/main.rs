use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use storage::{
    Checkin, CheckinStore, ConfigStore, McpStore, MessengerStore, ModelRef, Provider, ProviderApi,
    ProviderId, ReasoningLevel, SkillStore,
};
use storage_sqlite::{AppKey, SqliteStorage};

mod import;
mod run;
mod runtime;
mod web;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Release builds stamp BLOB_1_VERSION; workspace builds report 0.0.0.
const VERSION: &str = match option_env!("BLOB_1_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(name = "blob-1")]
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
    Serve {
        #[arg(long, default_value = "127.0.0.1:7878")]
        web: std::net::SocketAddr,
        /// Absolute http(s) URL the web app is publicly reached at (e.g. via
        /// Tailscale Serve); OAuth callbacks and WebSocket origins use it
        /// instead of the Host header.
        #[arg(long)]
        public_url: Option<String>,
    },
    Web {
        #[command(subcommand)]
        command: WebCommand,
    },
    Watch {
        #[command(subcommand)]
        command: WatchCommand,
    },
    Checkin {
        #[command(subcommand)]
        command: CheckinCommand,
    },
    Schedule {
        #[command(subcommand)]
        command: ScheduleCommand,
    },
    Timezone {
        #[command(subcommand)]
        command: TimezoneCommand,
    },
    Home {
        #[command(subcommand)]
        command: HomeCommand,
    },
    Instructions {
        #[command(subcommand)]
        command: InstructionsCommand,
    },
    AutoRespond {
        #[command(subcommand)]
        command: AutoRespondCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
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
    /// Download a release and switch to it.
    Update {
        /// Version to install (default: newest on --channel).
        version: Option<String>,
        /// Channel to resolve the newest version from.
        #[arg(long, value_enum, default_value_t = runtime::Channel::Latest)]
        channel: runtime::Channel,
        /// Allow switching to an older version.
        #[arg(long)]
        allow_downgrade: bool,
        /// Repoint launcher symlinks without asking a running launcher to restart.
        #[arg(long)]
        no_restart: bool,
    },
    /// Supervise `serve`, applying updates with database rollback.
    Launcher {
        #[arg(long, default_value = "127.0.0.1:7878")]
        web: std::net::SocketAddr,
        /// Absolute http(s) URL the web app is publicly reached at (e.g. via
        /// Tailscale Serve); OAuth callbacks and WebSocket origins use it
        /// instead of the Host header.
        #[arg(long)]
        public_url: Option<String>,
    },
}

#[derive(Subcommand)]
enum WebCommand {
    Password {
        #[command(subcommand)]
        command: WebPasswordCommand,
    },
}

#[derive(Subcommand)]
enum WebPasswordCommand {
    Set,
}

#[derive(Subcommand)]
enum WatchCommand {
    List,
    Remove { id: String },
}

#[derive(Subcommand)]
enum CheckinCommand {
    Show,
    Set {
        #[arg(long)]
        every: String,
        #[arg(long)]
        days: String,
        #[arg(long)]
        hours: String,
    },
    Off,
}

fn parse_checkin(every: &str, days: String, hours: &str) -> Result<Checkin> {
    let (number, multiplier) = [("m", 60_i64), ("h", 3600), ("d", 86400)]
        .into_iter()
        .find_map(|(unit, seconds)| every.strip_suffix(unit).map(|number| (number, seconds)))
        .ok_or("invalid interval")?;
    let every = number
        .parse::<i64>()?
        .checked_mul(multiplier)
        .ok_or("invalid interval")?;
    if every < 300 {
        return Err("interval must be at least 5 minutes".into());
    }
    let mut seen = std::collections::HashSet::new();
    for day in days.split(',') {
        if !matches!(day, "sun" | "mon" | "tue" | "wed" | "thu" | "fri" | "sat")
            || !seen.insert(day)
        {
            return Err("invalid days".into());
        }
    }
    let (start, end) = hours.split_once('-').ok_or("invalid hours")?;
    if start.len() != 2
        || end.len() != 2
        || !start.bytes().chain(end.bytes()).all(|b| b.is_ascii_digit())
    {
        return Err("invalid hours".into());
    }
    let start_hour: u8 = start.parse()?;
    let end_hour: u8 = end.parse()?;
    if start_hour >= end_hour || end_hour > 24 {
        return Err("invalid hours".into());
    }
    Ok(Checkin {
        every,
        days,
        start_hour,
        end_hour,
    })
}

#[derive(Subcommand)]
enum ScheduleCommand {
    Add {
        #[arg(long, conflicts_with = "cron", required_unless_present = "cron")]
        at: Option<String>,
        #[arg(long, conflicts_with = "at", required_unless_present = "at")]
        cron: Option<String>,
        #[arg(long)]
        tz: Option<String>,
        #[arg(long, conflicts_with = "reply")]
        to: Option<String>,
        #[arg(long, conflicts_with = "to")]
        reply: Option<String>,
        text: String,
    },
    List,
    Remove {
        id: String,
    },
}

#[derive(Subcommand)]
enum TimezoneCommand {
    Show,
    Set { timezone: String },
}

#[derive(Subcommand)]
enum HomeCommand {
    Show,
    Set { messenger: String, target: String },
    Clear,
}

#[derive(Subcommand)]
enum AutoRespondCommand {
    Show,
    On,
    Off,
}

#[derive(Subcommand)]
enum InstructionsCommand {
    Show,
    Set { file: PathBuf },
    Reset,
}

#[derive(Subcommand)]
enum MessengerCommand {
    Allow {
        messenger: String,
        user_id: String,
    },
    Disallow {
        messenger: String,
        user_id: String,
    },
    List {
        messenger: String,
    },
    Token {
        #[command(subcommand)]
        command: MessengerTokenCommand,
    },
    Mode {
        #[command(subcommand)]
        command: MessengerModeCommand,
    },
    Reply {
        #[command(subcommand)]
        command: MessengerReplyCommand,
    },
    Access {
        #[command(subcommand)]
        command: MessengerAccessCommand,
    },
}

#[derive(Subcommand)]
enum MessengerAccessCommand {
    /// Show the default access policy and all allow/block rules.
    Show { messenger: String },
    /// Set the default access policy (`block` or `allow`).
    Default {
        messenger: String,
        effect: AccessEffectSetting,
    },
    /// Set an access rule (`allow` or `block`) for a person (`--user`, optionally `--server`/`--channel`)
    /// or for everyone in a server (`--server`) or channel/thread (`--channel`).
    Set {
        messenger: String,
        effect: AccessEffectSetting,
        #[arg(long)]
        user: Option<String>,
        #[arg(long, conflicts_with = "channel")]
        server: Option<String>,
        #[arg(long, conflicts_with = "server")]
        channel: Option<String>,
    },
    /// Remove an access rule.
    Remove {
        messenger: String,
        #[arg(long)]
        user: Option<String>,
        #[arg(long, conflicts_with = "channel")]
        server: Option<String>,
        #[arg(long, conflicts_with = "server")]
        channel: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum AccessEffectSetting {
    Allow,
    Block,
}

impl From<AccessEffectSetting> for storage::AccessEffect {
    fn from(effect: AccessEffectSetting) -> Self {
        match effect {
            AccessEffectSetting::Allow => Self::Allow,
            AccessEffectSetting::Block => Self::Block,
        }
    }
}

#[derive(Subcommand)]
enum MessengerReplyCommand {
    List {
        messenger: String,
    },
    /// Show or set the Discord-wide default reply mode.
    Default {
        messenger: String,
        mode: Option<ReplyModeSetting>,
    },
    Set {
        messenger: String,
        scope: ReplyScopeSetting,
        id: String,
        mode: ReplyModeSetting,
    },
    Clear {
        messenger: String,
        scope: ReplyScopeSetting,
        id: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ReplyScopeSetting {
    Channel,
    Server,
}

impl From<ReplyScopeSetting> for storage::ReplyScope {
    fn from(scope: ReplyScopeSetting) -> Self {
        match scope {
            ReplyScopeSetting::Channel => Self::Channel,
            ReplyScopeSetting::Server => Self::Server,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ReplyModeSetting {
    Auto,
    Thread,
    Direct,
}

impl From<ReplyModeSetting> for storage::ReplyMode {
    fn from(mode: ReplyModeSetting) -> Self {
        match mode {
            ReplyModeSetting::Auto => Self::Auto,
            ReplyModeSetting::Thread => Self::Thread,
            ReplyModeSetting::Direct => Self::Direct,
        }
    }
}

#[derive(Subcommand)]
enum MessengerModeCommand {
    List {
        messenger: String,
    },
    /// Show or set the Discord-wide default interaction mode.
    Default {
        messenger: String,
        mode: Option<ModeSetting>,
    },
    Set {
        messenger: String,
        scope: ModeScope,
        id: String,
        mode: ModeSetting,
        /// Discord server (guild) id; required when scope is `user`.
        #[arg(long)]
        guild: Option<String>,
    },
    Clear {
        messenger: String,
        scope: ModeScope,
        id: String,
        /// Discord server (guild) id; pass for guild-scoped user overrides, or omit to clear a migrated unassigned user override.
        #[arg(long)]
        guild: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeScope {
    Channel,
    User,
    Server,
}

impl From<ModeScope> for storage::InteractionScope {
    fn from(scope: ModeScope) -> Self {
        match scope {
            ModeScope::Channel => Self::Channel,
            ModeScope::User => Self::User,
            ModeScope::Server => Self::Server,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeSetting {
    MentionOnly,
    Always,
    Auto,
}

impl From<ModeSetting> for storage::InteractionMode {
    fn from(mode: ModeSetting) -> Self {
        match mode {
            ModeSetting::MentionOnly => Self::MentionOnly,
            ModeSetting::Always => Self::Always,
            ModeSetting::Auto => Self::Auto,
        }
    }
}

#[derive(Subcommand)]
enum MessengerTokenCommand {
    /// Reads the token from stdin, never from argv.
    Set {
        messenger: String,
    },
    Clear {
        messenger: String,
    },
}

#[derive(Subcommand)]
enum McpCommand {
    List,
    /// stdio: `agent mcp add <id> [--env K=V] -- <command> [args]`;
    /// http: `agent mcp add <id> --url <url> [--header 'Name: ENV_VAR']`
    Add {
        id: String,
        #[arg(long)]
        env: Vec<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        header: Vec<String>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    Remove {
        id: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    /// Prints the URL to open in a logged-in browser.
    Login {
        id: String,
        #[arg(long, default_value = "127.0.0.1:7878")]
        web: String,
    },
    /// Connect to a server and list its tools.
    Tools {
        id: String,
    },
}

#[derive(Subcommand)]
enum ProviderCommand {
    /// Add a provider. Reads the API key from stdin unless `--no-key` is passed.
    Add {
        id: String,
        #[arg(long)]
        api: Api,
        /// Provider base URL; defaults to `https://opencode.ai/inference/go` for `--api opencode-go`.
        #[arg(long)]
        base_url: Option<String>,
        /// Save the provider without an API key (for local endpoints).
        #[arg(long)]
        no_key: bool,
        /// Explicit model IDs for generic decision providers (repeatable; optional for Cloudflare /ai/run which auto-seeds Clef).
        #[arg(long = "model")]
        models: Vec<String>,
    },
    Key {
        #[command(subcommand)]
        command: ProviderKeyCommand,
    },
    List,
    Remove {
        id: String,
    },
}

#[derive(Subcommand)]
enum ProviderKeyCommand {
    /// Reads the provider API key from stdin, never from argv.
    Set {
        id: String,
    },
    Clear {
        id: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Api {
    Responses,
    Completions,
    OpencodeGo,
    Decision,
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
    Utility {
        model: String,
        #[arg(long)]
        reasoning: Option<ReasoningLevel>,
    },
    Decision {
        model: String,
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
    let matches = Cli::command().version(VERSION).get_matches();
    let cli = Cli::from_arg_matches(&matches)?;
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
    match &cli.command {
        Command::Update {
            version,
            channel,
            allow_downgrade,
            no_restart,
        } => {
            return runtime::update(runtime::UpdateOpts {
                current: VERSION,
                channel: *channel,
                version: version.clone(),
                allow_downgrade: *allow_downgrade,
                no_restart: *no_restart,
            })
            .await;
        }
        Command::Launcher { web, public_url } => {
            if let Some(url) = public_url {
                web::public_url(url)?;
            }
            return runtime::launch(runtime::LauncherOpts {
                current: VERSION,
                data_dir,
                workspace,
                skills_dir,
                web: *web,
                public_url: public_url.clone(),
            })
            .await;
        }
        _ => {}
    }
    let path = data_dir.join("agent.db");
    let app_key = std::env::var("BLOB_1_APP_KEY")
        .ok()
        .filter(|key| !key.is_empty())
        .map(AppKey::new);
    let app_key = if matches!(cli.command, Command::Serve { .. }) {
        Some(app_key.ok_or("set BLOB_1_APP_KEY")?)
    } else {
        app_key
    };
    let store = Arc::new(
        if matches!(
            cli.command,
            Command::Model { .. }
                | Command::Models { .. }
                | Command::Provider { .. }
                | Command::Reasoning { .. }
                | Command::Web { .. }
                | Command::Messenger { .. }
                | Command::Mcp { .. }
                | Command::Instructions { .. }
                | Command::AutoRespond { .. }
                | Command::Timezone { .. }
                | Command::Home { .. }
                | Command::Schedule { .. }
                | Command::Watch { .. }
                | Command::Checkin { .. }
        ) {
            SqliteStorage::open_shared(path, app_key.clone())?
        } else {
            SqliteStorage::open(path, app_key.clone())?
        },
    );

    match cli.command {
        Command::Watch { command } => {
            let output = match command {
                WatchCommand::List => harness::watch::list(&store, store.as_ref())?,
                WatchCommand::Remove { id } => harness::watch::remove(&store, &id)?,
            };
            println!("{output}");
            Ok(())
        }
        Command::Checkin { command } => {
            match command {
                CheckinCommand::Show => match store.checkin()? {
                    Some(c) => println!(
                        "every {}s days {} hours {:02}-{:02}",
                        c.every, c.days, c.start_hour, c.end_hour
                    ),
                    None => println!("off"),
                },
                CheckinCommand::Set { every, days, hours } => {
                    store.set_checkin(&parse_checkin(&every, days, &hours)?)?
                }
                CheckinCommand::Off => store.clear_checkin()?,
            }
            Ok(())
        }
        Command::Schedule { command } => {
            let output = match command {
                ScheduleCommand::Add {
                    at,
                    cron,
                    tz,
                    to,
                    reply,
                    text,
                } => harness::schedule::add(
                    &store,
                    store.as_ref(),
                    harness::schedule::Add {
                        text,
                        at,
                        cron,
                        tz,
                        to,
                        reply,
                    },
                )?,
                ScheduleCommand::List => harness::schedule::list(&store)?,
                ScheduleCommand::Remove { id } => harness::schedule::remove(&store, &id)?,
            };
            println!("{output}");
            Ok(())
        }
        Command::Timezone { command } => {
            match command {
                TimezoneCommand::Show => println!("{}", store.timezone()?),
                TimezoneCommand::Set { timezone } => store.set_timezone(&timezone)?,
            }
            Ok(())
        }
        Command::Home { command } => {
            match command {
                HomeCommand::Show => {
                    if let Some(home) = store.home()? {
                        println!("{} {}", home.messenger, home.target);
                    }
                }
                HomeCommand::Set { messenger, target } => {
                    set_home(&store, Some(storage::Home { messenger, target }))?
                }
                HomeCommand::Clear => set_home(&store, None)?,
            }
            Ok(())
        }
        Command::Instructions { command } => instructions(&store, command),
        Command::AutoRespond { command } => {
            match command {
                AutoRespondCommand::Show => {
                    let enabled = store.auto_respond()?;
                    let has_gate =
                        store.utility_model()?.is_some() || store.decision_model()?.is_some();
                    if !enabled {
                        println!("off");
                    } else if has_gate {
                        println!("on");
                    } else {
                        println!("on (inactive: no decision or utility model configured)");
                    }
                }
                AutoRespondCommand::On => store.set_auto_respond(true)?,
                AutoRespondCommand::Off => store.set_auto_respond(false)?,
            }
            Ok(())
        }
        Command::Messenger { command } => messenger(&store, command).await,
        Command::Mcp { command } => mcp_command(&store, command).await,
        Command::Web {
            command:
                WebCommand::Password {
                    command: WebPasswordCommand::Set,
                },
        } => web::set_password(&store),
        Command::Serve {
            web: addr,
            public_url,
        } => {
            let public_url = public_url.map(|raw| web::public_url(&raw)).transpose()?;
            store.set_public_url(
                public_url
                    .as_ref()
                    .map(|url| url.as_str().trim_end_matches('/')),
            )?;
            let listener = tokio::net::TcpListener::bind(addr).await?;
            std::fs::create_dir_all(&workspace)?;
            let workspace = workspace.canonicalize()?;
            let pinned_workspace = harness::PinnedWorkspace::open(&workspace)?;
            let mcp = mcp::Mcp::new(store.clone());
            let mcp_for_web = mcp.clone();
            let mcp_shutdown = mcp.clone();
            let registry = run::BotRegistry::new(
                store.clone(),
                data_dir.clone(),
                app_key.clone(),
                workspace.clone(),
                pinned_workspace.clone(),
                skills_dir.clone(),
                mcp,
            );
            registry.start_all().await?;
            runtime::mark_ready();
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
            let shutdown_tx = Arc::new(shutdown_tx);
            let mut interrupt =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let signal_tx = shutdown_tx.clone();
            tokio::spawn(async move {
                tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
                let _ = signal_tx.send(true);
            });
            let web_result = web::serve(
                listener,
                store,
                registry.clone(),
                mcp_for_web,
                shutdown_rx,
                public_url,
            )
            .await;
            registry.shutdown_all().await;
            mcp_shutdown.shutdown().await;
            web_result
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
        Command::Chat { message } => {
            std::fs::create_dir_all(&workspace)?;
            let workspace = workspace.canonicalize()?;
            run::chat(store, &workspace, skills_dir.as_deref(), message).await
        }
        Command::Update { .. } | Command::Launcher { .. } => unreachable!(),
    }
}

fn set_home(store: &SqliteStorage, home: Option<storage::Home>) -> Result<()> {
    if let Some(home) = &home {
        if home.messenger != "discord" {
            return Err("home messenger must be discord".into());
        }
        let id = home.target.strip_prefix("dm:").unwrap_or(&home.target);
        if !id.bytes().all(|b| b.is_ascii_digit()) || id.parse::<u64>().map_or(true, |id| id == 0) {
            return Err("home target must be a numeric channel id or dm:<numeric user id>".into());
        }
    }
    store.set_home(home.as_ref())?;
    Ok(())
}

fn instructions(store: &SqliteStorage, command: InstructionsCommand) -> Result<()> {
    match command {
        InstructionsCommand::Show => print!(
            "{}",
            store
                .instructions()?
                .as_deref()
                .unwrap_or(harness::DEFAULT_INSTRUCTIONS)
        ),
        InstructionsCommand::Set { file } => {
            let text = if file == std::path::Path::new("-") {
                let mut text = String::new();
                std::io::stdin().read_to_string(&mut text)?;
                text
            } else {
                std::fs::read_to_string(file)?
            };
            set_instructions(store, &text)?;
        }
        InstructionsCommand::Reset => store.set_instructions(None)?,
    }
    Ok(())
}

fn set_instructions(store: &SqliteStorage, text: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Err("Instructions cannot be blank".into());
    }
    store.set_instructions(Some(text))?;
    Ok(())
}

fn provider(store: &SqliteStorage, command: ProviderCommand) -> Result<()> {
    match command {
        ProviderCommand::Add {
            id,
            api,
            base_url,
            no_key,
            models,
        } => {
            if id.is_empty() || id.contains('/') {
                return Err(format!(
                    "invalid provider id: {id:?}; it must be non-empty and contain no '/'"
                )
                .into());
            }
            let provider_api = match api {
                Api::Responses => ProviderApi::OpenAiResponses,
                Api::Completions => ProviderApi::OpenAiCompletions,
                Api::OpencodeGo => ProviderApi::OpenCodeGo,
                Api::Decision => ProviderApi::Decision,
            };
            let base_url = match (provider_api, base_url) {
                (_, Some(base_url)) => base_url,
                (ProviderApi::OpenCodeGo, None) => {
                    "https://opencode.ai/inference/go".to_owned()
                }
                (_, None) => return Err("--base-url is required".into()),
            };
            let explicit_models: Vec<storage::DiscoveredModel> = models
                .into_iter()
                .map(|m| m.trim().to_owned())
                .filter(|m| !m.is_empty())
                .map(|id| storage::DiscoveredModel {
                    id,
                    context_window: None,
                })
                .collect();
            if provider_api.is_decision() && !run::valid_http_url(&base_url) {
                return Err("Base URL must be a valid http(s) URL".into());
            }
            if provider_api.is_decision()
                && !llm_rig::is_cloudflare_ai_url(&base_url)
                && explicit_models.is_empty()
            {
                return Err(
                    "non-Cloudflare decision providers require at least one --model <id>".into(),
                );
            }
            let api_key = if no_key {
                None
            } else {
                store.require_key()?;
                let mut key = String::new();
                std::io::stdin().read_to_string(&mut key)?;
                let key = key.trim().to_owned();
                if key.is_empty() {
                    return Err(
                        "provider API key is empty; pipe it on stdin or pass --no-key".into(),
                    );
                }
                Some(key)
            };
            let provider_id = ProviderId(id);
            if store
                .providers()?
                .iter()
                .any(|p| p.id == provider_id && (provider_api.is_decision() || p.api.is_decision()))
            {
                return Err(format!("provider {} already exists", provider_id.0).into());
            }
            store.save_provider(
                Provider {
                    id: provider_id.clone(),
                    api: provider_api,
                    base_url: base_url.clone(),
                    has_api_key: api_key.is_some(),
                },
                api_key.as_deref(),
            )?;
            if provider_api.is_decision() {
                let seeded = if !explicit_models.is_empty() {
                    explicit_models
                } else if llm_rig::is_cloudflare_ai_url(&base_url) {
                    llm_rig::cloudflare_decision_models()
                } else {
                    Vec::new()
                };
                if !seeded.is_empty() {
                    store.replace_models(&provider_id, seeded)?;
                }
            }
        }
        ProviderCommand::Key { command } => match command {
            ProviderKeyCommand::Set { id } => {
                store.require_key()?;
                let mut key = String::new();
                std::io::stdin().read_to_string(&mut key)?;
                let key = key.trim().to_owned();
                if key.is_empty() {
                    return Err("provider API key is empty".into());
                }
                store.set_provider_api_key(&ProviderId(id), Some(&key))?;
            }
            ProviderKeyCommand::Clear { id } => {
                store.set_provider_api_key(&ProviderId(id), None)?;
            }
        },
        ProviderCommand::List => {
            for provider in store.providers()? {
                let api = match provider.api {
                    ProviderApi::OpenAiResponses => "responses",
                    ProviderApi::OpenAiCompletions => "completions",
                    ProviderApi::OpenCodeGo => "opencode-go",
                    ProviderApi::Decision => "decision",
                };
                let key_status = if provider.has_api_key {
                    "key set"
                } else {
                    "no key"
                };
                println!(
                    "{}\t{api}\t{}\t{key_status}",
                    provider.id.0, provider.base_url
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
                let count = refresh_provider(store, provider).await?;
                println!("{}: {} models", provider.id.0, count);
            }
        }
        ModelsCommand::List => {
            let settings = store.settings()?;
            println!("reasoning: {}", settings.reasoning.as_str());
            let default = settings.default_model;
            let compaction = settings.compaction_model;
            let utility = store.utility_model()?;
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
                    let utility_marker = if utility.as_ref().is_some_and(|(selected, _)| {
                        selected.provider == model.provider && selected.model == model.id
                    }) {
                        "u"
                    } else {
                        " "
                    };
                    let utility_reasoning = if utility_marker == "u" {
                        format!(
                            "\tutility reasoning: {}",
                            utility
                                .as_ref()
                                .and_then(|(_, level)| *level)
                                .unwrap_or(ReasoningLevel::Low)
                                .as_str()
                        )
                    } else {
                        String::new()
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
                        "{marker}{compaction_marker}{utility_marker} {}/{}{context}{map}{override_level}{utility_reasoning}{images}",
                        provider.id.0, model.id
                    );
                }
            }
        }
    }
    Ok(())
}

async fn refresh_provider(store: &SqliteStorage, provider: &Provider) -> Result<usize> {
    if provider.api.is_decision() {
        let existing = store.models(&provider.id)?;
        if !llm_rig::is_cloudflare_ai_url(&provider.base_url) {
            return Ok(existing.len());
        }
        // Merge built-in Cloudflare Clef models while preserving any explicitly
        // registered model IDs on this provider.
        let mut merged = llm_rig::cloudflare_decision_models();
        for model in existing {
            if !merged.iter().any(|m| m.id == model.id) {
                merged.push(storage::DiscoveredModel {
                    id: model.id,
                    context_window: model.context_window,
                });
            }
        }
        let count = merged.len();
        store.replace_models(&provider.id, merged)?;
        return Ok(count);
    }
    let key = api_key(store, provider)?;
    let models = llm_rig::discover(provider, &key).await?;
    let count = models.len();
    store.replace_models(&provider.id, models)?;
    Ok(count)
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

fn known_model_with_provider(
    store: &SqliteStorage,
    model: &str,
) -> Result<(storage::Provider, storage::Model)> {
    let model = known_model(store, model)?;
    let provider = store
        .providers()?
        .into_iter()
        .find(|p| p.id == model.provider)
        .ok_or_else(|| format!("unknown provider: {}", model.provider.0))?;
    Ok((provider, model))
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
            let (provider, model) = known_model_with_provider(store, &model)?;
            if provider.api.is_decision() {
                return Err("decision models can only be used in the Decision slot".into());
            }
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
                let (provider, model) = known_model_with_provider(store, &model)?;
                if provider.api.is_decision() {
                    return Err("decision models can only be used in the Decision slot".into());
                }
                settings.compaction_model = Some(ModelRef {
                    provider: model.provider,
                    model: model.id,
                });
                settings.compaction_reasoning = reasoning;
            }
            store.save_settings(settings)?;
        }
        ModelCommand::Utility { model, reasoning } => {
            let selected = if model == "off" {
                None
            } else {
                let (provider, model) = known_model_with_provider(store, &model)?;
                if provider.api.is_decision() {
                    return Err("decision models can only be used in the Decision slot".into());
                }
                Some((
                    ModelRef {
                        provider: model.provider,
                        model: model.id,
                    },
                    reasoning,
                ))
            };
            store.set_utility_model(selected)?;
        }
        ModelCommand::Decision { model } => {
            let selected = if model == "off" || model == "-" {
                None
            } else {
                let (provider, model) = known_model_with_provider(store, &model)?;
                if !provider.api.is_decision() {
                    return Err("only decision providers can be used in the Decision slot".into());
                }
                Some(ModelRef {
                    provider: model.provider,
                    model: model.id,
                })
            };
            store.set_decision_model(selected.as_ref())?;
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

pub(crate) fn valid_discord_id(id: &str) -> bool {
    (17..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

async fn messenger(store: &SqliteStorage, command: MessengerCommand) -> Result<()> {
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
        MessengerCommand::Token { command } => {
            if command.messenger() != "discord" {
                return Err(format!("unknown messenger: {}", command.messenger()).into());
            }
            store.require_key()?;
            match command {
                MessengerTokenCommand::Set { messenger } => {
                    let mut token = String::new();
                    std::io::stdin().read_to_string(&mut token)?;
                    let token = token.trim().to_owned();
                    if token.is_empty() {
                        return Err("token cannot be empty".into());
                    }
                    if let Err(error) = messenger_discord::validate_token(&token).await {
                        return Err(error.into());
                    }
                    store.set_messenger_token(&messenger, Some(&token))?;
                }
                MessengerTokenCommand::Clear { messenger } => {
                    store.set_messenger_token(&messenger, None)?;
                }
            }
        }
        MessengerCommand::Mode { command } => match command {
            MessengerModeCommand::List { messenger } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                println!(
                    "default {}",
                    store
                        .messenger_defaults(&messenger)?
                        .interaction_mode
                        .as_str()
                );
                for item in store.interaction_modes(&messenger)? {
                    match item.guild_id {
                        Some(guild) => println!(
                            "{} {} {} guild={guild}",
                            item.scope.as_str(),
                            item.target_id,
                            item.mode.as_str()
                        ),
                        None => println!(
                            "{} {} {}",
                            item.scope.as_str(),
                            item.target_id,
                            item.mode.as_str()
                        ),
                    }
                }
            }
            MessengerModeCommand::Set {
                messenger,
                scope,
                id,
                mode,
                guild,
            } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                if !valid_discord_id(&id) {
                    return Err("Discord IDs must be 17–20 digits".into());
                }
                let scope: storage::InteractionScope = scope.into();
                match (scope, guild.as_deref()) {
                    (
                        storage::InteractionScope::Channel | storage::InteractionScope::Server,
                        Some(_),
                    ) => {
                        return Err("--guild applies only to user overrides".into());
                    }
                    (storage::InteractionScope::User, None) => {
                        return Err("--guild <GUILD_ID> is required for user overrides".into());
                    }
                    (storage::InteractionScope::User, Some(g)) if !valid_discord_id(g) => {
                        return Err("Discord guild IDs must be 17–20 digits".into());
                    }
                    _ => {}
                }
                store.set_interaction_mode(
                    &messenger,
                    scope,
                    guild.as_deref(),
                    &id,
                    mode.into(),
                )?;
            }
            MessengerModeCommand::Clear {
                messenger,
                scope,
                id,
                guild,
            } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                if id.is_empty() {
                    return Err("target ID cannot be empty".into());
                }
                let scope: storage::InteractionScope = scope.into();
                if scope != storage::InteractionScope::User && guild.is_some() {
                    return Err("--guild applies only to user overrides".into());
                }
                let guild_ref = guild.as_deref().filter(|g| !g.is_empty());
                store.clear_interaction_mode(&messenger, scope, guild_ref, &id)?;
            }
            MessengerModeCommand::Default { messenger, mode } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                match mode {
                    Some(mode) => store.set_default_interaction_mode(&messenger, mode.into())?,
                    None => println!(
                        "{}",
                        store
                            .messenger_defaults(&messenger)?
                            .interaction_mode
                            .as_str()
                    ),
                }
            }
        },
        MessengerCommand::Reply { command } => match command {
            MessengerReplyCommand::List { messenger } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                println!(
                    "default {}",
                    store.messenger_defaults(&messenger)?.reply_mode.as_str()
                );
                for item in store.reply_modes(&messenger)? {
                    println!(
                        "{} {} {}",
                        item.scope.as_str(),
                        item.target_id,
                        item.mode.as_str()
                    );
                }
            }
            MessengerReplyCommand::Set {
                messenger,
                scope,
                id,
                mode,
            } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                if !valid_discord_id(&id) {
                    return Err("Discord IDs must be 17–20 digits".into());
                }
                store.set_reply_mode(&messenger, scope.into(), &id, mode.into())?;
            }
            MessengerReplyCommand::Clear {
                messenger,
                scope,
                id,
            } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                if id.is_empty() {
                    return Err("target ID cannot be empty".into());
                }
                store.clear_reply_mode(&messenger, scope.into(), &id)?;
            }
            MessengerReplyCommand::Default { messenger, mode } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                match mode {
                    Some(mode) => store.set_default_reply_mode(&messenger, mode.into())?,
                    None => println!(
                        "{}",
                        store.messenger_defaults(&messenger)?.reply_mode.as_str()
                    ),
                }
            }
        },
        MessengerCommand::Access { command } => match command {
            MessengerAccessCommand::Show { messenger } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                println!("default {}", store.default_access(&messenger)?.as_str());
                for rule in store.access_rules(&messenger)? {
                    let who = rule.user_id.as_deref().unwrap_or("everyone");
                    match rule.scope_id {
                        Some(scope_id) => println!(
                            "{} {} {} {}",
                            rule.effect.as_str(),
                            who,
                            rule.scope.as_str(),
                            scope_id
                        ),
                        None => println!("{} {} {}", rule.effect.as_str(), who, rule.scope.as_str()),
                    }
                }
            }
            MessengerAccessCommand::Default { messenger, effect } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                store.set_default_access(&messenger, effect.into())?;
            }
            MessengerAccessCommand::Set {
                messenger,
                effect,
                user,
                server,
                channel,
            } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                if let Some(ref u) = user {
                    if !valid_discord_id(u) {
                        return Err("Discord user IDs must be 17–20 digits".into());
                    }
                }
                let (scope, scope_id) = match (server.as_deref(), channel.as_deref()) {
                    (None, None) => {
                        if user.is_none() {
                            return Err(
                                "specify --user, --server, or --channel (use `access default` for everyone globally)"
                                    .into(),
                            );
                        }
                        (storage::AccessScope::Global, None)
                    }
                    (Some(s), None) => {
                        if !valid_discord_id(s) {
                            return Err("Discord server IDs must be 17–20 digits".into());
                        }
                        (storage::AccessScope::Server, Some(s))
                    }
                    (None, Some(c)) => {
                        if !valid_discord_id(c) {
                            return Err("Discord channel IDs must be 17–20 digits".into());
                        }
                        (storage::AccessScope::Channel, Some(c))
                    }
                    (Some(_), Some(_)) => {
                        return Err("cannot specify both --server and --channel".into());
                    }
                };
                store.set_access_rule(&messenger, user.as_deref(), scope, scope_id, effect.into())?;
            }
            MessengerAccessCommand::Remove {
                messenger,
                user,
                server,
                channel,
            } => {
                if messenger != "discord" {
                    return Err(format!("unknown messenger: {messenger}").into());
                }
                if user.as_deref() == Some("") {
                    return Err("user ID cannot be empty".into());
                }
                let (scope, scope_id) = match (server.as_deref(), channel.as_deref()) {
                    (None, None) => {
                        if user.is_none() {
                            return Err("specify --user, --server, or --channel".into());
                        }
                        (storage::AccessScope::Global, None)
                    }
                    (Some(s), None) if !s.is_empty() => (storage::AccessScope::Server, Some(s)),
                    (None, Some(c)) if !c.is_empty() => (storage::AccessScope::Channel, Some(c)),
                    (Some(_), Some(_)) => {
                        return Err("cannot specify both --server and --channel".into());
                    }
                    _ => return Err("scope ID cannot be empty".into()),
                };
                store.remove_access_rule(&messenger, user.as_deref(), scope, scope_id)?;
            }
        },
    }
    Ok(())
}

impl MessengerTokenCommand {
    fn messenger(&self) -> &str {
        match self {
            Self::Set { messenger } | Self::Clear { messenger } => messenger,
        }
    }
}

fn api_key(store: &SqliteStorage, provider: &Provider) -> Result<String> {
    Ok(store.provider_api_key(&provider.id)?.unwrap_or_default())
}

fn default_data_dir() -> Result<PathBuf> {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data).join("agent"));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/share/agent"))
}

async fn mcp_command(store: &Arc<SqliteStorage>, command: McpCommand) -> Result<()> {
    let find = |id: &str| -> Result<storage::McpServer> {
        store
            .mcp_servers()?
            .into_iter()
            .find(|server| server.id == id)
            .ok_or_else(|| format!("no MCP server '{id}'").into())
    };
    match command {
        McpCommand::List => {
            for server in store.mcp_servers()? {
                let (transport, detail) = match &server.transport {
                    storage::McpTransport::Stdio { command, args, .. } => (
                        "stdio",
                        std::iter::once(command.clone())
                            .chain(args.iter().cloned())
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    storage::McpTransport::Http { url, .. } => ("http", url.clone()),
                };
                println!(
                    "{}{}\t{transport}\t{detail}",
                    server.id,
                    if server.enabled { "" } else { " (disabled)" }
                );
            }
        }
        McpCommand::Add {
            id,
            env,
            url,
            header,
            command,
        } => {
            if !mcp::valid_id(&id) {
                return Err("Invalid id: use lowercase letters, digits and -".into());
            }
            let parse_pair = |pair: &str, what: &str| -> Result<(String, String)> {
                let (key, value) = pair
                    .split_once(':')
                    .or_else(|| pair.split_once('='))
                    .ok_or_else(|| format!("expected {what} as 'Name: VALUE', got '{pair}'"))?;
                Ok((key.trim().to_string(), value.trim().to_string()))
            };
            let transport = match (url, command.is_empty()) {
                (Some(url), true) => storage::McpTransport::Http {
                    url,
                    headers: header
                        .iter()
                        .map(|h| parse_pair(h, "--header"))
                        .collect::<Result<_>>()?,
                },
                (None, false) => storage::McpTransport::Stdio {
                    command: command[0].clone(),
                    args: command[1..].to_vec(),
                    env: env
                        .iter()
                        .map(|e| {
                            e.split_once('=')
                                .map(|(k, v)| (k.to_string(), v.to_string()))
                                .ok_or_else(|| format!("expected --env K=V, got '{e}'").into())
                        })
                        .collect::<Result<_>>()?,
                },
                (Some(_), false) => {
                    return Err("give either --url or a command after --, not both".into())
                }
                (None, true) => return Err("give a command after -- or --url <url>".into()),
            };
            store.save_mcp_server(&storage::McpServer {
                id: id.clone(),
                transport,
                enabled: true,
            })?;
            println!("added {id}");
        }
        McpCommand::Remove { id } => {
            if !store.remove_mcp_server(&id)? {
                return Err(format!("no MCP server '{id}'").into());
            }
        }
        McpCommand::Enable { id } => {
            let mut server = find(&id)?;
            server.enabled = true;
            store.save_mcp_server(&server)?;
        }
        McpCommand::Disable { id } => {
            let mut server = find(&id)?;
            server.enabled = false;
            store.save_mcp_server(&server)?;
        }
        McpCommand::Login { id, web } => {
            let server = find(&id)?;
            if !matches!(server.transport, storage::McpTransport::Http { .. }) {
                return Err(format!("MCP server '{id}' is not an HTTP server").into());
            }
            match store.public_url()? {
                Some(base) => println!("{base}/api/mcp/login?id={id}"),
                None => println!("http://{web}/api/mcp/login?id={id}"),
            }
        }
        McpCommand::Tools { id } => {
            find(&id)?;
            let mcp = mcp::Mcp::new(store.clone());
            let result = mcp.tools(&id).await;
            mcp.shutdown().await;
            for (name, description) in result? {
                println!("{name}\t{description}");
            }
        }
    }
    Ok(())
}
