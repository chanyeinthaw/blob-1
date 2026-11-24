use std::fs::{File, TryLockError};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use chacha20poly1305::XChaCha20Poly1305;
use rusqlite::Connection;
use storage::StoreError;

mod bot;
mod checkin;
mod config;
mod crypto;
mod mcp;
mod memory;
mod messenger;
mod public_file;
mod schedule;
mod session;
mod skill;
mod usage;
mod watch;
mod web_auth;
mod webhook;

pub use crypto::AppKey;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS web_password (id INTEGER PRIMARY KEY CHECK (id = 1), hash TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS web_sessions (hash BLOB PRIMARY KEY, expires INTEGER NOT NULL, renewed INTEGER NOT NULL) STRICT;

CREATE TABLE IF NOT EXISTS watches (id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL, to_dest INTEGER, reply TEXT, until INTEGER) STRICT;
CREATE TABLE IF NOT EXISTS checkin (id INTEGER PRIMARY KEY CHECK (id = 1), every INTEGER NOT NULL, days TEXT NOT NULL, start_hour INTEGER NOT NULL, end_hour INTEGER NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS clock (id INTEGER PRIMARY KEY CHECK (id = 1), timezone TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS home (id INTEGER PRIMARY KEY CHECK (id = 1), messenger TEXT NOT NULL, target TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS schedules (id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL, at TEXT, cron TEXT, timezone TEXT NOT NULL, to_dest INTEGER, reply TEXT, next_due INTEGER NOT NULL, CHECK ((at IS NULL) != (cron IS NULL))) STRICT;

CREATE TABLE IF NOT EXISTS memory_messages (i INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK (kind IN ('user','assistant','tool','echo','work')), text TEXT NOT NULL, size INTEGER NOT NULL, date TEXT NOT NULL, more INTEGER NOT NULL CHECK (more IN (0,1)), meta TEXT) STRICT;
CREATE TABLE IF NOT EXISTS memory_nodes (l INTEGER NOT NULL, i INTEGER NOT NULL, text TEXT NOT NULL, size INTEGER NOT NULL, PRIMARY KEY (l, i)) STRICT;
CREATE TABLE IF NOT EXISTS memory_views (name TEXT PRIMARY KEY CHECK (name IN ('chat','seen')), batch INTEGER NOT NULL CHECK (batch IN (0,1))) STRICT;
CREATE TABLE IF NOT EXISTS memory_view_parts (view TEXT NOT NULL CHECK (view IN ('chat','seen')), start INTEGER NOT NULL, l INTEGER NOT NULL, i INTEGER NOT NULL, PRIMARY KEY (view, start)) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    usage_tokens INTEGER,
    usage_boundary INTEGER
) STRICT;

CREATE TABLE IF NOT EXISTS task_choices (id TEXT PRIMARY KEY, provider TEXT NOT NULL, model TEXT NOT NULL, reasoning TEXT NOT NULL) STRICT;

CREATE TABLE IF NOT EXISTS messages (
    session_id TEXT NOT NULL REFERENCES sessions (id),
    position INTEGER NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('summary', 'user', 'assistant', 'tool_call', 'tool_result')),
    content TEXT NOT NULL,
    tool_call_id TEXT,
    tool_name TEXT,
    details TEXT,
    PRIMARY KEY (session_id, position),
    CHECK ((role IN ('tool_call', 'tool_result')) = (tool_call_id IS NOT NULL AND tool_name IS NOT NULL)),
    CHECK ((role = 'summary') = (details IS NOT NULL))
) STRICT;

CREATE TABLE IF NOT EXISTS message_images (session_id TEXT NOT NULL, position INTEGER NOT NULL, idx INTEGER NOT NULL, mime TEXT NOT NULL, data BLOB NOT NULL, PRIMARY KEY (session_id, position, idx), FOREIGN KEY (session_id, position) REFERENCES messages (session_id, position)) STRICT;
CREATE TABLE IF NOT EXISTS skills (name TEXT PRIMARY KEY, description TEXT NOT NULL, hash TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS skill_files (skill TEXT NOT NULL REFERENCES skills (name) ON DELETE CASCADE, path TEXT NOT NULL, mode INTEGER NOT NULL, content BLOB NOT NULL, PRIMARY KEY (skill, path)) STRICT;

CREATE TABLE IF NOT EXISTS destinations (id INTEGER PRIMARY KEY, messenger TEXT NOT NULL, channel TEXT NOT NULL, thread TEXT NOT NULL, dm INTEGER NOT NULL CHECK (dm IN (0,1)), UNIQUE (messenger, channel, thread)) STRICT;
CREATE TABLE IF NOT EXISTS access_defaults (messenger TEXT PRIMARY KEY, effect TEXT NOT NULL CHECK (effect IN ('allow', 'block'))) STRICT;
CREATE TABLE IF NOT EXISTS access_rules (messenger TEXT NOT NULL, user_id TEXT NOT NULL DEFAULT '', scope TEXT NOT NULL CHECK (scope IN ('global', 'server', 'channel')), scope_id TEXT NOT NULL DEFAULT '', effect TEXT NOT NULL CHECK (effect IN ('allow', 'block')), CHECK ((scope = 'global' AND scope_id = '' AND user_id != '') OR (scope IN ('server', 'channel') AND scope_id != '')), PRIMARY KEY (messenger, user_id, scope, scope_id)) STRICT;
CREATE TABLE IF NOT EXISTS interaction_modes (messenger TEXT NOT NULL, scope TEXT NOT NULL CHECK (scope IN ('channel', 'user', 'server')), guild_id TEXT NOT NULL DEFAULT '', target_id TEXT NOT NULL, mode TEXT NOT NULL CHECK (mode IN ('mention-only', 'always', 'auto')), CHECK ((scope IN ('channel', 'server') AND guild_id = '') OR scope = 'user'), PRIMARY KEY (messenger, scope, guild_id, target_id)) STRICT;
CREATE TABLE IF NOT EXISTS reply_modes (messenger TEXT NOT NULL, scope TEXT NOT NULL CHECK (scope IN ('channel', 'server')), target_id TEXT NOT NULL, mode TEXT NOT NULL CHECK (mode IN ('auto', 'thread', 'direct')), PRIMARY KEY (messenger, scope, target_id)) STRICT;
CREATE TABLE IF NOT EXISTS messenger_defaults (messenger TEXT PRIMARY KEY, interaction_mode TEXT NOT NULL CHECK (interaction_mode IN ('mention-only', 'always', 'auto')), reply_mode TEXT NOT NULL CHECK (reply_mode IN ('auto', 'thread', 'direct'))) STRICT;
CREATE TABLE IF NOT EXISTS seen_threads (messenger TEXT NOT NULL, channel TEXT NOT NULL, thread TEXT NOT NULL, PRIMARY KEY (messenger, channel, thread)) STRICT;

CREATE TABLE IF NOT EXISTS instructions (id INTEGER PRIMARY KEY CHECK (id = 1), text TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS public_url (id INTEGER PRIMARY KEY CHECK (id = 1), url TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS public_files (
    id TEXT PRIMARY KEY,
    workspace TEXT NOT NULL,
    path TEXT NOT NULL,
    content_type TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS usage_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('chat', 'task', 'utility', 'compaction')),
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    cached_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS usage_events_ts_idx ON usage_events (ts);

CREATE TABLE IF NOT EXISTS webhooks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    token TEXT NOT NULL,
    token_hash BLOB NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at INTEGER NOT NULL,
    last_triggered_at INTEGER
) STRICT;

CREATE TABLE IF NOT EXISTS callbacks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    token_hash BLOB NOT NULL UNIQUE,
    for_token TEXT,
    remaining_uses INTEGER NOT NULL CHECK (remaining_uses > 0),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS pending_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    name TEXT NOT NULL,
    for_token TEXT,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS providers (
    id TEXT PRIMARY KEY,
    api TEXT NOT NULL CHECK (api IN ('openai-responses', 'openai-completions', 'opencode-go', 'decision')),
    base_url TEXT NOT NULL,
    api_key TEXT
) STRICT;

CREATE TABLE IF NOT EXISTS models (
    provider TEXT NOT NULL REFERENCES providers (id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    context_window INTEGER,
    context_limit INTEGER CHECK (context_limit > 0),
    PRIMARY KEY (provider, id)
) STRICT;

CREATE TABLE IF NOT EXISTS utility_model (id INTEGER PRIMARY KEY CHECK (id = 1), provider TEXT NOT NULL, model TEXT NOT NULL, reasoning TEXT CHECK (reasoning IN ('off','low','medium','high','xhigh','max')), FOREIGN KEY (provider, model) REFERENCES models (provider, id) ON DELETE CASCADE) STRICT;
CREATE TABLE IF NOT EXISTS decision_model (id INTEGER PRIMARY KEY CHECK (id = 1), provider TEXT NOT NULL, model TEXT NOT NULL, FOREIGN KEY (provider, model) REFERENCES models (provider, id) ON DELETE CASCADE) STRICT;

CREATE TABLE IF NOT EXISTS bots (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at INTEGER NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0
) STRICT;
INSERT OR IGNORE INTO bots (id, name, enabled, created_at) VALUES ('default', 'Blob-1', 1, 0);

CREATE TABLE IF NOT EXISTS bot_sequence (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    next_gen INTEGER NOT NULL DEFAULT 1
) STRICT;
INSERT OR IGNORE INTO bot_sequence (id, next_gen) VALUES (1, 1);

CREATE TABLE IF NOT EXISTS bot_models (
    bot_id TEXT PRIMARY KEY REFERENCES bots (id) ON DELETE CASCADE,
    default_provider TEXT,
    default_model TEXT,
    reasoning TEXT NOT NULL DEFAULT 'medium' CHECK (reasoning IN ('off','low','medium','high','xhigh','max')),
    compaction_provider TEXT,
    compaction_model TEXT,
    compaction_reasoning TEXT CHECK (compaction_reasoning IN ('off','low','medium','high','xhigh','max')),
    utility_provider TEXT,
    utility_model TEXT,
    utility_reasoning TEXT CHECK (utility_reasoning IN ('off','low','medium','high','xhigh','max')),
    decision_provider TEXT,
    decision_model TEXT,
    FOREIGN KEY (default_provider, default_model) REFERENCES models (provider, id) ON DELETE SET NULL,
    FOREIGN KEY (compaction_provider, compaction_model) REFERENCES models (provider, id) ON DELETE SET NULL,
    FOREIGN KEY (utility_provider, utility_model) REFERENCES models (provider, id) ON DELETE SET NULL,
    FOREIGN KEY (decision_provider, decision_model) REFERENCES models (provider, id) ON DELETE SET NULL
) STRICT;

CREATE TABLE IF NOT EXISTS bot_mcp_disabled (
    bot_id TEXT NOT NULL REFERENCES bots (id) ON DELETE CASCADE,
    server_id TEXT NOT NULL REFERENCES mcp_servers (id) ON DELETE CASCADE,
    PRIMARY KEY (bot_id, server_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS mcp_servers (
    id TEXT PRIMARY KEY,
    transport TEXT NOT NULL CHECK (transport IN ('stdio','http')),
    command TEXT,
    args TEXT NOT NULL DEFAULT '[]',
    env TEXT NOT NULL DEFAULT '{}',
    url TEXT,
    headers TEXT NOT NULL DEFAULT '{}',
    enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
    CHECK ((transport = 'stdio') = (command IS NOT NULL)),
    CHECK ((transport = 'http') = (url IS NOT NULL))
) STRICT;

CREATE TABLE IF NOT EXISTS mcp_tokens (
    server TEXT PRIMARY KEY REFERENCES mcp_servers (id) ON DELETE CASCADE,
    credentials TEXT,
    registration TEXT
) STRICT;

CREATE TABLE IF NOT EXISTS mcp_oauth_states (
    csrf TEXT PRIMARY KEY,
    server TEXT NOT NULL REFERENCES mcp_servers (id) ON DELETE CASCADE,
    state TEXT NOT NULL,
    created INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS app_key (id INTEGER PRIMARY KEY CHECK (id = 1), salt BLOB NOT NULL, canary BLOB NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS messenger_tokens (messenger TEXT PRIMARY KEY, token TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS secret_scrub (id INTEGER PRIMARY KEY CHECK (id = 1)) STRICT;
CREATE TABLE IF NOT EXISTS model_images (provider TEXT NOT NULL, model TEXT NOT NULL, PRIMARY KEY (provider, model), FOREIGN KEY (provider, model) REFERENCES models (provider, id) ON DELETE CASCADE) STRICT;
CREATE TABLE IF NOT EXISTS model_reasoning (
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    level TEXT NOT NULL CHECK (level IN ('off', 'low', 'medium', 'high', 'xhigh', 'max')),
    value TEXT,
    PRIMARY KEY (provider, model, level),
    FOREIGN KEY (provider, model) REFERENCES models (provider, id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    default_provider TEXT,
    default_model TEXT,
    reasoning TEXT NOT NULL DEFAULT 'medium' CHECK (reasoning IN ('off', 'low', 'medium', 'high', 'xhigh', 'max')),
    compaction_provider TEXT,
    compaction_model TEXT,
    compaction_reasoning TEXT CHECK (compaction_reasoning IN ('off', 'low', 'medium', 'high', 'xhigh', 'max')),
    auto_respond INTEGER NOT NULL DEFAULT 1 CHECK (auto_respond IN (0, 1)),
    FOREIGN KEY (default_provider, default_model) REFERENCES models (provider, id) ON DELETE SET NULL,
    FOREIGN KEY (compaction_provider, compaction_model) REFERENCES models (provider, id) ON DELETE SET NULL
) STRICT;

INSERT OR IGNORE INTO settings (id) VALUES (1);
";

pub struct SqliteStorage {
    connection: Mutex<Connection>,
    pub(crate) key: Option<XChaCha20Poly1305>,
    _owner: Option<File>,
    _maint: File,
}

impl SqliteStorage {
    pub fn open(path: impl AsRef<Path>, app_key: Option<AppKey>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        let owner = File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(lock_path(path))
            .map_err(|error| StoreError::Backend(Box::new(error)))?;
        match owner.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(StoreError::Backend(
                    format!("{} is already open in another process", path.display()).into(),
                ));
            }
            Err(TryLockError::Error(error)) => return Err(StoreError::Backend(Box::new(error))),
        }
        Self::connect(path, Some(owner), app_key)
    }

    pub fn open_shared(
        path: impl AsRef<Path>,
        app_key: Option<AppKey>,
    ) -> Result<Self, StoreError> {
        Self::connect(path.as_ref(), None, app_key)
    }

    fn connect(
        path: &Path,
        owner: Option<File>,
        app_key: Option<AppKey>,
    ) -> Result<Self, StoreError> {
        let maint = File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(maint_lock_path(path))
            .map_err(|error| StoreError::Backend(Box::new(error)))?;
        match maint.try_lock_shared() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(StoreError::Backend(
                    format!(
                        "{} is locked for update maintenance; retry shortly",
                        path.display()
                    )
                    .into(),
                ));
            }
            Err(TryLockError::Error(error)) => return Err(StoreError::Backend(Box::new(error))),
        }
        let mut connection = Connection::open(path).map_err(backend)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(backend)?;
        connection
            .pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
            .map_err(backend)?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(backend)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(backend)?;
        // Hold a temporary owner lock only when a shared open needs a one-time
        // schema migration and no server currently owns the database.
        let mut upgrade_owner: Option<File> = None;
        let has_exclusive = owner.is_some();
        let mut require_exclusive = |msg: &'static str| -> Result<(), StoreError> {
            if has_exclusive || upgrade_owner.is_some() {
                return Ok(());
            }
            let file = File::options()
                .create(true)
                .write(true)
                .truncate(false)
                .open(lock_path(path))
                .map_err(|error| StoreError::Backend(Box::new(error)))?;
            match file.try_lock() {
                Ok(()) => {
                    upgrade_owner = Some(file);
                    Ok(())
                }
                Err(TryLockError::WouldBlock) => Err(StoreError::Backend(msg.into())),
                Err(TryLockError::Error(error)) => Err(StoreError::Backend(Box::new(error))),
            }
        };
        // Initialize schema + `pending_threads` + `access_entries` atomically
        // inside one immediate transaction on fresh DBs, so concurrent shared
        // opens never observe half-initialized tables.
        init_schema_and_pending_threads(&mut connection, &mut require_exclusive)?;
        for table in ["destinations", "allowed_users"] {
            let old_column: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = 'interface')",
                    [table],
                    |row| row.get(0),
                )
                .map_err(backend)?;
            if old_column {
                require_exclusive(
                    "this database needs an upgrade: stop agent serve and run any other agent command once",
                )?;
                connection
                    .execute_batch(&format!(
                        "ALTER TABLE {table} RENAME COLUMN interface TO messenger"
                    ))
                    .map_err(backend)?;
            }
        }
        let has_guild_id: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('interaction_modes') WHERE name = 'guild_id')",
                [],
                |row| row.get(0),
            )
            .map_err(backend)?;
        if !has_guild_id {
            require_exclusive(
                "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
            )?;
            let tx = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(backend)?;
            tx.execute_batch(
                "CREATE TABLE interaction_modes_v3 (
                     messenger TEXT NOT NULL,
                     scope TEXT NOT NULL CHECK (scope IN ('channel', 'user', 'server')),
                     guild_id TEXT NOT NULL DEFAULT '',
                     target_id TEXT NOT NULL,
                     mode TEXT NOT NULL CHECK (mode IN ('mention-only', 'always', 'auto')),
                     CHECK ((scope IN ('channel', 'server') AND guild_id = '') OR scope = 'user'),
                     PRIMARY KEY (messenger, scope, guild_id, target_id)
                 ) STRICT;
                 INSERT INTO interaction_modes_v3 (messenger, scope, guild_id, target_id, mode)
                 SELECT messenger, scope, '', target_id, mode FROM interaction_modes;
                 DROP TABLE interaction_modes;
                 ALTER TABLE interaction_modes_v3 RENAME TO interaction_modes;",
            )
            .map_err(backend)?;
            tx.commit().map_err(backend)?;
        }
        ensure_interaction_modes_v3(&mut connection, &mut require_exclusive)?;
        ensure_reply_modes_v2(&mut connection, &mut require_exclusive)?;
        ensure_unified_access(&mut connection, &mut require_exclusive)?;
        ensure_settings_auto_respond(&mut connection, &mut require_exclusive)?;
        ensure_bots_generation(&mut connection, &mut require_exclusive)?;
        ensure_model_enabled(&mut connection, &mut require_exclusive)?;
        migrate_identity_to_instructions(&mut connection, &mut require_exclusive)?;
        let key = match app_key {
            Some(app_key) => Some(crypto::setup(&connection, &app_key)?),
            None => None,
        };
        ensure_providers_api_key(&mut connection, key.as_ref(), has_exclusive)?;
        ensure_providers_decision_api(&mut connection, &mut require_exclusive)?;
        ensure_providers_opencode_go(&mut connection, &mut require_exclusive)?;
        if let Some(cipher) = &key {
            crypto::migrate(&connection, cipher)?;
        }
        drop(upgrade_owner);
        Ok(Self {
            connection: Mutex::new(connection),
            key,
            _owner: owner,
            _maint: maint,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn lock_path(path: &Path) -> std::path::PathBuf {
    let mut lock = path.as_os_str().to_owned();
    lock.push(".lock");
    lock.into()
}

fn maint_lock_path(path: &Path) -> std::path::PathBuf {
    let mut lock = path.as_os_str().to_owned();
    lock.push(".maint.lock");
    lock.into()
}

fn backend(error: rusqlite::Error) -> StoreError {
    StoreError::Backend(Box::new(error))
}

/// Initializes `SCHEMA` and `pending_threads` atomically inside one immediate
/// transaction when `pending_threads` does not yet exist. On a fresh database,
/// `destinations` and `pending_threads` become visible together so concurrent
/// opens never collide or misclassify the DB as needing an exclusive upgrade.
/// On a legacy database that already has `destinations`, requires exclusive
/// ownership and runs the one-time backfill in the same transaction.
fn init_schema_and_pending_threads(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    use std::collections::{HashMap, HashSet};
    let table_exists = |conn: &Connection, name: &str| -> Result<bool, StoreError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(backend)
    };
    if table_exists(connection, "pending_threads")? {
        connection.execute_batch(SCHEMA).map_err(backend)?;
        return Ok(());
    }
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    if table_exists(&tx, "pending_threads")? {
        tx.execute_batch(SCHEMA).map_err(backend)?;
        tx.commit().map_err(backend)?;
        return Ok(());
    }
    let had_destinations = table_exists(&tx, "destinations")?;
    if had_destinations {
        require_exclusive(
            "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
        )?;
    }
    tx.execute_batch(SCHEMA).map_err(backend)?;
    tx.execute_batch(
        "CREATE TABLE pending_threads (
             destination_id INTEGER PRIMARY KEY REFERENCES destinations (id) ON DELETE CASCADE
         ) STRICT;",
    )
    .map_err(backend)?;
    if !had_destinations {
        tx.commit().map_err(backend)?;
        return Ok(());
    }
    let destinations: HashMap<u64, (String, String)> = {
        let mut stmt = tx
            .prepare("SELECT id, channel, thread FROM destinations WHERE thread != '' AND dm = 0")
            .map_err(backend)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(backend)?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, channel, thread) = row.map_err(backend)?;
            map.insert(id, (channel, thread));
        }
        map
    };
    if destinations.is_empty() {
        tx.commit().map_err(backend)?;
        return Ok(());
    }
    let mut synthesized: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut observed_threads: HashSet<(String, String)> = HashSet::new();
    let mut sent_replies: HashSet<u64> = HashSet::new();
    let mut sent_tos: HashSet<u64> = HashSet::new();
    {
        let mut stmt = tx
            .prepare(
                "SELECT i, kind, text, meta FROM memory_messages WHERE kind IN ('user', 'tool')",
            )
            .map_err(backend)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(backend)?;
        for row in rows {
            let (i, kind, text, meta) = row.map_err(backend)?;
            if kind == "user" {
                let Some(meta_str) = meta else { continue };
                let Ok(storage::Meta::Message {
                    place,
                    message,
                    to,
                    inline,
                    ..
                }) = serde_json::from_str::<storage::Meta>(&meta_str)
                else {
                    continue;
                };
                if let Some(thread) = place.thread {
                    observed_threads.insert((place.channel, thread));
                } else if !place.dm && !inline {
                    if let Some((channel, thread)) = destinations.get(&to) {
                        if channel == &place.channel && thread == &message {
                            synthesized.entry(to).or_default().push(i);
                        }
                    }
                }
            } else if kind == "tool" {
                let Some(args) = text.strip_prefix("send ") else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(args) else {
                    continue;
                };
                if let Some(id) = value
                    .get("reply")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.strip_prefix('m'))
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    sent_replies.insert(id);
                }
                if let Some(id) = value
                    .get("to")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.strip_prefix('d'))
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    sent_tos.insert(id);
                }
            }
        }
    }
    for (dest_id, msg_ids) in synthesized {
        let Some(place_key) = destinations.get(&dest_id) else {
            continue;
        };
        if observed_threads.contains(place_key)
            || sent_tos.contains(&dest_id)
            || msg_ids.iter().any(|m| sent_replies.contains(m))
        {
            continue;
        }
        tx.execute(
            "INSERT OR IGNORE INTO pending_threads (destination_id) VALUES (?1)",
            [dest_id as i64],
        )
        .map_err(backend)?;
    }
    tx.commit().map_err(backend)?;
    Ok(())
}

/// Rebuilds `interaction_modes` when the stored schema predates the `server`
/// scope and `auto` mode, copying every existing row inside one atomic
/// immediate transaction. Requires the exclusive owner lock so an older
/// running server cannot write rows that miss migration.
fn ensure_interaction_modes_v3(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'interaction_modes'",
            [],
            |row| row.get(0),
        )
        .map_err(backend)?;
    if sql.contains("'server'") && sql.contains("'auto'") {
        return Ok(());
    }
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    let sql: String = tx
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'interaction_modes'",
            [],
            |row| row.get(0),
        )
        .map_err(backend)?;
    if sql.contains("'server'") && sql.contains("'auto'") {
        tx.commit().map_err(backend)?;
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    tx.execute_batch(
        "CREATE TABLE interaction_modes_v3 (
             messenger TEXT NOT NULL,
             scope TEXT NOT NULL CHECK (scope IN ('channel', 'user', 'server')),
             guild_id TEXT NOT NULL DEFAULT '',
             target_id TEXT NOT NULL,
             mode TEXT NOT NULL CHECK (mode IN ('mention-only', 'always', 'auto')),
             CHECK ((scope IN ('channel', 'server') AND guild_id = '') OR scope = 'user'),
             PRIMARY KEY (messenger, scope, guild_id, target_id)
         ) STRICT;
         INSERT INTO interaction_modes_v3 (messenger, scope, guild_id, target_id, mode)
         SELECT messenger, scope, guild_id, target_id, mode FROM interaction_modes;
         DROP TABLE interaction_modes;
         ALTER TABLE interaction_modes_v3 RENAME TO interaction_modes;",
    )
    .map_err(backend)?;
    tx.commit().map_err(backend)?;
    Ok(())
}

/// Rebuilds `reply_modes` and `messenger_defaults` when the stored schema
/// predates the `server` reply scope and the `auto` reply mode (legacy
/// `reply_modes` keyed by `channel_id`, `decision` mode values). Copies every
/// existing row, mapping channel rows to `scope = 'channel'` and rewriting
/// `decision` to `auto`, inside one atomic immediate transaction. Requires the
/// exclusive owner lock so an older running server cannot write rows that miss
/// migration.
fn ensure_reply_modes_v2(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let table_sql = |conn: &Connection, name: &str| -> Result<String, StoreError> {
        conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .map_err(backend)
    };
    let legacy = |conn: &Connection| -> Result<(bool, bool), StoreError> {
        Ok((
            table_sql(conn, "reply_modes")?.contains("channel_id"),
            table_sql(conn, "messenger_defaults")?.contains("'decision'"),
        ))
    };
    let (migrate_reply, migrate_defaults) = legacy(connection)?;
    if !migrate_reply && !migrate_defaults {
        return Ok(());
    }
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    let (migrate_reply, migrate_defaults) = legacy(&tx)?;
    if !migrate_reply && !migrate_defaults {
        tx.commit().map_err(backend)?;
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    if migrate_reply {
        tx.execute_batch(
            "CREATE TABLE reply_modes_v2 (
                 messenger TEXT NOT NULL,
                 scope TEXT NOT NULL CHECK (scope IN ('channel', 'server')),
                 target_id TEXT NOT NULL,
                 mode TEXT NOT NULL CHECK (mode IN ('auto', 'thread', 'direct')),
                 PRIMARY KEY (messenger, scope, target_id)
             ) STRICT;
             INSERT INTO reply_modes_v2 (messenger, scope, target_id, mode)
             SELECT messenger, 'channel', channel_id,
                 CASE WHEN mode = 'decision' THEN 'auto' ELSE mode END
             FROM reply_modes;
             DROP TABLE reply_modes;
             ALTER TABLE reply_modes_v2 RENAME TO reply_modes;",
        )
        .map_err(backend)?;
    }
    if migrate_defaults {
        tx.execute_batch(
            "CREATE TABLE messenger_defaults_v2 (
                 messenger TEXT PRIMARY KEY,
                 interaction_mode TEXT NOT NULL CHECK (interaction_mode IN ('mention-only', 'always', 'auto')),
                 reply_mode TEXT NOT NULL CHECK (reply_mode IN ('auto', 'thread', 'direct'))
             ) STRICT;
             INSERT INTO messenger_defaults_v2 (messenger, interaction_mode, reply_mode)
             SELECT messenger, interaction_mode,
                 CASE WHEN reply_mode = 'decision' THEN 'auto' ELSE reply_mode END
             FROM messenger_defaults;
             DROP TABLE messenger_defaults;
             ALTER TABLE messenger_defaults_v2 RENAME TO messenger_defaults;",
        )
        .map_err(backend)?;
    }
    tx.commit().map_err(backend)?;
    Ok(())
}

/// Migrates any legacy access tables (`allowed_users`, `access_modes`,
/// `access_entries`, `channel_access_modes`, `channel_access_entries`) into
/// unified `access_defaults` + `access_rules` and drops the legacy tables
/// inside one atomic immediate transaction. Always requires the exclusive owner
/// lock when any legacy table exists so an older running server cannot race
/// with migration.
fn ensure_unified_access(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    use rusqlite::OptionalExtension;
    use std::collections::BTreeSet;
    const LEGACY_TABLES: [&str; 5] = [
        "access_modes",
        "access_entries",
        "channel_access_modes",
        "channel_access_entries",
        "allowed_users",
    ];
    let table_exists = |conn: &Connection, name: &str| -> Result<bool, StoreError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(backend)
    };
    let has_any_legacy = |conn: &Connection| -> Result<bool, StoreError> {
        for name in LEGACY_TABLES {
            if table_exists(conn, name)? {
                return Ok(true);
            }
        }
        Ok(false)
    };
    if !has_any_legacy(connection)? {
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    if !has_any_legacy(&tx)? {
        tx.commit().map_err(backend)?;
        return Ok(());
    }

    let has_access_modes = table_exists(&tx, "access_modes")?;
    let has_access_entries = table_exists(&tx, "access_entries")?;
    let has_channel_modes = table_exists(&tx, "channel_access_modes")?;
    let has_channel_entries = table_exists(&tx, "channel_access_entries")?;
    let has_allowed_users = table_exists(&tx, "allowed_users")?;

    let mut messengers = BTreeSet::new();
    for (present, table) in [
        (has_access_modes, "access_modes"),
        (has_access_entries, "access_entries"),
        (has_channel_modes, "channel_access_modes"),
        (has_channel_entries, "channel_access_entries"),
        (has_allowed_users, "allowed_users"),
    ] {
        if !present {
            continue;
        }
        let mut stmt = tx
            .prepare(&format!("SELECT DISTINCT messenger FROM {table}"))
            .map_err(backend)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(backend)?;
        for row in rows {
            messengers.insert(row.map_err(backend)?);
        }
    }

    for messenger in messengers {
        let people_mode: String = if has_access_modes {
            tx.query_row(
                "SELECT mode FROM access_modes WHERE messenger = ?1",
                [&messenger],
                |row| row.get(0),
            )
            .optional()
            .map_err(backend)?
            .unwrap_or_else(|| "whitelist".into())
        } else {
            "whitelist".into()
        };
        let channel_mode: String = if has_channel_modes {
            tx.query_row(
                "SELECT mode FROM channel_access_modes WHERE messenger = ?1",
                [&messenger],
                |row| row.get(0),
            )
            .optional()
            .map_err(backend)?
            .unwrap_or_else(|| "allow-all".into())
        } else {
            "allow-all".into()
        };
        let default_effect = if people_mode == "whitelist" || channel_mode == "whitelist" {
            "block"
        } else {
            "allow"
        };
        tx.execute(
            "INSERT OR IGNORE INTO access_defaults (messenger, effect) VALUES (?1, ?2)",
            rusqlite::params![messenger, default_effect],
        )
        .map_err(backend)?;

        // Collect active channel/server entries when present so dual-gate
        // combinations never widen access beyond the legacy AND semantics.
        let active_channel_entries: Vec<(String, String)> =
            if has_channel_entries && (channel_mode == "whitelist" || channel_mode == "blacklist") {
                let mut stmt = tx
                    .prepare(
                        "SELECT scope, target_id FROM channel_access_entries WHERE messenger = ?1 AND list = ?2",
                    )
                    .map_err(backend)?;
                let rows = stmt
                    .query_map(rusqlite::params![&messenger, &channel_mode], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(backend)?;
                rows.collect::<Result<_, _>>().map_err(backend)?
            } else {
                Vec::new()
            };

        if people_mode == "whitelist" && channel_mode == "whitelist" {
            eprintln!(
                "storage migration ({messenger}): migrated dual people+channel whitelists fail-closed to default block; DMs and cross-scope entries require explicit Access rules."
            );
            // Dual-whitelist intersection (fail-closed; never emits `everyone = allow`
            // or global user `allow` that would widen access to unlisted users/channels).
            let whitelisted_users: Vec<(String, String, String)> = if has_access_entries {
                let mut stmt = tx
                    .prepare(
                        "SELECT user_id, CASE scope WHEN 'guild' THEN 'server' ELSE scope END, scope_id
                         FROM access_entries WHERE messenger = ?1 AND list = 'whitelist' AND user_id != ''",
                    )
                    .map_err(backend)?;
                let rows = stmt
                    .query_map([&messenger], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(backend)?;
                rows.collect::<Result<_, _>>().map_err(backend)?
            } else if has_allowed_users {
                let mut stmt = tx
                    .prepare(
                        "SELECT user_id FROM allowed_users WHERE messenger = ?1 AND user_id != ''",
                    )
                    .map_err(backend)?;
                let rows = stmt
                    .query_map([&messenger], |row| {
                        Ok((row.get::<_, String>(0)?, "global".to_owned(), String::new()))
                    })
                    .map_err(backend)?;
                rows.collect::<Result<_, _>>().map_err(backend)?
            } else {
                Vec::new()
            };
            for (uid, u_scope, u_sid) in &whitelisted_users {
                for (c_scope, c_tid) in &active_channel_entries {
                    let target = match (u_scope.as_str(), c_scope.as_str()) {
                        ("global", _) => Some((c_scope.as_str(), c_tid.as_str())),
                        ("server", "server") if u_sid == c_tid => Some(("server", c_tid.as_str())),
                        ("channel", "channel") if u_sid == c_tid => {
                            Some(("channel", c_tid.as_str()))
                        }
                        // Cannot statically prove channel ∈ server without guild metadata;
                        // skip rather than widening across servers.
                        _ => None,
                    };
                    if let Some((scope, sid)) = target {
                        tx.execute(
                            "INSERT OR IGNORE INTO access_rules (messenger, user_id, scope, scope_id, effect)
                             VALUES (?1, ?2, ?3, ?4, 'allow')",
                            rusqlite::params![&messenger, uid, scope, sid],
                        )
                        .map_err(backend)?;
                    }
                }
            }
        } else {
            let has_blacklisted_channels =
                channel_mode == "blacklist" && !active_channel_entries.is_empty();
            if people_mode == "whitelist" {
                if has_access_entries {
                    // When a legacy channel/server blacklist is also active, suppress
                    // narrower `channel`-scoped user allows whose guild/parent ancestry
                    // is unknown in SQLite so a channel/thread allow can never override
                    // a blacklisted server or parent channel.
                    let sql = if has_blacklisted_channels {
                        "INSERT OR IGNORE INTO access_rules (messenger, user_id, scope, scope_id, effect)
                         SELECT messenger, user_id, CASE scope WHEN 'guild' THEN 'server' ELSE scope END, scope_id, 'allow'
                         FROM access_entries WHERE messenger = ?1 AND list = 'whitelist' AND user_id != '' AND scope != 'channel'"
                    } else {
                        "INSERT OR IGNORE INTO access_rules (messenger, user_id, scope, scope_id, effect)
                         SELECT messenger, user_id, CASE scope WHEN 'guild' THEN 'server' ELSE scope END, scope_id, 'allow'
                         FROM access_entries WHERE messenger = ?1 AND list = 'whitelist' AND user_id != ''"
                    };
                    tx.execute(sql, [&messenger]).map_err(backend)?;
                    if has_blacklisted_channels {
                        eprintln!(
                            "storage migration ({messenger}): unified access suppressed channel-scoped user allows alongside legacy channel/server blacklist (fail-closed); review Access rules in Settings."
                        );
                    }
                } else if has_allowed_users {
                    tx.execute(
                        "INSERT OR IGNORE INTO access_rules (messenger, user_id, scope, scope_id, effect)
                         SELECT messenger, user_id, 'global', '', 'allow'
                         FROM allowed_users WHERE messenger = ?1 AND user_id != ''",
                        [&messenger],
                    )
                    .map_err(backend)?;
                }
            } else if people_mode == "blacklist" && has_access_entries {
                tx.execute(
                    "INSERT OR IGNORE INTO access_rules (messenger, user_id, scope, scope_id, effect)
                     SELECT messenger, user_id, CASE scope WHEN 'guild' THEN 'server' ELSE scope END, scope_id, 'block'
                     FROM access_entries WHERE messenger = ?1 AND list = 'blacklist' AND user_id != ''",
                    [&messenger],
                )
                .map_err(backend)?;
            }

            if channel_mode == "whitelist" {
                eprintln!(
                    "storage migration ({messenger}): migrated legacy channel whitelist to default block; DMs now require a Global allow person rule."
                );
            }

            if !active_channel_entries.is_empty() {
                let chan_effect = if channel_mode == "whitelist" {
                    "allow"
                } else {
                    "block"
                };
                for (c_scope, c_tid) in &active_channel_entries {
                    tx.execute(
                        "INSERT OR IGNORE INTO access_rules (messenger, user_id, scope, scope_id, effect)
                         VALUES (?1, '', ?2, ?3, ?4)",
                        rusqlite::params![&messenger, c_scope, c_tid, chan_effect],
                    )
                    .map_err(backend)?;
                }
                // When people=whitelist & channel=blacklist, add per-user block rules
                // for whitelisted users so step 2/3 blocks beat their step 3/4 allow.
                if people_mode == "whitelist" && channel_mode == "blacklist" {
                    let whitelisted_uids: Vec<String> = {
                        let mut stmt = tx
                            .prepare(
                                "SELECT DISTINCT user_id FROM access_rules
                                 WHERE messenger = ?1 AND user_id != '' AND effect = 'allow'",
                            )
                            .map_err(backend)?;
                        let rows = stmt
                            .query_map([&messenger], |row| row.get(0))
                            .map_err(backend)?;
                        rows.collect::<Result<_, _>>().map_err(backend)?
                    };
                    for uid in &whitelisted_uids {
                        for (c_scope, c_tid) in &active_channel_entries {
                            tx.execute(
                                "INSERT INTO access_rules (messenger, user_id, scope, scope_id, effect)
                                 VALUES (?1, ?2, ?3, ?4, 'block')
                                 ON CONFLICT (messenger, user_id, scope, scope_id) DO UPDATE SET effect = 'block'",
                                rusqlite::params![&messenger, uid, c_scope, c_tid],
                            )
                            .map_err(backend)?;
                        }
                    }
                }
            }
        }
    }

    tx.execute_batch(
        "DROP TABLE IF EXISTS access_modes;
         DROP TABLE IF EXISTS access_entries;
         DROP TABLE IF EXISTS channel_access_modes;
         DROP TABLE IF EXISTS channel_access_entries;
         DROP TABLE IF EXISTS allowed_users;",
    )
    .map_err(backend)?;
    tx.commit().map_err(backend)?;
    Ok(())
}

/// Adds `settings.auto_respond` (default `1` = enabled when a utility model is
/// configured) to existing databases.
fn ensure_settings_auto_respond(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let has_column = |conn: &Connection| -> Result<bool, StoreError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('settings') WHERE name = 'auto_respond')",
            [],
            |row| row.get(0),
        )
        .map_err(backend)
    };
    if has_column(connection)? {
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    if !has_column(&tx)? {
        tx.execute_batch(
            "ALTER TABLE settings ADD COLUMN auto_respond INTEGER NOT NULL DEFAULT 1 CHECK (auto_respond IN (0, 1));",
        )
        .map_err(backend)?;
    }
    tx.commit().map_err(backend)?;
    Ok(())
}

/// Adds `bots.generation` to databases created before bot generations existed.
/// Existing rows get generation 0; `bot_sequence` starts at 1 so new and
/// recreated bots always get a fresh, never-reused generation.
fn ensure_bots_generation(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let has_column = |conn: &Connection| -> Result<bool, StoreError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('bots') WHERE name = 'generation')",
            [],
            |row| row.get(0),
        )
        .map_err(backend)
    };
    if has_column(connection)? {
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    if !has_column(&tx)? {
        tx.execute_batch("ALTER TABLE bots ADD COLUMN generation INTEGER NOT NULL DEFAULT 0;")
            .map_err(backend)?;
    }
    tx.commit().map_err(backend)?;
    Ok(())
}

/// Creates `model_enabled` (presence = enabled; models default to disabled) and,
/// on first creation only, seeds it with every model currently assigned to a
/// role slot so existing assignments keep working. Because the flag table is
/// created here rather than in SCHEMA, an existing table means the seed already
/// ran — models disabled later are never re-enabled on reopen.
fn ensure_model_enabled(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let has_table = |conn: &Connection| -> Result<bool, StoreError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'model_enabled')",
            [],
            |row| row.get(0),
        )
        .map_err(backend)
    };
    if has_table(connection)? {
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    if !has_table(&tx)? {
        tx.execute_batch(
            "CREATE TABLE model_enabled (
                 provider TEXT NOT NULL,
                 model TEXT NOT NULL,
                 PRIMARY KEY (provider, model),
                 FOREIGN KEY (provider, model) REFERENCES models (provider, id) ON DELETE CASCADE
             ) STRICT;
             INSERT OR IGNORE INTO model_enabled (provider, model)
             SELECT refs.provider, refs.model FROM (
                 SELECT default_provider AS provider, default_model AS model FROM settings
                 UNION SELECT compaction_provider, compaction_model FROM settings
                 UNION SELECT provider, model FROM utility_model
                 UNION SELECT provider, model FROM decision_model
                 UNION SELECT default_provider, default_model FROM bot_models
                 UNION SELECT compaction_provider, compaction_model FROM bot_models
                 UNION SELECT utility_provider, utility_model FROM bot_models
                 UNION SELECT decision_provider, decision_model FROM bot_models
             ) refs JOIN models ON models.provider = refs.provider AND models.id = refs.model;",
        )
        .map_err(backend)?;
    }
    tx.commit().map_err(backend)?;
    Ok(())
}

/// Copies any row verbatim from the legacy `identity` table into `instructions`
/// and drops `identity`.
fn migrate_identity_to_instructions(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let has_legacy = |conn: &Connection| -> Result<bool, StoreError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'identity')",
            [],
            |row| row.get(0),
        )
        .map_err(backend)
    };
    if !has_legacy(connection)? {
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    if !has_legacy(&tx)? {
        return Ok(());
    }
    tx.execute_batch(
        "INSERT OR REPLACE INTO instructions (id, text) SELECT id, text FROM identity;
         DROP TABLE identity;",
    )
    .map_err(backend)?;
    tx.commit().map_err(backend)?;
    Ok(())
}

/// Widens `providers.api` CHECK constraint on existing databases to allow `'decision'`.
fn ensure_providers_decision_api(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let needs_upgrade = |conn: &Connection| -> Result<bool, StoreError> {
        let has_legacy_col: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('providers') WHERE name = 'api_key_env')",
                [],
                |row| row.get(0),
            )
            .map_err(backend)?;
        if has_legacy_col {
            return Ok(false);
        }
        let sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'providers'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(sql.is_some_and(|s| !s.contains("'decision'")))
    };
    if !needs_upgrade(connection)? {
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .map_err(backend)?;
    let result = (|| -> Result<(), StoreError> {
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        if !needs_upgrade(&tx)? {
            tx.commit().map_err(backend)?;
            return Ok(());
        }
        tx.execute_batch(
            "CREATE TABLE providers_v2 (
                 id TEXT PRIMARY KEY,
                 api TEXT NOT NULL CHECK (api IN ('openai-responses', 'openai-completions', 'decision')),
                 base_url TEXT NOT NULL,
                 api_key TEXT
             ) STRICT;
             INSERT INTO providers_v2 (id, api, base_url, api_key)
             SELECT id, api, base_url, api_key FROM providers;
             DROP TABLE providers;
             ALTER TABLE providers_v2 RENAME TO providers;",
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(())
    })();
    let restore = connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(backend);
    result?;
    restore?;
    Ok(())
}

/// Widens `providers.api` CHECK constraint on existing databases to allow `'opencode-go'`.
fn ensure_providers_opencode_go(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let needs_upgrade = |conn: &Connection| -> Result<bool, StoreError> {
        let has_legacy_col: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('providers') WHERE name = 'api_key_env')",
                [],
                |row| row.get(0),
            )
            .map_err(backend)?;
        if has_legacy_col {
            return Ok(false);
        }
        let sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'providers'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(sql.is_some_and(|s| !s.contains("'opencode-go'")))
    };
    if !needs_upgrade(connection)? {
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .map_err(backend)?;
    let result = (|| -> Result<(), StoreError> {
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        if !needs_upgrade(&tx)? {
            tx.commit().map_err(backend)?;
            return Ok(());
        }
        tx.execute_batch(
            "CREATE TABLE providers_v3 (
                 id TEXT PRIMARY KEY,
                 api TEXT NOT NULL CHECK (api IN ('openai-responses', 'openai-completions', 'opencode-go', 'decision')),
                 base_url TEXT NOT NULL,
                 api_key TEXT
             ) STRICT;
             INSERT INTO providers_v3 (id, api, base_url, api_key)
             SELECT id, api, base_url, api_key FROM providers;
             DROP TABLE providers;
             ALTER TABLE providers_v3 RENAME TO providers;",
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(())
    })();
    let restore = connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(backend);
    result?;
    restore?;
    Ok(())
}

/// One-time migration from `providers.api_key_env` to encrypted `providers.api_key`.
/// Runs only on an exclusive open (`serve` / `chat`, where the runtime environment
/// holds the legacy env vars) so a read-only CLI command cannot drop `api_key_env`
/// references before the server migrates them. Keeps all foreign keys (`models`,
/// `settings`, `utility_model`) untouched via `ALTER TABLE ... ADD/DROP COLUMN`.
fn ensure_providers_api_key(
    connection: &mut Connection,
    cipher: Option<&XChaCha20Poly1305>,
    has_exclusive: bool,
) -> Result<(), StoreError> {
    ensure_providers_api_key_with_env(connection, cipher, has_exclusive, |name| {
        std::env::var(name).ok()
    })
}

fn ensure_providers_api_key_with_env(
    connection: &mut Connection,
    cipher: Option<&XChaCha20Poly1305>,
    has_exclusive: bool,
    mut lookup_env: impl FnMut(&str) -> Option<String>,
) -> Result<(), StoreError> {
    let has_column = |conn: &Connection, name: &str| -> Result<bool, StoreError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('providers') WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(backend)
    };
    if !has_column(connection, "api_key_env")? {
        return Ok(());
    }
    if !has_exclusive {
        return Err(StoreError::Backend(
            "this database needs an upgrade: run blob-1 serve once with BLOB_1_APP_KEY and legacy provider env vars set"
                .into(),
        ));
    }
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    if !has_column(&tx, "api_key_env")? {
        tx.commit().map_err(backend)?;
        return Ok(());
    }
    let existing: Vec<(String, String)> = {
        let mut stmt = tx
            .prepare("SELECT id, api_key_env FROM providers")
            .map_err(backend)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(backend)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(backend)?
    };
    let mut sealed_rows: Vec<(String, String)> = Vec::new();
    for (id, env_name) in existing {
        if env_name.is_empty() {
            continue;
        }
        let raw = lookup_env(&env_name)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                StoreError::Backend(
                    format!(
                        "cannot migrate provider '{id}': environment variable {env_name} is not set"
                    )
                    .into(),
                )
            })?;
        let cipher = cipher.ok_or_else(|| {
            StoreError::Backend("set BLOB_1_APP_KEY to migrate provider API keys".into())
        })?;
        sealed_rows.push((id, crypto::seal_with(cipher, &raw)?));
    }
    if !has_column(&tx, "api_key")? {
        tx.execute_batch("ALTER TABLE providers ADD COLUMN api_key TEXT;")
            .map_err(backend)?;
    }
    for (id, sealed) in &sealed_rows {
        tx.execute(
            "UPDATE providers SET api_key = ?2 WHERE id = ?1",
            rusqlite::params![id, sealed],
        )
        .map_err(backend)?;
    }
    tx.execute_batch("ALTER TABLE providers DROP COLUMN api_key_env;")
        .map_err(backend)?;
    tx.commit().map_err(backend)?;
    Ok(())
}

#[cfg(test)]
mod provider_key_tests {
    use super::*;
    use storage::{ConfigStore, Provider, ProviderApi, ProviderId};

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("blob-1-provider-key-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("agent.db")
    }

    #[test]
    fn provider_api_key_is_encrypted_at_rest_and_fingerprint_changes_on_rotation() {
        let path = temp_db("enc");
        let store = SqliteStorage::open(&path, Some(AppKey::new("test-key".into()))).unwrap();
        let id = ProviderId("openai".into());
        store
            .save_provider(
                Provider {
                    id: id.clone(),
                    api: ProviderApi::OpenAiResponses,
                    base_url: "https://api.openai.com/v1".into(),
                    has_api_key: true,
                },
                Some("sk-secret-123"),
            )
            .unwrap();
        let raw: Option<String> = store
            .lock()
            .query_row(
                "SELECT api_key FROM providers WHERE id = 'openai'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let raw = raw.expect("api_key stored");
        assert!(raw.starts_with("enc:v1:"));
        assert!(!raw.contains("sk-secret-123"));

        let fp1 = store.provider_key_fingerprint(&id).unwrap();
        assert!(fp1.is_some());
        assert_eq!(
            store.provider_api_key(&id).unwrap().as_deref(),
            Some("sk-secret-123")
        );
        assert!(store.providers().unwrap()[0].has_api_key);

        store.set_provider_api_key(&id, Some("sk-rotated")).unwrap();
        let fp2 = store.provider_key_fingerprint(&id).unwrap();
        assert_ne!(fp1, fp2);
        assert_eq!(
            store.provider_api_key(&id).unwrap().as_deref(),
            Some("sk-rotated")
        );

        store.set_provider_api_key(&id, None).unwrap();
        assert_eq!(store.provider_key_fingerprint(&id).unwrap(), None);
        assert_eq!(store.provider_api_key(&id).unwrap(), None);
        assert!(!store.providers().unwrap()[0].has_api_key);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn migrates_legacy_api_key_env_only_on_exclusive_open_and_preserves_models_fk() {
        let path = temp_db("legacy");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE providers (
                     id TEXT PRIMARY KEY,
                     api TEXT NOT NULL CHECK (api IN ('openai-responses', 'openai-completions')),
                     base_url TEXT NOT NULL,
                     api_key_env TEXT NOT NULL
                 ) STRICT;
                 CREATE TABLE models (
                     provider TEXT NOT NULL REFERENCES providers (id) ON DELETE CASCADE,
                     id TEXT NOT NULL,
                     context_window INTEGER,
                     context_limit INTEGER CHECK (context_limit > 0),
                     PRIMARY KEY (provider, id)
                 ) STRICT;
                 INSERT INTO providers VALUES ('cpa1', 'openai-responses', 'http://127.0.0.1:8320/v1', 'TEST_LEGACY_PROVIDER_KEY');
                 INSERT INTO models VALUES ('cpa1', 'gpt-6-luna', 1000000, NULL);",
            )
            .unwrap();
        }
        // Shared open refuses to drop `api_key_env` without running exclusive serve first.
        let shared_err = SqliteStorage::open_shared(&path, Some(AppKey::new("test-key".into())));
        assert!(shared_err.is_err());

        // Exclusive migration aborts cleanly if a legacy env var is missing, leaving api_key_env intact.
        {
            let mut conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            let cipher = crypto::setup(&conn, &AppKey::new("test-key".into())).unwrap();
            let missing_err =
                ensure_providers_api_key_with_env(&mut conn, Some(&cipher), true, |_| None)
                    .unwrap_err()
                    .to_string();
            assert!(missing_err.contains("cpa1"));
            assert!(missing_err.contains("TEST_LEGACY_PROVIDER_KEY"));
            // Exclusive migration seals the injected env lookup without mutating process env.
            ensure_providers_api_key_with_env(&mut conn, Some(&cipher), true, |name| {
                (name == "TEST_LEGACY_PROVIDER_KEY").then(|| "sk-from-legacy-env".into())
            })
            .unwrap();
        }

        let store = SqliteStorage::open(&path, Some(AppKey::new("test-key".into()))).unwrap();
        let providers = store.providers().unwrap();
        assert_eq!(providers.len(), 1);
        assert!(providers[0].has_api_key);
        assert_eq!(
            store
                .provider_api_key(&ProviderId("cpa1".into()))
                .unwrap()
                .as_deref(),
            Some("sk-from-legacy-env")
        );
        let models = store.models(&ProviderId("cpa1".into())).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-6-luna");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn migrates_legacy_identity_table_to_instructions_verbatim() {
        let dir = std::env::temp_dir().join(format!(
            "blob-1-migrate-instructions-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.db");
        let legacy_text = "Custom instructions\nwith trailing newline\n";

        {
            let store = SqliteStorage::open(&path, None).unwrap();
            let conn = store.lock();
            conn.execute_batch(
                "DROP TABLE instructions;
                 CREATE TABLE identity (id INTEGER PRIMARY KEY CHECK (id = 1), text TEXT NOT NULL) STRICT;",
            )
            .unwrap();
            conn.execute("INSERT INTO identity (id, text) VALUES (1, ?1)", [legacy_text])
                .unwrap();
        }

        let store = SqliteStorage::open(&path, None).unwrap();
        assert_eq!(store.instructions().unwrap().as_deref(), Some(legacy_text));
        let legacy_exists: bool = store
            .lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'identity')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!legacy_exists);
        // Downgrade + edit + re-upgrade: legacy row overwrites stale instructions row.
        let newer_legacy = "Updated during rollback\n";
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE identity (id INTEGER PRIMARY KEY CHECK (id = 1), text TEXT NOT NULL) STRICT;",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO identity (id, text) VALUES (1, ?1)",
                [newer_legacy],
            )
            .unwrap();
        }
        drop(store);
        let store = SqliteStorage::open(&path, None).unwrap();
        assert_eq!(store.instructions().unwrap().as_deref(), Some(newer_legacy));
        // Downgrade without editing (old build recreates empty `identity` table) + re-upgrade:
        // must preserve saved `instructions` rather than wiping them.
        {
            let conn = store.lock();
            conn.execute_batch(
                "CREATE TABLE identity (id INTEGER PRIMARY KEY CHECK (id = 1), text TEXT NOT NULL) STRICT;",
            )
            .unwrap();
        }
        drop(store);
        let store = SqliteStorage::open(&path, None).unwrap();
        assert_eq!(store.instructions().unwrap().as_deref(), Some(newer_legacy));
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
