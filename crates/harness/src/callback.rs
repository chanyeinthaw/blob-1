use std::borrow::Cow;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use environment::ToolSpec;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use storage::{CallbackStore, ConfigStore};

type Error = Box<dyn std::error::Error + Send + Sync>;

const MAX_NAME_LEN: usize = 100;
const DEFAULT_TTL_MINUTES: u32 = 1440;
const MIN_TTL_MINUTES: u32 = 1;
const MAX_TTL_MINUTES: u32 = 10_080;
const DEFAULT_MAX_USES: u32 = 1;
const MIN_MAX_USES: u32 = 1;
const MAX_MAX_USES: u32 = 20;

pub const TOOL: ToolSpec = ToolSpec {
    name: Cow::Borrowed("callback"),
    description: Cow::Borrowed(
        "Create a temporary inbound callback webhook URL that wakes the main agent when an external async job or agent HTTP POSTs a result to it. Default is one-shot (max_uses: 1) and expires after 24 hours (ttl_minutes: 1440, max 10080 / 7 days). Pass the originating reply/destination token (e.g. m12 or d5) in for when known so the main agent knows where to report back. Returns the callback URL.",
    ),
    input_schema: Cow::Borrowed(
        r#"{"type":"object","properties":{"name":{"type":"string","description":"Short label for the callback (e.g. 'pr-build' or 'research-thread')"},"for":{"type":"string","description":"Optional originating reply or destination token (m… or d…)"},"ttl_minutes":{"type":"integer","minimum":1,"maximum":10080,"description":"Expiry in minutes (default 1440 = 24h, max 10080 = 7d)"},"max_uses":{"type":"integer","minimum":1,"maximum":20,"description":"Maximum POST hits before expiry (default 1 = one-shot, max 20)"}},"required":["name"]}"#,
    ),
};

#[derive(Deserialize)]
struct CallbackArgs {
    name: String,
    #[serde(rename = "for")]
    for_token: Option<String>,
    ttl_minutes: Option<u32>,
    max_uses: Option<u32>,
}

/// Shared handle for the `callback` tool used by background tasks.
#[derive(Clone)]
pub struct CallbackTools {
    callbacks: Arc<dyn CallbackStore>,
    config: Arc<dyn ConfigStore>,
}

impl CallbackTools {
    pub fn new(callbacks: Arc<dyn CallbackStore>, config: Arc<dyn ConfigStore>) -> Self {
        Self { callbacks, config }
    }

    pub fn execute(&self, arguments: &str) -> Result<String, Error> {
        let args: CallbackArgs = serde_json::from_str(arguments)?;
        let Some(base_url) = self.config.public_url()? else {
            return Err(
                "public URL is not configured; start agent serve with --public-url to create callback URLs"
                    .into(),
            );
        };
        let name = args.name.trim();
        if name.is_empty() {
            return Err("name is required".into());
        }
        if name.chars().count() > MAX_NAME_LEN {
            return Err(format!("name must be at most {MAX_NAME_LEN} characters").into());
        }
        let ttl_minutes = args.ttl_minutes.unwrap_or(DEFAULT_TTL_MINUTES);
        if !(MIN_TTL_MINUTES..=MAX_TTL_MINUTES).contains(&ttl_minutes) {
            return Err(
                format!("ttl_minutes must be between {MIN_TTL_MINUTES} and {MAX_TTL_MINUTES}")
                    .into(),
            );
        }
        let max_uses = args.max_uses.unwrap_or(DEFAULT_MAX_USES);
        if !(MIN_MAX_USES..=MAX_MAX_USES).contains(&max_uses) {
            return Err(
                format!("max_uses must be between {MIN_MAX_USES} and {MAX_MAX_USES}").into(),
            );
        }
        let for_token = match args.for_token.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(token) => {
                if !valid_for_token(token) {
                    return Err("for token must look like m<digits> or d<digits>".into());
                }
                Some(token)
            }
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_secs() as i64;
        let expires_at = now.saturating_add(i64::from(ttl_minutes) * 60);
        let id = nanoid::nanoid!(16);
        let token = format!("cbk_{}", nanoid::nanoid!(43));
        let token_hash = Sha256::digest(token.as_bytes());
        self.callbacks.create_callback(
            &id,
            name,
            &token_hash,
            for_token,
            max_uses,
            expires_at,
            now,
        )?;
        let url = format!("{}/hooks/cb/{token}", base_url.trim_end_matches('/'));
        Ok(format!(
            "url: {url}\nmax_uses: {max_uses}\nexpires_in_minutes: {ttl_minutes}"
        ))
    }
}

fn valid_for_token(token: &str) -> bool {
    if token.len() < 2 || token.len() > 32 {
        return false;
    }
    let bytes = token.as_bytes();
    matches!(bytes[0], b'm' | b'd') && bytes[1..].iter().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_for_token_format() {
        assert!(valid_for_token("m1"));
        assert!(valid_for_token("m12345"));
        assert!(valid_for_token("d42"));
        assert!(!valid_for_token(""));
        assert!(!valid_for_token("m"));
        assert!(!valid_for_token("x12"));
        assert!(!valid_for_token("m12\""));
        assert!(!valid_for_token("m12 3"));
    }
}
