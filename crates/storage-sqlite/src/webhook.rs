use rusqlite::{params, OptionalExtension};
use storage::{StoreError, Webhook, WebhookStore};

use crate::{backend, SqliteStorage};

type RawWebhookRow = (String, String, String, i64, i64, Option<i64>);

fn raw_webhook(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawWebhookRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

impl SqliteStorage {
    fn hydrate_webhook(&self, row: RawWebhookRow) -> Result<Webhook, StoreError> {
        let (id, name, stored_token, enabled, created_at, last_triggered_at) = row;
        Ok(Webhook {
            id,
            name,
            token: self.unseal(&stored_token)?,
            enabled: enabled != 0,
            created_at,
            last_triggered_at,
        })
    }
}

impl WebhookStore for SqliteStorage {
    fn webhooks(&self) -> Result<Vec<Webhook>, StoreError> {
        let rows = self
            .lock()
            .prepare(
                "SELECT id, name, token, enabled, created_at, last_triggered_at
                 FROM webhooks
                 ORDER BY created_at DESC, id ASC",
            )
            .map_err(backend)?
            .query_map([], raw_webhook)
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        rows.into_iter()
            .map(|row| self.hydrate_webhook(row))
            .collect()
    }

    fn create_webhook(
        &self,
        id: &str,
        name: &str,
        token: &str,
        token_hash: &[u8],
        created_at: i64,
    ) -> Result<Webhook, StoreError> {
        let sealed = self.seal(token)?;
        self.lock()
            .execute(
                "INSERT INTO webhooks (id, name, token, token_hash, enabled, created_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                params![id, name, sealed, token_hash, created_at],
            )
            .map_err(backend)?;
        Ok(Webhook {
            id: id.to_owned(),
            name: name.to_owned(),
            token: token.to_owned(),
            enabled: true,
            created_at,
            last_triggered_at: None,
        })
    }

    fn set_webhook_enabled(&self, id: &str, enabled: bool) -> Result<bool, StoreError> {
        self.lock()
            .execute(
                "UPDATE webhooks SET enabled = ?2 WHERE id = ?1",
                params![id, i64::from(enabled)],
            )
            .map(|n| n > 0)
            .map_err(backend)
    }

    fn remove_webhook(&self, id: &str) -> Result<bool, StoreError> {
        self.lock()
            .execute("DELETE FROM webhooks WHERE id = ?1", [id])
            .map(|n| n > 0)
            .map_err(backend)
    }

    fn webhook_by_token_hash(&self, token_hash: &[u8]) -> Result<Option<Webhook>, StoreError> {
        self.lock()
            .query_row(
                "SELECT id, name, token, enabled, created_at, last_triggered_at
                 FROM webhooks
                 WHERE token_hash = ?1",
                [token_hash],
                raw_webhook,
            )
            .optional()
            .map_err(backend)?
            .map(|row| self.hydrate_webhook(row))
            .transpose()
    }

    fn touch_webhook(&self, id: &str, triggered_at: i64) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "UPDATE webhooks SET last_triggered_at = ?2 WHERE id = ?1",
                params![id, triggered_at],
            )
            .map_err(backend)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppKey;

    #[test]
    fn webhook_crud_and_encryption_at_rest() {
        let dir = std::env::temp_dir().join(format!("blob-1-webhook-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("agent.db");
        let store = SqliteStorage::open(&db, Some(AppKey::new("test-passphrase".into()))).unwrap();

        let hash_a = [1u8; 32];
        let hash_b = [2u8; 32];

        let created_a = store
            .create_webhook("wh-a", "GitHub", "secret-token-a", &hash_a, 100)
            .unwrap();
        assert_eq!(created_a.id, "wh-a");
        assert_eq!(created_a.name, "GitHub");
        assert_eq!(created_a.token, "secret-token-a");
        assert!(created_a.enabled);
        assert_eq!(created_a.created_at, 100);
        assert_eq!(created_a.last_triggered_at, None);

        let created_b = store
            .create_webhook("wh-b", "Stripe", "secret-token-b", &hash_b, 200)
            .unwrap();

        // Raw token column in SQLite must be encrypted at rest.
        let raw_token: String = store
            .lock()
            .query_row(
                "SELECT token FROM webhooks WHERE id = ?1",
                ["wh-a"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(raw_token.starts_with("enc:v1:"));
        assert_ne!(raw_token, "secret-token-a");

        // Ordered by created_at DESC, id ASC.
        let listed = store.webhooks().unwrap();
        assert_eq!(listed, vec![created_b.clone(), created_a.clone()]);

        // Lookup by token_hash unseals the token.
        assert_eq!(
            store.webhook_by_token_hash(&hash_a).unwrap(),
            Some(created_a.clone())
        );
        assert_eq!(store.webhook_by_token_hash(&[9u8; 32]).unwrap(), None);

        // Toggle enabled and touch last_triggered_at.
        assert!(store.set_webhook_enabled("wh-a", false).unwrap());
        assert!(!store.set_webhook_enabled("missing", false).unwrap());
        store.touch_webhook("wh-a", 350).unwrap();

        let updated = store.webhook_by_token_hash(&hash_a).unwrap().unwrap();
        assert!(!updated.enabled);
        assert_eq!(updated.last_triggered_at, Some(350));

        // Remove webhook.
        assert!(store.remove_webhook("wh-a").unwrap());
        assert!(!store.remove_webhook("wh-a").unwrap());
        assert_eq!(store.webhook_by_token_hash(&hash_a).unwrap(), None);
        assert_eq!(store.webhooks().unwrap(), vec![created_b]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
