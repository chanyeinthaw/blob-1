use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
};
use axum::{
    body::Body,
    extract::{
        ws::{CloseFrame, Message, WebSocket},
        Path as AxumPath, Query, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
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
    Checkin, CheckinStore, ConfigStore, McpServer, McpStore, McpTransport, MessengerStore,
    PublicFileStore, ReasoningLevel, ScheduleStore, UsageStore, WatchStore, WebAuthStore,
    WebhookStore,
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
    workspace: harness::PinnedWorkspace,
    live_chat: crate::run::LiveChat,
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
    workspace: harness::PinnedWorkspace,
    live_chat: crate::run::LiveChat,
) -> crate::Result<()> {
    let store = Arc::new(WebContext {
        store,
        discord,
        settings_lock: tokio::sync::Mutex::new(()),
        mcp,
        public_url,
        workspace,
        live_chat,
    });
    let app = Router::new()
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/mcp/login", get(mcp_login))
        .route("/api/mcp/callback", get(mcp_callback))
        .route("/ws", get(ws))
        .route("/p/{id}", get(public_file))
        .route("/p/{id}/{name}", get(public_file_named))
        .route("/hooks/{token}", post(inbound_webhook))
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

async fn mcp_cached_reply(mcp: &mcp::Mcp) -> crate::Result<Value> {
    Ok(serde_json::to_value(mcp.cached_server_statuses().await)?)
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

fn general_reply(store: &SqliteStorage) -> crate::Result<Value> {
    Ok(json!({
        "autoRespond": store.auto_respond()?,
        "hasUtilityModel": store.utility_model()?.is_some(),
    }))
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
        result.push(json!({
            "id": p.id.0,
            "api": match p.api {
                storage::ProviderApi::OpenAiResponses => "responses",
                storage::ProviderApi::OpenAiCompletions => "completions",
            },
            "baseUrl": p.base_url,
            "apiKey": if p.has_api_key { "set" } else { "missing" },
            "modelCount": count,
        }));
    }
    Ok(json!(result))
}
fn describe_target(
    store: &SqliteStorage,
    to_dest: Option<u64>,
    reply: Option<&str>,
) -> Option<String> {
    if let Some(id) = to_dest {
        if let Ok(Some((_, place))) = store.place(id) {
            if place.dm {
                return Some("Direct message".to_string());
            }
            if let Some(thread) = place.thread {
                return Some(format!("Thread {thread}"));
            }
            return Some(format!("Channel {}", place.channel));
        }
        return Some(format!("d{id}"));
    }
    reply.map(|id| format!("Reply {id}"))
}

const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

const OCCURRENCE_PAGE_LIMIT: usize = 500;

fn occurrences_reply(
    store: &SqliteStorage,
    start: i64,
    end: i64,
    cursor: Option<(i64, &str)>,
) -> crate::Result<Value> {
    let effective_start = cursor.map_or(start, |(at, _)| start.max(at));
    let mut all: Vec<(i64, String)> = Vec::new();
    for s in store.schedules()? {
        let id = format!("s{}", s.id);
        // Fetch up to PAGE_LIMIT + 1 items strictly after `cursor` per schedule
        // so the merged sort is exact even when multiple schedules fire at the
        // same timestamp.
        let mut count = 0usize;
        for at in
            harness::schedule::occurrences(&s, effective_start, end, OCCURRENCE_PAGE_LIMIT + 2)
        {
            if let Some((c_at, c_id)) = cursor {
                if at < c_at || (at == c_at && id.as_str() <= c_id) {
                    continue;
                }
            }
            all.push((at, id.clone()));
            count += 1;
            if count > OCCURRENCE_PAGE_LIMIT {
                break;
            }
        }
    }
    all.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let has_more = all.len() > OCCURRENCE_PAGE_LIMIT;
    all.truncate(OCCURRENCE_PAGE_LIMIT);
    let next_cursor = if has_more {
        all.last()
            .map(|(at, id)| json!({ "at": at, "scheduleId": id }))
    } else {
        None
    };
    let occurrences = all
        .into_iter()
        .map(|(at, id)| json!({ "scheduleId": id, "at": at }))
        .collect::<Vec<_>>();
    Ok(json!({ "occurrences": occurrences, "nextCursor": next_cursor }))
}

fn schedule_reply(store: &SqliteStorage) -> crate::Result<Value> {
    let timezone = store.timezone()?;
    let now_s = now();
    let schedules = store
        .schedules()?
        .into_iter()
        .map(|s| {
            let (kind, when) = match (s.at, s.cron) {
                (Some(at), _) => ("at", at),
                (_, Some(cron)) => ("cron", cron),
                _ => ("at", String::new()),
            };
            json!({
                "id": format!("s{}", s.id),
                "text": s.text,
                "kind": kind,
                "when": when,
                "timezone": s.timezone,
                "nextDue": s.next_due,
                "target": describe_target(store, s.to_dest, s.reply.as_deref()),
            })
        })
        .collect::<Vec<_>>();
    let checkin_cfg = store.checkin()?;
    let next_checkin = checkin_cfg
        .as_ref()
        .and_then(|cfg| harness::schedule::next_checkin_timestamp(cfg, &timezone));
    let checkin = checkin_cfg.map(|c| {
        let days = c
            .days
            .split(',')
            .filter(|d| WEEKDAYS.contains(d))
            .collect::<Vec<_>>();
        json!({
            "every": c.every,
            "days": days,
            "startHour": c.start_hour,
            "endHour": c.end_hour,
        })
    });
    let watches = store
        .watches()?
        .into_iter()
        .filter(|w| w.until.is_none_or(|until| until > now_s))
        .map(|w| {
            json!({
                "id": format!("w{}", w.id),
                "text": w.text,
                "until": w.until,
                "target": describe_target(store, w.to_dest, w.reply.as_deref()),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "timezone": timezone,
        "schedules": schedules,
        "checkin": checkin,
        "nextCheckin": next_checkin,
        "watches": watches,
    }))
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
    let defaults = store.messenger_defaults("discord")?;
    Ok(json!({
        "allowed": store.allowed_users("discord")?,
        "accessMode": store.access_mode("discord")?.as_str(),
        "accessEntries": store.access_entries("discord")?.into_iter().map(|e| json!({"list":e.list.as_str(),"scope":e.scope.as_str(),"scopeId":e.scope_id,"userId":e.user_id})).collect::<Vec<_>>(),
        "channelAccessMode": store.channel_access_mode("discord")?.as_str(),
        "channelAccessEntries": store.channel_access_entries("discord")?.into_iter().map(|e| json!({"list":e.list.as_str(),"scope":e.scope.as_str(),"targetId":e.target_id})).collect::<Vec<_>>(),
        "home": home,
        "token": if store.messenger_token("discord")?.is_some() { "set" } else { "missing" },
        "connection": connection,
        "interactionModes": store.interaction_modes("discord")?.into_iter().map(|o| json!({"scope":o.scope.as_str(),"guildId":o.guild_id,"targetId":o.target_id,"mode":o.mode.as_str()})).collect::<Vec<_>>(),
        "defaultInteractionMode": defaults.interaction_mode.as_str(),
        "replyModes": store.reply_modes("discord")?.into_iter().map(|o| json!({"scope":o.scope.as_str(),"targetId":o.target_id,"mode":o.mode.as_str()})).collect::<Vec<_>>(),
        "defaultReplyMode": defaults.reply_mode.as_str(),
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
            "general.get" => general_reply(store),
            "general.setAutoRespond" => {
                let enabled = payload
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(invalid)?;
                store.set_auto_respond(enabled)?;
                general_reply(store)
            }
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
                let id = get("id")?.trim();
                let base_url = get("baseUrl")?.trim();
                let api_key = match payload.get("apiKey") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) => {
                        let trimmed = s.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        }
                    }
                    Some(_) => return Err(invalid().into()),
                };
                let api = match get("api")? {
                    "responses" => storage::ProviderApi::OpenAiResponses,
                    "completions" => storage::ProviderApi::OpenAiCompletions,
                    _ => return Err(invalid().into()),
                };
                if id.is_empty() || base_url.is_empty() {
                    return Err("ID and base URL are required".into());
                }
                if id.contains('/') {
                    return Err("Provider ID cannot contain '/'".into());
                }
                if store.providers()?.iter().any(|p| p.id.0 == id) {
                    return Err(format!("Provider {id} already exists").into());
                }
                url::Url::parse(base_url).map_err(|_| "Base URL is not a valid URL")?;
                store.save_provider(
                    storage::Provider {
                        id: storage::ProviderId(id.into()),
                        api,
                        base_url: base_url.into(),
                        has_api_key: api_key.is_some(),
                    },
                    api_key,
                )?;
                providers_reply(store)
            }
            "providers.setKey" => {
                let id = get("id")?;
                let api_key = match payload.get("apiKey") {
                    Some(Value::Null) => None,
                    Some(Value::String(s)) => {
                        let trimmed = s.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        }
                    }
                    _ => return Err(invalid().into()),
                };
                store.set_provider_api_key(&storage::ProviderId(id.into()), api_key)?;
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
                    "server" => storage::InteractionScope::Server,
                    _ => return Err(invalid().into()),
                };
                let target = get("targetId")?;
                let guild_id = match payload.get("guildId") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) => Some(s.as_str()),
                    Some(_) => return Err(invalid().into()),
                };
                if request_type == "discord.setInteractionMode" {
                    if !crate::valid_discord_id(target) {
                        return Err("Ids must be 17–20 digits".into());
                    }
                    match (scope, guild_id) {
                        (
                            storage::InteractionScope::Channel | storage::InteractionScope::Server,
                            None,
                        ) => {}
                        (storage::InteractionScope::User, Some(g))
                            if crate::valid_discord_id(g) => {}
                        (storage::InteractionScope::User, _) => {
                            return Err("Server IDs must be 17–20 digits".into());
                        }
                        (
                            storage::InteractionScope::Channel | storage::InteractionScope::Server,
                            Some(_),
                        ) => {
                            return Err(invalid().into());
                        }
                    }
                    let mode = match get("mode")? {
                        "always" => storage::InteractionMode::Always,
                        "mention-only" => storage::InteractionMode::MentionOnly,
                        "auto" => storage::InteractionMode::Auto,
                        _ => return Err(invalid().into()),
                    };
                    store.set_interaction_mode("discord", scope, guild_id, target, mode)?;
                } else {
                    if target.is_empty() {
                        return Err(invalid().into());
                    }
                    if scope != storage::InteractionScope::User && guild_id.is_some() {
                        return Err(invalid().into());
                    }
                    let guild_ref = guild_id.filter(|g| !g.is_empty());
                    store.clear_interaction_mode("discord", scope, guild_ref, target)?;
                }
                discord_reply(ctx)
            }
            "discord.setDefaultInteractionMode" => {
                let mode = match get("mode")? {
                    "mention-only" => storage::InteractionMode::MentionOnly,
                    "always" => storage::InteractionMode::Always,
                    "auto" => storage::InteractionMode::Auto,
                    _ => return Err(invalid().into()),
                };
                store.set_default_interaction_mode("discord", mode)?;
                discord_reply(ctx)
            }
            "discord.setReplyMode" | "discord.clearReplyMode" => {
                let scope = match get("scope")? {
                    "channel" => storage::ReplyScope::Channel,
                    "server" => storage::ReplyScope::Server,
                    _ => return Err(invalid().into()),
                };
                let target = get("targetId")?;
                if request_type == "discord.setReplyMode" {
                    if !crate::valid_discord_id(target) {
                        return Err("Ids must be 17–20 digits".into());
                    }
                    let mode = match get("mode")? {
                        "auto" => storage::ReplyMode::Auto,
                        "thread" => storage::ReplyMode::Thread,
                        "direct" => storage::ReplyMode::Direct,
                        _ => return Err(invalid().into()),
                    };
                    store.set_reply_mode("discord", scope, target, mode)?;
                } else {
                    if !crate::valid_discord_id(target) {
                        return Err("Ids must be 17–20 digits".into());
                    }
                    store.clear_reply_mode("discord", scope, target)?;
                }
                discord_reply(ctx)
            }
            "discord.setDefaultReplyMode" => {
                let mode = match get("mode")? {
                    "auto" => storage::ReplyMode::Auto,
                    "thread" => storage::ReplyMode::Thread,
                    "direct" => storage::ReplyMode::Direct,
                    _ => return Err(invalid().into()),
                };
                store.set_default_reply_mode("discord", mode)?;
                discord_reply(ctx)
            }
            "discord.setAccessMode" => {
                let mode = match get("mode")? {
                    "allow-all" => storage::AccessMode::AllowAll,
                    "whitelist" => storage::AccessMode::Whitelist,
                    "blacklist" => storage::AccessMode::Blacklist,
                    _ => return Err(invalid().into()),
                };
                store.set_access_mode("discord", mode)?;
                discord_reply(ctx)
            }
            "discord.addAccessEntry" | "discord.removeAccessEntry" => {
                let list = match get("list")? {
                    "whitelist" => storage::AccessList::Whitelist,
                    "blacklist" => storage::AccessList::Blacklist,
                    _ => return Err(invalid().into()),
                };
                let scope = match get("scope")? {
                    "global" => storage::AccessScope::Global,
                    "guild" => storage::AccessScope::Guild,
                    "channel" => storage::AccessScope::Channel,
                    _ => return Err(invalid().into()),
                };
                let scope_id = match payload.get("scopeId") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) => Some(s.as_str()),
                    Some(_) => return Err(invalid().into()),
                };
                let user_id = get("userId")?;
                if request_type == "discord.addAccessEntry" {
                    if !crate::valid_discord_id(user_id) {
                        return Err("User IDs must be 17–20 digits".into());
                    }
                    match (scope, scope_id) {
                        (storage::AccessScope::Global, None) => {}
                        (storage::AccessScope::Guild, Some(id)) if crate::valid_discord_id(id) => {}
                        (storage::AccessScope::Channel, Some(id))
                            if crate::valid_discord_id(id) => {}
                        (storage::AccessScope::Global, Some(_)) => return Err(invalid().into()),
                        (storage::AccessScope::Guild, _) => {
                            return Err("Server IDs must be 17–20 digits".into());
                        }
                        (storage::AccessScope::Channel, _) => {
                            return Err("Channel IDs must be 17–20 digits".into());
                        }
                    }
                    store.add_access_entry("discord", list, scope, scope_id, user_id)?;
                } else {
                    if user_id.is_empty() {
                        return Err(invalid().into());
                    }
                    match (scope, scope_id) {
                        (storage::AccessScope::Global, None) => {}
                        (storage::AccessScope::Guild | storage::AccessScope::Channel, Some(id))
                            if !id.is_empty() => {}
                        _ => return Err(invalid().into()),
                    }
                    store.remove_access_entry("discord", list, scope, scope_id, user_id)?;
                }
                discord_reply(ctx)
            }
            "discord.setChannelAccessMode" => {
                let mode = match get("mode")? {
                    "allow-all" => storage::AccessMode::AllowAll,
                    "whitelist" => storage::AccessMode::Whitelist,
                    "blacklist" => storage::AccessMode::Blacklist,
                    _ => return Err(invalid().into()),
                };
                store.set_channel_access_mode("discord", mode)?;
                discord_reply(ctx)
            }
            "discord.addChannelAccessEntry" | "discord.removeChannelAccessEntry" => {
                let list = match get("list")? {
                    "whitelist" => storage::AccessList::Whitelist,
                    "blacklist" => storage::AccessList::Blacklist,
                    _ => return Err(invalid().into()),
                };
                let scope = match get("scope")? {
                    "server" => storage::ChannelAccessScope::Server,
                    "channel" => storage::ChannelAccessScope::Channel,
                    _ => return Err(invalid().into()),
                };
                let target_id = get("targetId")?;
                if request_type == "discord.addChannelAccessEntry" {
                    if !crate::valid_discord_id(target_id) {
                        return Err(match scope {
                            storage::ChannelAccessScope::Server => {
                                "Server IDs must be 17–20 digits"
                            }
                            storage::ChannelAccessScope::Channel => {
                                "Channel IDs must be 17–20 digits"
                            }
                        }
                        .into());
                    }
                    store.add_channel_access_entry("discord", list, scope, target_id)?;
                } else {
                    if target_id.is_empty() {
                        return Err(invalid().into());
                    }
                    store.remove_channel_access_entry("discord", list, scope, target_id)?;
                }
                discord_reply(ctx)
            }
            "discord.directory" => {
                let (guilds, mut channels, people) = match ctx.discord.directory().await {
                    Ok(result) => result,
                    Err(error) => {
                        eprintln!("discord directory: {error}");
                        Default::default()
                    }
                };
                // Resolve any stored channel/thread overrides missing from the
                // top-level guild channel list so threads/channels still show
                // their name and server in Settings.
                let mut extra_ids = Vec::new();
                if let Ok(overrides) = store.interaction_modes("discord") {
                    extra_ids.extend(
                        overrides
                            .into_iter()
                            .filter(|o| o.scope == storage::InteractionScope::Channel)
                            .map(|o| o.target_id),
                    );
                }
                if let Ok(overrides) = store.reply_modes("discord") {
                    extra_ids.extend(
                        overrides
                            .into_iter()
                            .filter(|o| o.scope == storage::ReplyScope::Channel)
                            .map(|o| o.target_id),
                    );
                }
                if let Ok(entries) = store.access_entries("discord") {
                    extra_ids.extend(
                        entries
                            .into_iter()
                            .filter(|e| e.scope == storage::AccessScope::Channel)
                            .filter_map(|e| e.scope_id),
                    );
                }
                if let Ok(entries) = store.channel_access_entries("discord") {
                    extra_ids.extend(
                        entries
                            .into_iter()
                            .filter(|e| e.scope == storage::ChannelAccessScope::Channel)
                            .map(|e| e.target_id),
                    );
                }
                for cid in extra_ids {
                    if !channels.iter().any(|(id, _, _, _, _)| id == &cid) {
                        if let Ok(Some(resolved)) = ctx.discord.resolve_channel(&cid, &guilds).await
                        {
                            channels.push(resolved);
                        }
                    }
                }
                Ok(
                    json!({"guilds":guilds.into_iter().map(|(id,name)| json!({"id":id,"name":name})).collect::<Vec<_>>(),"channels":channels.into_iter().map(|(id,name,server,guild_id,is_thread)| json!({"id":id,"name":name,"server":server,"guildId":guild_id,"kind":if is_thread { "thread" } else { "channel" }})).collect::<Vec<_>>(),"people":people.into_iter().map(|(p,g)| json!({"id":p.id,"name":p.name,"guilds":g})).collect::<Vec<_>>() }),
                )
            }
            "mcp.list" => {
                ctx.mcp.probe_enabled().await;
                Ok(mcp_reply(&ctx.mcp).await?)
            }
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
                ctx.mcp.probe(id).await;
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
                if enabled {
                    let mcp = ctx.mcp.clone();
                    let id_owned = id.to_owned();
                    tokio::spawn(async move {
                        mcp.probe(&id_owned).await;
                    });
                }
                Ok(mcp_cached_reply(&ctx.mcp).await?)
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
            "schedule.get" => Ok(schedule_reply(store)?),
            "schedule.occurrences" => {
                let start = payload
                    .get("start")
                    .and_then(Value::as_i64)
                    .ok_or_else(invalid)?;
                let end = payload
                    .get("end")
                    .and_then(Value::as_i64)
                    .ok_or_else(invalid)?;
                if end < start || end.saturating_sub(start) > 90 * 86400 {
                    return Err("Range must be at most 90 days".into());
                }
                let cursor = match payload.get("cursor") {
                    None | Some(Value::Null) => None,
                    Some(obj) => {
                        let at = obj.get("at").and_then(Value::as_i64).ok_or_else(invalid)?;
                        let id = obj
                            .get("scheduleId")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?;
                        Some((at, id))
                    }
                };
                Ok(occurrences_reply(store, start, end, cursor)?)
            }
            "schedule.setTimezone" => {
                let tz = harness::schedule::validate_timezone(get("timezone")?.trim())
                    .map_err(|_| "Invalid timezone".to_string())?;
                store.set_timezone(&tz)?;
                Ok(schedule_reply(store)?)
            }
            "schedule.add" => {
                let text = get("text")?.trim().to_string();
                if text.is_empty() {
                    return Err("Task is required".into());
                }
                let kind = get("kind")?;
                let when = get("when")?.trim().to_string();
                let tz = match payload.get("timezone") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) if s.trim().is_empty() => None,
                    Some(Value::String(s)) => Some(s.trim().to_string()),
                    Some(_) => return Err(invalid().into()),
                };
                let (at, cron) = match kind {
                    "at" => (Some(when), None),
                    "cron" => (None, Some(when)),
                    _ => return Err(invalid().into()),
                };
                harness::schedule::add(
                    store.as_ref(),
                    store.as_ref(),
                    harness::schedule::Add {
                        text,
                        at,
                        cron,
                        tz,
                        to: None,
                        reply: None,
                    },
                )?;
                Ok(schedule_reply(store)?)
            }
            "schedule.remove" => {
                harness::schedule::remove(store.as_ref(), get("id")?)?;
                Ok(schedule_reply(store)?)
            }
            "checkin.set" => {
                let every = payload
                    .get("every")
                    .and_then(Value::as_i64)
                    .ok_or_else(invalid)?;
                if every < 300 {
                    return Err("Interval must be at least 5 minutes".into());
                }
                let raw_days = payload
                    .get("days")
                    .and_then(Value::as_array)
                    .ok_or_else(invalid)?;
                let mut selected = std::collections::HashSet::new();
                for value in raw_days {
                    let day = value.as_str().ok_or_else(invalid)?;
                    if !WEEKDAYS.contains(&day) || !selected.insert(day) {
                        return Err("Invalid days".into());
                    }
                }
                if selected.is_empty() {
                    return Err("Choose at least one day".into());
                }
                let days = WEEKDAYS
                    .iter()
                    .copied()
                    .filter(|day| selected.contains(day))
                    .collect::<Vec<_>>()
                    .join(",");
                let start_hour = payload
                    .get("startHour")
                    .and_then(Value::as_u64)
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or_else(invalid)?;
                let end_hour = payload
                    .get("endHour")
                    .and_then(Value::as_u64)
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or_else(invalid)?;
                if start_hour >= end_hour || end_hour > 24 {
                    return Err("End hour must be after start hour".into());
                }
                store.set_checkin(&Checkin {
                    every,
                    days,
                    start_hour,
                    end_hour,
                })?;
                Ok(schedule_reply(store)?)
            }
            "checkin.clear" => {
                store.clear_checkin()?;
                Ok(schedule_reply(store)?)
            }
            "watch.add" => {
                let text = get("text")?.trim().to_string();
                let until = match payload.get("until") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) if s.trim().is_empty() => None,
                    Some(Value::String(s)) => Some(s.trim().to_string()),
                    Some(_) => return Err(invalid().into()),
                };
                harness::watch::add(
                    store.as_ref(),
                    store.as_ref(),
                    harness::watch::Add {
                        text,
                        to: None,
                        reply: None,
                        until,
                    },
                )?;
                Ok(schedule_reply(store)?)
            }
            "watch.remove" => {
                harness::watch::remove(store.as_ref(), get("id")?)?;
                Ok(schedule_reply(store)?)
            }
            "usage.get" => {
                let days = payload
                    .get("days")
                    .and_then(Value::as_u64)
                    .unwrap_or(7)
                    .clamp(1, 90);
                let since = now().saturating_sub((days as i64) * 86_400);
                let summary = store.usage_summary(since)?;
                Ok(json!({
                    "days": days,
                    "totals": {
                        "calls": summary.totals.calls,
                        "inputTokens": summary.totals.input_tokens,
                        "cachedTokens": summary.totals.cached_tokens,
                        "outputTokens": summary.totals.output_tokens,
                    },
                    "byKind": summary.by_kind.into_iter().map(|bucket| json!({
                        "kind": bucket.kind.as_str(),
                        "calls": bucket.totals.calls,
                        "inputTokens": bucket.totals.input_tokens,
                        "cachedTokens": bucket.totals.cached_tokens,
                        "outputTokens": bucket.totals.output_tokens,
                    })).collect::<Vec<_>>(),
                    "byModel": summary.by_model.into_iter().map(|bucket| json!({
                        "provider": bucket.provider,
                        "model": bucket.model,
                        "calls": bucket.totals.calls,
                        "inputTokens": bucket.totals.input_tokens,
                        "cachedTokens": bucket.totals.cached_tokens,
                        "outputTokens": bucket.totals.output_tokens,
                    })).collect::<Vec<_>>(),
                    "daily": summary.daily.into_iter().map(|bucket| json!({
                        "day": bucket.day,
                        "calls": bucket.totals.calls,
                        "inputTokens": bucket.totals.input_tokens,
                        "cachedTokens": bucket.totals.cached_tokens,
                        "outputTokens": bucket.totals.output_tokens,
                    })).collect::<Vec<_>>(),
                }))
            }
            "webhooks.list" => Ok(webhooks_reply(ctx)?),
            "webhooks.create" => {
                let name = get("name")?.trim().to_string();
                if name.is_empty() {
                    return Err("Webhook name is required".into());
                }
                if name.chars().count() > 80 {
                    return Err("Webhook name must be at most 80 characters".into());
                }
                let mut id_bytes = [0u8; 8];
                let mut token_bytes = [0u8; 24];
                OsRng.fill_bytes(&mut id_bytes);
                OsRng.fill_bytes(&mut token_bytes);
                let id = hex::encode(id_bytes);
                let token = hex::encode(token_bytes);
                let token_hash = Sha256::digest(token.as_bytes());
                store.create_webhook(&id, &name, &token, &token_hash, now())?;
                Ok(webhooks_reply(ctx)?)
            }
            "webhooks.setEnabled" => {
                let id = get("id")?;
                let enabled = payload
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(invalid)?;
                if !store.set_webhook_enabled(id, enabled)? {
                    return Err("Webhook not found".into());
                }
                Ok(webhooks_reply(ctx)?)
            }
            "webhooks.remove" => {
                let id = get("id")?;
                if !store.remove_webhook(id)? {
                    return Err("Webhook not found".into());
                }
                Ok(webhooks_reply(ctx)?)
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
const PUBLIC_FILE_CHUNK_BYTES: usize = 64 * 1024;

async fn public_file(
    method: Method,
    State(ctx): State<Context>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    serve_public_file(&ctx, &id, method == Method::HEAD).await
}

async fn public_file_named(
    method: Method,
    State(ctx): State<Context>,
    AxumPath((id, _name)): AxumPath<(String, String)>,
) -> Response {
    serve_public_file(&ctx, &id, method == Method::HEAD).await
}

async fn serve_public_file(ctx: &WebContext, id: &str, head_only: bool) -> Response {
    let record = match ctx.store.public_file(id) {
        Ok(Some(record)) => record,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    // Reject links recorded under a different --workspace.
    if ctx.workspace.canonical_path() != record.workspace {
        return StatusCode::NOT_FOUND.into_response();
    }
    let workspace = ctx.workspace.clone();
    let rel_path = record.path.clone();
    let opened =
        tokio::task::spawn_blocking(move || -> Result<(std::fs::File, u64), StatusCode> {
            let file = workspace
                .open_file_nofollow(&rel_path)
                .map_err(|_| StatusCode::NOT_FOUND)?;
            let meta = file.metadata().map_err(|_| StatusCode::NOT_FOUND)?;
            let limit = harness::share::MAX_PUBLIC_FILE_BYTES;
            if meta.len() > limit {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            Ok((file, meta.len()))
        })
        .await;
    let (file, len) = match opened {
        Ok(Ok(pair)) => pair,
        Ok(Err(status)) => return status.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let body = if head_only {
        Body::empty()
    } else {
        // Stream directly from the pinned file handle in 64 KiB chunks, capped
        // strictly at `len` (<= MAX_PUBLIC_FILE_BYTES) so concurrent growth
        // cannot buffer or exceed the cap.
        let reader = tokio::io::AsyncReadExt::take(tokio::fs::File::from_std(file), len);
        let stream = futures_util::stream::try_unfold(reader, |mut reader| async move {
            use tokio::io::AsyncReadExt;
            let mut buf = vec![0u8; PUBLIC_FILE_CHUNK_BYTES];
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                Ok::<_, std::io::Error>(None)
            } else {
                buf.truncate(n);
                Ok(Some((buf, reader)))
            }
        });
        Body::from_stream(stream)
    };
    let content_type = HeaderValue::from_str(&record.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, content_type);
    // Set Content-Length only on HEAD; streaming GET omits it so a concurrent
    // file truncation does not abort the HTTP transfer.
    if head_only {
        if let Ok(len_header) = HeaderValue::from_str(&len.to_string()) {
            headers.insert(header::CONTENT_LENGTH, len_header);
        }
    }
    // Isolate generated HTML/SVG in an opaque null origin so scripts can render
    // freely without any access to same-origin cookies, /api, or /ws.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox allow-scripts"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=60"),
    );
    response
}

const MAX_WEBHOOK_BODY_BYTES: usize = 64 * 1024;

fn webhooks_reply(ctx: &WebContext) -> crate::Result<Value> {
    let webhooks = ctx.store.webhooks()?;
    let public_url = ctx
        .public_url
        .as_ref()
        .map(|u| u.as_str().trim_end_matches('/').to_owned());
    Ok(json!({
        "publicUrl": public_url,
        "webhooks": webhooks.into_iter().map(|hook| json!({
            "id": hook.id,
            "name": hook.name,
            "token": hook.token,
            "enabled": hook.enabled,
            "createdAt": hook.created_at,
            "lastTriggeredAt": hook.last_triggered_at,
        })).collect::<Vec<_>>(),
    }))
}

async fn inbound_webhook(
    State(ctx): State<Context>,
    AxumPath(token): AxumPath<String>,
    body: Body,
) -> Response {
    let token_hash = Sha256::digest(token.as_bytes());
    match ctx.store.webhook_by_token_hash(&token_hash) {
        Ok(Some(hook)) if hook.enabled => {}
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    let bytes = match axum::body::to_bytes(body, MAX_WEBHOOK_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    // Revalidate token and enabled state after body upload before admitting.
    let hook = match ctx.store.webhook_by_token_hash(&token_hash) {
        Ok(Some(hook)) if hook.enabled => hook,
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let text = String::from_utf8_lossy(&bytes).trim().to_owned();
    let payload = if text.is_empty() {
        "(empty payload)".to_owned()
    } else {
        text
    };
    if let Err(error) = ctx.live_chat.inject(
        payload,
        Some(storage::Meta::Event {
            source: "webhook".into(),
            name: hook.name,
        }),
    ) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ok": false, "error": error.to_string() })),
        )
            .into_response();
    }
    let _ = ctx.store.touch_webhook(&hook.id, now());
    (StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response()
}

async fn assets(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api"
        || path.starts_with("api/")
        || path == "p"
        || path.starts_with("p/")
        || path == "hooks"
        || path.starts_with("hooks/")
    {
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

#[cfg(test)]
mod public_file_tests {
    use super::*;
    use storage::PublicFile;

    #[tokio::test]
    async fn serves_shared_files_with_sandbox_csp_and_blocks_escapes() {
        let dir = std::env::temp_dir().join(format!("blob-1-web-share-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let store = Arc::new(SqliteStorage::open(dir.join("agent.db"), None).unwrap());
        store
            .set_public_url(Some("https://blob-1.example.com"))
            .unwrap();

        let pinned = harness::PinnedWorkspace::open(&workspace).unwrap();
        let tools = harness::ShareTools::new(store.clone(), store.clone(), pinned.clone());
        let md_url = tools
            .execute(r##"{"content":"# Hello\nworld","filename":"report"}"##)
            .unwrap();
        assert!(md_url.ends_with("/report.md"));
        let md_id = md_url
            .strip_prefix("https://blob-1.example.com/p/")
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .to_owned();

        std::fs::write(workspace.join("page.html"), "<script>1+1</script>").unwrap();
        let html_url = tools.execute(r#"{"path":"page.html"}"#).unwrap();
        let html_id = html_url
            .strip_prefix("https://blob-1.example.com/p/")
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .to_owned();

        // Store an entry pointing outside the workspace to verify serve-time containment.
        let outside = dir.join("secret.txt");
        std::fs::write(&outside, "secret").unwrap();
        store
            .create_public_file(&PublicFile {
                id: "escape".into(),
                workspace: pinned.canonical_path().to_owned(),
                path: "../secret.txt".into(),
                content_type: "text/plain; charset=utf-8".into(),
                created_at: 0,
            })
            .unwrap();

        // Store an entry recorded under a different workspace to verify workspace binding.
        let other_ws = dir.join("other-ws");
        std::fs::create_dir_all(&other_ws).unwrap();
        std::fs::write(other_ws.join("page.html"), "other").unwrap();
        let other_pinned = harness::PinnedWorkspace::open(&other_ws).unwrap();
        store
            .create_public_file(&PublicFile {
                id: "other-ws-link".into(),
                workspace: other_pinned.canonical_path().to_owned(),
                path: "page.html".into(),
                content_type: "text/html; charset=utf-8".into(),
                created_at: 0,
            })
            .unwrap();

        let ctx = WebContext {
            store: store.clone(),
            discord: Arc::new(messenger_discord::Discord::new()),
            settings_lock: tokio::sync::Mutex::new(()),
            mcp: mcp::Mcp::new(store.clone()),
            public_url: Some(public_url("https://blob-1.example.com").unwrap()),
            workspace: pinned,
            live_chat: crate::run::LiveChat::default(),
        };

        let md_resp = serve_public_file(&ctx, &md_id, false).await;
        assert_eq!(md_resp.status(), StatusCode::OK);
        assert_eq!(
            md_resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            md_resp.headers().get(header::CONTENT_SECURITY_POLICY).unwrap(),
            "sandbox allow-scripts"
        );
        assert_eq!(
            md_resp.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff"
        );
        assert!(md_resp.headers().get(header::CONTENT_LENGTH).is_none());
        let md_body = axum::body::to_bytes(md_resp.into_body(), 1024).await.unwrap();
        assert_eq!(md_body.as_ref(), b"# Hello\nworld");

        let head_resp = serve_public_file(&ctx, &md_id, true).await;
        assert_eq!(head_resp.status(), StatusCode::OK);
        assert_eq!(
            head_resp.headers().get(header::CONTENT_LENGTH).unwrap(),
            "13"
        );
        let head_body = axum::body::to_bytes(head_resp.into_body(), 1024)
            .await
            .unwrap();
        assert!(head_body.is_empty());

        let html_resp = serve_public_file(&ctx, &html_id, false).await;
        assert_eq!(html_resp.status(), StatusCode::OK);
        assert_eq!(
            html_resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let csp = html_resp
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains("sandbox"));
        assert!(!csp.contains("allow-same-origin"));

        assert_eq!(
            serve_public_file(&ctx, "missing", false).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve_public_file(&ctx, "escape", false).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve_public_file(&ctx, "other-ws-link", false).await.status(),
            StatusCode::NOT_FOUND
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn inbound_webhook_validates_token_enabled_and_body_limit() {
        let dir =
            std::env::temp_dir().join(format!("blob-1-web-hook-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let key = storage_sqlite::AppKey::new("test-key".into());
        let store = Arc::new(SqliteStorage::open(dir.join("agent.db"), Some(key)).unwrap());
        let pinned = harness::PinnedWorkspace::open(&workspace).unwrap();

        let token = "secret-hook-token";
        let hash = Sha256::digest(token.as_bytes());
        store
            .create_webhook("h1", "ci", token, &hash, now())
            .unwrap();

        let ctx = Arc::new(WebContext {
            store: store.clone(),
            discord: Arc::new(messenger_discord::Discord::new()),
            settings_lock: tokio::sync::Mutex::new(()),
            mcp: mcp::Mcp::new(store.clone()),
            public_url: Some(public_url("https://blob-1.example.com").unwrap()),
            workspace: pinned,
            live_chat: crate::run::LiveChat::default(),
        });

        // Unknown token -> 404.
        let r = inbound_webhook(
            State(ctx.clone()),
            AxumPath("wrong".into()),
            Body::from("hi"),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);

        // Oversized payload (> 64 KiB) -> 413.
        let big = vec![b'a'; MAX_WEBHOOK_BODY_BYTES + 1];
        let r = inbound_webhook(
            State(ctx.clone()),
            AxumPath(token.into()),
            Body::from(big),
        )
        .await;
        assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);

        // Valid token when chat harness is not attached yet -> 503 and does not update last_triggered_at.
        let r = inbound_webhook(
            State(ctx.clone()),
            AxumPath(token.into()),
            Body::from(r#"{"event":"push"}"#),
        )
        .await;
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            store.webhook_by_token_hash(&hash).unwrap().unwrap().last_triggered_at,
            None
        );

        // Disabled webhook -> 404.
        store.set_webhook_enabled("h1", false).unwrap();
        let r = inbound_webhook(
            State(ctx.clone()),
            AxumPath(token.into()),
            Body::from("hi"),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
