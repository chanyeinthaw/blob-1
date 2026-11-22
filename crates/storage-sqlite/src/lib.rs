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
CREATE TABLE IF NOT EXISTS interaction_modes (messenger TEXT NOT NULL, scope TEXT NOT NULL CHECK (scope IN ('channel', 'user')), guild_id TEXT NOT NULL DEFAULT '', target_id TEXT NOT NULL, mode TEXT NOT NULL CHECK (mode IN ('mention-only', 'always')), CHECK ((scope = 'channel' AND guild_id = '') OR scope = 'user'), PRIMARY KEY (messenger, scope, guild_id, target_id)) STRICT;
CREATE TABLE IF NOT EXISTS reply_modes (messenger TEXT NOT NULL, channel_id TEXT NOT NULL, mode TEXT NOT NULL CHECK (mode IN ('decision', 'thread', 'direct')), PRIMARY KEY (messenger, channel_id)) STRICT;
CREATE TABLE IF NOT EXISTS seen_threads (messenger TEXT NOT NULL, channel TEXT NOT NULL, thread TEXT NOT NULL, PRIMARY KEY (messenger, channel, thread)) STRICT;

CREATE TABLE IF NOT EXISTS identity (id INTEGER PRIMARY KEY CHECK (id = 1), text TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS public_url (id INTEGER PRIMARY KEY CHECK (id = 1), url TEXT NOT NULL) STRICT;

CREATE TABLE IF NOT EXISTS providers (
    id TEXT PRIMARY KEY,
    api TEXT NOT NULL CHECK (api IN ('openai-responses', 'openai-completions')),
    base_url TEXT NOT NULL,
    api_key_env TEXT NOT NULL
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
        // Initialize schema + `pending_threads` atomically inside one immediate
        // transaction when `pending_threads` is absent, so a concurrent shared
        // open on a fresh DB can never observe `destinations` without
        // `pending_threads`, and an interrupted legacy backfill rolls back cleanly.
        init_schema_and_pending_threads(&mut connection, owner.is_some())?;
        for table in ["destinations", "allowed_users"] {
            let old_column: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = 'interface')",
                    [table],
                    |row| row.get(0),
                )
                .map_err(backend)?;
            if old_column {
                if owner.is_none() {
                    return Err(StoreError::Backend(
                        "this database needs an upgrade: stop agent serve and run any other agent command once".into(),
                    ));
                }
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
            if owner.is_none() {
                return Err(StoreError::Backend(
                    "this database needs an upgrade: restart blob-1 serve or run an exclusive command once".into(),
                ));
            }
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
        let key = match app_key {
            Some(app_key) => Some(crypto::setup(&connection, &app_key)?),
            None => None,
        };
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
    exclusive: bool,
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
    if had_destinations && !exclusive {
        return Err(StoreError::Backend(
            "this database needs an upgrade: restart blob-1 serve or run an exclusive command once".into(),
        ));
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
