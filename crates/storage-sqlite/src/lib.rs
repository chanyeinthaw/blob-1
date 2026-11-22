use std::fs::{File, TryLockError};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use chacha20poly1305::XChaCha20Poly1305;
use rusqlite::Connection;
use storage::StoreError;

mod checkin;
mod config;
mod crypto;
mod mcp;
mod memory;
mod messenger;
mod schedule;
mod session;
mod skill;
mod watch;
mod web_auth;

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
CREATE TABLE IF NOT EXISTS allowed_users (messenger TEXT NOT NULL, user_id TEXT NOT NULL, PRIMARY KEY (messenger, user_id)) STRICT;
CREATE TABLE IF NOT EXISTS access_modes (messenger TEXT PRIMARY KEY, mode TEXT NOT NULL CHECK (mode IN ('allow-all', 'whitelist', 'blacklist'))) STRICT;
CREATE TABLE IF NOT EXISTS interaction_modes (messenger TEXT NOT NULL, scope TEXT NOT NULL CHECK (scope IN ('channel', 'user')), guild_id TEXT NOT NULL DEFAULT '', target_id TEXT NOT NULL, mode TEXT NOT NULL CHECK (mode IN ('mention-only', 'always')), CHECK ((scope = 'channel' AND guild_id = '') OR scope = 'user'), PRIMARY KEY (messenger, scope, guild_id, target_id)) STRICT;
CREATE TABLE IF NOT EXISTS reply_modes (messenger TEXT NOT NULL, channel_id TEXT NOT NULL, mode TEXT NOT NULL CHECK (mode IN ('decision', 'thread', 'direct')), PRIMARY KEY (messenger, channel_id)) STRICT;
CREATE TABLE IF NOT EXISTS seen_threads (messenger TEXT NOT NULL, channel TEXT NOT NULL, thread TEXT NOT NULL, PRIMARY KEY (messenger, channel, thread)) STRICT;

CREATE TABLE IF NOT EXISTS identity (id INTEGER PRIMARY KEY CHECK (id = 1), text TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS public_url (id INTEGER PRIMARY KEY CHECK (id = 1), url TEXT NOT NULL) STRICT;

CREATE TABLE IF NOT EXISTS providers (
    id TEXT PRIMARY KEY,
    api TEXT NOT NULL CHECK (api IN ('openai-responses', 'openai-completions')),
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
                "CREATE TABLE interaction_modes_v2 (
                     messenger TEXT NOT NULL,
                     scope TEXT NOT NULL CHECK (scope IN ('channel', 'user')),
                     guild_id TEXT NOT NULL DEFAULT '',
                     target_id TEXT NOT NULL,
                     mode TEXT NOT NULL CHECK (mode IN ('mention-only', 'always')),
                     CHECK ((scope = 'channel' AND guild_id = '') OR scope = 'user'),
                     PRIMARY KEY (messenger, scope, guild_id, target_id)
                 ) STRICT;
                 INSERT INTO interaction_modes_v2 (messenger, scope, guild_id, target_id, mode)
                 SELECT messenger, scope, '', target_id, mode FROM interaction_modes;
                 DROP TABLE interaction_modes;
                 ALTER TABLE interaction_modes_v2 RENAME TO interaction_modes;",
            )
            .map_err(backend)?;
            tx.commit().map_err(backend)?;
        }
        ensure_access_entries(&mut connection, &mut require_exclusive)?;
        ensure_settings_auto_respond(&mut connection, &mut require_exclusive)?;
        drop(upgrade_owner);
        let key = match app_key {
            Some(app_key) => Some(crypto::setup(&connection, &app_key)?),
            None => None,
        };
        ensure_providers_api_key(&mut connection, key.as_ref(), has_exclusive)?;
        if let Some(cipher) = &key {
            crypto::migrate(&connection, cipher)?;
        }
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
    let had_allowed_users = table_exists(&tx, "allowed_users")?;
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
        if !had_allowed_users {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS access_entries (
                     messenger TEXT NOT NULL,
                     list TEXT NOT NULL CHECK (list IN ('whitelist', 'blacklist')),
                     scope TEXT NOT NULL CHECK (scope IN ('global', 'guild', 'channel')),
                     scope_id TEXT NOT NULL DEFAULT '',
                     user_id TEXT NOT NULL,
                     CHECK ((scope = 'global' AND scope_id = '') OR (scope IN ('guild', 'channel') AND scope_id != '')),
                     PRIMARY KEY (messenger, list, scope, scope_id, user_id)
                 ) STRICT;",
            )
            .map_err(backend)?;
        }
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
            .prepare("SELECT i, kind, text, meta FROM memory_messages WHERE kind IN ('user', 'tool')")
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

/// Creates `access_entries` on legacy databases (where it wasn't created during
/// fresh-DB initialization) and migrates any existing `allowed_users` rows into
/// the global whitelist inside one atomic immediate transaction. Always requires
/// the exclusive owner lock (even if `allowed_users` is empty) so an older
/// running server cannot write legacy rows that miss migration.
fn ensure_access_entries(
    connection: &mut Connection,
    require_exclusive: &mut impl FnMut(&'static str) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let table_exists = |conn: &Connection, name: &str| -> Result<bool, StoreError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(backend)
    };
    if table_exists(connection, "access_entries")? {
        return Ok(());
    }
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(backend)?;
    if table_exists(&tx, "access_entries")? {
        tx.commit().map_err(backend)?;
        return Ok(());
    }
    require_exclusive(
        "this database needs an upgrade: restart blob-1 serve or run an exclusive command once",
    )?;
    tx.execute_batch(
        "CREATE TABLE access_entries (
             messenger TEXT NOT NULL,
             list TEXT NOT NULL CHECK (list IN ('whitelist', 'blacklist')),
             scope TEXT NOT NULL CHECK (scope IN ('global', 'guild', 'channel')),
             scope_id TEXT NOT NULL DEFAULT '',
             user_id TEXT NOT NULL,
             CHECK ((scope = 'global' AND scope_id = '') OR (scope IN ('guild', 'channel') AND scope_id != '')),
             PRIMARY KEY (messenger, list, scope, scope_id, user_id)
         ) STRICT;
         INSERT OR IGNORE INTO access_entries (messenger, list, scope, scope_id, user_id)
         SELECT messenger, 'whitelist', 'global', '', user_id FROM allowed_users;",
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
        let dir = std::env::temp_dir().join(format!(
            "blob-1-provider-key-{name}-{}",
            std::process::id()
        ));
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
            .query_row("SELECT api_key FROM providers WHERE id = 'openai'", [], |r| {
                r.get(0)
            })
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
}
