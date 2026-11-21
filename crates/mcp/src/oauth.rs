use std::sync::Arc;

use rmcp::transport::{
    AuthError, AuthorizationManager, CredentialStore, StateStore, StoredAuthorizationState,
    StoredCredentials,
};
use storage::{McpStore, McpTransport};

use crate::{Mcp, Registration};

/// OAuth credentials persisted in `mcp_tokens.credentials`.
pub struct Credentials {
    pub store: Arc<dyn McpStore>,
    pub server: String,
}

fn store_error(error: impl std::fmt::Display) -> AuthError {
    AuthError::InternalError(error.to_string())
}

#[async_trait::async_trait]
impl CredentialStore for Credentials {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        Ok(self
            .store
            .mcp_oauth(&self.server)
            .map_err(store_error)?
            .0
            .and_then(|value| serde_json::from_value(value).ok()))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        self.store
            .save_mcp_credentials(
                &self.server,
                Some(&serde_json::to_value(credentials).unwrap()),
            )
            .map_err(store_error)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.store
            .save_mcp_credentials(&self.server, None)
            .map_err(store_error)
    }
}

/// In-flight OAuth state persisted in `mcp_oauth_states` so the callback can
/// complete even after a restart.
pub struct States {
    pub store: Arc<dyn McpStore>,
    pub server: String,
}

#[async_trait::async_trait]
impl StateStore for States {
    async fn save(
        &self,
        csrf_token: &str,
        state: StoredAuthorizationState,
    ) -> Result<(), AuthError> {
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        self.store
            .save_mcp_oauth_state(
                &self.server,
                csrf_token,
                &serde_json::to_value(state).unwrap(),
                created,
            )
            .map_err(store_error)
    }

    async fn load(&self, csrf_token: &str) -> Result<Option<StoredAuthorizationState>, AuthError> {
        Ok(self
            .store
            .mcp_oauth_state(csrf_token)
            .map_err(store_error)?
            .filter(|(server, _)| server == &self.server)
            .and_then(|(_, state)| serde_json::from_value(state).ok()))
    }

    async fn delete(&self, csrf_token: &str) -> Result<(), AuthError> {
        self.store
            .delete_mcp_oauth_state(csrf_token)
            .map_err(store_error)
    }
}

impl Mcp {
    /// URL of an HTTP server after config sync; errors on missing/non-HTTP.
    async fn http_url(&self, server: &str) -> Result<(String, Option<String>), String> {
        self.sync().await?;
        let entry = {
            self.inner
                .servers
                .lock()
                .await
                .get(server)
                .cloned()
                .ok_or_else(|| format!("no MCP server '{server}'"))?
        };
        match &entry.config.transport {
            McpTransport::Http { url, .. } => {
                Ok((url.clone(), entry.state.lock().await.challenge.clone()))
            }
            McpTransport::Stdio { .. } => {
                Err(format!("MCP server '{server}' is not an HTTP server"))
            }
        }
    }

    fn manager(&self, server: &str) -> ManagerBuilder<'_> {
        ManagerBuilder {
            mcp: self,
            server: server.to_string(),
        }
    }

    /// Begin the OAuth flow for an HTTP server: discover metadata, register
    /// the client dynamically, and return the authorization URL to open.
    /// `redirect_uri` is the `/api/mcp/callback` address built from the
    /// request host. The registration and the challenge that seeded
    /// discovery are kept with the pending state so each callback rebuilds
    /// the exact client its login round created.
    pub async fn begin_login(&self, server: &str, redirect_uri: &str) -> Result<String, String> {
        let (url, challenge) = self.http_url(server).await?;
        let challenge = match challenge {
            Some(challenge) => Some(challenge),
            None => self.challenge_via_initialize(&url).await,
        };
        let mut manager = self.manager(server).build(&url).await?;
        let resolution = manager
            .resolve_metadata_from_challenge(challenge.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        manager.set_metadata(resolution.metadata);
        let scopes = manager.select_initial_scopes(&[]).await;
        let scopes: Vec<&str> = scopes.iter().map(String::as_str).collect();
        let config = manager
            .register_client("Blob-1", redirect_uri, &scopes)
            .await
            .map_err(|error| error.to_string())?;
        manager
            .configure_client(config.clone())
            .map_err(|error| error.to_string())?;
        let auth_url = manager
            .get_authorization_url(&scopes)
            .await
            .map_err(|error| error.to_string())?;
        let csrf = url::Url::parse(&auth_url).ok().and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "state")
                .map(|(_, value)| value.into_owned())
        });
        if let Some(csrf) = csrf {
            if let Ok(Some((_, mut state))) = self.inner.store.mcp_oauth_state(&csrf) {
                if let serde_json::Value::Object(map) = &mut state {
                    let mut registration = Registration::from(config);
                    registration.challenge = challenge;
                    map.insert(
                        "registration".to_string(),
                        serde_json::to_value(registration).unwrap(),
                    );
                }
                let created = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or_default();
                let _ = self
                    .inner
                    .store
                    .save_mcp_oauth_state(server, &csrf, &state, created);
            }
        }
        Ok(auth_url)
    }

    async fn challenge_via_initialize(&self, url: &str) -> Option<String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        let request = client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 0,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "clientInfo": { "name": "blob-1", "version": env!("CARGO_PKG_VERSION") }
                    }
                })
                .to_string(),
            )
            .send();
        let response = tokio::time::timeout(std::time::Duration::from_secs(15), request)
            .await
            .ok()?
            .ok()?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return None;
        }
        response
            .headers()
            .get_all(reqwest::header::WWW_AUTHENTICATE)
            .iter()
            .find_map(|value| value.to_str().ok().map(str::to_string))
    }

    /// Finish the OAuth flow from `/api/mcp/callback`; returns the server id.
    /// The saved CSRF state identifies which server this callback belongs to
    /// and carries the registration and discovery inputs its login used.
    pub async fn complete_login(
        &self,
        code: &str,
        csrf: &str,
        issuer: Option<&str>,
    ) -> Result<String, String> {
        let (server, state) = self
            .inner
            .store
            .mcp_oauth_state(csrf)
            .map_err(|error| error.to_string())?
            .ok_or("unknown or expired OAuth state; start login again")?;
        let registration = state
            .get("registration")
            .cloned()
            .and_then(|value| serde_json::from_value::<Registration>(value).ok())
            .ok_or("OAuth registration was not saved; start login again")?;
        let challenge = registration.challenge.clone();
        let (url, _) = self.http_url(&server).await?;
        let mut manager = self.manager(&server).build(&url).await?;
        let resolution = manager
            .resolve_metadata_from_challenge(challenge.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        manager.set_metadata(resolution.metadata);
        manager
            .configure_client(registration.clone().into())
            .map_err(|error| error.to_string())?;
        manager
            .exchange_code_for_token_with_issuer(code, csrf, issuer)
            .await
            .map_err(|error| error.to_string())?;
        self.inner
            .store
            .save_mcp_registration(&server, &serde_json::to_value(registration).unwrap())
            .map_err(|error| error.to_string())?;
        let entry = self.inner.servers.lock().await.get(&server).cloned();
        if let Some(entry) = entry {
            let mut state = entry.state.lock().await;
            if let Some((_, conn)) = state.conn.take() {
                conn.service.cancellation_token().cancel();
            }
            state.needs_login = false;
            state.challenge = None;
            state.error = None;
            state.last_failure = None;
        }
        Ok(server)
    }
}

struct ManagerBuilder<'a> {
    mcp: &'a Mcp,
    server: String,
}

impl ManagerBuilder<'_> {
    async fn build(self, url: &str) -> Result<AuthorizationManager, String> {
        let mut manager = AuthorizationManager::new_allowing_private_addresses(url)
            .await
            .map_err(|error| error.to_string())?;
        manager.set_credential_store(Credentials {
            store: self.mcp.inner.store.clone(),
            server: self.server.clone(),
        });
        manager.set_state_store(States {
            store: self.mcp.inner.store.clone(),
            server: self.server,
        });
        Ok(manager)
    }
}
