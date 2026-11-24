use rusqlite::{params, OptionalExtension};
use storage::{SecretInfo, SecretRequest, SecretStore, StoreError};

use crate::{backend, SqliteStorage};

impl SecretStore for SqliteStorage {
    fn secrets(&self) -> Result<Vec<SecretInfo>, StoreError> {
        self.lock()
            .prepare("SELECT name, created_at, updated_at FROM secrets ORDER BY name")
            .map_err(backend)?
            .query_map([], |row| {
                Ok(SecretInfo {
                    name: row.get(0)?,
                    created_at: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            })
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)
    }

    fn secret_value(&self, name: &str) -> Result<Option<String>, StoreError> {
        let stored: Option<String> = self
            .lock()
            .query_row("SELECT value FROM secrets WHERE name = ?1", [name], |row| {
                row.get(0)
            })
            .optional()
            .map_err(backend)?;
        stored.map(|stored| self.unseal(&stored)).transpose()
    }

    fn set_secret(&self, name: &str, value: &str, now: i64) -> Result<(), StoreError> {
        let sealed = self.seal(value)?;
        self.lock()
            .execute(
                "INSERT INTO secrets (name, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT (name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![name, sealed, now],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn remove_secret(&self, name: &str) -> Result<bool, StoreError> {
        let removed = self
            .lock()
            .execute("DELETE FROM secrets WHERE name = ?1", [name])
            .map_err(backend)?;
        Ok(removed > 0)
    }

    fn create_secret_request(
        &self,
        token_hash: &[u8],
        name: &str,
        label: Option<&str>,
        expires_at: i64,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(backend)?;
        tx.execute("DELETE FROM secret_requests WHERE expires_at <= ?1", [now])
            .map_err(backend)?;
        tx.execute(
            "INSERT INTO secret_requests (token_hash, name, label, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![token_hash, name, label, expires_at, now],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)
    }

    fn secret_request(
        &self,
        token_hash: &[u8],
        now: i64,
    ) -> Result<Option<SecretRequest>, StoreError> {
        self.lock()
            .query_row(
                "SELECT name, label, expires_at FROM secret_requests
                 WHERE token_hash = ?1 AND expires_at > ?2",
                params![token_hash, now],
                |row| {
                    Ok(SecretRequest {
                        name: row.get(0)?,
                        label: row.get(1)?,
                        expires_at: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(backend)
    }

    fn fulfill_secret_request(
        &self,
        token_hash: &[u8],
        value: &str,
        now: i64,
    ) -> Result<Option<String>, StoreError> {
        let sealed = self.seal(value)?;
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        let name: Option<String> = tx
            .query_row(
                "DELETE FROM secret_requests WHERE token_hash = ?1 AND expires_at > ?2
                 RETURNING name",
                params![token_hash, now],
                |row| row.get(0),
            )
            .optional()
            .map_err(backend)?;
        let Some(name) = name else {
            return Ok(None);
        };
        tx.execute(
            "INSERT INTO secrets (name, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![&name, sealed, now],
        )
        .map_err(backend)?;
        // The event names the secret but never carries its value.
        tx.execute(
            "INSERT INTO pending_events (source, name, for_token, payload, created_at)
             VALUES ('secret', ?1, NULL, ?2, ?3)",
            params![
                &name,
                format!(
                    "The user provided secret \"{name}\". A task can read it with the secret tool."
                ),
                now
            ],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(Some(name))
    }
}
