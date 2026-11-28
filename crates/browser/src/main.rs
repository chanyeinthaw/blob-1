mod cef_layer;
mod ops;
mod snapshot;
mod socket;

use cef::{api_hash, args::Args, execute_process, initialize, run_message_loop, shutdown, CefString, Settings, LogSeverity};
use cef_layer::{BrowserState, HeadlessApp, SharedState};
use clap::Parser;
use socket::{install_stdin_watch, spawn_socket_server, Shutdown};
use std::{path::PathBuf, process::ExitCode, ptr, sync::{Arc, Mutex}};

#[derive(Parser)]
#[command(name = "blob-1-browser", about = "Headless browser companion for blob-1")]
struct Cli {
    /// Unix socket on which serve connects.
    #[arg(long)]
    socket: PathBuf,
    /// Parent directory for persistent browser profiles.
    #[arg(long)]
    profiles_dir: PathBuf,
}

fn main() -> ExitCode {
    // Chromium subprocesses re-execute this binary with --type=...; this call
    // must run before clap sees those Chromium-specific arguments.
    let args = Args::new();
    let _ = api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let mut process_app = HeadlessApp::new();
    let subprocess_code = execute_process(
        Some(args.as_main_args()),
        Some(&mut process_app),
        ptr::null_mut(),
    );
    if subprocess_code >= 0 {
        return ExitCode::from(subprocess_code as u8);
    }
    if subprocess_code != -1 {
        eprintln!("CEF process execution failed with code {subprocess_code}");
        return ExitCode::FAILURE;
    }

    let cli = Cli::parse();
    if let Err(error) = std::fs::create_dir_all(&cli.profiles_dir) {
        eprintln!(
            "cannot create profiles directory '{}': {error}",
            cli.profiles_dir.display()
        );
        return ExitCode::FAILURE;
    }
    let profiles_dir = match cli.profiles_dir.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            eprintln!(
                "cannot resolve profiles directory '{}': {error}",
                cli.profiles_dir.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let shared: SharedState = Arc::new(Mutex::new(BrowserState::new(profiles_dir.clone())));
    let shutdown_signal = Shutdown::new(shared.clone());

    let executable = std::env::current_exe().ok();
    let executable_dir = executable
        .as_ref()
        .and_then(|path| path.parent().map(PathBuf::from));
    let mut settings = Settings {
        no_sandbox: 1,
        windowless_rendering_enabled: 1,
        persist_session_cookies: 1,
        root_cache_path: cef_string(&profiles_dir),
        log_severity: LogSeverity::WARNING,
        ..Default::default()
    };
    if let Some(executable) = &executable {
        settings.browser_subprocess_path = cef_string(executable);
    }
    if let Some(executable_dir) = &executable_dir {
        settings.resources_dir_path = cef_string(executable_dir);
        settings.locales_dir_path = cef_string(&executable_dir.join("locales"));
    }

    let mut app = process_app;
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        ptr::null_mut(),
    ) != 1
    {
        eprintln!("CEF initialization failed");
        return ExitCode::FAILURE;
    }

    if let Err(error) = spawn_socket_server(cli.socket, shared.clone(), shutdown_signal.clone()) {
        eprintln!("{error}");
        shutdown();
        return ExitCode::FAILURE;
    }
    install_stdin_watch(shutdown_signal);

    run_message_loop();
    shutdown();
    ExitCode::SUCCESS
}

fn cef_string(path: &std::path::Path) -> CefString {
    CefString::from(path.to_string_lossy().as_ref())
}
