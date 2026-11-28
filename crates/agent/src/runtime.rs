//! Self-update and the supervised launcher.
//!
//! Releases install side-by-side under `$BLOB_1_HOME/runtime` (default
//! `~/.blob-1/runtime`):
//!
//! - `versions/<version>/blob-1` plus a `.install-complete` marker containing
//!   `<version>\n`; partial installs stage in `versions/.staging-*` first.
//! - `launchers` lists newline-separated symlink paths (`~/.local/bin/blob-1`)
//!   repointed atomically on update.
//! - `service-state.json` tracks `protocol: 1`, `activeVersion` and an optional
//!   `update` record (`id`, `fromVersion`, `targetVersion`, `dbPath`,
//!   `status` = pending | committed | rolled-back | failed, `reason?`).
//! - `db-backup/<updateId>/` snapshots `agent.db` (+ `-wal`/`-shm`) before a
//!   supervised update so a bad trial can be rolled back.
//! - `launcher.sock` (mode 0600) accepts `{"type":"apply-update",...}` from
//!   `blob-1 update` while `blob-1 launcher` supervises `serve`.
//!
//! The launcher restarts `serve` on a trial binary; once the child writes
//! `$BLOB_1_READY_FILE` the update is committed, otherwise the database is
//! restored and the previous version is restarted.

use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use crate::Result;

const REPO: &str = "chanyeinthaw/blob-1";
pub const READY_ENV: &str = "BLOB_1_READY_FILE";
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// `BLOB_1_HOME` or `~/.blob-1`; install.sh unpacks into the same root.
fn home_dir() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("BLOB_1_HOME") {
        return Ok(home.into());
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".blob-1"))
}

pub fn runtime_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join("runtime"))
}

fn versions_dir(root: &Path) -> PathBuf {
    root.join("versions")
}

fn version_dir(root: &Path, version: &str) -> PathBuf {
    versions_dir(root).join(version)
}

fn state_path(root: &Path) -> PathBuf {
    root.join("service-state.json")
}

fn queued_update_path(root: &Path) -> PathBuf {
    root.join("queued-update.json")
}

fn socket_path(root: &Path) -> PathBuf {
    root.join("launcher.sock")
}

fn launcher_lock_path(root: &Path) -> PathBuf {
    root.join("launcher.lock")
}

fn versions_lock_path(root: &Path) -> PathBuf {
    versions_dir(root).join(".lock")
}

fn pin_path(root: &Path) -> PathBuf {
    versions_dir(root).join(".pin")
}

/// Pin a target version under `versions/.lock` while probing it before
/// `pending` state is persisted so concurrent pruning cannot delete it.
struct VersionPin(PathBuf);

impl VersionPin {
    fn acquire(root: &Path, version: &str) -> Result<Self> {
        let _lock = lock_versions(root)?;
        if !installed(root, version) {
            return Err("target version is not installed".into());
        }
        let path = pin_path(root);
        std::fs::write(&path, format!("{version}\n"))?;
        Ok(VersionPin(path))
    }
}

impl Drop for VersionPin {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Acquire the blocking exclusive lock shared with `install.sh` (`versions/.lock`)
/// to serialize version publication, launcher repointing, and pruning.
fn lock_versions(root: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(versions_dir(root))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(versions_lock_path(root))?;
    file.lock()?;
    Ok(file)
}

/// Acquire exclusive launcher ownership of `$BLOB_1_HOME/runtime` so two
/// launchers can never race over `service-state.json`, recovery, or `launcher.sock`.
fn lock_launcher(root: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(launcher_lock_path(root))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err("another blob-1 launcher is already running for this runtime".into())
        }
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

fn launchers_path(root: &Path) -> PathBuf {
    root.join("launchers")
}

fn backups_dir(root: &Path) -> PathBuf {
    root.join("db-backup")
}

/// A supervised `serve` child signals readiness by writing `ready\n` once the
/// database is open and the web listener is bound.
pub fn mark_ready() {
    let Some(path) = std::env::var_os(READY_ENV).map(PathBuf::from) else {
        return;
    };
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, b"ready\n").is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

// --- Versions ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pre {
    Num(u64),
    Text(String),
}

impl Ord for Pre {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (Pre::Num(a), Pre::Num(b)) => a.cmp(b),
            (Pre::Num(_), Pre::Text(_)) => Ordering::Less,
            (Pre::Text(_), Pre::Num(_)) => Ordering::Greater,
            (Pre::Text(a), Pre::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Pre {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Strict SemVer: `X.Y.Z` with an optional `-a.b.c` prerelease; numeric
/// identifiers compare as integers, so `0230` equals `230` and `10 > 2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Pre>,
    latest_channel: bool,
}

impl Version {
    /// Parse a release version (`X.Y.Z` or `X.Y.Z-prerelease`) and validate
    /// that the entire string is a single safe path component (no `/`, `\`, `+`,
    /// or `..`).
    pub fn parse(raw: &str) -> Result<Version> {
        let raw = raw.strip_prefix('v').unwrap_or(raw);
        let invalid = || format!("invalid version '{raw}'");
        if raw.is_empty()
            || raw == "."
            || raw == ".."
            || raw.starts_with('.')
            || raw.contains("..")
            || !raw
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            return Err(invalid().into());
        }
        let (core, pre) = match raw.split_once('-') {
            Some((core, pre)) => {
                if pre.is_empty() {
                    return Err(invalid().into());
                }
                (core, pre)
            }
            None => (raw, ""),
        };
        let parse_core = |part: Option<&str>| -> Result<u64> {
            let part = part.ok_or_else(invalid)?;
            if part.len() > 1 && part.starts_with('0') {
                return Err(invalid().into());
            }
            part.parse::<u64>().map_err(|_| invalid().into())
        };
        let mut parts = core.split('.');
        let (major, minor, patch) = (
            parse_core(parts.next())?,
            parse_core(parts.next())?,
            parse_core(parts.next())?,
        );
        if parts.next().is_some() {
            return Err(invalid().into());
        }
        let mut parsed = Vec::new();
        let mut latest_channel = false;
        if !pre.is_empty() {
            let raw_ids: Vec<&str> = pre.split('.').collect();
            latest_channel = raw_ids.len() == 3
                && raw_ids[0] == "latest"
                && raw_ids[1].len() == 8
                && raw_ids[1].bytes().all(|b| b.is_ascii_digit())
                && raw_ids[2].len() == 4
                && raw_ids[2].bytes().all(|b| b.is_ascii_digit());
            for id in raw_ids {
                if id.is_empty()
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                {
                    return Err(invalid().into());
                }
                parsed.push(match id.parse::<u64>() {
                    Ok(n) => Pre::Num(n),
                    Err(_) => Pre::Text(id.to_owned()),
                });
            }
        }
        Ok(Version {
            major,
            minor,
            patch,
            pre: parsed,
            latest_channel,
        })
    }

    /// The `0.0.0` workspace version: a dev build may move anywhere.
    fn is_dev_zero(&self) -> bool {
        self.major == 0 && self.minor == 0 && self.patch == 0 && self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(other.pre.iter()) {
                        match a.cmp(b) {
                            Ordering::Equal => {}
                            ord => return ord,
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Release channel: `latest` publishes `X.Y.Z-latest.YYYYMMDD.HHMM`
/// prereleases, `stable` publishes plain `X.Y.Z`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Channel {
    Latest,
    Stable,
}

impl Channel {
    fn matches(&self, version: &Version) -> bool {
        match self {
            Channel::Stable => version.pre.is_empty(),
            Channel::Latest => version.latest_channel,
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Channel::Latest => "latest",
            Channel::Stable => "stable",
        })
    }
}

/// Upgrades move forward; a dev `0.0.0` build may adopt its first release;
/// anything else needs --allow-downgrade.
fn upgrade_allowed(current: &Version, target: &Version, allow_downgrade: bool) -> bool {
    allow_downgrade || target > current || current.is_dev_zero()
}

// --- Release downloads ------------------------------------------------------

fn release_base() -> String {
    std::env::var("BLOB_1_RELEASE_BASE_URL")
        .unwrap_or_else(|_| format!("https://github.com/{REPO}/releases/download"))
        .trim_end_matches('/')
        .to_owned()
}

fn platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x64"),
        ("macos", "aarch64") => Ok("darwin-arm64"),
        (os, arch) => Err(format!("unsupported platform {os}-{arch}").into()),
    }
}

fn github_token() -> Option<String> {
    ["GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|t| !t.is_empty()))
}

fn is_github_url(url: &str) -> bool {
    url.starts_with("https://github.com/") || url.starts_with("https://api.github.com/")
}

/// Fetch a URL, or read a file when BLOB_1_RELEASE_BASE_URL points at a
/// local directory (used for mirrors and tests).
async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let path = Path::new(url);
    if url.starts_with("http://") || url.starts_with("https://") || !path.exists() {
        let mut req = client.get(url);
        if is_github_url(url) {
            if let Some(token) = github_token() {
                req = req.bearer_auth(token);
            }
        }
        Ok(req.send().await?.error_for_status()?.bytes().await?.to_vec())
    } else {
        Ok(std::fs::read(path)?)
    }
}

/// Newest tag on the channel: the GitHub releases API (up to 10 pages of 100,
/// skipping drafts) or `<base>/versions` (one tag per line) on a mirror.
async fn resolve_target(client: &reqwest::Client, channel: Channel) -> Result<String> {
    if std::env::var_os("BLOB_1_RELEASE_BASE_URL").is_some() {
        let text = fetch(client, &format!("{}/versions", release_base())).await?;
        let text = String::from_utf8(text)?;
        return pick_version(text.lines(), channel);
    }
    let mut tags = Vec::new();
    for page in 1..=10u32 {
        let url =
            format!("https://api.github.com/repos/{REPO}/releases?per_page=100&page={page}");
        let mut req = client.get(&url);
        if let Some(token) = github_token() {
            req = req.bearer_auth(token);
        }
        let releases: Vec<Release> = req.send().await?.error_for_status()?.json().await?;
        let last = releases.len() < 100;
        tags.extend(
            releases
                .into_iter()
                .filter(|release| !release.draft)
                .map(|release| release.tag_name),
        );
        if last {
            break;
        }
    }
    pick_version(tags.iter().map(String::as_str), channel)
}

fn pick_version<'a>(tags: impl Iterator<Item = &'a str>, channel: Channel) -> Result<String> {
    tags.map(str::trim)
        .filter(|tag| !tag.is_empty())
        .filter_map(|tag| Version::parse(tag).ok().map(|v| (v, tag)))
        .filter(|(version, _)| channel.matches(version))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, tag)| tag.strip_prefix('v').unwrap_or(tag).to_owned())
        .ok_or_else(|| format!("no releases found on the {channel} channel").into())
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
}

/// `SHA256SUMS` entry for `name`, in `<sum>  <name>` or `<sum> *<name>` form.
fn checksum_for(sums: &str, name: &str) -> Result<String> {
    sums.lines()
        .find_map(|line| {
            let (sum, file) = line.split_once(char::is_whitespace)?;
            (file.trim().trim_start_matches('*') == name).then(|| sum.to_owned())
        })
        .ok_or_else(|| format!("{name} is not listed in SHA256SUMS").into())
}

fn staging_dir(versions: &Path) -> Result<PathBuf> {
    for n in 0..100u32 {
        let dir = versions.join(format!(".staging-{}-{n}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("could not create a staging directory".into())
}

fn installed(root: &Path, version: &str) -> bool {
    std::fs::read_to_string(version_dir(root, version).join(".install-complete"))
        .map(|marker| marker.trim() == version)
        .unwrap_or(false)
}

/// Where `update --browser` unpacks the companion inside a version dir.
const BROWSER_DIR: &str = "browser";

/// Installed, and with the browser companion when it is wanted.
fn installed_with(root: &Path, version: &str, browser: bool) -> bool {
    installed(root, version)
        && (!browser
            || version_dir(root, version)
                .join(BROWSER_DIR)
                .join("blob-1-browser")
                .is_file())
}

/// Downloads `archive` from the release, checks it against `sums`, and
/// unpacks it into `into`.
async fn fetch_and_extract(
    client: &reqwest::Client,
    version: &str,
    sums: &str,
    archive: &str,
    into: &Path,
) -> Result<()> {
    let bytes = fetch(client, &format!("{}/v{version}/{archive}", release_base())).await?;
    let expected = checksum_for(sums, archive)?;
    let actual = hex::encode(sha2::Sha256::digest(&bytes));
    if actual != expected {
        return Err(format!("checksum mismatch for {archive}").into());
    }
    std::fs::create_dir_all(into)?;
    let archive_path = into.join(archive);
    std::fs::write(&archive_path, &bytes)?;
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(into)
        .status()?;
    std::fs::remove_file(&archive_path)?;
    if !status.success() {
        return Err(format!("tar could not extract {archive}").into());
    }
    Ok(())
}

/// Adds the browser companion to an already installed `version` (the one
/// running), staging beside it and renaming into place under the versions lock.
async fn add_browser(client: &reqwest::Client, root: &Path, version: &str) -> Result<()> {
    if !installed(root, version) {
        return Err(format!("{version} is not an installed release; run `blob-1 update --browser` to a release").into());
    }
    let sums = fetch(client, &format!("{}/v{version}/SHA256SUMS", release_base())).await?;
    let sums = String::from_utf8(sums)?;
    let dir = version_dir(root, version);
    let staging = dir.join(format!(".{BROWSER_DIR}-staging-{}", std::process::id()));
    let archive = format!("blob-1-browser-{version}-{}.tar.gz", platform()?);
    if let Err(error) = fetch_and_extract(client, version, &sums, &archive, &staging).await {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("browser companion: {error}").into());
    }
    let _lock = lock_versions(root)?;
    let target = dir.join(BROWSER_DIR);
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::rename(&staging, &target)?;
    Ok(())
}

/// Download, verify and unpack `blob-1-<version>-<platform>.tar.gz` into an
/// already-created staging directory — plus the browser companion archive
/// into `browser/` when `browser` is set — then smoke-test and mark it
/// complete.
async fn install_into(
    client: &reqwest::Client,
    staging: &Path,
    version: &str,
    browser: bool,
) -> Result<()> {
    let platform = platform()?;
    let sums = fetch(client, &format!("{}/v{version}/SHA256SUMS", release_base())).await?;
    let sums = String::from_utf8(sums)?;
    let archive = format!("blob-1-{version}-{platform}.tar.gz");
    fetch_and_extract(client, version, &sums, &archive, staging).await?;
    if browser {
        let archive = format!("blob-1-browser-{version}-{platform}.tar.gz");
        fetch_and_extract(client, version, &sums, &archive, &staging.join(BROWSER_DIR))
            .await
            .map_err(|error| format!("browser companion: {error}"))?;
    }
    let bin = staging.join("blob-1");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))?;
    let probe = if version == "0.0.0" { "--help" } else { "--version" };
    if !std::process::Command::new(&bin)
        .arg(probe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?
        .success()
    {
        return Err("the downloaded executable does not run".into());
    }
    std::fs::write(staging.join(".install-complete"), format!("{version}\n"))?;
    Ok(())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// Stage into `versions/.staging-*`, then acquire `versions/.lock` ONCE to
/// atomically publish `versions/<version>` and run `register` (queueing or
/// repointing) before releasing the lock so concurrent pruning can never delete
/// a freshly published version before it is referenced. Re-validates cached
/// installs under the lock and downloads if a concurrent prune removed them.
async fn ensure_installed_and_register<F, T>(
    client: &reqwest::Client,
    root: &Path,
    version: &str,
    browser: bool,
    register: F,
) -> Result<(bool, T)>
where
    F: FnOnce(&std::fs::File) -> Result<T>,
{
    Version::parse(version)?;
    let mut attempt = 0;
    loop {
        let staging = if attempt == 0 && installed_with(root, version, browser) {
            None
        } else {
            let versions = versions_dir(root);
            std::fs::create_dir_all(&versions)?;
            let dir = staging_dir(&versions)?;
            if let Err(error) = install_into(client, &dir, version, browser).await {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(error);
            }
            Some(dir)
        };
        let lock = lock_versions(root)?;
        let fresh = if let Some(staging) = staging {
            if installed_with(root, version, browser) {
                let _ = std::fs::remove_dir_all(&staging);
                false
            } else {
                let target = version_dir(root, version);
                if target.exists() {
                    if let Err(error) = std::fs::remove_dir_all(&target) {
                        let _ = std::fs::remove_dir_all(&staging);
                        return Err(error.into());
                    }
                }
                if let Err(error) = std::fs::rename(&staging, &target) {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(error.into());
                }
                true
            }
        } else if installed_with(root, version, browser) {
            false
        } else {
            // A concurrent prune removed the cached version before we acquired
            // the lock; drop the lock and download it cleanly.
            drop(lock);
            attempt += 1;
            continue;
        };
        let out = register(&lock)?;
        return Ok((fresh, out));
    }
}

// --- Launchers and version pruning ------------------------------------------

fn resolve_link(path: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(path).ok()?;
    Some(if target.is_absolute() {
        target
    } else {
        path.parent()?.join(target)
    })
}

/// Symlink paths to repoint: the `launchers` file plus argv0 when it is a
/// symlink into `versions/` (e.g. a `~/.local/bin/blob-1` we were run through).
fn launcher_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_to_string(launchers_path(root))
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    if let Some(arg0) = std::env::args().next() {
        let arg0 = PathBuf::from(arg0);
        let arg0 = if arg0.is_absolute() {
            arg0
        } else {
            std::env::current_dir()
                .map(|dir| dir.join(&arg0))
                .unwrap_or(arg0)
        };
        let is_versions_link = arg0
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
            && resolve_link(&arg0)
                .map(|target| target.starts_with(versions_dir(root)))
                .unwrap_or(false);
        if is_versions_link && !paths.contains(&arg0) {
            paths.push(arg0);
        }
    }
    paths
}

/// The executable a service should start: the first launcher symlink that
/// exists (updates repoint it to the active version), else this binary.
pub fn service_binary() -> Result<PathBuf> {
    let root = runtime_dir()?;
    match launcher_paths(&root).into_iter().find(|path| path.exists()) {
        Some(path) => Ok(path),
        None => Ok(std::env::current_exe()?),
    }
}

/// `BLOB_1_HOME` (default `~/.blob-1`), where service env files live.
pub fn blob_home() -> Result<PathBuf> {
    home_dir()
}

/// Versions referenced by launcher symlinks, so prune keeps what is linked.
fn referenced_versions(root: &Path) -> BTreeSet<String> {
    launcher_paths(root)
        .iter()
        .filter_map(|path| resolve_link(path))
        .filter_map(|target| {
            target
                .strip_prefix(versions_dir(root))
                .ok()?
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
        })
        .collect()
}

/// Atomically repoint every launcher symlink while the caller holds
/// `versions/.lock`.
fn repoint_launchers_locked(_lock: &std::fs::File, root: &Path, version: &str) -> Vec<PathBuf> {
    let bin = version_dir(root, version).join("blob-1");
    let mut repointed = Vec::new();
    for path in launcher_paths(root) {
        let Some(parent) = path.parent() else { continue };
        let tmp = parent.join(format!(
            ".{}.tmp-{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        let done = std::os::unix::fs::symlink(&bin, &tmp)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .is_ok();
        if done {
            repointed.push(path);
        } else {
            let _ = std::fs::remove_file(&tmp);
            eprintln!("warning: could not repoint {}", path.display());
        }
    }
    repointed
}

fn repoint_launchers(root: &Path, version: &str) -> Result<Vec<PathBuf>> {
    let lock = lock_versions(root)?;
    Ok(repoint_launchers_locked(&lock, root, version))
}

/// Drop unused version directories while the caller holds `versions/.lock`.
/// Re-reads launcher references, active/pending service state, and any queued
/// update while holding the lock, and leaves `.staging-*` directories untouched.
fn prune_versions_locked(_lock: &std::fs::File, root: &Path, keep: &BTreeSet<String>) {
    let mut keep = keep.clone();
    keep.extend(referenced_versions(root));
    if let Ok(Some(queued)) = peek_queued_update(root) {
        keep.insert(queued.target_version);
    }
    if let Ok(pinned) = std::fs::read_to_string(pin_path(root)) {
        let pinned = pinned.trim();
        if !pinned.is_empty() {
            keep.insert(pinned.to_owned());
        }
    }
    if let Ok(Some(state)) = load_state(root) {
        if !state.active_version.is_empty() {
            keep.insert(state.active_version);
        }
        if let Some(update) = state.update {
            keep.insert(update.from_version);
            keep.insert(update.target_version);
        }
    }
    let Ok(entries) = std::fs::read_dir(versions_dir(root)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && !name.starts_with(".staging-") && !keep.contains(name.as_ref()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
fn prune_versions(root: &Path, keep: &BTreeSet<String>) {
    if let Ok(lock) = lock_versions(root) {
        prune_versions_locked(&lock, root, keep);
    }
}

// --- `blob-1 update` --------------------------------------------------------

pub struct UpdateOpts {
    /// Version of the running binary (`VERSION` in main.rs).
    pub current: &'static str,
    pub channel: Channel,
    pub version: Option<String>,
    pub allow_downgrade: bool,
    pub no_restart: bool,
    /// Also install the browser companion; implied when the running version
    /// already has one.
    pub browser: bool,
}

enum UpdateRegistration {
    Managed { queued: Option<QueuedUpdate> },
    Standalone { repointed: usize },
}

pub async fn update(opts: UpdateOpts) -> Result<()> {
    let root = runtime_dir()?;
    let current = Version::parse(opts.current)?;
    let client = reqwest::Client::builder()
        .user_agent("blob-1-update")
        .build()?;
    let target = match &opts.version {
        Some(version) => version.strip_prefix('v').unwrap_or(version).to_owned(),
        None => resolve_target(&client, opts.channel).await?,
    };
    let target_version = Version::parse(&target)?;
    if opts.browser && target == opts.current && !installed_with(&root, &target, true) {
        add_browser(&client, &root, &target).await?;
        println!("installed the browser companion into {target}; restart serve to use it");
        return Ok(());
    }
    if !upgrade_allowed(&current, &target_version, opts.allow_downgrade) {
        return Err(format!(
            "{target} is not newer than {}; pass --allow-downgrade to switch anyway",
            opts.current
        )
        .into());
    }
    let browser = opts.browser
        || std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(BROWSER_DIR).join("blob-1-browser")))
            .is_some_and(|path| path.is_file());
    // Publish and register the target reference under the same `versions/.lock`
    // so pruning can never remove `target` between publication and activation.
    let (fresh, registration) = ensure_installed_and_register(&client, &root, &target, browser, |lock| {
        if let Some(state) = load_state(&root)? {
            let queued = if state.active_version != target {
                let entry = QueuedUpdate {
                    target_version: target.clone(),
                    allow_downgrade: opts.allow_downgrade,
                    created_at_ms: now_ms(),
                };
                save_json(&queued_update_path(&root), &entry)?;
                Some(entry)
            } else {
                None
            };
            let mut keep = BTreeSet::new();
            keep.insert(target.clone());
            keep.insert(opts.current.to_owned());
            prune_versions_locked(lock, &root, &keep);
            Ok(UpdateRegistration::Managed { queued })
        } else {
            let repointed = repoint_launchers_locked(lock, &root, &target);
            let mut keep = BTreeSet::new();
            keep.insert(target.clone());
            keep.insert(opts.current.to_owned());
            prune_versions_locked(lock, &root, &keep);
            Ok(UpdateRegistration::Standalone {
                repointed: repointed.len(),
            })
        }
    })
    .await?;
    if fresh {
        println!("installed {target}");
    } else {
        println!("{target} already installed");
    }
    match registration {
        UpdateRegistration::Managed { queued } => {
            if !opts.no_restart {
                match apply_via_launcher(&root, &target, opts.allow_downgrade).await {
                    Ok(Some(())) => return Ok(()),
                    Ok(None) => {}
                    Err(error) => {
                        // Live attempt failed/rolled back/rejected: remove the
                        // temporary queue entry we just wrote so a restart
                        // won't unexpectedly re-run a failed update.
                        if let Some(ref entry) = queued {
                            clear_matching_queued_update(&root, entry);
                        }
                        return Err(error);
                    }
                }
            }
            println!("staged {target}; restart blob-1 launcher to apply");
            Ok(())
        }
        UpdateRegistration::Standalone { repointed } => {
            match repointed {
                0 => println!("no launcher symlinks to update"),
                n => println!("repointed {n} launcher symlink(s) to {target}"),
            }
            println!("restart blob-1 to run {target}");
            Ok(())
        }
    }
}

/// Talk to the running launcher. `Ok(None)` means no live socket: the caller
/// falls back to repointing launchers itself.
async fn apply_via_launcher(root: &Path, target: &str, allow_downgrade: bool) -> Result<Option<()>> {
    let sock = socket_path(root);
    let stream = match tokio::net::UnixStream::connect(&sock).await {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error.into()),
    };
    let request = json!({
        "type": "apply-update",
        "targetVersion": target,
        "allowDowngrade": allow_downgrade,
    });
    let mut stream = stream;
    stream
        .write_all(format!("{request}\n").as_bytes())
        .await?;
    let mut reply = String::new();
    let mut reader = BufReader::new(&mut stream);
    tokio::time::timeout(Duration::from_secs(300), reader.read_line(&mut reply)).await??;
    let reply = reply.trim();
    if reply.is_empty() {
        return Err("the launcher closed the connection without a reply".into());
    }
    let reply: Value = serde_json::from_str(reply)?;
    let status = reply["status"].as_str().unwrap_or("failed");
    let reason = reply["reason"].as_str().unwrap_or("no reason given");
    match status {
        "committed" => {
            println!("updated to {target}");
            Ok(Some(()))
        }
        other => Err(format!("update {other}: {reason}").into()),
    }
}

// --- Service state ----------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
struct ServiceState {
    protocol: u32,
    active_version: String,
    update: Option<UpdateRecord>,
    #[serde(default)]
    last_committed_at_ms: u128,
}

impl Default for ServiceState {
    fn default() -> Self {
        ServiceState {
            protocol: 1,
            active_version: String::new(),
            update: None,
            last_committed_at_ms: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct QueuedUpdate {
    target_version: String,
    allow_downgrade: bool,
    #[serde(default)]
    created_at_ms: u128,
}

fn peek_queued_update(root: &Path) -> Result<Option<QueuedUpdate>> {
    let path = queued_update_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
fn take_queued_update(root: &Path) -> Result<Option<QueuedUpdate>> {
    let _lock = lock_versions(root)?;
    let Some(queued) = peek_queued_update(root)? else {
        return Ok(None);
    };
    let _ = std::fs::remove_file(queued_update_path(root));
    Ok(Some(queued))
}

fn clear_matching_queued_update(root: &Path, expected: &QueuedUpdate) {
    let Ok(_lock) = lock_versions(root) else {
        return;
    };
    if let Ok(Some(current)) = peek_queued_update(root) {
        if &current == expected {
            let _ = std::fs::remove_file(queued_update_path(root));
        }
    }
}

/// Remove any queued update that existed before or matches a newly committed
/// live update, while preserving a newer queued request written afterward.
fn clear_superseded_queued_update_locked(
    _lock: &std::fs::File,
    root: &Path,
    prior: Option<&QueuedUpdate>,
    committed_target: &str,
    started_at_ms: u128,
) {
    if let Ok(Some(current)) = peek_queued_update(root) {
        let is_prior = prior.is_some_and(|p| p == &current);
        let is_superseded = is_prior
            || current.target_version == committed_target
            || current.created_at_ms <= started_at_ms;
        if is_superseded {
            let _ = std::fs::remove_file(queued_update_path(root));
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UpdateRecord {
    id: String,
    from_version: String,
    target_version: String,
    db_path: PathBuf,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claimed_queue: Option<QueuedUpdate>,
}

fn load_state(root: &Path) -> Result<Option<ServiceState>> {
    match std::fs::read_to_string(state_path(root)) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Persist a JSON file atomically: temp file, fsync, rename, fsync parent.
fn save_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() {
        fsync_dir(parent)?;
    }
    Ok(())
}

fn save_state(root: &Path, state: &ServiceState) -> Result<()> {
    save_json(&state_path(root), state)
}

// --- Database backup / restore ----------------------------------------------

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

fn fsync_dir(dir: &Path) -> Result<()> {
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    const REFLINK: &str = "--reflink=auto";
    #[cfg(target_os = "macos")]
    const REFLINK: &str = "-c";
    let copied = std::process::Command::new("cp")
        .arg(REFLINK)
        .arg(src)
        .arg(dst)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !copied {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

struct DbLock {
    _owner: std::fs::File,
    _maint: std::fs::File,
}

impl Drop for DbLock {
    fn drop(&mut self) {
        let _ = self._maint.unlock();
        let _ = self._owner.unlock();
    }
}

impl DbLock {
    #[cfg(test)]
    fn unlock(self) -> Result<()> {
        self._maint.unlock()?;
        self._owner.unlock()?;
        Ok(())
    }
}

/// Acquire exclusive ownership of both `<db>.lock` (held by `SqliteStorage::open`)
/// and `<db>.maint.lock` (held shared by both `open` and `open_shared`) so no
/// `serve` or CLI command can touch SQLite files during backup or restore.
fn lock_db(db: &Path) -> Result<DbLock> {
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let owner = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(suffixed(db, ".lock"))?;
    match owner.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err("database is still in use by another blob-1 process".into());
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
    }
    let maint = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(suffixed(db, ".maint.lock"))?;
    match maint.try_lock() {
        Ok(()) => Ok(DbLock {
            _owner: owner,
            _maint: maint,
        }),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err("database is currently open in another blob-1 command".into())
        }
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Snapshot `agent.db` plus its `-wal`/`-shm` sidecars (when present) into
/// `db-backup/<updateId>/` while holding the exclusive DB locks. Copies into a
/// sibling staging dir first, fsyncs every file and directory (propagating
/// errors), and renames atomically so a partial snapshot is never published.
fn backup_db(db: &Path, backup: &Path) -> Result<()> {
    let _lock = lock_db(db)?;
    let parent = backup.parent().ok_or("invalid backup path")?;
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        ".staging-{}-{}",
        backup.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging)?;
    let result = (|| -> Result<()> {
        let name = db.file_name().ok_or("invalid db path")?;
        for suffix in ["", "-wal", "-shm"] {
            let src = suffixed(db, suffix);
            if src.exists() {
                let mut dst_name = name.to_os_string();
                dst_name.push(suffix);
                let dst = staging.join(dst_name);
                copy_file(&src, &dst)?;
                std::fs::File::open(&dst)?.sync_all()?;
            }
        }
        fsync_dir(&staging)?;
        std::fs::rename(&staging, backup)?;
        fsync_dir(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// Restore a complete backup made by `backup_db`: acquire the exclusive DB
/// locks, copy back every saved file, and remove any file (`agent.db`, `-wal`,
/// `-shm`) created by a failed trial that did not exist before the update.
fn restore_db(backup: &Path, db: &Path) -> Result<()> {
    let _lock = lock_db(db)?;
    let name = db.file_name().ok_or("invalid db path")?;
    for suffix in ["", "-wal", "-shm"] {
        let mut file_name = name.to_os_string();
        file_name.push(suffix);
        let saved = backup.join(&file_name);
        let target = suffixed(db, suffix);
        if saved.exists() {
            std::fs::copy(&saved, &target)?;
            std::fs::File::open(&target)?.sync_all()?;
        } else if target.exists() {
            std::fs::remove_file(&target)?;
        }
    }
    fsync_dir(db.parent().unwrap_or_else(|| Path::new(".")))?;
    Ok(())
}

// --- The launcher ------------------------------------------------------------

pub struct LauncherOpts {
    pub current: &'static str,
    pub data_dir: PathBuf,
    pub workspace: PathBuf,
    pub skills_dir: Option<PathBuf>,
    pub web: std::net::SocketAddr,
    pub public_url: Option<String>,
}

struct Supervisor {
    root: PathBuf,
    db_path: PathBuf,
    /// Full argv for the child: global flags, then `serve ...`.
    child_args: Vec<String>,
    ready_timeout: Duration,
    state: ServiceState,
    seq: u64,
    child: Option<Child>,
}

impl Supervisor {
    fn bin(&self, version: &str) -> Result<PathBuf> {
        let bin = version_dir(&self.root, version).join("blob-1");
        if bin.is_file() {
            Ok(bin)
        } else {
            Ok(std::env::current_exe()?)
        }
    }

    fn spawn(&mut self, version: &str) -> Result<(Child, PathBuf)> {
        self.seq += 1;
        let ready = self
            .root
            .join(format!(".ready-{}-{}", std::process::id(), self.seq));
        let _ = std::fs::remove_file(&ready);
        let child = Command::new(self.bin(version)?)
            .args(&self.child_args)
            .env(READY_ENV, &ready)
            .kill_on_drop(true)
            .spawn()?;
        Ok((child, ready))
    }

    /// Start `serve` for `version` and wait for its ready file. Stores the
    /// child immediately so signal cancellation during startup can still run
    /// graceful `stop_child()`.
    async fn start(&mut self, version: &str) -> Result<()> {
        let (child, ready) = self.spawn(version)?;
        self.child = Some(child);
        let result = wait_ready(
            self.child.as_mut().expect("child set above"),
            &ready,
            self.ready_timeout,
        )
        .await;
        let _ = std::fs::remove_file(&ready);
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                self.stop_child().await;
                Err(error)
            }
        }
    }

    /// SIGTERM, escalate to SIGKILL after STOP_TIMEOUT.
    async fn stop_child(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                if let Some(pid) = child.id() {
                    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                }
                if tokio::time::timeout(STOP_TIMEOUT, child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
            }
        }
        self.child = None;
    }

    /// One `{"type":"apply-update",...}` request: validate, pin target under
    /// `versions/.lock`, probe `--version`, persist pending, stop child,
    /// snapshot db, trial target, commit or roll back.
    async fn apply_update(
        &mut self,
        target: &str,
        allow_downgrade: bool,
        claimed_queue: Option<QueuedUpdate>,
    ) -> Value {
        let from = self.state.active_version.clone();
        let reject = |reason: &str| json!({"status": "rejected", "reason": reason});
        if self
            .state
            .update
            .as_ref()
            .is_some_and(|r| r.status == "pending")
        {
            return reject("a previous update is still pending recovery");
        }
        let (Ok(current), Ok(target_v)) = (Version::parse(&from), Version::parse(target)) else {
            return reject("unparseable version");
        };
        if !upgrade_allowed(&current, &target_v, allow_downgrade) {
            return reject("downgrade requires --allow-downgrade");
        }
        let _pin = match VersionPin::acquire(&self.root, target) {
            Ok(pin) => pin,
            Err(_) => return reject("target version is not installed"),
        };
        // Pre-self-update releases (such as v0.0.0) lack `--version` and
        // `BLOB_1_READY_FILE`, so reject them before stopping the running
        // service. Use an async, cancellable, bounded subprocess so SIGTERM
        // can interrupt a hung probe immediately.
        let target_bin = version_dir(&self.root, target).join("blob-1");
        let supports_supervised = match Command::new(&target_bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(mut probe) => matches!(
                tokio::time::timeout(Duration::from_secs(10), probe.wait()).await,
                Ok(Ok(status)) if status.success()
            ),
            Err(_) => false,
        };
        if !supports_supervised {
            return reject("target release does not support supervised updates");
        }
        let started_at_ms = now_ms();
        let prior_queued = peek_queued_update(&self.root).ok().flatten();
        self.seq += 1;
        let id = format!(
            "update-{}-{}-{}",
            started_at_ms,
            std::process::id(),
            self.seq
        );
        self.state.update = Some(UpdateRecord {
            id: id.clone(),
            from_version: from.clone(),
            target_version: target.to_owned(),
            db_path: self.db_path.clone(),
            status: "pending".to_owned(),
            reason: None,
            claimed_queue,
        });
        if let Err(error) = save_state(&self.root, &self.state) {
            self.state.update = None;
            return json!({"status": "failed", "reason": format!("could not persist state: {error}")});
        }
        self.stop_child().await;
        let backup = backups_dir(&self.root).join(&id);
        if let Err(error) = backup_db(&self.db_path, &backup) {
            self.fail(&from, &id, format!("db backup failed: {error}"))
                .await;
            return json!({"status": "failed", "reason": format!("db backup failed: {error}")});
        }
        match self.start(target).await {
            Ok(()) => {
                let mut keep = BTreeSet::new();
                keep.insert(target.to_owned());
                keep.insert(from);
                let mut next = self.state.clone();
                next.active_version = target.to_owned();
                next.last_committed_at_ms = next.last_committed_at_ms.max(started_at_ms);
                if let Some(record) = next.update.as_mut() {
                    record.status = "committed".to_owned();
                    record.reason = None;
                }
                let state_synced = match save_state(&self.root, &next) {
                    Ok(()) => true,
                    Err(error) => {
                        // Check whether rename already published `committed`
                        // before a post-rename directory sync failed; if so,
                        // keep the backup until a future durable sync succeeds.
                        let disk_committed = load_state(&self.root)
                            .ok()
                            .flatten()
                            .is_some_and(|s| {
                                s.active_version == target
                                    && s.update.as_ref().is_some_and(|u| {
                                        u.id == id && u.status == "committed"
                                    })
                            });
                        if !disk_committed {
                            self.stop_child().await;
                            return json!({"status": "failed", "reason": format!("could not persist committed state (trial stopped, backup preserved): {error}")});
                        }
                        eprintln!("warning: committed state renamed but directory sync failed (backup preserved): {error}");
                        false
                    }
                };
                self.state = next;
                match lock_versions(&self.root) {
                    Ok(lock) => {
                        let repointed = repoint_launchers_locked(&lock, &self.root, target);
                        if repointed.is_empty() {
                            eprintln!("warning: no launcher symlinks repointed");
                        }
                        clear_superseded_queued_update_locked(
                            &lock,
                            &self.root,
                            prior_queued.as_ref(),
                            target,
                            started_at_ms,
                        );
                        prune_versions_locked(&lock, &self.root, &keep);
                    }
                    Err(error) => {
                        eprintln!("warning: could not lock versions for activation/pruning: {error}");
                    }
                }
                if state_synced {
                    let _ = std::fs::remove_dir_all(&backup);
                }
                json!({"status": "committed", "version": target})
            }
            Err(error) => self.rollback(&from, &id, error.to_string()).await,
        }
    }

    /// Trial failed: restore the db snapshot, persist state, only then remove
    /// the backup and restart the previous version. Keeps `status == "pending"`
    /// and the backup intact if restore or state persistence fails so startup
    /// recovery will retry on the next launcher boot.
    async fn rollback(&mut self, from: &str, id: &str, reason: String) -> Value {
        let backup = backups_dir(&self.root).join(id);
        if backup.exists() {
            if let Err(error) = restore_db(&backup, &self.db_path) {
                let msg = format!("{reason}; db restore failed (recovery still pending, backup preserved): {error}");
                return json!({"status": "failed", "reason": msg});
            }
        }
        let mut next = self.state.clone();
        next.active_version = from.to_owned();
        if let Some(record) = next.update.as_mut() {
            record.status = "rolled-back".to_owned();
            record.reason = Some(reason.clone());
        }
        let state_synced = match save_state(&self.root, &next) {
            Ok(()) => true,
            Err(error) => {
                let disk_rolled_back = load_state(&self.root)
                    .ok()
                    .flatten()
                    .is_some_and(|s| {
                        s.active_version == from
                            && s.update.as_ref().is_some_and(|u| {
                                u.id == id && u.status == "rolled-back"
                            })
                    });
                if !disk_rolled_back {
                    let msg = format!("{reason}; could not persist rollback state (recovery still pending, backup preserved): {error}");
                    return json!({"status": "failed", "reason": msg});
                }
                eprintln!("warning: rollback state renamed but directory sync failed (backup preserved): {error}");
                false
            }
        };
        self.state = next;
        if let Some(claimed) = self
            .state
            .update
            .as_ref()
            .and_then(|u| u.claimed_queue.as_ref())
        {
            clear_matching_queued_update(&self.root, claimed);
        }
        if state_synced && backup.exists() {
            let _ = std::fs::remove_dir_all(&backup);
        }
        if let Err(error) = self.start(from).await {
            let msg = format!("{reason}; restart failed: {error}");
            self.set_update_status("failed", Some(msg.clone()));
            let _ = save_state(&self.root, &self.state);
            return json!({"status": "failed", "reason": msg});
        }
        json!({"status": "rolled-back", "reason": reason})
    }

    /// The update could not proceed; mark it failed and restart `from`.
    async fn fail(&mut self, from: &str, _id: &str, reason: String) {
        self.set_update_status("failed", Some(reason));
        self.state.active_version = from.to_owned();
        let _ = save_state(&self.root, &self.state);
        let _ = self.start(from).await;
    }

    fn set_update_status(&mut self, status: &str, reason: Option<String>) {
        if let Some(record) = self.state.update.as_mut() {
            record.status = status.to_owned();
            record.reason = reason;
        }
    }

    async fn handle(&mut self, stream: tokio::net::UnixStream) {
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        let request = tokio::time::timeout(
            Duration::from_secs(30),
            stream.read_line(&mut line),
        )
        .await;
        let reply = match request {
            Ok(Ok(_)) => match serde_json::from_str::<Value>(line.trim()) {
                Ok(value) if value["type"] == "apply-update" => {
                    let target = value["targetVersion"].as_str().unwrap_or("");
                    let allow = value["allowDowngrade"].as_bool().unwrap_or(false);
                    if target.is_empty() {
                        json!({"status": "rejected", "reason": "missing targetVersion"})
                    } else {
                        self.apply_update(target, allow, None).await
                    }
                }
                Ok(value) if value["type"] == "status" => json!({
                    "status": "ok",
                    "activeVersion": self.state.active_version,
                }),
                _ => json!({"status": "rejected", "reason": "unknown request"}),
            },
            _ => json!({"status": "rejected", "reason": "bad request"}),
        };
        let mut stream = stream.into_inner();
        let _ = stream
            .write_all(format!("{reply}\n").as_bytes())
            .await;
        let _ = stream.shutdown().await;
    }
}

/// Wait until the child writes its ready file; checks for early exit before
/// accepting readiness so a child that crashes right after signaling fails.
async fn wait_ready(child: &mut Child, ready: &Path, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("exited before becoming ready ({status})").into());
        }
        if ready.exists() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for readiness".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Recover an interrupted pending update on launcher startup. Acquires the DB
/// lock before restoring, persists state before deleting the backup, and cleans
/// up any incomplete `.staging-*` backup directories.
fn recover_pending(root: &Path, state: &mut ServiceState) -> Result<bool> {
    if let Ok(entries) = std::fs::read_dir(backups_dir(root)) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(".staging-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let Some(record) = state.update.as_ref().filter(|r| r.status == "pending") else {
        return Ok(false);
    };
    let backup = backups_dir(root).join(&record.id);
    let from = record.from_version.clone();
    let db = record.db_path.clone();
    let id = record.id.clone();
    let claimed_queue = record.claimed_queue.clone();
    if backup.exists() {
        restore_db(&backup, &db)?;
    }
    state.active_version = from;
    if let Some(record) = state.update.as_mut() {
        record.status = "rolled-back".to_owned();
        record.reason = Some(format!("launcher restarted while update {id} was pending"));
    }
    save_state(root, state)?;
    if let Some(claimed) = claimed_queue {
        clear_matching_queued_update(root, &claimed);
    }
    if backup.exists() {
        let _ = std::fs::remove_dir_all(&backup);
    }
    Ok(true)
}

/// Supervise `serve` under `launcher.sock`, applying updates with db rollback.
pub async fn launch(opts: LauncherOpts) -> Result<()> {
    let root = runtime_dir()?;
    let _launcher_lock = lock_launcher(&root)?;
    let _ = std::fs::remove_file(pin_path(&root));
    let mut interrupt =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    std::fs::create_dir_all(versions_dir(&root))?;
    std::fs::create_dir_all(backups_dir(&root))?;
    // Initialize `service-state.json` under `versions/.lock` so `install.sh`
    // sees managed status atomically before repointing symlinks.
    let mut state = {
        let _lock = lock_versions(&root)?;
        match load_state(&root)? {
            Some(state) => state,
            None => {
                let state = ServiceState {
                    active_version: opts.current.to_owned(),
                    ..ServiceState::default()
                };
                save_state(&root, &state)?;
                state
            }
        }
    };
    recover_pending(&root, &mut state)?;

    let mut child_args = vec![
        "--data-dir".into(),
        opts.data_dir.to_string_lossy().into_owned(),
        "--workspace".into(),
        opts.workspace.to_string_lossy().into_owned(),
    ];
    if let Some(dir) = &opts.skills_dir {
        child_args.push("--skills-dir".into());
        child_args.push(dir.to_string_lossy().into_owned());
    }
    child_args.extend(["serve".into(), "--web".into(), opts.web.to_string()]);
    if let Some(url) = &opts.public_url {
        child_args.extend(["--public-url".into(), url.clone()]);
    }
    // Peek queued update without removing it yet so `prune_versions_locked`
    // keeps its target version alive until `pending` state takes over. Discard
    // any entry superseded by an already-committed update, already active, or
    // matching the claimed queue entry of a rolled-back/failed update.
    let claimed_terminal = state
        .update
        .as_ref()
        .filter(|u| matches!(u.status.as_str(), "rolled-back" | "failed"))
        .and_then(|u| u.claimed_queue.as_ref());
    let queued = match peek_queued_update(&root)? {
        Some(entry)
            if entry.created_at_ms <= state.last_committed_at_ms
                || entry.target_version == state.active_version
                || claimed_terminal == Some(&entry) =>
        {
            clear_matching_queued_update(&root, &entry);
            None
        }
        other => other,
    };
    let mut supervisor = Supervisor {
        root: root.clone(),
        db_path: opts.data_dir.join("agent.db"),
        child_args,
        ready_timeout: READY_TIMEOUT,
        state,
        seq: 0,
        child: None,
    };
    let startup: Result<bool> = tokio::select! {
        res = async {
            if let Some(queued) = queued {
                let reply = supervisor
                    .apply_update(
                        &queued.target_version,
                        queued.allow_downgrade,
                        Some(queued.clone()),
                    )
                    .await;
                clear_matching_queued_update(&root, &queued);
                if reply["status"] == "rejected" {
                    eprintln!(
                        "queued update to {} rejected: {}",
                        queued.target_version,
                        reply["reason"].as_str().unwrap_or("unknown reason")
                    );
                    let active = supervisor.state.active_version.clone();
                    supervisor
                        .start(&active)
                        .await
                        .map_err(|error| format!("could not start blob-1 {active}: {error}"))?;
                } else if supervisor.child.is_none() {
                    return Err(format!(
                        "queued update to {} failed: {}",
                        queued.target_version,
                        reply["reason"].as_str().unwrap_or("unknown error")
                    )
                    .into());
                }
            } else {
                let active = supervisor.state.active_version.clone();
                supervisor
                    .start(&active)
                    .await
                    .map_err(|error| format!("could not start blob-1 {active}: {error}"))?;
            }
            Ok(true)
        } => res,
        _ = interrupt.recv() => Ok(false),
        _ = terminate.recv() => Ok(false),
    };
    match startup {
        Ok(false) => {
            supervisor.stop_child().await;
            return Ok(());
        }
        Err(error) => {
            supervisor.stop_child().await;
            return Err(error);
        }
        Ok(true) => {}
    }
    // Reconcile registered CLI symlinks with the committed active version in
    // case a previous launcher exited after committing and before repointing.
    if installed(&root, &supervisor.state.active_version) {
        if let Err(error) = repoint_launchers(&root, &supervisor.state.active_version) {
            eprintln!("warning: could not reconcile launcher symlinks: {error}");
        }
    }
    eprintln!("supervising blob-1 {}", supervisor.state.active_version);

    let sock = socket_path(&root);
    let _ = std::fs::remove_file(&sock);
    let listener = tokio::net::UnixListener::bind(&sock)?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;

    let result: Result<()> = loop {
        if supervisor.child.is_none() {
            break Err("serve is not running".into());
        }
        let mut child = supervisor.child.take().expect("checked above");
        tokio::select! {
            conn = listener.accept() => {
                supervisor.child = Some(child);
                match conn {
                    Ok((stream, _)) => {
                        let shutdown = tokio::select! {
                            () = supervisor.handle(stream) => false,
                            _ = interrupt.recv() => true,
                            _ = terminate.recv() => true,
                        };
                        if shutdown {
                            break Ok(());
                        }
                    }
                    Err(error) => break Err(error.into()),
                }
            }
            status = child.wait() => {
                break match status {
                    Ok(status) if status.success() => Ok(()),
                    Ok(status) => Err(format!("serve exited: {status}").into()),
                    Err(error) => Err(error.into()),
                };
            }
            _ = interrupt.recv() => {
                supervisor.child = Some(child);
                break Ok(());
            }
            _ = terminate.recv() => {
                supervisor.child = Some(child);
                break Ok(());
            }
        }
    };
    supervisor.stop_child().await;
    let _ = std::fs::remove_file(&sock);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(raw: &str) -> Version {
        Version::parse(raw).unwrap()
    }

    #[test]
    fn version_parsing_and_order() {
        assert!(v("0.0.0-latest.20261122.0230") < v("0.0.0-latest.20261122.0245"));
        assert!(v("0.0.0-latest.20261122.0245") < v("0.0.0"));
        assert!(v("0.0.0") < v("0.0.1"));
        assert!(v("1.0.0-alpha") < v("1.0.0"));
        assert!(v("1.0.0-alpha.1") < v("1.0.0-alpha.beta"));
        assert!(v("1.0.0-2") < v("1.0.0-10"));
        assert!(v("0.0.0-latest.20261122.2") < v("0.0.0-latest.20261122.10"));
        assert_eq!(v("v0.1.0"), v("0.1.0"));
        assert!(Version::parse("0.0").is_err());
        assert!(Version::parse("1.010.0").is_err());
        assert!(Version::parse("01.0.0").is_err());
        assert!(Version::parse("0.0.0-latest..1").is_err());
        assert!(Version::parse("0.0.0-a_b").is_err());
        assert!(Version::parse("0.0.0+../../evil").is_err());
        assert!(Version::parse("../0.0.1").is_err());
        assert!(Version::parse("0.0.0-latest.20261122.0230\nextra").is_err());
    }

    #[test]
    fn upgrade_rules() {
        // A dev 0.0.0 build may adopt its first latest release without flags.
        assert!(upgrade_allowed(
            &v("0.0.0"),
            &v("0.0.0-latest.20261122.0230"),
            false
        ));
        assert!(!upgrade_allowed(
            &v("0.0.0-latest.20261122.0245"),
            &v("0.0.0-latest.20261122.0230"),
            false
        ));
        assert!(upgrade_allowed(
            &v("0.0.0-latest.20261122.0245"),
            &v("0.0.0-latest.20261122.0230"),
            true
        ));
        assert!(upgrade_allowed(&v("1.0.0"), &v("1.0.1"), false));
        assert!(!upgrade_allowed(&v("1.0.0"), &v("1.0.0"), false));
        assert!(!upgrade_allowed(&v("1.0.0"), &v("0.9.9"), false));
    }

    #[test]
    fn channel_matching() {
        assert!(Channel::Latest.matches(&v("0.0.0-latest.20261122.0230")));
        assert!(!Channel::Latest.matches(&v("0.0.0-latest.20261122.230")));
        assert!(!Channel::Latest.matches(&v("0.0.0")));
        assert!(!Channel::Latest.matches(&v("0.0.0-beta.1")));
        assert!(Channel::Stable.matches(&v("1.2.3")));
        assert!(!Channel::Stable.matches(&v("1.2.3-latest.20261122.0230")));
        let tags = [
            "v0.0.0-latest.20261121.0100",
            "v0.0.0-latest.20261122.0230",
            "v0.0.0-latest.20261122.230",
            "v1.0.0",
        ];
        assert_eq!(
            pick_version(tags.into_iter(), Channel::Latest).unwrap(),
            "0.0.0-latest.20261122.0230"
        );
        assert_eq!(pick_version(tags.into_iter(), Channel::Stable).unwrap(), "1.0.0");
    }

    #[test]
    fn sums_parsing() {
        let sums = "aaa111  blob-1-1.0.0-linux-x64.tar.gz\nbbb222 *blob-1-1.0.0-darwin-arm64.tar.gz\n";
        assert_eq!(
            checksum_for(sums, "blob-1-1.0.0-linux-x64.tar.gz").unwrap(),
            "aaa111"
        );
        assert_eq!(
            checksum_for(sums, "blob-1-1.0.0-darwin-arm64.tar.gz").unwrap(),
            "bbb222"
        );
        assert!(checksum_for(sums, "blob-1-9.9.9-linux-x64.tar.gz").is_err());
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "blob-1-runtime-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_executable(path: &Path, script: &str) {
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A mock versioned binary: `versions/<version>/blob-1` as a shell script
    /// plus the `.install-complete` marker.
    fn mock_version(root: &Path, version: &str, script: &str) {
        let dir = version_dir(root, version);
        std::fs::create_dir_all(&dir).unwrap();
        write_executable(&dir.join("blob-1"), script);
        std::fs::write(
            dir.join(".install-complete"),
            format!("{version}\n"),
        )
        .unwrap();
    }

    fn supervisor(root: &Path, data_dir: &Path, active: &str) -> Supervisor {
        Supervisor {
            root: root.to_path_buf(),
            db_path: data_dir.join("agent.db"),
            child_args: vec!["serve".into()],
            ready_timeout: Duration::from_secs(10),
            state: ServiceState {
                active_version: active.to_owned(),
                ..ServiceState::default()
            },
            seq: 0,
            child: None,
        }
    }

    #[test]
    fn db_backup_and_restore() {
        let dir = temp_dir("db");
        let db = dir.join("agent.db");
        let backup = dir.join("backup");
        std::fs::write(&db, b"original").unwrap();
        std::fs::write(suffixed(&db, "-wal"), b"wal-one").unwrap();
        backup_db(&db, &backup).unwrap();
        assert_eq!(std::fs::read(backup.join("agent.db")).unwrap(), b"original");
        assert_eq!(
            std::fs::read(backup.join("agent.db-wal")).unwrap(),
            b"wal-one"
        );
        // Trial overwrites the db and leaves a wal/shm the backup lacks.
        std::fs::write(&db, b"trial").unwrap();
        std::fs::write(suffixed(&db, "-wal"), b"wal-two").unwrap();
        std::fs::write(suffixed(&db, "-shm"), b"shm").unwrap();
        restore_db(&backup, &db).unwrap();
        assert_eq!(std::fs::read(&db).unwrap(), b"original");
        assert_eq!(std::fs::read(suffixed(&db, "-wal")).unwrap(), b"wal-one");
        assert!(!suffixed(&db, "-shm").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launcher_repoint() {
        let root = temp_dir("repoint");
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        mock_version(&root, "1.0.0", "#!/bin/sh\nexit 0\n");
        std::fs::write(launchers_path(&root), format!("{}\n", bin_dir.join("blob-1").display()))
            .unwrap();
        let repointed = repoint_launchers(&root, "1.0.0").unwrap();
        assert_eq!(repointed, vec![bin_dir.join("blob-1")]);
        assert_eq!(
            std::fs::read_link(bin_dir.join("blob-1")).unwrap(),
            version_dir(&root, "1.0.0").join("blob-1")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    const MOCK_OK: &str =
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'blob-1 test'; exit 0; fi\nprintf 'ready\\n' > \"$BLOB_1_READY_FILE\"\nexec sleep 60\n";
    const MOCK_CRASH: &str =
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'blob-1 test'; exit 0; fi\nexit 1\n";
    const MOCK_LEGACY: &str = "#!/bin/sh\nexit 2\n";

    #[tokio::test]
    async fn supervised_update_commits() {
        let root = temp_dir("commit");
        let data = temp_dir("commit-data");
        mock_version(&root, "0.0.0", MOCK_OK);
        mock_version(&root, "1.0.0", MOCK_OK);
        std::fs::write(data.join("agent.db"), b"db").unwrap();
        let mut sup = supervisor(&root, &data, "0.0.0");
        sup.start("0.0.0").await.unwrap();
        let first_pid = sup.child.as_ref().unwrap().id();
        let reply = sup.apply_update("1.0.0", false, None).await;
        assert_eq!(reply["status"], "committed");
        assert_eq!(sup.state.active_version, "1.0.0");
        assert_ne!(sup.child.as_ref().unwrap().id(), first_pid);
        assert!(backups_dir(&root).read_dir().unwrap().next().is_none());
        let on_disk = load_state(&root).unwrap().unwrap();
        assert_eq!(on_disk.active_version, "1.0.0");
        assert_eq!(on_disk.update.unwrap().status, "committed");
        sup.stop_child().await;
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn supervised_update_rolls_back() {
        let root = temp_dir("rollback");
        let data = temp_dir("rollback-data");
        mock_version(&root, "0.0.0", MOCK_OK);
        mock_version(&root, "1.0.0", MOCK_CRASH);
        std::fs::write(data.join("agent.db"), b"original").unwrap();
        let mut sup = supervisor(&root, &data, "0.0.0");
        sup.start("0.0.0").await.unwrap();
        let reply = sup.apply_update("1.0.0", false, None).await;
        assert_eq!(reply["status"], "rolled-back");
        assert_eq!(sup.state.active_version, "0.0.0");
        // The db snapshot was restored and the old version is running again.
        assert_eq!(std::fs::read(data.join("agent.db")).unwrap(), b"original");
        assert!(sup.child.as_ref().unwrap().id().is_some());
        let on_disk = load_state(&root).unwrap().unwrap();
        assert_eq!(on_disk.update.unwrap().status, "rolled-back");
        sup.stop_child().await;
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn pending_update_recovers() {
        let root = temp_dir("pending");
        let data = temp_dir("pending-data");
        std::fs::create_dir_all(&data).unwrap();
        let db = data.join("agent.db");
        let backup = backups_dir(&root).join("u1");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("agent.db"), b"backed-up").unwrap();
        // A leftover staging dir from an interrupted backup must be cleaned up.
        let stale_staging = backups_dir(&root).join(".staging-u2-99");
        std::fs::create_dir_all(&stale_staging).unwrap();
        std::fs::write(&db, b"trial").unwrap();
        std::fs::write(suffixed(&db, "-shm"), b"leftover").unwrap();
        let claimed = QueuedUpdate {
            target_version: "1.0.0".into(),
            allow_downgrade: false,
            created_at_ms: 42,
        };
        save_json(&queued_update_path(&root), &claimed).unwrap();
        let state = ServiceState {
            active_version: "1.0.0".into(),
            update: Some(UpdateRecord {
                id: "u1".into(),
                from_version: "0.0.0".into(),
                target_version: "1.0.0".into(),
                db_path: db.clone(),
                status: "pending".into(),
                reason: None,
                claimed_queue: Some(claimed),
            }),
            ..ServiceState::default()
        };
        save_state(&root, &state).unwrap();
        let mut loaded = load_state(&root).unwrap().unwrap();
        // Holding the exclusive DB lock simulates a still-running trial child:
        // recovery must refuse and keep the backup intact.
        let lock = lock_db(&db).unwrap();
        assert!(recover_pending(&root, &mut loaded).is_err());
        assert!(backup.exists());
        assert_eq!(std::fs::read(&db).unwrap(), b"trial");
        lock.unlock().unwrap();
        // Once the lock is released, recovery restores the DB, clears the
        // claimed queued update so it isn't retried, and cleans up.
        assert!(recover_pending(&root, &mut loaded).unwrap());
        assert_eq!(loaded.active_version, "0.0.0");
        assert_eq!(loaded.update.as_ref().unwrap().status, "rolled-back");
        assert_eq!(std::fs::read(&db).unwrap(), b"backed-up");
        assert!(!suffixed(&db, "-shm").exists());
        assert!(!backup.exists());
        assert!(!stale_staging.exists());
        assert!(peek_queued_update(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn prune_preserves_staging_dirs() {
        let root = temp_dir("prune-staging");
        let versions = versions_dir(&root);
        std::fs::create_dir_all(versions.join(".staging-999-0")).unwrap();
        std::fs::create_dir_all(versions.join("0.0.1")).unwrap();
        std::fs::create_dir_all(versions.join("0.0.2")).unwrap();
        let keep: BTreeSet<String> = ["0.0.2".into()].into_iter().collect();
        prune_versions(&root, &keep);
        assert!(versions.join(".staging-999-0").exists());
        assert!(!versions.join("0.0.1").exists());
        assert!(versions.join("0.0.2").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn github_token_restricted_to_github_urls() {
        assert!(is_github_url("https://github.com/chanyeinthaw/blob-1/releases/download/v0.0.0/SHA256SUMS"));
        assert!(is_github_url("https://api.github.com/repos/chanyeinthaw/blob-1/releases"));
        assert!(!is_github_url("https://mirror.example.com/v0.0.0/SHA256SUMS"));
        assert!(!is_github_url("http://127.0.0.1:8080/v0.0.0/SHA256SUMS"));
    }

    #[test]
    fn queued_update_roundtrip_separate_from_service_state() {
        let root = temp_dir("queued-update");
        let state = ServiceState {
            active_version: "0.0.0".into(),
            ..ServiceState::default()
        };
        save_state(&root, &state).unwrap();
        save_json(
            &queued_update_path(&root),
            &QueuedUpdate {
                target_version: "0.0.0-latest.20261122.0230".into(),
                allow_downgrade: false,
                created_at_ms: 1,
            },
        )
        .unwrap();
        // Taking consumes queued-update.json without disturbing service-state.json.
        let taken = take_queued_update(&root).unwrap().unwrap();
        assert_eq!(taken.target_version, "0.0.0-latest.20261122.0230");
        assert!(!taken.allow_downgrade);
        assert!(take_queued_update(&root).unwrap().is_none());
        assert_eq!(load_state(&root).unwrap().unwrap().active_version, "0.0.0");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_removes_db_created_by_failed_trial_when_originally_absent() {
        let dir = temp_dir("restore-absent");
        let db = dir.join("agent.db");
        let backup = dir.join("backup");
        // DB does not exist yet when backup runs.
        backup_db(&db, &backup).unwrap();
        // Trial creates a brand-new DB and WAL, then fails.
        std::fs::write(&db, b"trial-created").unwrap();
        std::fs::write(suffixed(&db, "-wal"), b"trial-wal").unwrap();
        restore_db(&backup, &db).unwrap();
        assert!(!db.exists());
        assert!(!suffixed(&db, "-wal").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launcher_lock_blocks_second_launcher() {
        let root = temp_dir("launcher-lock");
        let first = lock_launcher(&root).unwrap();
        assert!(lock_launcher(&root).is_err());
        first.unlock().unwrap();
        drop(first);
        let second = lock_launcher(&root).unwrap();
        second.unlock().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn legacy_target_rejected_before_stopping_active_child() {
        let root = temp_dir("legacy-reject");
        let data = temp_dir("legacy-reject-data");
        mock_version(&root, "1.0.0", MOCK_OK);
        mock_version(&root, "0.0.0", MOCK_LEGACY);
        let mut sup = supervisor(&root, &data, "1.0.0");
        sup.start("1.0.0").await.unwrap();
        let first_pid = sup.child.as_ref().unwrap().id();
        let reply = sup.apply_update("0.0.0", true, None).await;
        assert_eq!(reply["status"], "rejected");
        assert_eq!(sup.child.as_ref().unwrap().id(), first_pid);
        sup.stop_child().await;
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn open_shared_blocks_db_maintenance_lock() {
        let dir = temp_dir("shared-maint");
        let db = dir.join("agent.db");
        let shared = storage_sqlite::SqliteStorage::open_shared(&db, None).unwrap();
        assert!(lock_db(&db).is_err());
        drop(shared);
        let lock = lock_db(&db).unwrap();
        assert!(storage_sqlite::SqliteStorage::open_shared(&db, None).is_err());
        lock.unlock().unwrap();
        assert!(storage_sqlite::SqliteStorage::open_shared(&db, None).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn live_commit_clears_superseded_queued_update() {
        let root = temp_dir("clear-queued");
        let data = temp_dir("clear-queued-data");
        mock_version(&root, "1.0.0", MOCK_OK);
        mock_version(&root, "0.9.0", MOCK_OK);
        mock_version(&root, "2.0.0", MOCK_OK);
        std::fs::write(data.join("agent.db"), b"db").unwrap();
        save_json(
            &queued_update_path(&root),
            &QueuedUpdate {
                target_version: "0.9.0".into(),
                allow_downgrade: true,
                created_at_ms: 1,
            },
        )
        .unwrap();
        let mut sup = supervisor(&root, &data, "1.0.0");
        sup.start("1.0.0").await.unwrap();
        let reply = sup.apply_update("2.0.0", false, None).await;
        assert_eq!(reply["status"], "committed");
        assert!(peek_queued_update(&root).unwrap().is_none());
        sup.stop_child().await;
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&data);
    }
}
