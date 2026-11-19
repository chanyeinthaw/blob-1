use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::Connection;
use storage::StoreError;

mod session;
mod workspace;

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY
) STRICT;

CREATE TABLE IF NOT EXISTS messages (
    session_id TEXT NOT NULL REFERENCES sessions (id),
    position INTEGER NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool_call', 'tool_result')),
    content TEXT NOT NULL,
    tool_call_id TEXT,
    tool_name TEXT,
    PRIMARY KEY (session_id, position),
    CHECK ((role IN ('tool_call', 'tool_result')) = (tool_call_id IS NOT NULL AND tool_name IS NOT NULL))
) STRICT;

CREATE TABLE IF NOT EXISTS workspace_nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL CHECK (kind IN ('file', 'directory')),
    mode INTEGER NOT NULL,
    size INTEGER NOT NULL CHECK (size >= 0),
    modified_ns INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS workspace_entries (
    parent INTEGER NOT NULL REFERENCES workspace_nodes (id),
    name TEXT NOT NULL,
    node INTEGER NOT NULL UNIQUE REFERENCES workspace_nodes (id),
    PRIMARY KEY (parent, name)
) STRICT;

CREATE TABLE IF NOT EXISTS workspace_chunks (
    node INTEGER NOT NULL REFERENCES workspace_nodes (id),
    idx INTEGER NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (node, idx)
) STRICT, WITHOUT ROWID;

INSERT OR IGNORE INTO workspace_nodes (id, kind, mode, size, modified_ns)
VALUES (1, 'directory', 493, 0, 0);

DELETE FROM workspace_chunks WHERE node IN (
    SELECT id FROM workspace_nodes
    WHERE id != 1 AND id NOT IN (SELECT node FROM workspace_entries)
);

DELETE FROM workspace_nodes
WHERE id != 1 AND id NOT IN (SELECT node FROM workspace_entries);
";

pub struct SqliteStorage {
    connection: Mutex<Connection>,
}

impl SqliteStorage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
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
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn backend(error: rusqlite::Error) -> StoreError {
    StoreError::Backend(Box::new(error))
}
