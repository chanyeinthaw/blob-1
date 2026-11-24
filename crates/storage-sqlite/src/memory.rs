use rusqlite::{params, OptionalExtension};
use storage::{
    MemoryKind, MemoryMessage, MemoryNode, MemoryStore, MemoryView, StoreError, ViewChange,
    ViewName,
};

use crate::{backend, SqliteStorage};

fn sql_integer(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|error| StoreError::Backend(Box::new(error)))
}

fn unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn view_name(name: ViewName) -> &'static str {
    match name {
        ViewName::Chat => "chat",
        ViewName::Seen => "seen",
    }
}

fn parse_memory_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryMessage> {
    let kind: String = row.get(1)?;
    let kind = match kind.as_str() {
        "user" => MemoryKind::User,
        "assistant" => MemoryKind::Assistant,
        "tool" => MemoryKind::Tool,
        "echo" => MemoryKind::Echo,
        "work" => MemoryKind::Work,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                format!("invalid memory kind: {kind}").into(),
            ));
        }
    };
    Ok(MemoryMessage {
        i: unsigned(row, 0)?,
        kind,
        meta: row
            .get::<_, Option<String>>(6)?
            .map(|value| {
                serde_json::from_str(&value).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .transpose()?,
        text: row.get(2)?,
        size: unsigned(row, 3)?,
        date: row.get(4)?,
        more: row.get(5)?,
    })
}

impl MemoryStore for SqliteStorage {
    fn append_messages(
        &self,
        rows: &[MemoryMessage],
        changes: &[ViewChange],
    ) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let tx = connection.transaction().map_err(backend)?;
        let next: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(i), -1) + 1 FROM memory_messages",
                [],
                |row| row.get(0),
            )
            .map_err(backend)?;
        for (next, row) in (next..).zip(rows.iter()) {
            if sql_integer(row.i)? != next {
                return Err(StoreError::Backend("memory message id mismatch".into()));
            }
            let meta = row
                .meta
                .as_ref()
                .map(|m| -> Result<String, StoreError> {
                    let mut val = serde_json::to_value(m)
                        .map_err(|error| StoreError::Backend(Box::new(error)))?;
                    storage::redact_json_value(&mut val);
                    serde_json::to_string(&val)
                        .map_err(|error| StoreError::Backend(Box::new(error)))
                })
                .transpose()?;
            let text = storage::redact_callback_tokens(&row.text);
            tx.execute("INSERT INTO memory_messages (i, kind, text, size, date, more, meta) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![next, row.kind.as_str(), text.as_ref(), sql_integer(row.size)?, row.date, row.more, meta]).map_err(backend)?;
        }
        save_views(&tx, changes)?;
        tx.commit().map_err(backend)
    }

    fn message(&self, i: u64) -> Result<Option<MemoryMessage>, StoreError> {
        self.lock()
            .query_row(
                "SELECT i, kind, text, size, date, more, meta FROM memory_messages WHERE i = ?1",
                [sql_integer(i)?],
                parse_memory_row,
            )
            .optional()
            .map_err(backend)
    }

    fn message_count(&self) -> Result<u64, StoreError> {
        self.lock()
            .query_row("SELECT COUNT(*) FROM memory_messages", [], |row| {
                unsigned(row, 0)
            })
            .map_err(backend)
    }

    fn node(&self, l: u32, i: u64) -> Result<Option<MemoryNode>, StoreError> {
        self.lock()
            .query_row(
                "SELECT l, i, text, size FROM memory_nodes WHERE l = ?1 AND i = ?2",
                params![l, sql_integer(i)?],
                |row| {
                    Ok(MemoryNode {
                        l: row.get(0)?,
                        i: unsigned(row, 1)?,
                        text: row.get(2)?,
                        size: unsigned(row, 3)?,
                    })
                },
            )
            .optional()
            .map_err(backend)
    }

    fn built_nodes(&self) -> Result<Vec<(u32, u64)>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT l, i FROM memory_nodes ORDER BY l, i")
            .map_err(backend)?;
        let nodes = statement
            .query_map([], |row| Ok((row.get(0)?, unsigned(row, 1)?)))
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        Ok(nodes)
    }

    fn put_nodes(&self, nodes: &[MemoryNode], changes: &[ViewChange]) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let tx = connection.transaction().map_err(backend)?;
        for node in nodes {
            let text = storage::redact_callback_tokens(&node.text);
            tx.execute(
                "INSERT INTO memory_nodes (l, i, text, size) VALUES (?1, ?2, ?3, ?4)",
                params![
                    node.l,
                    sql_integer(node.i)?,
                    text.as_ref(),
                    sql_integer(node.size)?
                ],
            )
            .map_err(backend)?;
        }
        save_views(&tx, changes)?;
        tx.commit().map_err(backend)
    }

    fn view(&self, name: ViewName) -> Result<Option<MemoryView>, StoreError> {
        let connection = self.lock();
        let batch: Option<bool> = connection
            .query_row(
                "SELECT batch FROM memory_views WHERE name = ?1",
                [view_name(name)],
                |row| row.get(0),
            )
            .optional()
            .map_err(backend)?;
        let Some(batch) = batch else { return Ok(None) };
        let mut statement = connection
            .prepare("SELECT l, i FROM memory_view_parts WHERE view = ?1 ORDER BY start")
            .map_err(backend)?;
        let parts = statement
            .query_map([view_name(name)], |row| {
                Ok((row.get(0)?, unsigned(row, 1)?))
            })
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        Ok(Some(MemoryView { parts, batch }))
    }
}

fn part_start(l: u32, i: u64) -> Result<i64, StoreError> {
    let start = i
        .checked_shl(l)
        .filter(|&start| (start >> l) == i)
        .ok_or_else(|| StoreError::Backend("invalid memory part".into()))?;
    sql_integer(start)
}

fn save_views(tx: &rusqlite::Transaction<'_>, changes: &[ViewChange]) -> Result<(), StoreError> {
    for change in changes {
        let name = view_name(change.view);
        tx.execute("INSERT INTO memory_views (name, batch) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET batch = CASE WHEN ?3 THEN excluded.batch ELSE memory_views.batch END", params![name, change.batch.unwrap_or(false), change.batch.is_some()]).map_err(backend)?;
        for &(l, i) in &change.removed {
            tx.execute("DELETE FROM memory_view_parts WHERE view = ?1 AND start = ?2 AND l = ?3 AND i = ?4", params![name, part_start(l, i)?, l, sql_integer(i)?]).map_err(backend)?;
        }
        for &(l, i) in &change.added {
            tx.execute(
                "INSERT INTO memory_view_parts (view, start, l, i) VALUES (?1, ?2, ?3, ?4)",
                params![name, part_start(l, i)?, l, sql_integer(i)?],
            )
            .map_err(backend)?;
        }
    }
    Ok(())
}
