use rusqlite::{params, OptionalExtension};
use storage::{MessengerStore, Place, StoreError};

use crate::{backend, SqliteStorage};

impl MessengerStore for SqliteStorage {
    fn destination(&self, messenger: &str, place: &Place) -> Result<u64, StoreError> {
        let thread = place.thread.as_deref().unwrap_or("");
        let connection = self.lock();
        connection
            .execute(
                "INSERT OR IGNORE INTO destinations (messenger, channel, thread, dm) VALUES (?1, ?2, ?3, ?4)",
                params![messenger, place.channel, thread, place.dm],
            )
            .map_err(backend)?;
        connection
            .query_row(
                "SELECT id FROM destinations WHERE messenger = ?1 AND channel = ?2 AND thread = ?3",
                params![messenger, place.channel, thread],
                |row| row.get::<_, i64>(0),
            )
            .map_err(backend)
            .map(|id| id as u64)
    }

    fn place(&self, id: u64) -> Result<Option<(String, Place)>, StoreError> {
        self.lock()
            .query_row(
                "SELECT messenger, channel, thread, dm FROM destinations WHERE id = ?1",
                [id as i64],
                |row| {
                    let thread: String = row.get(2)?;
                    Ok((
                        row.get(0)?,
                        Place {
                            channel: row.get(1)?,
                            thread: (!thread.is_empty()).then_some(thread),
                            dm: row.get(3)?,
                        },
                    ))
                },
            )
            .optional()
            .map_err(backend)
    }

    fn allowed_users(&self, messenger: &str) -> Result<Vec<String>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT user_id FROM allowed_users WHERE messenger = ?1 ORDER BY user_id")
            .map_err(backend)?;
        let rows = statement
            .query_map([messenger], |row| row.get(0))
            .map_err(backend)?;
        rows.collect::<Result<_, _>>().map_err(backend)
    }

    fn allow_user(&self, messenger: &str, user: &str) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT OR IGNORE INTO allowed_users (messenger, user_id) VALUES (?1, ?2)",
                params![messenger, user],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn disallow_user(&self, messenger: &str, user: &str) -> Result<bool, StoreError> {
        self.lock()
            .execute(
                "DELETE FROM allowed_users WHERE messenger = ?1 AND user_id = ?2",
                params![messenger, user],
            )
            .map(|n| n > 0)
            .map_err(backend)
    }
}
