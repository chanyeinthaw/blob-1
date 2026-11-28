//! Finds and downloads skills from a public GitHub folder: one commit lookup
//! through the REST API, then the commit's tarball from codeload.

use std::io::Read;
use std::time::Duration;

use environment::{parse_frontmatter, valid_skill_name, SKILL_FILE};

use crate::Result;

/// Biggest tarball we download; skills repos are usually far smaller.
const MAX_ARCHIVE_BYTES: usize = 64 << 20;
/// Longest a whole preview or import may take; the web client waits a bit longer.
pub const OPERATION_TIMEOUT: Duration = Duration::from_secs(120);
/// Biggest total size of the files kept from the archive.
const MAX_KEPT_BYTES: u64 = 32 << 20;

/// The repo folder a preview looked at, pinned to a commit.
#[derive(Debug, Clone)]
pub struct Source {
    /// `owner/name`
    pub repo: String,
    pub commit: String,
    /// Folder inside the repo, `""` for the root.
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct FoundSkill {
    /// The skill's folder inside the repo.
    pub folder: String,
    pub name: String,
    pub description: String,
    pub files: usize,
    pub bytes: u64,
}

struct Entry {
    path: String,
    mode: u32,
    content: Vec<u8>,
}

/// Resolves a GitHub URL (repo, `tree/<ref>/<path>` or `blob/<ref>/<path>`)
/// plus an optional extra `path`, and lists the skills under that folder.
/// Without any path the folder is `skills/` at the repo root.
pub async fn find(url: &str, path: &str) -> Result<(Source, Vec<FoundSkill>)> {
    let (repo, mut rest) = parse_url(url)?;
    let extra = path.trim_matches('/');
    let client = client()?;
    // After `tree/<ref>` the path continues the URL, so a ref with `/` in it
    // still resolves however the two fields split it.
    let (commit, mut folder) = match (rest.is_empty(), extra.is_empty()) {
        (false, _) => {
            rest.extend(extra.split('/').filter(|s| !s.is_empty()).map(str::to_owned));
            resolve(&client, &repo, &rest).await?
        }
        (true, false) => (resolve(&client, &repo, &[]).await?.0, extra.to_owned()),
        // No path anywhere: skills live in `skills/` at the repo root.
        (true, true) => (resolve(&client, &repo, &[]).await?.0, "skills".to_owned()),
    };
    if folder == SKILL_FILE || folder.ends_with(&format!("/{SKILL_FILE}")) {
        folder = folder
            .strip_suffix(SKILL_FILE)
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_owned();
    }
    let prefix = folder.clone();
    let entries = download(&client, &repo, &commit, move |path| within(path, &prefix)).await?;
    let mut skills: Vec<FoundSkill> = entries
        .iter()
        .filter_map(|entry| skill_folder(&entry.path))
        .filter_map(|skill| {
            let manifest = entries
                .iter()
                .find(|entry| entry.path == join(&skill, SKILL_FILE))?;
            let frontmatter = parse_frontmatter(std::str::from_utf8(&manifest.content).ok()?)?;
            if !valid_skill_name(&frontmatter.name) {
                return None;
            }
            let files: Vec<&Entry> = entries
                .iter()
                .filter(|entry| within(&entry.path, &skill))
                .collect();
            Some(FoundSkill {
                files: files.len(),
                bytes: files.iter().map(|entry| entry.content.len() as u64).sum(),
                folder: skill,
                name: frontmatter.name,
                description: frontmatter.description,
            })
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    if skills.is_empty() {
        return Err(format!("no skills found in {repo}/{folder}").into());
    }
    Ok((
        Source {
            repo,
            commit,
            path: folder,
        },
        skills,
    ))
}

/// Downloads `folders` of `repo` at `commit` as skill bundles, paths relative
/// to each folder.
pub async fn fetch(
    repo: &str,
    commit: &str,
    folders: &[String],
) -> Result<Vec<Vec<storage::SkillFile>>> {
    if !valid_repo(repo) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) || commit.len() != 40 {
        return Err("invalid repo or commit".into());
    }
    if folders.is_empty() {
        return Err("choose at least one skill".into());
    }
    let wanted = folders.to_vec();
    let entries = download(&client()?, repo, commit, move |path| {
        wanted.iter().any(|folder| within(path, folder))
    })
    .await?;
    folders
        .iter()
        .map(|folder| {
            let files: Vec<storage::SkillFile> = entries
                .iter()
                .filter(|entry| within(&entry.path, folder))
                .map(|entry| storage::SkillFile {
                    path: relative(&entry.path, folder).to_owned(),
                    mode: entry.mode,
                    content: entry.content.clone(),
                })
                .collect();
            if !files.iter().any(|file| file.path == SKILL_FILE) {
                return Err(format!("no {SKILL_FILE} in {folder}").into());
            }
            Ok(files)
        })
        .collect()
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("blob-1")
        .timeout(Duration::from_secs(60))
        .build()?)
}

/// `https://github.com/<owner>/<repo>[/tree|blob/<ref and path…>]` →
/// (`owner/repo`, segments after `tree`/`blob`).
fn parse_url(raw: &str) -> Result<(String, Vec<String>)> {
    let url = url::Url::parse(raw.trim()).map_err(|_| "Not a GitHub URL")?;
    if !matches!(url.host_str(), Some("github.com" | "www.github.com")) {
        return Err("Not a GitHub URL".into());
    }
    let segments: Vec<&str> = url
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let [owner, name, rest @ ..] = segments.as_slice() else {
        return Err("GitHub URL needs an owner and a repo".into());
    };
    let repo = format!("{owner}/{}", name.trim_end_matches(".git"));
    if !valid_repo(&repo) {
        return Err("Not a GitHub repo URL".into());
    }
    let rest = match rest {
        [] => Vec::new(),
        ["tree" | "blob", rest @ ..] if !rest.is_empty() => {
            rest.iter().map(|s| (*s).to_owned()).collect()
        }
        _ => return Err("Use a repo URL or a tree/blob URL".into()),
    };
    Ok((repo, rest))
}

fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |part: Option<&str>| {
        part.is_some_and(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// Finds the commit for the ref at the front of `rest` (refs may contain
/// `/`, so the shortest prefix GitHub knows wins) and returns the rest as the
/// folder. No ref means the default branch.
async fn resolve(
    client: &reqwest::Client,
    repo: &str,
    rest: &[String],
) -> Result<(String, String)> {
    if rest.is_empty() {
        return Ok((
            commit_for(client, repo, "HEAD")
                .await?
                .ok_or("repo not found")?,
            String::new(),
        ));
    }
    for split in 1..=rest.len() {
        let reference = rest[..split].join("/");
        if let Some(commit) = commit_for(client, repo, &reference).await? {
            return Ok((commit, rest[split..].join("/")));
        }
    }
    Err(format!("no branch, tag or commit of {repo} matches the URL").into())
}

async fn commit_for(
    client: &reqwest::Client,
    repo: &str,
    reference: &str,
) -> Result<Option<String>> {
    let encoded: String = url::form_urlencoded::byte_serialize(reference.as_bytes()).collect();
    let response = client
        .get(format!(
            "https://api.github.com/repos/{repo}/commits/{encoded}"
        ))
        .header("Accept", "application/vnd.github.sha")
        .send()
        .await?;
    match response.status().as_u16() {
        200 => {
            let commit = response.text().await?.trim().to_owned();
            Ok((commit.len() == 40).then_some(commit))
        }
        404 | 422 => Ok(None),
        403 | 429 => Err("GitHub rate limit reached; try again later".into()),
        status => Err(format!("GitHub returned {status}").into()),
    }
}

/// Downloads the commit's tarball and keeps the regular files `keep` accepts,
/// paths relative to the repo root.
async fn download(
    client: &reqwest::Client,
    repo: &str,
    commit: &str,
    keep: impl Fn(&str) -> bool + Send + 'static,
) -> Result<Vec<Entry>> {
    let mut response = client
        .get(format!(
            "https://codeload.github.com/{repo}/tar.gz/{commit}"
        ))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!("GitHub returned {} for the archive", response.status()).into());
    }
    let mut archive = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if archive.len() + chunk.len() > MAX_ARCHIVE_BYTES {
            return Err("repository archive is too large".into());
        }
        archive.extend_from_slice(&chunk);
    }
    tokio::task::spawn_blocking(move || unpack(&archive, keep)).await?
}

fn unpack(archive: &[u8], keep: impl Fn(&str) -> bool) -> Result<Vec<Entry>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut entries = Vec::new();
    let mut kept = 0u64;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let raw = entry.path()?.to_string_lossy().into_owned();
        // Every path starts with the `<repo>-<commit>/` folder.
        let Some((_, path)) = raw.split_once('/') else {
            continue;
        };
        let path = path.trim_end_matches('/').to_owned();
        if path.is_empty() || !keep(&path) {
            continue;
        }
        match entry.header().entry_type() {
            tar::EntryType::Regular => {}
            tar::EntryType::Directory => continue,
            // pax headers carry metadata, not files.
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader => continue,
            _ => {
                return Err(
                    format!("{path} is a link or special file; skills must be plain files").into(),
                )
            }
        }
        kept += entry.size();
        if kept > MAX_KEPT_BYTES {
            return Err("skills are too large".into());
        }
        let executable = entry.header().mode()? & 0o111 != 0;
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        entries.push(Entry {
            path,
            mode: if executable { 0o755 } else { 0o644 },
            content,
        });
    }
    Ok(entries)
}

fn join(folder: &str, path: &str) -> String {
    match folder.is_empty() {
        true => path.to_owned(),
        false => format!("{folder}/{path}"),
    }
}

/// Whether `path` is `folder` itself or inside it; `""` is the repo root.
fn within(path: &str, folder: &str) -> bool {
    folder.is_empty() || path == folder || path.starts_with(&format!("{folder}/"))
}

fn relative<'a>(path: &'a str, folder: &str) -> &'a str {
    match folder.is_empty() {
        true => path,
        false => path
            .strip_prefix(folder)
            .unwrap_or(path)
            .trim_start_matches('/'),
    }
}

/// The folder of a `SKILL.md` path.
fn skill_folder(path: &str) -> Option<String> {
    match path.rsplit_once('/') {
        Some((folder, SKILL_FILE)) => Some(folder.to_owned()),
        None if path == SKILL_FILE => Some(String::new()),
        _ => None,
    }
}
