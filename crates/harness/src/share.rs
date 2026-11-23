use std::borrow::Cow;
use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use environment::ToolSpec;
use serde::Deserialize;
use storage::{ConfigStore, PublicFile, PublicFileStore};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Maximum file size served or published via `share` and `/p/{id}` (25 MiB).
pub const MAX_PUBLIC_FILE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_NAMED_URL_CHARS: usize = 160;
const MAX_ENCODED_NAME_LEN: usize = 80;

pub const TOOL: ToolSpec = ToolSpec {
    name: Cow::Borrowed("share"),
    description: Cow::Borrowed(
        "Create a public link to a file in the workspace so users can read long replies, reports, Markdown, HTML, or other generated files without hitting chat length limits. Give either path (an existing file inside the workspace) or content (written under shares/ as Markdown by default unless filename specifies another extension). Returns the public URL.",
    ),
    input_schema: Cow::Borrowed(
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Relative path to an existing file inside the workspace"},"content":{"type":"string","description":"File content to write into workspace/shares/ and publish"},"filename":{"type":"string","description":"Optional filename when passing content (defaults to .md when no extension is given, e.g. report.md or chart.html)"}}}"#,
    ),
};

#[derive(Deserialize)]
struct ShareArgs {
    path: Option<String>,
    content: Option<String>,
    filename: Option<String>,
}

/// A workspace root directory pinned once at startup via an open directory
/// descriptor plus its canonical path string. All `share` and `/p/{id}` file
/// operations anchor on this descriptor so root/ancestor swaps cannot redirect
/// opens outside the workspace.
#[derive(Clone)]
pub struct PinnedWorkspace {
    dir: Arc<OwnedFd>,
    canonical_path: Arc<str>,
}

impl PinnedWorkspace {
    pub fn open(workspace: &Path) -> std::io::Result<Self> {
        let canon = std::fs::canonicalize(workspace)?;
        let canonical_path = encode_workspace_id(&canon);
        let dir = open_dir_fd(&canon)?;
        Ok(Self {
            dir: Arc::new(dir),
            canonical_path: Arc::from(canonical_path),
        })
    }

    pub fn canonical_path(&self) -> &str {
        &self.canonical_path
    }

    /// Opens a regular file relative to this pinned workspace without following
    /// any symlinks or blocking on FIFOs (`O_NOFOLLOW | O_NONBLOCK`).
    pub fn open_file_nofollow(&self, rel_path: &str) -> std::io::Result<File> {
        let segments = parse_relative_segments(rel_path).map_err(|err| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string())
        })?;
        let (file_name, parents) = segments.split_last().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty path")
        })?;
        let mut owned_parent: Option<OwnedFd> = None;
        for segment in parents {
            let parent_fd = owned_parent.as_ref().unwrap_or(&self.dir);
            owned_parent = Some(openat_fd(
                parent_fd,
                segment,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
            )?);
        }
        let final_dir = owned_parent.as_ref().unwrap_or(&self.dir);
        let file_fd = openat_fd(
            final_dir,
            file_name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0,
        )?;
        let file = File::from(file_fd);
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(file)
    }

    /// Creates a new file under `shares/<stored_name>` anchored on this pinned
    /// workspace descriptor with `O_CREAT | O_EXCL | O_NOFOLLOW`.
    pub fn create_shared_file(&self, stored_name: &str, bytes: &[u8]) -> std::io::Result<()> {
        let c_shares = CString::new("shares").expect("static cstr");
        let rc = unsafe { libc::mkdirat(self.dir.as_raw_fd(), c_shares.as_ptr(), 0o755) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(err);
            }
        }
        let shares_fd = openat_fd(
            &self.dir,
            "shares",
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )?;
        let file_fd = openat_fd(
            &shares_fd,
            stored_name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o644,
        )?;
        let mut file = File::from(file_fd);
        file.write_all(bytes)?;
        file.flush()?;
        Ok(())
    }
}

/// Shared handle for the `share` tool used by background tasks.
#[derive(Clone)]
pub struct ShareTools {
    files: Arc<dyn PublicFileStore>,
    config: Arc<dyn ConfigStore>,
    workspace: PinnedWorkspace,
}

impl ShareTools {
    pub fn new(
        files: Arc<dyn PublicFileStore>,
        config: Arc<dyn ConfigStore>,
        workspace: PinnedWorkspace,
    ) -> Self {
        Self {
            files,
            config,
            workspace,
        }
    }

    pub fn execute(&self, arguments: &str) -> Result<String, Error> {
        let args: ShareArgs = serde_json::from_str(arguments)?;
        let Some(base_url) = self.config.public_url()? else {
            return Err(
                "public URL is not configured; start agent serve with --public-url to share files"
                    .into(),
            );
        };
        let base_url = base_url.trim_end_matches('/');
        let id = nanoid::nanoid!(22);

        let (rel_path, display_name) = match (args.path, args.content) {
            (Some(raw_path), None) => {
                if args.filename.is_some() {
                    return Err("filename can only be used with content, not path".into());
                }
                let segments = parse_relative_segments(&raw_path)?;
                let file = self
                    .workspace
                    .open_file_nofollow(&raw_path)
                    .map_err(|err| format!("cannot share '{raw_path}': {err}"))?;
                let meta = file.metadata()?;
                if !meta.is_file() {
                    return Err("path must be a regular file".into());
                }
                if meta.len() > MAX_PUBLIC_FILE_BYTES {
                    return Err(format!(
                        "file exceeds {} MiB limit",
                        MAX_PUBLIC_FILE_BYTES / (1024 * 1024)
                    )
                    .into());
                }
                let display = segments
                    .last()
                    .ok_or("path must name a file")?
                    .to_owned();
                (segments.join("/"), display)
            }
            (None, Some(content)) => {
                if content.len() as u64 > MAX_PUBLIC_FILE_BYTES {
                    return Err(format!(
                        "content exceeds {} MiB limit",
                        MAX_PUBLIC_FILE_BYTES / (1024 * 1024)
                    )
                    .into());
                }
                let safe_name = normalize_filename(args.filename.as_deref());
                let stored_name = format!("{id}-{safe_name}");
                self.workspace
                    .create_shared_file(&stored_name, content.as_bytes())?;
                (format!("shares/{stored_name}"), safe_name)
            }
            (Some(_), Some(_)) => {
                return Err("provide either path or content, not both".into());
            }
            (None, None) => {
                return Err("provide either path (existing file) or content".into());
            }
        };

        let content_type = content_type_for_path(Path::new(&rel_path)).to_owned();
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_secs() as i64;
        self.files.create_public_file(&PublicFile {
            id: id.clone(),
            workspace: self.workspace.canonical_path().to_owned(),
            path: rel_path,
            content_type,
            created_at,
        })?;
        Ok(format_public_url(base_url, &id, &display_name))
    }
}

fn format_public_url(base_url: &str, id: &str, display_name: &str) -> String {
    let short = format!("{base_url}/p/{id}");
    let encoded = percent_encode_segment(display_name);
    if encoded.is_empty() || encoded.len() > MAX_ENCODED_NAME_LEN {
        return short;
    }
    let named = format!("{short}/{encoded}");
    if named.chars().count() <= MAX_NAMED_URL_CHARS {
        named
    } else {
        short
    }
}

fn parse_relative_segments(raw: &str) -> Result<Vec<String>, Error> {
    if raw.is_empty() {
        return Err("path must not be empty".into());
    }
    if raw == "." || raw.ends_with('/') || raw.ends_with("/.") {
        return Err("path must name a file, not a directory".into());
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return Err("path must be relative to the workspace".into());
    }
    let mut segments = Vec::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                let s = part.to_str().ok_or("path must be valid UTF-8")?;
                if s.contains('\0') {
                    return Err("path must not contain null bytes".into());
                }
                segments.push(s.to_owned());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("path must stay inside the workspace".into());
            }
        }
    }
    if segments.is_empty() {
        return Err("path must name a file".into());
    }
    Ok(segments)
}

fn encode_workspace_id(path: &Path) -> String {
    match path.to_str() {
        Some(utf8) => format!("utf8:{utf8}"),
        None => {
            let bytes = path.as_os_str().as_bytes();
            let mut out = String::with_capacity(4 + bytes.len() * 2);
            out.push_str("hex:");
            for b in bytes {
                out.push_str(&format!("{b:02x}"));
            }
            out
        }
    }
}

fn open_dir_fd(path: &Path) -> std::io::Result<OwnedFd> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn openat_fd(
    dir: &OwnedFd,
    name: &str,
    flags: libc::c_int,
    mode: libc::c_uint,
) -> std::io::Result<OwnedFd> {
    let c_name = CString::new(name)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid name"))?;
    let fd = unsafe { libc::openat(dir.as_raw_fd(), c_name.as_ptr(), flags, mode) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn normalize_filename(raw: Option<&str>) -> String {
    let base = raw
        .and_then(|s| Path::new(s.trim()).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let (raw_stem, raw_ext) = match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, Some(ext)),
        _ => (base, None),
    };
    let ext: String = raw_ext
        .map(|e| {
            e.chars()
                .filter(char::is_ascii_alphanumeric)
                .take(12)
                .map(|c| c.to_ascii_lowercase())
                .collect()
        })
        .filter(|s: &String| !s.is_empty())
        .unwrap_or_else(|| "md".into());
    let cleaned_stem: String = raw_stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed_stem = cleaned_stem.trim_matches(['.', '-']);
    let stem: String = if trimmed_stem.is_empty() {
        "note".into()
    } else {
        let capped: String = trimmed_stem.chars().take(60).collect();
        let capped = capped.trim_end_matches(['.', '-']);
        if capped.is_empty() {
            "note".into()
        } else {
            capped.to_owned()
        }
    };
    format!("{stem}.{ext}")
}

fn percent_encode_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Maps a file extension to its served `Content-Type`. Markdown defaults to
/// `text/plain; charset=utf-8` so browsers display raw Markdown inline instead
/// of forcing a file download.
pub fn content_type_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some(
            "md" | "markdown" | "txt" | "log" | "toml" | "yaml" | "yml" | "rs" | "ts" | "tsx"
            | "py" | "sh" | "sql" | "ini" | "conf",
        ) => "text/plain; charset=utf-8",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("csv") => "text/csv; charset=utf-8",
        Some("xml") => "application/xml; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_filenames_preserving_extension_and_bounds_url_length() {
        assert_eq!(normalize_filename(None), "note.md");
        assert_eq!(normalize_filename(Some("report")), "report.md");
        assert_eq!(normalize_filename(Some("dir/chart.html")), "chart.html");
        assert_eq!(normalize_filename(Some("../..")), "note.md");
        assert_eq!(normalize_filename(Some("日本語.html")), "note.html");
        let long_html = format!("{}.html", "a".repeat(100));
        assert!(normalize_filename(Some(&long_html)).ends_with(".html"));

        let jp_name = format!("{}.md", "あ".repeat(33));
        let url = format_public_url(
            "https://blob-1.p.si14.space",
            "1234567890123456789012",
            &jp_name,
        );
        assert_eq!(url, "https://blob-1.p.si14.space/p/1234567890123456789012");
        assert!(url.chars().count() < 160);
    }

    #[test]
    fn pinned_workspace_blocks_symlinks_fifos_and_trailing_slashes() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!("blob-1-share-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("ok.md"), "hi").unwrap();
        std::fs::write(dir.join("outside.txt"), "secret").unwrap();
        symlink(dir.join("outside.txt"), ws.join("link.txt")).unwrap();
        symlink(&dir, ws.join("linked_dir")).unwrap();

        let fifo_path = ws.join("pipe");
        let c_fifo = CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o644) }, 0);

        let pinned = PinnedWorkspace::open(&ws).unwrap();

        assert!(pinned.open_file_nofollow("ok.md").is_ok());
        // Trailing space and terminal directory markers are rejected.
        assert!(pinned.open_file_nofollow("ok.md ").is_err());
        assert!(pinned.open_file_nofollow("ok.md/").is_err());
        assert!(pinned.open_file_nofollow("ok.md/.").is_err());
        // Symlinks, escapes, and FIFOs fail immediately without blocking.
        assert!(pinned.open_file_nofollow("link.txt").is_err());
        assert!(pinned.open_file_nofollow("linked_dir/outside.txt").is_err());
        assert!(pinned.open_file_nofollow("pipe").is_err());
        assert!(pinned.open_file_nofollow("../outside.txt").is_err());
        assert!(pinned.open_file_nofollow("/etc/passwd").is_err());
        assert!(pinned.open_file_nofollow("").is_err());

        // Symlinked shares/ directory is rejected on inline create.
        symlink(&dir, ws.join("shares")).unwrap();
        assert!(pinned.create_shared_file("id-note.md", b"hello").is_err());

        // Non-UTF-8 Unix paths encode losslessly without failing startup.
        use std::ffi::OsStr;
        let non_utf8 = Path::new(OsStr::from_bytes(b"/tmp/\xff/ws"));
        let encoded_non_utf8 = encode_workspace_id(non_utf8);
        assert!(encoded_non_utf8.starts_with("hex:"));
        assert_ne!(encoded_non_utf8, encode_workspace_id(Path::new("/tmp/ws")));
        #[cfg(target_os = "linux")]
        {
            let non_utf8_dir = dir.join(OsStr::from_bytes(b"ws-\xff"));
            std::fs::create_dir_all(&non_utf8_dir).unwrap();
            let pinned_non_utf8 = PinnedWorkspace::open(&non_utf8_dir).unwrap();
            let expected = encode_workspace_id(&std::fs::canonicalize(&non_utf8_dir).unwrap());
            assert_eq!(pinned_non_utf8.canonical_path(), expected);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
