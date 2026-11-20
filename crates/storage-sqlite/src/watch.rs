use crate::{backend, SqliteStorage};
use rusqlite::params;
use storage::{StoreError, Watch, WatchStore};

impl WatchStore for SqliteStorage {
    fn add_watch(&self, watch: &Watch) -> Result<u64, StoreError> {
        let connection = self.lock();
        connection
            .execute(
                "INSERT INTO watches (text, to_dest, reply, until) VALUES (?1, ?2, ?3, ?4)",
                params![
                    watch.text,
                    watch.to_dest.map(|id| id as i64),
                    watch.reply,
                    watch.until
                ],
            )
            .map_err(backend)?;
        Ok(connection.last_insert_rowid() as u64)
    }
    fn watches(&self) -> Result<Vec<Watch>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT id, text, to_dest, reply, until FROM watches ORDER BY id")
            .map_err(backend)?;
        let rows = statement
            .query_map([], |row| {
                Ok(Watch {
                    id: row.get::<_, i64>(0)? as u64,
                    text: row.get(1)?,
                    to_dest: row.get::<_, Option<i64>>(2)?.map(|id| id as u64),
                    reply: row.get(3)?,
                    until: row.get(4)?,
                })
            })
            .map_err(backend)?;
        rows.collect::<Result<_, _>>().map_err(backend)
    }
    fn remove_watch(&self, id: u64) -> Result<bool, StoreError> {
        self.lock()
            .execute("DELETE FROM watches WHERE id = ?1", [id as i64])
            .map(|n| n > 0)
            .map_err(backend)
    }
}
