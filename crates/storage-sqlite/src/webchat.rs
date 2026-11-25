use rusqlite::{params, OptionalExtension};
use storage::{StoreError, WebChatStore, WebMessage};

use crate::{backend, SqliteStorage};

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebMessage> {
    Ok(WebMessage {
        id: row.get(0)?,
        bot: row.get::<_, i64>(1)? != 0,
        text: row.get(2)?,
        created_at: row.get(3)?,
    })
}

impl WebChatStore for SqliteStorage {
    fn add_web_message(&self, bot: bool, text: &str, now: i64) -> Result<i64, StoreError> {
        let connection = self.lock();
        connection
            .execute(
                "INSERT INTO web_messages (bot, text, created_at) VALUES (?1, ?2, ?3)",
                params![i64::from(bot), text, now],
            )
            .map_err(backend)?;
        Ok(connection.last_insert_rowid())
    }

    fn web_message(&self, id: i64) -> Result<Option<WebMessage>, StoreError> {
        self.lock()
            .query_row(
                "SELECT id, bot, text, created_at FROM web_messages WHERE id = ?1",
                [id],
                row,
            )
            .optional()
            .map_err(backend)
    }

    fn web_messages(&self, before: Option<i64>, limit: u32) -> Result<Vec<WebMessage>, StoreError> {
        let mut messages = self
            .lock()
            .prepare(
                "SELECT id, bot, text, created_at FROM web_messages
                 WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(backend)?
            .query_map(params![before.unwrap_or(i64::MAX), limit], row)
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        messages.reverse();
        Ok(messages)
    }
}
