//! `blob-1 service install | restart | uninstall`: runs the launcher (primary)
//! or a node as a per-user background service that starts at boot — a
//! systemd user unit on Linux, a LaunchAgent on macOS. Settings and secrets go
//! in a `0600` env file the service loads; the unit or plist holds no secret.

use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Result;

/// Which program the service runs.
pub enum Role {
    /// `blob-1 launcher`, which supervises `serve` and applies updates.
    Primary {
        data_dir: PathBuf,
        workspace: PathBuf,
        skills_dir: Option<PathBuf>,
        web: std::net::SocketAddr,
        public_url: Option<String>,
        app_key: String,
    },
    /// `blob-1 node --url …`, dialing a primary.
    Node {
        url: String,
        workspaces: Option<PathBuf>,
    },
}

/// The service's name: the systemd unit or launchd label.
pub fn name(node: bool) -> &'static str {
    if node {
        "blob-1-node"
    } else {
        "blob-1"
    }
}

/// Writes the env file and service definition, then enables and (re)starts it.
pub fn install(role: Role) -> Result<()> {
    let node = matches!(role, Role::Node { .. });
    let name = name(node);
    let binary = crate::runtime::service_binary()?;
    let env_file = env_file(name)?;

    // PATH is captured so bot tools see the same programs as this shell.
    let mut env = vec![(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()),
    )];
    let mut args = Vec::new();
    match role {
        Role::Primary {
            data_dir,
            workspace,
            skills_dir,
            web,
            public_url,
            app_key,
        } => {
            env.push(("BLOB_1_APP_KEY", app_key));
            args.extend(["--data-dir".into(), path_arg(&data_dir)?]);
            args.extend(["--workspace".into(), path_arg(&workspace)?]);
            if let Some(dir) = skills_dir {
                args.extend(["--skills-dir".into(), path_arg(&dir)?]);
            }
            args.extend(["launcher".into(), "--web".into(), web.to_string()]);
            if let Some(url) = public_url {
                args.extend(["--public-url".into(), url]);
            }
        }
        Role::Node { url, workspaces } => {
            args.extend(["node".into(), "--url".into(), url]);
            if let Some(dir) = workspaces {
                args.extend(["--workspaces".into(), path_arg(&dir)?]);
            }
        }
    }
    write_env_file(&env_file, &env)?;

    if cfg!(target_os = "macos") {
        launchd::install(name, &binary, &args, &env_file)
    } else {
        systemd::install(name, &binary, &args, &env_file)
    }?;
    println!("installed and started {name}");
    Ok(())
}

pub fn restart(node: bool) -> Result<()> {
    let name = name(node);
    if cfg!(target_os = "macos") {
        launchd::restart(name)
    } else {
        systemd::restart(name)
    }?;
    println!("restarted {name}");
    Ok(())
}

/// Stops and disables the service, then removes its definition and env file.
pub fn uninstall(node: bool) -> Result<()> {
    let name = name(node);
    if cfg!(target_os = "macos") {
        launchd::uninstall(name)
    } else {
        systemd::uninstall(name)
    }?;
    remove_if_present(&env_file(name)?)?;
    println!("uninstalled {name}");
    Ok(())
}

fn env_file(name: &str) -> Result<PathBuf> {
    Ok(crate::runtime::blob_home()?.join(format!("{name}.env")))
}

/// Writes `KEY='value'` lines, owner-only. Single-quoted values read the
/// same in systemd's `EnvironmentFile=` and in `sh`, which can't express a
/// single quote or newline inside them, so those are rejected.
fn write_env_file(path: &Path, env: &[(&str, String)]) -> Result<()> {
    use std::io::Write;
    let mut text = String::from("# Written by `blob-1 service install`.\n");
    for (key, value) in env {
        if value.contains(['\'', '\n', '\r']) {
            return Err(format!("{key} can't contain a single quote or a newline").into());
        }
        text.push_str(&format!("{key}='{value}'\n"));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Replace any earlier file so the 0600 mode applies even if it was looser.
    remove_if_present(path)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn path_arg(path: &Path) -> Result<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("path is not UTF-8: {}", path.display()).into())
}

fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// Runs a service-manager command, failing with its stderr.
fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{program} {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )
    .into())
}

fn home() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("HOME").ok_or("HOME is not set")?,
    ))
}

mod systemd {
    use super::*;

    fn unit_path(name: &str) -> Result<PathBuf> {
        let config = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) => PathBuf::from(dir),
            None => home()?.join(".config"),
        };
        Ok(config.join("systemd/user").join(format!("{name}.service")))
    }

    /// One `ExecStart=` word: double-quoted, with systemd's specifier and
    /// variable characters escaped.
    fn word(arg: &str) -> String {
        let escaped = arg
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$");
        format!("\"{escaped}\"")
    }

    pub fn install(name: &str, binary: &Path, args: &[String], env_file: &Path) -> Result<()> {
        let exec: Vec<String> = std::iter::once(path_arg(binary)?)
            .chain(args.iter().cloned())
            .map(|arg| word(&arg))
            .collect();
        let unit = format!(
            "[Unit]\nDescription=Blob-1 ({name})\nWants=network-online.target\nAfter=network-online.target\n\n[Service]\nEnvironmentFile={}\nExecStart={}\nRestart=always\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
            path_arg(env_file)?,
            exec.join(" "),
        );
        let path = unit_path(name)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, unit)?;
        run("systemctl", &["--user", "daemon-reload"])?;
        run(
            "systemctl",
            &["--user", "enable", &format!("{name}.service")],
        )?;
        run(
            "systemctl",
            &["--user", "restart", &format!("{name}.service")],
        )?;
        // Lingering keeps user services running after logout and starts them
        // at boot; some systems need admin rights for it.
        if let Ok(user) = std::env::var("USER") {
            if run("loginctl", &["enable-linger", &user]).is_err() {
                eprintln!(
                    "warning: could not enable lingering; {name} stops when you log out (run `sudo loginctl enable-linger {user}`)"
                );
            }
        }
        Ok(())
    }

    pub fn restart(name: &str) -> Result<()> {
        run(
            "systemctl",
            &["--user", "restart", &format!("{name}.service")],
        )
    }

    pub fn uninstall(name: &str) -> Result<()> {
        let path = unit_path(name)?;
        if path.exists() {
            run(
                "systemctl",
                &["--user", "disable", "--now", &format!("{name}.service")],
            )?;
        }
        remove_if_present(&path)?;
        run("systemctl", &["--user", "daemon-reload"])
    }
}

mod launchd {
    use super::*;

    fn plist_path(name: &str) -> Result<PathBuf> {
        Ok(home()?
            .join("Library/LaunchAgents")
            .join(format!("{name}.plist")))
    }

    fn domain() -> Result<String> {
        let output = Command::new("id").arg("-u").output()?;
        Ok(format!(
            "gui/{}",
            String::from_utf8_lossy(&output.stdout).trim()
        ))
    }

    fn xml(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    pub fn install(name: &str, binary: &Path, args: &[String], env_file: &Path) -> Result<()> {
        // launchd can't read an env file, so `sh` sources it and then execs
        // blob-1: the key stays in the 0600 file, never in the plist.
        let mut program = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            r#"set -a; . "$0"; set +a; exec "$@""#.to_owned(),
            path_arg(env_file)?,
            path_arg(binary)?,
        ];
        program.extend(args.iter().cloned());
        let log = crate::runtime::blob_home()?.join(format!("logs/{name}.log"));
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let strings: String = program
            .iter()
            .map(|arg| format!("    <string>{}</string>\n", xml(arg)))
            .collect();
        let log = xml(&path_arg(&log)?);
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{name}</string>\n  <key>ProgramArguments</key>\n  <array>\n{strings}  </array>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <true/>\n  <key>StandardOutPath</key>\n  <string>{log}</string>\n  <key>StandardErrorPath</key>\n  <string>{log}</string>\n</dict>\n</plist>\n"
        );
        let path = plist_path(name)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let domain = domain()?;
        // Reinstalling replaces a loaded agent; not being loaded is fine.
        let _ = run("launchctl", &["bootout", &format!("{domain}/{name}")]);
        std::fs::write(&path, plist)?;
        run("launchctl", &["bootstrap", &domain, &path_arg(&path)?])
    }

    pub fn restart(name: &str) -> Result<()> {
        run(
            "launchctl",
            &["kickstart", "-k", &format!("{}/{name}", domain()?)],
        )
    }

    pub fn uninstall(name: &str) -> Result<()> {
        let path = plist_path(name)?;
        if path.exists() {
            let _ = run("launchctl", &["bootout", &format!("{}/{name}", domain()?)]);
        }
        remove_if_present(&path)
    }
}
