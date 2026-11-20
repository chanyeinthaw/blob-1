use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use environment::{
    parse_frontmatter, valid_skill_name, Environment, Replacement, SkillBundle, SkillEntry,
    ToolCall, ToolError, ToolInput, ToolResult, SKILL_FILE,
};
use tokio::time::{sleep_until, Instant};

const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 50 * 1024;
const TAIL_KEEP_BYTES: usize = 2 * MAX_BYTES;
const TAIL_TRIM_AT: usize = 4 * MAX_BYTES;
const MAX_TIMEOUT_SECS: f64 = 2_147_483.647;
const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);

struct ProcessGroup(Option<u32>);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .status();
        }
    }
}

pub struct LocalEnvironment {
    root: PathBuf,
    skills_dir: PathBuf,
}

impl LocalEnvironment {
    pub fn new(root: impl Into<PathBuf>, skills_dir: Option<PathBuf>) -> Result<Self, ToolError> {
        let root = root.into().canonicalize().map_err(io_error)?;
        let skills_dir = match skills_dir {
            Some(dir) => dir,
            None => PathBuf::from(
                std::env::var_os("HOME")
                    .ok_or_else(|| ToolError::Invalid("HOME is not set".into()))?,
            )
            .join(".agents/skills"),
        };
        std::fs::create_dir_all(&skills_dir).map_err(io_error)?;
        let skills_dir = skills_dir.canonicalize().map_err(io_error)?;
        Ok(Self { root, skills_dir })
    }

    async fn read_manifest(&self) -> Result<BTreeMap<String, String>, ToolError> {
        match tokio::fs::read(self.skills_dir.join(".managed.json")).await {
            Ok(data) => {
                serde_json::from_slice(&data).map_err(|e| ToolError::Invalid(e.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(io_error(e)),
        }
    }

    async fn write_manifest(&self, manifest: &BTreeMap<String, String>) -> Result<(), ToolError> {
        let temp = temporary_path(&self.skills_dir);
        let data = serde_json::to_vec(manifest).map_err(|e| ToolError::Invalid(e.to_string()))?;
        tokio::fs::write(&temp, data).await.map_err(io_error)?;
        tokio::fs::rename(temp, self.skills_dir.join(".managed.json"))
            .await
            .map_err(io_error)
    }

    fn resolve_readable(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let joined = self.root.join(path);
        let lexical_skill = joined.is_absolute()
            && !joined
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            && joined.starts_with(&self.skills_dir);
        let resolved = joined
            .canonicalize()
            .map_err(|_| ToolError::NotFound(path.display().to_string()))?;
        if lexical_skill || resolved.starts_with(&self.skills_dir) {
            return Ok(resolved);
        }
        self.ensure_inside_root(&resolved)?;
        Ok(resolved)
    }

    fn resolve_existing(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let resolved = self
            .root
            .join(path)
            .canonicalize()
            .map_err(|_| ToolError::NotFound(path.display().to_string()))?;
        self.ensure_inside_root(&resolved)?;
        Ok(resolved)
    }

    fn resolve_for_write(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let joined: PathBuf = self.root.join(path).components().collect();
        match joined.symlink_metadata() {
            Ok(_) => return self.resolve_existing(&joined),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
        let escape = || ToolError::PathEscape(path.display().to_string());
        let file_name = joined.file_name().ok_or_else(escape)?;
        let mut existing = joined.parent().ok_or_else(escape)?;
        let mut missing = Vec::new();
        while !existing.exists() {
            missing.push(existing.file_name().ok_or_else(escape)?);
            existing = existing.parent().ok_or_else(escape)?;
        }
        let mut resolved = existing.canonicalize().map_err(io_error)?;
        self.ensure_inside_root(&resolved)?;
        for component in missing.into_iter().rev() {
            if component == ".." {
                return Err(escape());
            }
            resolved.push(component);
        }
        resolved.push(file_name);
        Ok(resolved)
    }

    fn ensure_inside_root(&self, path: &Path) -> Result<(), ToolError> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err(ToolError::PathEscape(path.display().to_string()))
        }
    }

    async fn read(
        &self,
        path: &Path,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<String, ToolError> {
        let resolved = self.resolve_readable(path)?;
        let content = tokio::fs::read_to_string(resolved)
            .await
            .map_err(io_error)?;
        let lines: Vec<&str> = content.split('\n').collect();
        let total = lines.len();

        let start = offset.unwrap_or(1).saturating_sub(1);
        if start >= total {
            return Err(ToolError::Invalid(format!(
                "Offset {} is beyond end of file ({total} lines total)",
                start + 1
            )));
        }
        let end = limit.map_or(total, |limit| start.saturating_add(limit).min(total));
        let selected = &lines[start..end];

        let (kept, by_bytes) = head(selected);
        if let (0, Some(first)) = (kept, selected.first()) {
            let line = start + 1;
            return Ok(format!(
                "[Line {line} is {}, exceeds {} limit. Use bash: sed -n '{line}p' {} | head -c {MAX_BYTES}]",
                format_size(first.len()),
                format_size(MAX_BYTES),
                path.display(),
            ));
        }

        let mut output = selected[..kept].join("\n");
        let last_shown = start + kept;
        if kept < selected.len() {
            let limit_note = if by_bytes {
                format!(" ({} limit)", format_size(MAX_BYTES))
            } else {
                String::new()
            };
            output.push_str(&format!(
                "\n\n[Showing lines {}-{last_shown} of {total}{limit_note}. Use offset={} to continue.]",
                start + 1,
                last_shown + 1,
            ));
        } else if end < total {
            output.push_str(&format!(
                "\n\n[{} more lines in file. Use offset={} to continue.]",
                total - end,
                end + 1,
            ));
        }
        Ok(output)
    }

    async fn write(&self, path: &Path, content: &str) -> Result<String, ToolError> {
        let resolved = self.resolve_for_write(path)?;
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
        }
        tokio::fs::write(&resolved, content)
            .await
            .map_err(io_error)?;
        Ok(format!("Successfully wrote to {}", path.display()))
    }

    async fn edit(&self, path: &Path, edits: &[Replacement]) -> Result<String, ToolError> {
        if edits.is_empty() {
            return Err(ToolError::Invalid(
                "Edit tool input is invalid. edits must contain at least one replacement.".into(),
            ));
        }
        let display = path.display().to_string();
        let resolved = self.resolve_existing(path)?;
        let raw = tokio::fs::read_to_string(&resolved)
            .await
            .map_err(io_error)?;

        let (bom, content) = match raw.strip_prefix('\u{feff}') {
            Some(rest) => ("\u{feff}", rest),
            None => ("", raw.as_str()),
        };
        let crlf = content
            .find('\n')
            .is_some_and(|index| content[..index].ends_with('\r'));
        let content = to_lf(content);

        let updated = apply_edits(&content, edits, &display)?;
        let updated = if crlf {
            updated.replace('\n', "\r\n")
        } else {
            updated
        };
        tokio::fs::write(&resolved, format!("{bom}{updated}"))
            .await
            .map_err(io_error)?;
        Ok(format!(
            "Successfully replaced {} block(s) in {display}.",
            edits.len()
        ))
    }

    async fn bash(&self, command: &str, timeout: Option<f64>) -> Result<String, ToolError> {
        let deadline = match timeout {
            None => None,
            Some(secs) if !(secs.is_finite() && secs > 0.0) => {
                return Err(ToolError::Invalid(
                    "Invalid timeout: must be a finite number of seconds".into(),
                ));
            }
            Some(secs) if secs > MAX_TIMEOUT_SECS => {
                return Err(ToolError::Invalid(
                    "Invalid timeout: maximum is 2147483.647 seconds".into(),
                ));
            }
            Some(secs) => Some(Instant::now() + Duration::from_secs_f64(secs)),
        };

        let (mut reader, writer) = std::io::pipe().map_err(io_error)?;
        let started = Instant::now();
        let mut child = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(writer.try_clone().map_err(io_error)?)
            .stderr(writer)
            .process_group(0)
            .spawn()
            .map_err(io_error)?;
        let pid = child.id();
        let mut process_group = ProcessGroup(pid);

        let (sender, mut chunks) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        std::thread::spawn(move || {
            let mut buffer = vec![0; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if sender.blocking_send(buffer[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });

        let mut output = Output::default();
        let mut status = None;
        let mut exited_after = None;
        let mut eof = false;
        let mut grace = None;
        let mut timed_out = false;
        let mut exited_before_timeout = None;
        loop {
            if status.is_some() && eof {
                break;
            }
            tokio::select! {
                chunk = chunks.recv(), if !eof => match chunk {
                    Some(chunk) => {
                        output.push(&chunk);
                        if status.is_some() {
                            grace = Some(Instant::now() + EXIT_STDIO_GRACE);
                        }
                    }
                    None => eof = true,
                },
                exited = child.wait(), if status.is_none() => {
                    status = Some(exited.map_err(io_error)?);
                    exited_after = Some(started.elapsed());
                    grace = Some(Instant::now() + EXIT_STDIO_GRACE);
                }
                _ = sleep_until(grace.unwrap_or_else(Instant::now)), if grace.is_some() => break,
                _ = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() && !timed_out => {
                    timed_out = true;
                    exited_before_timeout = exited_after;
                    if let Some(pid) = pid {
                        let _ = std::process::Command::new("kill")
                            .args(["-KILL", "--", &format!("-{pid}")])
                            .status();
                    }
                }
            }
        }

        process_group.0 = None;
        let body = output.render();
        let exit_code = status.map_or(1, |status| {
            status
                .code()
                .or_else(|| status.signal().map(|signal| 128 + signal))
                .unwrap_or(1)
        });

        if timed_out {
            let mut message = format!(
                "Command timed out after {} seconds",
                timeout.unwrap_or_default()
            );
            if let Some(after) = exited_before_timeout {
                message.push_str(&format!(
                    ". The shell exited with code {exit_code} after {:.1} seconds, but background processes it started kept writing output until the timeout.",
                    after.as_secs_f64()
                ));
            }
            return Ok(if body.is_empty() {
                message
            } else {
                format!("{body}\n\n{message}")
            });
        }

        let mut result = if body.is_empty() {
            "(no output)".to_owned()
        } else {
            body
        };
        if exit_code != 0 {
            result.push_str(&format!("\n\nCommand exited with code {exit_code}"));
        }
        Ok(result)
    }
}

fn apply_edits(content: &str, edits: &[Replacement], path: &str) -> Result<String, ToolError> {
    let multi = edits.len() > 1;
    let edits: Vec<(String, String)> = edits
        .iter()
        .map(|edit| (to_lf(&edit.old_text), to_lf(&edit.new_text)))
        .collect();

    for (index, (old, _)) in edits.iter().enumerate() {
        if old.is_empty() {
            return Err(ToolError::Invalid(if multi {
                format!("edits[{index}].oldText must not be empty in {path}.")
            } else {
                format!("oldText must not be empty in {path}.")
            }));
        }
    }

    let exact = edits.iter().all(|(old, _)| content.contains(old.as_str()));
    let base = if exact {
        content.to_owned()
    } else {
        fuzzy(content)
    };

    let mut matches = Vec::with_capacity(edits.len());
    for (index, (old, new)) in edits.iter().enumerate() {
        let needle = if exact { old.clone() } else { fuzzy(old) };
        let mut hits = base.match_indices(needle.as_str());
        let first = hits.next().filter(|_| !needle.is_empty());
        let Some((start, _)) = first else {
            return Err(ToolError::Invalid(if multi {
                format!(
                    "Could not find edits[{index}] in {path}. The oldText must match exactly including all whitespace and newlines."
                )
            } else {
                format!(
                    "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
                )
            }));
        };
        let count = 1 + hits.count();
        if count > 1 {
            return Err(ToolError::Invalid(if multi {
                format!(
                    "Found {count} occurrences of edits[{index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
                )
            } else {
                format!(
                    "Found {count} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
                )
            }));
        }
        matches.push(Match {
            start,
            end: start + needle.len(),
            index,
            new_text: new.as_str(),
        });
    }

    matches.sort_by_key(|found| found.start);
    for pair in matches.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(ToolError::Invalid(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                pair[0].index, pair[1].index
            )));
        }
    }

    let updated = if exact {
        splice(&base, 0, &matches)
    } else {
        splice_lines(content, &base, &matches)
    };

    if updated == content {
        return Err(ToolError::Invalid(if multi {
            format!("No changes made to {path}. The replacements produced identical content.")
        } else {
            format!(
                "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
            )
        }));
    }
    Ok(updated)
}

struct Match<'a> {
    start: usize,
    end: usize,
    index: usize,
    new_text: &'a str,
}

fn splice(text: &str, offset: usize, matches: &[Match]) -> String {
    let mut result = text.to_owned();
    for found in matches.iter().rev() {
        result.replace_range(found.start - offset..found.end - offset, found.new_text);
    }
    result
}

fn splice_lines(original: &str, base: &str, matches: &[Match]) -> String {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(base.match_indices('\n').map(|(index, _)| index + 1))
        .collect();
    let line_of = |offset: usize| line_starts.partition_point(|&start| start <= offset) - 1;
    let line_end = |line: usize| {
        line_starts
            .get(line + 1)
            .map_or(base.len(), |next| next - 1)
    };

    let mut groups: Vec<(usize, usize, Vec<&Match>)> = Vec::new();
    for found in matches {
        let first = line_of(found.start);
        let last = line_of(found.end);
        match groups.last_mut() {
            Some((_, group_last, members)) if *group_last >= first => {
                *group_last = (*group_last).max(last);
                members.push(found);
            }
            _ => groups.push((first, last, vec![found])),
        }
    }

    let mut lines: Vec<String> = original.split('\n').map(str::to_owned).collect();
    for (first, last, members) in groups.into_iter().rev() {
        let block_start = line_starts[first];
        let mut block = base[block_start..line_end(last)].to_owned();
        for found in members.iter().rev() {
            block.replace_range(
                found.start - block_start..found.end - block_start,
                found.new_text,
            );
        }
        lines.splice(first..=last, block.split('\n').map(str::to_owned));
    }
    lines.join("\n")
}

fn to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn fuzzy(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            line.trim_end()
                .chars()
                .map(|c| match c {
                    '\u{2018}'..='\u{201B}' => '\'',
                    '\u{201C}'..='\u{201F}' => '"',
                    '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
                    '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => {
                        ' '
                    }
                    other => other,
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Default)]
struct Output {
    tail: Vec<u8>,
    trimmed: bool,
    newlines: usize,
    last: Option<u8>,
}

impl Output {
    fn push(&mut self, chunk: &[u8]) {
        self.newlines += chunk.iter().filter(|&&byte| byte == b'\n').count();
        self.last = chunk.last().copied().or(self.last);
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > TAIL_TRIM_AT {
            self.tail.drain(..self.tail.len() - TAIL_KEEP_BYTES);
            self.trimmed = true;
        }
    }

    fn total_lines(&self) -> usize {
        self.newlines + usize::from(self.last.is_some_and(|byte| byte != b'\n'))
    }

    fn render(&self) -> String {
        let decoded = String::from_utf8_lossy(&self.tail);
        let mut text: &str = &decoded;
        if self.trimmed {
            text = text.find('\n').map_or(text, |index| &text[index + 1..]);
        }
        let text = text.strip_suffix('\n').unwrap_or(text);
        if text.is_empty() {
            return String::new();
        }

        let lines: Vec<&str> = text.split('\n').collect();
        let total = self.total_lines();
        let mut bytes = 0;
        let mut kept = 0;
        let mut by_bytes = false;
        for line in lines.iter().rev() {
            if kept == MAX_LINES {
                break;
            }
            bytes += line.len() + usize::from(kept > 0);
            if bytes > MAX_BYTES {
                by_bytes = true;
                break;
            }
            kept += 1;
        }

        if kept == lines.len() && !self.trimmed {
            return text.to_owned();
        }
        if kept == 0 {
            let last = lines[lines.len() - 1];
            let mut cut = last.len() - MAX_BYTES;
            while !last.is_char_boundary(cut) {
                cut += 1;
            }
            let shown = &last[cut..];
            return format!(
                "{shown}\n\n[Showing last {} of line {total} (line is {}).]",
                format_size(shown.len()),
                format_size(last.len()),
            );
        }
        let limit_note = if by_bytes {
            format!(" ({} limit)", format_size(MAX_BYTES))
        } else {
            String::new()
        };
        format!(
            "{}\n\n[Showing lines {}-{total} of {total}{limit_note}.]",
            lines[lines.len() - kept..].join("\n"),
            total - kept + 1,
        )
    }
}

fn head(lines: &[&str]) -> (usize, bool) {
    let mut bytes = 0;
    for (index, line) in lines.iter().enumerate() {
        if index == MAX_LINES {
            return (index, false);
        }
        bytes += line.len() + usize::from(index > 0);
        if bytes > MAX_BYTES {
            return (index, true);
        }
    }
    (lines.len(), false)
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn io_error(error: std::io::Error) -> ToolError {
    ToolError::Io(error.to_string())
}

#[async_trait::async_trait]
impl Environment for LocalEnvironment {
    async fn managed_skills(&self) -> Result<BTreeMap<String, (String, bool)>, ToolError> {
        Ok(self
            .read_manifest()
            .await?
            .into_iter()
            .map(|(name, hash)| {
                let installed = self.skills_dir.join(&name).join(SKILL_FILE).is_file();
                (name, (hash, installed))
            })
            .collect())
    }

    async fn apply_skills(
        &self,
        put: Vec<SkillBundle>,
        remove: Vec<String>,
    ) -> Result<Vec<String>, ToolError> {
        for name in &remove {
            validate_name(name)?;
        }
        let mut manifest = self.read_manifest().await?;
        tokio::fs::create_dir_all(&self.skills_dir)
            .await
            .map_err(io_error)?;
        let mut skipped = Vec::new();
        for bundle in put {
            validate_name(&bundle.name)?;
            for file in &bundle.files {
                let path = Path::new(&file.path);
                if file.path.is_empty()
                    || path.is_absolute()
                    || path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                    || !path
                        .components()
                        .any(|c| matches!(c, std::path::Component::Normal(_)))
                {
                    return Err(ToolError::Invalid(format!(
                        "Invalid skill file path: {}",
                        file.path
                    )));
                }
            }
            let dest = self.skills_dir.join(&bundle.name);
            if dest.symlink_metadata().is_ok() && !manifest.contains_key(&bundle.name) {
                skipped.push(bundle.name);
                continue;
            }
            let temp = temporary_path(&self.skills_dir);
            tokio::fs::create_dir(&temp).await.map_err(io_error)?;
            for file in bundle.files {
                let path = temp.join(file.path);
                tokio::fs::create_dir_all(path.parent().unwrap())
                    .await
                    .map_err(io_error)?;
                tokio::fs::write(&path, file.content)
                    .await
                    .map_err(io_error)?;
                tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(file.mode))
                    .await
                    .map_err(io_error)?;
            }
            if dest.symlink_metadata().is_ok() {
                tokio::fs::remove_dir_all(&dest).await.map_err(io_error)?;
            }
            tokio::fs::rename(&temp, dest).await.map_err(io_error)?;
            manifest.insert(bundle.name, bundle.hash);
            self.write_manifest(&manifest).await?;
        }
        for name in remove {
            if manifest.remove(&name).is_some() {
                match tokio::fs::remove_dir_all(self.skills_dir.join(name)).await {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(io_error(e)),
                }
                self.write_manifest(&manifest).await?;
            }
        }
        Ok(skipped)
    }

    async fn skill_catalog(&self) -> Result<Vec<SkillEntry>, ToolError> {
        let mut entries = match tokio::fs::read_dir(&self.skills_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error(e)),
        };
        let mut skills = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(io_error)? {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = self.skills_dir.join(entry.file_name()).join(SKILL_FILE);
            let Ok(content) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            let Some(frontmatter) = parse_frontmatter(&content) else {
                continue;
            };
            let location = path;
            skills.push(SkillEntry {
                name: frontmatter.name,
                description: frontmatter.description,
                location,
            });
        }
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(skills)
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolResult, ToolError> {
        let content = match &call.input {
            ToolInput::Read {
                path,
                offset,
                limit,
            } => self.read(path, *offset, *limit).await?,
            ToolInput::Write { path, content } => self.write(path, content).await?,
            ToolInput::Edit { path, edits } => self.edit(path, edits).await?,
            ToolInput::Bash { command, timeout } => self.bash(command, *timeout).await?,
        };

        Ok(ToolResult {
            id: call.id,
            content,
        })
    }
}

fn validate_name(name: &str) -> Result<(), ToolError> {
    if !valid_skill_name(name) {
        return Err(ToolError::Invalid(format!("Invalid skill name: {name}")));
    }
    Ok(())
}

fn temporary_path(root: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    root.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
