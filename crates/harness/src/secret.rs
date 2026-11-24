use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use environment::{Environment, ToolSpec};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use storage::{ConfigStore, SecretStore};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// How long a request link stays open.
pub const REQUEST_TTL_SECS: i64 = 600;
const MAX_LABEL_LEN: usize = 200;
const DEFAULT_FILE_TTL_MINUTES: u64 = 15;
const MAX_FILE_TTL_MINUTES: u64 = 60;

pub const REQUEST: ToolSpec = ToolSpec {
    name: Cow::Borrowed("request_secret"),
    description: Cow::Borrowed(
        "Create a one-time link, valid 10 minutes, where the user enters a secret (an API key, token or password) instead of sending it in chat. Give the link to the user. When they submit it, an event tells you the secret is stored; a task can then read it with the secret tool. Submitting under an existing name replaces it. Returns the link.",
    ),
    input_schema: Cow::Borrowed(
        r#"{"type":"object","properties":{"name":{"type":"string","pattern":"^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}$","description":"Name to store the secret under (e.g. github_token)"},"label":{"type":"string","description":"What to ask for, shown on the form"}},"required":["name"]}"#,
    ),
};

pub const READ: ToolSpec = ToolSpec {
    name: Cow::Borrowed("secret"),
    description: Cow::Borrowed(
        "Put a stored secret in a private file and get the file's path; the value never enters this conversation. Use it from the file (e.g. `gh auth login --with-token < PATH`), and never print, echo or cat it. The file is deleted after ttl_minutes (default 15, max 60). An unknown name lists the stored names; ask the user for a missing one with request_secret.",
    ),
    input_schema: Cow::Borrowed(
        r#"{"type":"object","properties":{"name":{"type":"string","description":"The secret's name"},"ttl_minutes":{"type":"integer","minimum":1,"maximum":60,"description":"Minutes until the file is deleted (default 15)"}},"required":["name"]}"#,
    ),
};

#[derive(Deserialize)]
struct RequestArgs {
    name: String,
    label: Option<String>,
}

#[derive(Deserialize)]
struct ReadArgs {
    name: String,
    ttl_minutes: Option<u64>,
}

/// The per-bot `request_secret` (chat and tasks) and `secret` (tasks) tools.
#[derive(Clone)]
pub struct SecretTools {
    secrets: Arc<dyn SecretStore>,
    config: Arc<dyn ConfigStore>,
}

impl SecretTools {
    pub fn new(secrets: Arc<dyn SecretStore>, config: Arc<dyn ConfigStore>) -> Self {
        Self { secrets, config }
    }

    /// `request_secret`: opens a one-time request and returns its link.
    pub fn request(&self, arguments: &str) -> Result<String, Error> {
        let args: RequestArgs = serde_json::from_str(arguments)?;
        let label = args
            .label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty());
        let link = create_request(
            self.secrets.as_ref(),
            self.config.as_ref(),
            args.name.trim(),
            label,
        )?;
        Ok(format!(
            "url: {}\nexpires_in_minutes: {}",
            link.url,
            REQUEST_TTL_SECS / 60
        ))
    }

    /// `secret`: writes the value to a file in `environment` and returns its path.
    pub async fn read<E: Environment + ?Sized>(
        &self,
        environment: &E,
        arguments: &str,
    ) -> Result<String, Error> {
        let args: ReadArgs = serde_json::from_str(arguments)?;
        let name = args.name.trim();
        let minutes = args.ttl_minutes.unwrap_or(DEFAULT_FILE_TTL_MINUTES);
        if !(1..=MAX_FILE_TTL_MINUTES).contains(&minutes) {
            return Err(format!("ttl_minutes must be between 1 and {MAX_FILE_TTL_MINUTES}").into());
        }
        let Some(value) = self.secrets.secret_value(name)? else {
            let names: Vec<String> = self
                .secrets
                .secrets()?
                .into_iter()
                .map(|secret| secret.name)
                .collect();
            return Err(if names.is_empty() {
                format!("no secret named '{name}', and none are stored").into()
            } else {
                format!("no secret named '{name}'; stored: {}", names.join(", ")).into()
            });
        };
        let path = environment
            .write_secret(name, &value, Duration::from_secs(minutes * 60))
            .await?;
        Ok(format!("path: {path}\ndeleted_in_minutes: {minutes}"))
    }
}

/// A request link and when it stops working (Unix seconds).
pub struct RequestLink {
    pub url: String,
    pub expires_at: i64,
}

/// Opens a one-time secret request: shared by the `request_secret` tool, the
/// web page and the CLI. Needs the public URL, since the user opens the link.
pub fn create_request(
    secrets: &dyn SecretStore,
    config: &dyn ConfigStore,
    name: &str,
    label: Option<&str>,
) -> Result<RequestLink, Error> {
    let Some(base_url) = config.public_url()? else {
        return Err(
            "public URL is not configured; start agent serve with --public-url to request secrets"
                .into(),
        );
    };
    if !storage::valid_secret_name(name) {
        return Err(
            "name must be 1-64 of letters, digits, '_', '.' or '-', not starting with '.'".into(),
        );
    }
    if label.is_some_and(|label| label.chars().count() > MAX_LABEL_LEN) {
        return Err(format!("label must be at most {MAX_LABEL_LEN} characters").into());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs() as i64;
    let expires_at = now + REQUEST_TTL_SECS;
    let token = format!("sec_{}", nanoid::nanoid!(43));
    let token_hash = Sha256::digest(token.as_bytes());
    secrets.create_secret_request(&token_hash, name, label, expires_at, now)?;
    Ok(RequestLink {
        url: format!("{}/secret/{token}", base_url.trim_end_matches('/')),
        expires_at,
    })
}
