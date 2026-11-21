use crate::{backend, SqliteStorage};
use rusqlite::{params, OptionalExtension};
use storage::{StoreError, WebAuthStore};

impl WebAuthStore for SqliteStorage {
    fn web_password(&self) -> Result<Option<String>, StoreError> {
        self.lock()
            .query_row("SELECT hash FROM web_password WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(backend)
    }
    fn set_web_password(&self, hash: &str) -> Result<(), StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(backend)?;
        tx.execute("INSERT INTO web_password VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET hash = excluded.hash", [hash]).map_err(backend)?;
        tx.execute("DELETE FROM web_sessions", [])
            .map_err(backend)?;
        tx.commit().map_err(backend)
    }
    fn create_web_session(
        &self,
        hash: &[u8],
        expires: i64,
        password: &str,
    ) -> Result<bool, StoreError> {
        let inserted = self
            .lock()
            .execute(
                "INSERT INTO web_sessions SELECT ?1, ?2, ?3 WHERE EXISTS(SELECT 1 FROM web_password WHERE id = 1 AND hash = ?4)",
                params![hash, expires, expires - 30 * 86400, password],
            )
            .map_err(backend)?;
        Ok(inserted == 1)
    }
    fn authenticate_web_session(
        &self,
        hash: &[u8],
        now: i64,
        expires: i64,
    ) -> Result<bool, StoreError> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM web_sessions WHERE hash = ?1 AND expires <= ?2",
            params![hash, now],
        )
        .map_err(backend)?;
        let valid: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM web_sessions WHERE hash = ?1 AND expires > ?2)",
                params![hash, now],
                |r| r.get(0),
            )
            .map_err(backend)?;
        if valid {
            conn.execute("UPDATE web_sessions SET expires = ?2, renewed = ?3 WHERE hash = ?1 AND renewed <= ?3 - 3600", params![hash, expires, now]).map_err(backend)?;
        }
        Ok(valid)
    }
    fn delete_web_session(&self, hash: &[u8]) -> Result<(), StoreError> {
        self.lock()
            .execute("DELETE FROM web_sessions WHERE hash = ?1", [hash])
            .map_err(backend)?;
        Ok(())
    }
}
