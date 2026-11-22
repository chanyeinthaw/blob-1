use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
};
use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use storage::{
    ConfigStore, McpServer, McpStore, McpTransport, MessengerStore, ReasoningLevel, WebAuthStore,
};
use storage_sqlite::SqliteStorage;

const LIFETIME: i64 = 30 * 86400;
const COOKIE: &str = "blob_session";
struct WebContext {
    store: Arc<SqliteStorage>,
    discord: Arc<messenger_discord::Discord>,
    settings_lock: tokio::sync::Mutex<()>,
    mcp: mcp::Mcp,
    public_url: Option<url::Url>,
}
impl WebContext {
    fn secure_cookie(&self) -> bool {
        self.public_url
            .as_ref()
            .is_some_and(|url| url.scheme() == "https")
    }
}
impl std::ops::Deref for WebContext {
    type Target = SqliteStorage;
    fn deref(&self) -> &SqliteStorage {
        &self.store
    }
}
type Context = Arc<WebContext>;

#[derive(RustEmbed)]
#[folder = "../../apps/web/dist/"]
#[allow_missing = true]
struct Assets;

pub fn set_password(store: &SqliteStorage) -> crate::Result<()> {
    use std::io::{BufRead, IsTerminal};
    let password = if std::io::stdin().is_terminal() {
        let password = rpassword::prompt_password("Password: ")?;
        if password != rpassword::prompt_password("Confirm password: ")? {
            return Err("passwords do not match".into());
        }
        password
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_owned()
    };
    if password.is_empty() {
        return Err("password cannot be empty".into());
    }
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string();
    store.set_web_password(&hash)?;
    Ok(())
}

/// Validate `--public-url`: an absolute http(s) base URL the deployment is
/// reached at (e.g. a TLS-terminating proxy like Tailscale Serve). No path,
/// query or fragment.
pub fn public_url(raw: &str) -> crate::Result<url::Url> {
    let url =
        url::Url::parse(raw).map_err(|error| format!("invalid --public-url '{raw}': {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || !url.has_host() {
        return Err("--public-url must be an absolute http(s) URL".into());
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err("--public-url must not have a path, query or fragment".into());
    }
    Ok(url)
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    store: Arc<SqliteStorage>,
    discord: Arc<messenger_discord::Discord>,
    mcp: mcp::Mcp,
    shutdown: tokio::sync::watch::Receiver<bool>,
    public_url: Option<url::Url>,
) -> crate::Result<()> {
    let store = Arc::new(WebContext {
        store,
        discord,
        settings_lock: tokio::sync::Mutex::new(()),
        mcp,
        public_url,
    });
    let app = Router::new()
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/mcp/login", get(mcp_login))
        .route("/api/mcp/callback", get(mcp_callback))
        .route("/ws", get(ws))
        .fallback(assets)
        .with_state(store);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let mut shutdown = shutdown;
            let _ = shutdown.changed().await;
        })
        .await?;
    Ok(())
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs() as i64
}
fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == COOKIE).then_some(value)
        })
}
fn hash(token: &str) -> Option<Vec<u8>> {
    let bytes = hex::decode(token).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    Some(Sha256::digest(bytes).to_vec())
}
fn cookie(token: &str, secure: bool) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={LIFETIME}{}",
        if secure { "; Secure" } else { "" }
    ))
    .unwrap()
}
fn authenticated(ctx: &WebContext, headers: &HeaderMap) -> Result<Option<HeaderValue>, StatusCode> {
    let Some(token) = token(headers) else {
        return Ok(None);
    };
    let Some(hash) = hash(token) else {
        return Ok(None);
    };
    let now = now();
    ctx.store
        .authenticate_web_session(&hash, now, now + LIFETIME)
        .map(|valid| valid.then(|| cookie(token, ctx.secure_cookie())))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
async fn session(State(store): State<Context>, headers: HeaderMap) -> Response {
    match authenticated(&store, &headers) {
        Ok(Some(cookie)) => {
            (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
        }
        Ok(None) => StatusCode::UNAUTHORIZED.into_response(),
        Err(e) => e.into_response(),
    }
}
#[derive(Deserialize)]
struct Login {
    password: String,
}
async fn login(State(store): State<Context>, Json(input): Json<Login>) -> Response {
    let password = match store.web_password() {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"Set a password with `agent web password set`"})),
            )
                .into_response()
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let valid = PasswordHash::new(&password).ok().is_some_and(|h| {
        Argon2::default()
            .verify_password(input.password.as_bytes(), &h)
            .is_ok()
    });
    if !valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Wrong password"})),
        )
            .into_response();
    }
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    match store.create_web_session(&Sha256::digest(bytes), now() + LIFETIME, &password) {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"Wrong password"})),
            )
                .into_response()
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, cookie(&token, store.secure_cookie()))],
    )
        .into_response()
}
async fn logout(State(store): State<Context>, headers: HeaderMap) -> Response {
    if let Some(hash) = token(&headers).and_then(hash) {
        if store.delete_web_session(&hash).is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    (
        StatusCode::NO_CONTENT,
        [(
            header::SET_COOKIE,
            HeaderValue::from_static("blob_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"),
        )],
    )
        .into_response()
}
async fn ws(
    State(store): State<Context>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let cookie = match authenticated(&store, &headers) {
        Ok(Some(c)) => c,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(e) => return e.into_response(),
    };
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<Uri>().ok());
    if !origin_ok(origin.as_ref(), &headers, store.public_url.as_ref()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = upgrade
        .on_upgrade(move |socket| socket_loop(socket, store, headers))
        .into_response();
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    response
}
/// A bare origin (`scheme://host[:port]`) must equal the public URL's; a
/// browser omits the port when it is the scheme's default.
fn origin_ok(origin: Option<&Uri>, headers: &HeaderMap, public_url: Option<&url::Url>) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    if !(matches!(origin.path(), "" | "/") && origin.query().is_none()) {
        return false;
    }
    match public_url {
        Some(url) => {
            let port = origin.port_u16().or_else(|| match origin.scheme_str() {
                Some("http") => Some(80),
                Some("https") => Some(443),
                _ => None,
            });
            origin.scheme_str() == Some(url.scheme())
                && origin.host() == url.host_str()
                && port == url.port_or_known_default()
        }
        None => {
            let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
            matches!(origin.scheme_str(), Some("http" | "https"))
                && origin.authority().map(|a| a.as_str()) == host
        }
    }
}
/// Scheme+host for the OAuth redirect/callback URI: the `--public-url` base
/// when set, else the Host header honoring a TLS-terminating proxy via
/// X-Forwarded-Proto (e.g. Tailscale).
fn request_origin(ctx: &WebContext, headers: &HeaderMap) -> Option<String> {
    if let Some(url) = &ctx.public_url {
        return Some(url.as_str().trim_end_matches('/').to_string());
    }
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    Some(format!("{scheme}://{host}"))
}

/// `/api/mcp/login?id=<server>` begins the OAuth flow and redirects to the
/// authorization URL. Session-authenticated like the rest of the web API.
async fn mcp_login(
    State(ctx): State<Context>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    match authenticated(&ctx, &headers) {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(e) => return e.into_response(),
    }
    let Some(id) = params.get("id") else {
        return (StatusCode::BAD_REQUEST, "missing ?id=").into_response();
    };
    let Some(origin) = request_origin(&ctx, &headers) else {
        return (StatusCode::BAD_REQUEST, "missing Host header").into_response();
    };
    match ctx
        .mcp
        .begin_login(id, &format!("{origin}/api/mcp/callback"))
        .await
    {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, error).into_response(),
    }
}

/// `/api/mcp/callback` completes OAuth and lands the browser on the MCP
/// settings page. The `state` parameter identifies the server and is checked
/// against stored OAuth state.
async fn mcp_callback(
    State(ctx): State<Context>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    match authenticated(&ctx, &headers) {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(e) => return e.into_response(),
    }
    let (Some(code), Some(state)) = (params.get("code"), params.get("state")) else {
        return (StatusCode::BAD_REQUEST, "missing ?code= or ?state=").into_response();
    };
    match ctx
        .mcp
        .complete_login(code, state, params.get("iss").map(String::as_str))
        .await
    {
        Ok(_) => Redirect::to("/settings/mcp").into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, error).into_response(),
    }
}

async fn mcp_reply(mcp: &mcp::Mcp) -> crate::Result<Value> {
    Ok(serde_json::to_value(mcp.server_statuses().await)?)
}

fn str_map(payload: &Value, key: &str) -> BTreeMap<String, String> {
    payload
        .get(key)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn identity_reply(store: &SqliteStorage) -> crate::Result<Value> {
    let identity = store.identity()?;
    Ok(
        json!({"text": identity.as_deref().unwrap_or(harness::DEFAULT_IDENTITY), "isDefault": identity.is_none()}),
    )
}
fn models_reply(store: &SqliteStorage) -> crate::Result<Value> {
    let settings = store.settings()?;
    let slot = |model: storage::ModelRef, reasoning: ReasoningLevel| json!({"provider":model.provider.0,"model":model.model,"reasoning":reasoning.as_str()});
    let main = settings
        .default_model
        .map(|m| slot(m, settings.reasoning))
        .unwrap_or_else(
            || json!({"provider":"","model":"","reasoning":settings.reasoning.as_str()}),
        );
    let compaction = settings.compaction_model.map(|m| {
        slot(
            m,
            settings.compaction_reasoning.unwrap_or(settings.reasoning),
        )
    });
    let utility = store
        .utility_model()?
        .map(|(m, r)| slot(m, r.unwrap_or(ReasoningLevel::Low)));
    let mut providers = Vec::new();
    for p in store.providers()? {
        let models: Vec<Value> = store.models(&p.id)?.into_iter().map(|m| json!({"id":m.id,"context":m.context_window,"limit":m.context_limit,"images":m.images})).collect();
        providers.push(json!({"id":p.id.0,"models":models}));
    }
    Ok(json!({"main":main,"compaction":compaction,"utility":utility,"providers":providers}))
}
fn providers_reply(store: &SqliteStorage) -> crate::Result<Value> {
    let mut result = Vec::new();
    for p in store.providers()? {
        let count = store.models(&p.id)?.len();
        result.push(json!({"id":p.id.0,"api":match p.api { storage::ProviderApi::OpenAiResponses => "responses", storage::ProviderApi::OpenAiCompletions => "completions" },"baseUrl":p.base_url,"keyEnv":p.api_key_env,"modelCount":count}));
    }
    Ok(json!(result))
}
fn discord_reply(ctx: &Context) -> crate::Result<Value> {
    let store = &ctx.store;
    let home = match store.home()? {
        Some(h) if h.messenger == "discord" => match h.target.strip_prefix("dm:") {
            Some(user) => json!({"kind":"dm","user":user}),
            None => json!({"kind":"channel","id":h.target}),
        },
        _ => json!({"kind":"none"}),
    };
    let connection = match ctx.discord.connection() {
        messenger_discord::Connection::Off => json!({"state":"off"}),
        messenger_discord::Connection::Connecting => json!({"state":"connecting"}),
        messenger_discord::Connection::Online { bot } => json!({"state":"online","bot":bot}),
        messenger_discord::Connection::Error { message } => {
            json!({"state":"error","message":message})
        }
    };
    Ok(json!({
        "allowed": store.allowed_users("discord")?,
        "home": home,
        "token": if store.messenger_token("discord")?.is_some() { "set" } else { "missing" },
        "connection": connection,
        "interactionModes": store.interaction_modes("discord")?.into_iter().map(|o| json!({"scope":o.scope.as_str(),"targetId":o.target_id,"mode":o.mode.as_str()})).collect::<Vec<_>>(),
    }))
}
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| "Invalid payload".into())
}
fn numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
async fn dispatch(
    ctx: &Context,
    request_type: &str,
    payload: &Value,
    headers: &HeaderMap,
) -> crate::Result<Value> {
    let store = &ctx.store;
    {
        let invalid = || format!("Invalid {request_type} payload");
        let get = |key| field(payload, key).map_err(|_| invalid());
        match request_type {
            "identity.get" => identity_reply(store),
            "identity.set" => {
                let text = get("text")?;
                if text.trim().is_empty() {
                    return Err("Identity cannot be blank".into());
                }
                crate::set_identity(store, text)?;
                identity_reply(store)
            }
            "identity.reset" => {
                store.set_identity(None)?;
                identity_reply(store)
            }
            "models.get" => models_reply(store),
            "models.setMain" | "models.setCompaction" | "models.setUtility" => {
                let data = if request_type == "models.setMain" {
                    payload
                } else {
                    payload.get("slot").ok_or_else(invalid)?
                };
                let selected = if data.is_null() && request_type != "models.setMain" {
                    None
                } else {
                    let provider = field(data, "provider").map_err(|_| invalid())?;
                    let model = field(data, "model").map_err(|_| invalid())?;
                    let reasoning: ReasoningLevel = field(data, "reasoning")
                        .map_err(|_| invalid())?
                        .parse()
                        .map_err(|_| invalid())?;
                    crate::known_model(store, &format!("{provider}/{model}"))
                        .map_err(|_| "Unknown model")?;
                    Some((format!("{provider}/{model}"), reasoning))
                };
                let _guard = ctx.settings_lock.lock().await;
                match request_type {
                    "models.setMain" => {
                        let (model, reasoning) = selected.ok_or_else(invalid)?;
                        let model = crate::known_model(store, &model)?;
                        let mut settings = store.settings()?;
                        settings.default_model = Some(storage::ModelRef {
                            provider: model.provider,
                            model: model.id,
                        });
                        settings.reasoning = reasoning;
                        store.save_settings(settings)?;
                    }
                    "models.setCompaction" => {
                        let (model, reasoning) = selected
                            .map(|(m, r)| (m, Some(r)))
                            .unwrap_or(("-".into(), None));
                        crate::model_command(
                            store,
                            crate::ModelCommand::Compaction { model, reasoning },
                        )?;
                    }
                    _ => {
                        let (model, reasoning) = selected
                            .map(|(m, r)| (m, Some(r)))
                            .unwrap_or(("off".into(), None));
                        crate::model_command(
                            store,
                            crate::ModelCommand::Utility { model, reasoning },
                        )?;
                    }
                }
                models_reply(store)
            }
            "models.setContextLimit" | "models.setImages" => {
                let provider = get("provider")?;
                let model = get("model")?;
                let name = format!("{provider}/{model}");
                let command = if request_type == "models.setContextLimit" {
                    let limit = payload.get("limit").ok_or_else(invalid)?;
                    let tokens = if limit.is_null() {
                        "-".to_string()
                    } else {
                        let n = limit
                            .as_u64()
                            .filter(|n| *n > 0 && *n <= u32::MAX as u64)
                            .ok_or("Context limit must be a positive integer")?;
                        n.to_string()
                    };
                    crate::ModelCommand::Context {
                        model: name,
                        tokens,
                    }
                } else {
                    let images = payload
                        .get("images")
                        .and_then(Value::as_bool)
                        .ok_or_else(invalid)?;
                    crate::ModelCommand::Input {
                        model: name,
                        input: if images {
                            crate::InputKind::Image
                        } else {
                            crate::InputKind::Text
                        },
                    }
                };
                let model_name = format!("{provider}/{model}");
                crate::known_model(store, &model_name).map_err(|_| "Unknown model")?;
                crate::model_command(store, command)?;
                models_reply(store)
            }
            "providers.list" => providers_reply(store),
            "providers.add" => {
                let id = get("id")?;
                let base_url = get("baseUrl")?;
                let key_env = get("keyEnv")?;
                let api = match get("api")? {
                    "responses" => crate::Api::Responses,
                    "completions" => crate::Api::Completions,
                    _ => return Err(invalid().into()),
                };
                if [id, base_url, key_env].iter().any(|s| s.trim().is_empty()) {
                    return Err("All fields are required".into());
                }
                if store.providers()?.iter().any(|p| p.id.0 == id) {
                    return Err(format!("Provider {id} already exists").into());
                }
                url::Url::parse(base_url).map_err(|_| "Base URL is not a valid URL")?;
                crate::provider(
                    store,
                    crate::ProviderCommand::Add {
                        id: id.into(),
                        api,
                        base_url: base_url.into(),
                        key_env: key_env.into(),
                    },
                )?;
                providers_reply(store)
            }
            "providers.remove" | "providers.refresh" => {
                let id = get("id")?;
                let provider = store
                    .providers()?
                    .into_iter()
                    .find(|p| p.id.0 == id)
                    .ok_or_else(|| format!("Unknown provider {id}"))?;
                if request_type == "providers.refresh" {
                    crate::refresh_provider(store, &provider).await?;
                } else {
                    let s = store.settings()?;
                    let utility = store.utility_model()?;
                    if s.default_model
                        .as_ref()
                        .is_some_and(|m| m.provider == provider.id)
                        || s.compaction_model
                            .as_ref()
                            .is_some_and(|m| m.provider == provider.id)
                        || utility
                            .as_ref()
                            .is_some_and(|(m, _)| m.provider == provider.id)
                    {
                        return Err(format!("Provider {id} is used by a model slot").into());
                    }
                    crate::provider(store, crate::ProviderCommand::Remove { id: id.into() })?;
                }
                providers_reply(store)
            }
            "discord.get" => discord_reply(ctx),
            "discord.allow" | "discord.disallow" => {
                let user = get("user")?;
                if !numeric(user) {
                    return Err("User id must be numeric".into());
                }
                let exists = store.allowed_users("discord")?.iter().any(|u| u == user);
                if request_type == "discord.allow" {
                    if exists {
                        return Err("User is already allowed".into());
                    }
                    crate::messenger(
                        store,
                        crate::MessengerCommand::Allow {
                            messenger: "discord".into(),
                            user_id: user.into(),
                        },
                    )
                    .await?;
                } else {
                    if !exists {
                        return Err("User is not allowed".into());
                    }
                    crate::messenger(
                        store,
                        crate::MessengerCommand::Disallow {
                            messenger: "discord".into(),
                            user_id: user.into(),
                        },
                    )
                    .await?;
                }
                discord_reply(ctx)
            }
            "discord.setToken" => {
                let token = get("token")?.trim().to_owned();
                if token.is_empty() {
                    return Err("Token is required".into());
                }
                if let Err(error) = messenger_discord::validate_token(&token).await {
                    return Err(error.into());
                }
                store.set_messenger_token("discord", Some(&token))?;
                if !ctx.discord.uses_token(&token) {
                    ctx.discord.connecting();
                }
                discord_reply(ctx)
            }
            "discord.clearToken" => {
                store.set_messenger_token("discord", None)?;
                ctx.discord.disconnect();
                discord_reply(ctx)
            }
            "discord.setHome" => {
                let home = payload.get("home").ok_or_else(invalid)?;
                match field(home, "kind").map_err(|_| invalid())? {
                    "none" => crate::set_home(store, None)?,
                    kind @ ("channel" | "dm") => {
                        let id = field(home, if kind == "channel" { "id" } else { "user" })
                            .map_err(|_| invalid())?;
                        if !numeric(id) || id.parse::<u64>().map_or(true, |n| n == 0) {
                            return Err("Channel and user ids must be numeric".into());
                        }
                        let target = if kind == "dm" {
                            format!("dm:{id}")
                        } else {
                            id.into()
                        };
                        crate::set_home(
                            store,
                            Some(storage::Home {
                                messenger: "discord".into(),
                                target,
                            }),
                        )?;
                    }
                    _ => return Err(invalid().into()),
                }
                discord_reply(ctx)
            }
            "discord.setInteractionMode" | "discord.clearInteractionMode" => {
                let scope = match get("scope")? {
                    "channel" => storage::InteractionScope::Channel,
                    "user" => storage::InteractionScope::User,
                    _ => return Err(invalid().into()),
                };
                let target = get("targetId")?;
                if request_type == "discord.setInteractionMode" {
                    if !crate::valid_discord_id(target) {
                        return Err("Ids must be 17–20 digits".into());
                    }
                    let mode = match get("mode")? {
                        "always" => storage::InteractionMode::Always,
                        "mention-only" => storage::InteractionMode::MentionOnly,
                        _ => return Err(invalid().into()),
                    };
                    store.set_interaction_mode("discord", scope, target, mode)?;
                } else {
                    if target.is_empty() {
                        return Err(invalid().into());
                    }
                    store.clear_interaction_mode("discord", scope, target)?;
                }
                discord_reply(ctx)
            }
            "discord.directory" => {
                let (channels, people) = match ctx.discord.directory().await {
                    Ok(result) => result,
                    Err(error) => {
                        eprintln!("discord directory: {error}");
                        Default::default()
                    }
                };
                Ok(
                    json!({"channels":channels.into_iter().map(|(id,name,server)| json!({"id":id,"name":name,"server":server})).collect::<Vec<_>>(),"people":people.into_iter().map(|p| json!({"id":p.id,"name":p.name})).collect::<Vec<_>>() }),
                )
            }
            "mcp.list" => Ok(mcp_reply(&ctx.mcp).await?),
            "mcp.add" => {
                let id = get("id")?;
                if !mcp::valid_id(id) {
                    return Err("Invalid id: use lowercase letters, digits and -".into());
                }
                let args = payload
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let transport = match get("transport")? {
                    "stdio" => McpTransport::Stdio {
                        command: get("command")?.to_string(),
                        args,
                        env: str_map(payload, "env"),
                    },
                    "http" => McpTransport::Http {
                        url: get("url")?.to_string(),
                        headers: str_map(payload, "headers"),
                    },
                    _ => return Err(invalid().into()),
                };
                store.save_mcp_server(&McpServer {
                    id: id.to_string(),
                    transport,
                    enabled: true,
                })?;
                Ok(mcp_reply(&ctx.mcp).await?)
            }
            "mcp.remove" => {
                let id = get("id")?;
                store.remove_mcp_server(id)?;
                Ok(mcp_reply(&ctx.mcp).await?)
            }
            "mcp.setEnabled" => {
                let id = get("id")?;
                let enabled = payload
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(invalid)?;
                let mut server = store
                    .mcp_servers()?
                    .into_iter()
                    .find(|server| server.id == id)
                    .ok_or_else(|| format!("no MCP server '{id}'"))?;
                server.enabled = enabled;
                store.save_mcp_server(&server)?;
                Ok(mcp_reply(&ctx.mcp).await?)
            }
            "mcp.login" => {
                let id = get("id")?;
                let server = store
                    .mcp_servers()?
                    .into_iter()
                    .find(|server| server.id == id)
                    .ok_or_else(|| format!("no MCP server '{id}'"))?;
                if !matches!(server.transport, McpTransport::Http { .. }) {
                    return Err(format!("MCP server '{id}' is not an HTTP server").into());
                }
                let origin = request_origin(ctx, headers).ok_or("missing Host header")?;
                Ok(json!({"url": format!("{origin}/api/mcp/login?id={id}")}))
            }
            _ => Err(format!("Unknown request: {request_type}").into()),
        }
    }
}
async fn handle(
    ctx: &Context,
    request_type: &str,
    payload: Value,
    headers: &HeaderMap,
) -> Result<Value, String> {
    dispatch(ctx, request_type, &payload, headers)
        .await
        .map_err(|e| e.to_string())
}
async fn socket_loop(socket: WebSocket, ctx: Context, headers: HeaderMap) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });
    while let Some(Ok(message)) = receiver.next().await {
        let text = match message {
            Message::Text(text) => Some(text),
            Message::Binary(_) => None,
            _ => continue,
        };
        if !matches!(authenticated(&ctx, &headers), Ok(Some(_))) {
            let _ = tx.send(Message::Close(Some(CloseFrame {
                code: 4401,
                reason: "unauthorized".into(),
            })));
            drop(tx);
            let _ = writer.await;
            return;
        }
        let frame = text
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|v| {
                let id = v.get("id")?.as_str()?.to_owned();
                let kind = v.get("type")?.as_str()?.to_owned();
                Some((id, kind, v.get("payload").cloned().unwrap_or(Value::Null)))
            });
        let Some((id, kind, payload)) = frame else {
            let reply = json!({"id": "", "ok": false, "error": "Invalid request"});
            let _ = tx.send(Message::Text(reply.to_string().into()));
            continue;
        };
        let tx = tx.clone();
        let ctx = ctx.clone();
        let headers = headers.clone();
        tokio::spawn(async move {
            let reply = match handle(&ctx, &kind, payload, &headers).await {
                Ok(payload) => json!({"id": id, "ok": true, "payload": payload}),
                Err(error) => json!({"id": id, "ok": false, "error": error}),
            };
            let _ = tx.send(Message::Text(reply.to_string().into()));
        });
    }
    drop(tx);
    writer.abort();
}
async fn assets(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (path, file) = match Assets::get(path) {
        Some(file) => (path, file),
        None => match Assets::get("index.html") {
            Some(file) => ("index.html", file),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    let mut response = (
        [(
            header::CONTENT_TYPE,
            mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string(),
        )],
        file.data.into_owned(),
    )
        .into_response();
    if path.starts_with("assets/") {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    response
}
