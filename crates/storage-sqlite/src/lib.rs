use std::fs::{File, TryLockError};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::Connection;
use storage::StoreError;

mod config;
mod memory;
mod session;
mod skill;

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS memory_messages (i INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK (kind IN ('user','assistant','tool','echo','work')), text TEXT NOT NULL, size INTEGER NOT NULL, date TEXT NOT NULL, more INTEGER NOT NULL CHECK (more IN (0,1))) STRICT;
CREATE TABLE IF NOT EXISTS memory_nodes (l INTEGER NOT NULL, i INTEGER NOT NULL, text TEXT NOT NULL, size INTEGER NOT NULL, PRIMARY KEY (l, i)) STRICT;
CREATE TABLE IF NOT EXISTS memory_views (name TEXT PRIMARY KEY CHECK (name IN ('chat','seen')), batch INTEGER NOT NULL CHECK (batch IN (0,1))) STRICT;
CREATE TABLE IF NOT EXISTS memory_view_parts (view TEXT NOT NULL CHECK (view IN ('chat','seen')), start INTEGER NOT NULL, l INTEGER NOT NULL, i INTEGER NOT NULL, PRIMARY KEY (view, start)) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY
) STRICT;

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

CREATE TABLE IF NOT EXISTS skills (name TEXT PRIMARY KEY, description TEXT NOT NULL, hash TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS skill_files (skill TEXT NOT NULL REFERENCES skills (name) ON DELETE CASCADE, path TEXT NOT NULL, mode INTEGER NOT NULL, content BLOB NOT NULL, PRIMARY KEY (skill, path)) STRICT;

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
    _owner: File,
}

impl SqliteStorage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
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
        let connection = Connection::open(path).map_err(backend)?;
        connection
            .pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
            .map_err(backend)?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(backend)?;
        connection.execute_batch(SCHEMA).map_err(backend)?;
        Ok(Self {
            connection: Mutex::new(connection),
            _owner: owner,
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

fn backend(error: rusqlite::Error) -> StoreError {
    StoreError::Backend(Box::new(error))
}
