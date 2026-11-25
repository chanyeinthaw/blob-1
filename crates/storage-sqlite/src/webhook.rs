use rusqlite::{params, OptionalExtension};
use storage::{Callback, CallbackStore, PendingEvent, StoreError, Webhook, WebhookStore};

use crate::{backend, SqliteStorage};

const MAX_ACTIVE_CALLBACKS: i64 = 200;

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

    fn enqueue_webhook_event(
        &self,
        token_hash: &[u8],
        now: i64,
        payload: &str,
    ) -> Result<Option<String>, StoreError> {
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT id, name FROM webhooks WHERE token_hash = ?1 AND enabled = 1",
                [token_hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(backend)?;
        let Some((id, name)) = row else {
            tx.commit().map_err(backend)?;
            return Ok(None);
        };
        let redacted_payload = storage::redact_json_str(payload);
        tx.execute(
            "INSERT INTO pending_events (source, name, for_token, payload, created_at)
             VALUES ('webhook', ?1, NULL, ?2, ?3)",
            params![&name, redacted_payload.as_ref(), now],
        )
        .map_err(backend)?;
        tx.execute(
            "UPDATE webhooks SET last_triggered_at = ?2 WHERE id = ?1",
            params![&id, now],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(Some(name))
    }
}

impl CallbackStore for SqliteStorage {
    fn create_callback(
        &self,
        id: &str,
        name: &str,
        token_hash: &[u8],
        for_token: Option<&str>,
        max_uses: u32,
        expires_at: i64,
        created_at: i64,
    ) -> Result<Callback, StoreError> {
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        tx.execute(
            "DELETE FROM callbacks WHERE expires_at <= ?1 OR remaining_uses <= 0",
            [created_at],
        )
        .map_err(backend)?;
        let active: i64 = tx
            .query_row("SELECT COUNT(*) FROM callbacks", [], |row| row.get(0))
            .map_err(backend)?;
        if active >= MAX_ACTIVE_CALLBACKS {
            return Err(StoreError::Backend(
                format!("too many active temporary callbacks (limit {MAX_ACTIVE_CALLBACKS})").into(),
            ));
        }
        tx.execute(
            "INSERT INTO callbacks (id, name, token_hash, for_token, remaining_uses, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                name,
                token_hash,
                for_token,
                i64::from(max_uses),
                expires_at,
                created_at
            ],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(Callback {
            id: id.to_owned(),
            name: name.to_owned(),
            for_token: for_token.map(str::to_owned),
            remaining_uses: max_uses,
            expires_at,
            created_at,
        })
    }

    fn has_active_callback(&self, token_hash: &[u8], now: i64) -> Result<bool, StoreError> {
        self.lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM callbacks WHERE token_hash = ?1 AND expires_at > ?2 AND remaining_uses > 0)",
                params![token_hash, now],
                |row| row.get(0),
            )
            .map_err(backend)
    }

    fn consume_callback(
        &self,
        token_hash: &[u8],
        now: i64,
        payload: &str,
    ) -> Result<Option<Callback>, StoreError> {
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        tx.execute(
            "DELETE FROM callbacks WHERE expires_at <= ?1 OR remaining_uses <= 0",
            [now],
        )
        .map_err(backend)?;
        let row: Option<(String, String, Option<String>, i64, i64, i64)> = tx
            .query_row(
                "SELECT id, name, for_token, remaining_uses, expires_at, created_at
                 FROM callbacks
                 WHERE token_hash = ?1 AND expires_at > ?2 AND remaining_uses > 0",
                params![token_hash, now],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(backend)?;
        let Some((id, name, for_token, remaining, expires_at, created_at)) = row else {
            tx.commit().map_err(backend)?;
            return Ok(None);
        };
        let after = (remaining - 1).max(0) as u32;
        if after == 0 {
            tx.execute("DELETE FROM callbacks WHERE id = ?1", [&id])
                .map_err(backend)?;
        } else {
            tx.execute(
                "UPDATE callbacks SET remaining_uses = ?2 WHERE id = ?1",
                params![&id, i64::from(after)],
            )
            .map_err(backend)?;
        }
        let redacted_payload = storage::redact_json_str(payload);
        tx.execute(
            "INSERT INTO pending_events (source, name, for_token, payload, created_at)
             VALUES ('callback', ?1, ?2, ?3, ?4)",
            params![&name, &for_token, redacted_payload.as_ref(), now],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(Some(Callback {
            id,
            name,
            for_token,
            remaining_uses: after,
            expires_at,
            created_at,
        }))
    }

    fn next_pending_event(&self) -> Result<Option<PendingEvent>, StoreError> {
        self.lock()
            .query_row(
                "SELECT id, source, name, for_token, payload, created_at
                 FROM pending_events
                 ORDER BY id ASC
                 LIMIT 1",
                [],
                |row| {
                    Ok(PendingEvent {
                        id: row.get(0)?,
                        source: row.get(1)?,
                        name: row.get(2)?,
                        for_token: row.get(3)?,
                        payload: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(backend)
    }

    fn delete_pending_event(&self, id: i64) -> Result<(), StoreError> {
        self.lock()
            .execute("DELETE FROM pending_events WHERE id = ?1", [id])
            .map_err(backend)?;
        Ok(())
    }

    fn add_pending_event(
        &self,
        source: &str,
        name: &str,
        for_token: Option<&str>,
        payload: &str,
        created_at: i64,
    ) -> Result<i64, StoreError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO pending_events (source, name, for_token, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![source, name, for_token, payload, created_at],
        )
        .map_err(backend)?;
        Ok(conn.last_insert_rowid())
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

        // Temporary callbacks: one-shot, multi-use, expiry, and `for_token`.
        let cb1 = store
            .create_callback("cb-1", "job-1", &[10u8; 32], Some("m42"), 1, 200, 100)
            .unwrap();
        assert_eq!(cb1.remaining_uses, 1);
        assert_eq!(cb1.for_token.as_deref(), Some("m42"));
        assert!(store.has_active_callback(&[10u8; 32], 150).unwrap());
        assert!(!store.has_active_callback(&[10u8; 32], 200).unwrap());

        let hit = store
            .consume_callback(&[10u8; 32], 150, "job-1-done")
            .unwrap()
            .unwrap();
        assert_eq!(hit.name, "job-1");
        assert_eq!(hit.for_token.as_deref(), Some("m42"));
        assert_eq!(hit.remaining_uses, 0);
        // Atomic enqueue persisted the event in `pending_events` in the same tx.
        let pending = store.next_pending_event().unwrap().unwrap();
        assert_eq!(pending.source, "callback");
        assert_eq!(pending.name, "job-1");
        assert_eq!(pending.for_token.as_deref(), Some("m42"));
        assert_eq!(pending.payload, "job-1-done");
        store.delete_pending_event(pending.id).unwrap();
        assert_eq!(store.next_pending_event().unwrap(), None);
        // Second consume on one-shot returns None.
        assert_eq!(
            store.consume_callback(&[10u8; 32], 151, "again").unwrap(),
            None
        );

        // Multi-use decrements and deletes after final use.
        store
            .create_callback("cb-2", "job-2", &[11u8; 32], None, 2, 300, 100)
            .unwrap();
        let first = store
            .consume_callback(&[11u8; 32], 150, "part-1")
            .unwrap()
            .unwrap();
        assert_eq!(first.remaining_uses, 1);
        let second = store
            .consume_callback(&[11u8; 32], 160, "part-2")
            .unwrap()
            .unwrap();
        assert_eq!(second.remaining_uses, 0);
        assert_eq!(
            store.consume_callback(&[11u8; 32], 170, "part-3").unwrap(),
            None
        );

        // Expired callback is pruned and rejected.
        store
            .create_callback("cb-3", "expired", &[12u8; 32], None, 1, 200, 100)
            .unwrap();
        assert_eq!(
            store.consume_callback(&[12u8; 32], 200, "late").unwrap(),
            None
        );

        // Transcript & memory metadata persistence redact generated 43-char `cbk_`
        // credentials (in URLs, bare env vars, AND after JSON-escaped newlines)
        // while leaving ordinary identifiers like `cbk_worker` untouched.
        use storage::{
            MemoryKind, MemoryMessage, MemoryStore, Message, Meta, Person, Place, Quote, Session,
            SessionId, SessionStore,
        };
        let token_43 = "cbk_0123456789abcdef0123456789abcdef0123456789a";
        let token_43_b = "cbk_fedcba9876543210fedcba9876543210fedcba98765";
        // Clean identifiers (including escaped prefix characters like `\u005f` before `cbk_`)
        // must return `Cow::Borrowed` without false redaction or reserialization.
        let untouched = r#"{ "id" : "cbk_worker" }"#;
        assert!(matches!(
            storage::redact_json_str(untouched),
            std::borrow::Cow::Borrowed(s) if s == untouched
        ));
        let escaped_non_boundary = format!(r#"{{"id":"prefix\u005f{token_43}"}}"#);
        assert!(matches!(
            storage::redact_json_str(&escaped_non_boundary),
            std::borrow::Cow::Borrowed(s) if s == escaped_non_boundary
        ));
        // Undecodable literal (lone surrogate `\uD800`), multibyte malformed escapes
        // (`\u12é`, `\é`), invalid-escape boundaries (`a\xcb_...`), and unterminated
        // quotes with preceding redacted gaps must not panic, double gaps, or leak.
        let lone_surrogate =
            r#"{"bad":"\uD800\u0063bk_0123456789abcdef0123456789abcdef0123456789a"}"#;
        let redacted_surrogate = storage::redact_json_str(lone_surrogate);
        assert!(!redacted_surrogate.contains("0123456789abcdef0123456789abcdef0123456789a"));
        assert!(redacted_surrogate.contains("<redacted>"));

        let multibyte_bad = format!(r#"{{"x":"\u12é\é\x{token_43}"}}"#);
        let redacted_mb = storage::redact_json_str(&multibyte_bad);
        assert!(!redacted_mb.contains(token_43));
        assert!(redacted_mb.contains("<redacted>"));

        // Unknown escape whose escaped character starts a credential (`\cbk_...`,
        // `\c\u0062k_...`, and `\ccbk_...`) must also be redacted.
        for malformed in [
            format!(r#"{{"a":"\{token_43}"}}"#),
            r#"{"b":"\c\u0062k_0123456789abcdef0123456789abcdef0123456789a"}"#.to_owned(),
            format!(r#"{{"c":"\c{token_43}"}}"#),
        ] {
            let out = storage::redact_json_str(&malformed);
            assert!(
                !out.contains("0123456789abcdef0123456789abcdef0123456789a"),
                "leaked in: {out}"
            );
            assert!(out.contains("<redacted>"), "missing redacted in: {out}");
        }

        // Mixed malformed recovery modes in the SAME string (both closed and
        // unterminated) must redact every credential.
        for mixed in [
            format!(r#"{{"m":"\{token_43} \x{token_43_b}"}}"#),
            format!(r#"{{"m":"\c\u0062k_0123456789abcdef0123456789abcdef0123456789a \c{token_43_b}"}}"#),
            format!(r#""unterm \{token_43} \x{token_43_b}"#),
        ] {
            let out = storage::redact_json_str(&mixed);
            assert!(
                !out.contains("0123456789abcdef0123456789abcdef0123456789a"),
                "token_43 leaked in: {out}"
            );
            assert!(
                !out.contains("fedcba9876543210fedcba9876543210fedcba98765"),
                "token_43_b leaked in: {out}"
            );
        }

        let unterminated = format!(
            r#"{token_43} "unterm \u0063bk_0123456789abcdef0123456789abcdef0123456789a"#
        );
        let redacted_unterm = storage::redact_json_str(&unterminated);
        assert_eq!(redacted_unterm, r#"<redacted> "unterm <redacted>"#);

        // Redacted key avoids colliding with a plain `<redacted>` key appearing later,
        // and repeated copies of the same original credential key reuse the same name.
        let later_collision = format!(
            r#"{{"{token_43}":"first","{token_43}":"dup-same","{token_43_b}":"second","<redacted>":"plain"}}"#
        );
        let redacted_keys = storage::redact_json_str(&later_collision);
        assert_eq!(
            redacted_keys,
            r#"{"<redacted>#2":"first","<redacted>#2":"dup-same","<redacted>#3":"second","<redacted>":"plain"}"#
        );

        // Colliding credential-bearing keys in `redact_json_value` preserve both values.
        let mut colliding = serde_json::json!({
            token_43: "first",
            token_43_b: "second",
        });
        assert!(storage::redact_json_value(&mut colliding));
        let obj = colliding.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        let vals: Vec<&str> = obj.values().filter_map(|v| v.as_str()).collect();
        assert!(vals.contains(&"first") && vals.contains(&"second"));

        // Outbox JSON payload with duplicate keys (credential only on the earlier key),
        // escaped newline, Unicode-escaped `cbk_`, and colliding redacted keys is redacted
        // in place with distinct suffixes on colliding redacted keys so replay preserves both.
        let cb_json_payload = format!(
            r#"{{"dup":"\u0063bk_0123456789abcdef0123456789abcdef0123456789a","dup":"ok","{token_43}":"v1","{token_43_b}":"v2"}}"#
        );
        store
            .create_callback("cb-json", "job-json", &[13u8; 32], None, 1, 400, 100)
            .unwrap();
        store
            .consume_callback(&[13u8; 32], 150, &cb_json_payload)
            .unwrap()
            .unwrap();
        let ev_json = store.next_pending_event().unwrap().unwrap();
        let mut found_json = None;
        let mut cur = Some(ev_json);
        while let Some(ev) = cur {
            store.delete_pending_event(ev.id).unwrap();
            if ev.name == "job-json" {
                found_json = Some(ev.payload);
            }
            cur = store.next_pending_event().unwrap();
        }
        let saved_cb_payload = found_json.unwrap();
        assert!(!saved_cb_payload.contains("0123456789abcdef0123456789abcdef0123456789a"));
        assert!(!saved_cb_payload.contains("fedcba9876543210fedcba9876543210fedcba98765"));
        assert!(saved_cb_payload.contains(r#""dup":"<redacted>""#));
        assert!(saved_cb_payload.contains(r#""dup":"ok""#));
        assert!(saved_cb_payload.contains(r#""<redacted>":"v1""#));
        assert!(saved_cb_payload.contains(r#""<redacted>#2":"v2""#));

        let mut session = Session::new(SessionId("task:cb-redact".into()));
        session.messages.push(Message::ToolResult {
            id: "c1".into(),
            name: "callback".into(),
            content: format!("url: https://blob.example/hooks/cb/{token_43}\nmax_uses: 1"),
            images: Vec::new(),
        });
        session.messages.push(Message::ToolCall {
            id: "c2".into(),
            name: "bash".into(),
            arguments: format!(
                r#"{{"command":"cbk_worker=1\n\u0063bk_0123456789abcdef0123456789abcdef0123456789a","{token_43}":"k"}}"#
            ),
        });
        let session_id = session.id.clone();
        store.save(session).unwrap();
        let loaded = store.load(&session_id).unwrap().unwrap();
        for msg in &loaded.messages {
            let text = match msg {
                Message::ToolResult { content, .. } => content.as_str(),
                Message::ToolCall { arguments, .. } => arguments.as_str(),
                _ => "",
            };
            assert!(!text.contains(token_43));
            assert!(text.contains("<redacted>"));
        }
        if let Message::ToolCall { arguments, .. } = &loaded.messages[1] {
            assert!(arguments.contains("cbk_worker=1"));
        }

        // Quoted Discord reply containing a callback URL and a bare token after a
        // newline (`\ncbk_...`) is also redacted in `memory_messages.meta`.
        let quoted_meta = Meta::Message {
            messenger: "discord".into(),
            place: Place {
                channel: "c1".into(),
                thread: None,
                dm: false,
            },
            message: "m1".into(),
            author: Person {
                id: "u1".into(),
                name: "Chan".into(),
            },
            to: 1,
            location: "#dev".into(),
            earlier: None,
            quote: Some(Quote {
                message: "m0".into(),
                author: Person {
                    id: "u2".into(),
                    name: "Bot".into(),
                },
                text: format!("line1\n{token_43}\nsee https://blob.example/hooks/cb/{token_43} and cbk_worker"),
                attachments: Vec::new(),
            }),
            attachments: Vec::new(),
            inline: false,
            unaddressed: false,
            missed: Vec::new(),
            gap_open: false,
        };
        store
            .append_messages(
                &[MemoryMessage {
                    i: 0,
                    kind: MemoryKind::User,
                    text: "replied".into(),
                    size: 7,
                    date: "2026-11-23".into(),
                    more: false,
                    meta: Some(quoted_meta),
                }],
                &[],
            )
            .unwrap();
        let saved_msg = store.message(0).unwrap().unwrap();
        let saved_meta_json = serde_json::to_string(&saved_msg.meta).unwrap();
        assert!(!saved_meta_json.contains(token_43));
        assert!(saved_meta_json.contains("/hooks/cb/<redacted>"));
        assert!(saved_meta_json.contains("cbk_worker"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
