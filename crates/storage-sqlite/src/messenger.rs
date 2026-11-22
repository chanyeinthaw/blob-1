use rusqlite::{params, OptionalExtension};
use storage::{
    InteractionMode, InteractionOverride, InteractionScope, MessengerStore, Place, ReplyMode,
    ReplyOverride, StoreError,
};

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

    fn messenger_token(&self, messenger: &str) -> Result<Option<String>, StoreError> {
        self.lock()
            .query_row(
                "SELECT token FROM messenger_tokens WHERE messenger = ?1",
                params![messenger],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(backend)?
            .map(|token| self.unseal(&token))
            .transpose()
    }

    fn set_messenger_token(&self, messenger: &str, token: Option<&str>) -> Result<(), StoreError> {
        match token {
            Some(token) => {
                let token = self.seal(token)?;
                self.lock()
                    .execute(
                        "INSERT INTO messenger_tokens (messenger, token) VALUES (?1, ?2) ON CONFLICT (messenger) DO UPDATE SET token = excluded.token",
                        params![messenger, token],
                    )
                    .map_err(backend)?;
            }
            None => {
                self.lock()
                    .execute(
                        "DELETE FROM messenger_tokens WHERE messenger = ?1",
                        params![messenger],
                    )
                    .map_err(backend)?;
            }
        }
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

    fn interaction_modes(&self, messenger: &str) -> Result<Vec<InteractionOverride>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare(
                "SELECT scope, guild_id, target_id, mode FROM interaction_modes WHERE messenger = ?1 ORDER BY scope, guild_id, target_id",
            )
            .map_err(backend)?;
        let rows = statement
            .query_map([messenger], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(backend)?;
        rows.map(|row| {
            let (scope, guild_id, target_id, mode) = row.map_err(backend)?;
            let invalid = |kind: &str, value: &str| {
                StoreError::Backend(format!("invalid {kind} in interaction_modes: {value}").into())
            };
            Ok(InteractionOverride {
                scope: InteractionScope::parse(&scope)
                    .ok_or_else(|| invalid("scope", &scope))?,
                guild_id: (!guild_id.is_empty()).then_some(guild_id),
                target_id,
                mode: InteractionMode::parse(&mode).ok_or_else(|| invalid("mode", &mode))?,
            })
        })
        .collect()
    }

    fn set_interaction_mode(
        &self,
        messenger: &str,
        scope: InteractionScope,
        guild_id: Option<&str>,
        target_id: &str,
        mode: InteractionMode,
    ) -> Result<(), StoreError> {
        let guild = normalize_guild(scope, guild_id)?;
        self.lock()
            .execute(
                "INSERT INTO interaction_modes (messenger, scope, guild_id, target_id, mode) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (messenger, scope, guild_id, target_id) DO UPDATE SET mode = excluded.mode",
                params![messenger, scope.as_str(), guild, target_id, mode.as_str()],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn clear_interaction_mode(
        &self,
        messenger: &str,
        scope: InteractionScope,
        guild_id: Option<&str>,
        target_id: &str,
    ) -> Result<bool, StoreError> {
        let guild = normalize_clear_guild(scope, guild_id)?;
        self.lock()
            .execute(
                "DELETE FROM interaction_modes WHERE messenger = ?1 AND scope = ?2 AND guild_id = ?3 AND target_id = ?4",
                params![messenger, scope.as_str(), guild, target_id],
            )
            .map(|n| n > 0)
            .map_err(backend)
    }

    fn reply_modes(&self, messenger: &str) -> Result<Vec<ReplyOverride>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare(
                "SELECT channel_id, mode FROM reply_modes WHERE messenger = ?1 ORDER BY channel_id",
            )
            .map_err(backend)?;
        let rows = statement
            .query_map([messenger], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(backend)?;
        rows.map(|row| {
            let (channel_id, mode) = row.map_err(backend)?;
            let mode = ReplyMode::parse(&mode).ok_or_else(|| {
                StoreError::Backend(format!("invalid mode in reply_modes: {mode}").into())
            })?;
            Ok(ReplyOverride { channel_id, mode })
        })
        .collect()
    }

    fn set_reply_mode(
        &self,
        messenger: &str,
        channel_id: &str,
        mode: ReplyMode,
    ) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO reply_modes (messenger, channel_id, mode) VALUES (?1, ?2, ?3) ON CONFLICT (messenger, channel_id) DO UPDATE SET mode = excluded.mode",
                params![messenger, channel_id, mode.as_str()],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn clear_reply_mode(&self, messenger: &str, channel_id: &str) -> Result<bool, StoreError> {
        self.lock()
            .execute(
                "DELETE FROM reply_modes WHERE messenger = ?1 AND channel_id = ?2",
                params![messenger, channel_id],
            )
            .map(|n| n > 0)
            .map_err(backend)
    }

    fn mark_pending_thread(&self, destination: u64) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT OR IGNORE INTO pending_threads (destination_id) VALUES (?1)",
                [destination as i64],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn clear_pending_thread(&self, destination: u64) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "DELETE FROM pending_threads WHERE destination_id = ?1",
                [destination as i64],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn clear_pending_thread_by_place(
        &self,
        messenger: &str,
        channel: &str,
        thread: &str,
    ) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "DELETE FROM pending_threads WHERE destination_id IN (SELECT id FROM destinations WHERE messenger = ?1 AND channel = ?2 AND thread = ?3)",
                params![messenger, channel, thread],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn is_pending_thread(&self, destination: u64) -> Result<bool, StoreError> {
        self.lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_threads WHERE destination_id = ?1)",
                [destination as i64],
                |row| row.get(0),
            )
            .map_err(backend)
    }
}

fn normalize_guild<'a>(
    scope: InteractionScope,
    guild_id: Option<&'a str>,
) -> Result<&'a str, StoreError> {
    match (scope, guild_id) {
        (InteractionScope::Channel, None | Some("")) => Ok(""),
        (InteractionScope::Channel, Some(_)) => Err(StoreError::Backend(
            "channel interaction overrides cannot specify a guild id".into(),
        )),
        (InteractionScope::User, Some(g)) if !g.is_empty() => Ok(g),
        (InteractionScope::User, _) => Err(StoreError::Backend(
            "user interaction overrides require a guild id".into(),
        )),
    }
}

/// Clearing allows `None` on `User` so legacy unassigned migrated rows (`guild_id = ''`)
/// can be removed cleanly.
fn normalize_clear_guild<'a>(
    scope: InteractionScope,
    guild_id: Option<&'a str>,
) -> Result<&'a str, StoreError> {
    match (scope, guild_id) {
        (InteractionScope::Channel, None | Some("")) => Ok(""),
        (InteractionScope::Channel, Some(_)) => Err(StoreError::Backend(
            "channel interaction overrides cannot specify a guild id".into(),
        )),
        (InteractionScope::User, None | Some("")) => Ok(""),
        (InteractionScope::User, Some(g)) => Ok(g),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interaction_modes_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "blob-1-messenger-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();

        assert!(store.interaction_modes("discord").unwrap().is_empty());
        // User overrides require a guild id; channel overrides forbid one.
        assert!(
            store
                .set_interaction_mode(
                    "discord",
                    InteractionScope::User,
                    None,
                    "u1",
                    InteractionMode::Always,
                )
                .is_err()
        );
        assert!(
            store
                .set_interaction_mode(
                    "discord",
                    InteractionScope::Channel,
                    Some("g1"),
                    "c1",
                    InteractionMode::Always,
                )
                .is_err()
        );

        store
            .set_interaction_mode(
                "discord",
                InteractionScope::User,
                Some("g1"),
                "u1",
                InteractionMode::Always,
            )
            .unwrap();
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::User,
                Some("g2"),
                "u1",
                InteractionMode::MentionOnly,
            )
            .unwrap();
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Channel,
                None,
                "c2",
                InteractionMode::Always,
            )
            .unwrap();
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Channel,
                None,
                "c1",
                InteractionMode::MentionOnly,
            )
            .unwrap();
        // Setting the same key again replaces the mode.
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Channel,
                None,
                "c2",
                InteractionMode::MentionOnly,
            )
            .unwrap();
        // Rows are scoped per messenger.
        store
            .set_interaction_mode(
                "slack",
                InteractionScope::Channel,
                None,
                "c9",
                InteractionMode::Always,
            )
            .unwrap();

        assert_eq!(
            store.interaction_modes("discord").unwrap(),
            vec![
                InteractionOverride {
                    scope: InteractionScope::Channel,
                    guild_id: None,
                    target_id: "c1".into(),
                    mode: InteractionMode::MentionOnly,
                },
                InteractionOverride {
                    scope: InteractionScope::Channel,
                    guild_id: None,
                    target_id: "c2".into(),
                    mode: InteractionMode::MentionOnly,
                },
                InteractionOverride {
                    scope: InteractionScope::User,
                    guild_id: Some("g1".into()),
                    target_id: "u1".into(),
                    mode: InteractionMode::Always,
                },
                InteractionOverride {
                    scope: InteractionScope::User,
                    guild_id: Some("g2".into()),
                    target_id: "u1".into(),
                    mode: InteractionMode::MentionOnly,
                },
            ]
        );
        assert!(
            store
                .clear_interaction_mode("discord", InteractionScope::User, Some("g1"), "u1")
                .unwrap()
        );
        assert!(
            !store
                .clear_interaction_mode("discord", InteractionScope::User, Some("g1"), "u1")
                .unwrap()
        );
        // Clearing g1 leaves g2 intact.
        assert_eq!(store.interaction_modes("discord").unwrap().len(), 3);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_legacy_interaction_modes_table() {
        let dir = std::env::temp_dir().join(format!(
            "blob-1-messenger-migrate-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("agent.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE interaction_modes (
                    messenger TEXT NOT NULL,
                    scope TEXT NOT NULL CHECK (scope IN ('channel', 'user')),
                    target_id TEXT NOT NULL,
                    mode TEXT NOT NULL CHECK (mode IN ('mention-only', 'always')),
                    PRIMARY KEY (messenger, scope, target_id)
                ) STRICT;
                INSERT INTO interaction_modes VALUES ('discord', 'channel', 'c1', 'always');
                INSERT INTO interaction_modes VALUES ('discord', 'user', 'u1', 'always');",
            )
            .unwrap();
        }
        let store = SqliteStorage::open(&db, None).unwrap();
        // Preserves channel rows and keeps legacy unscoped user rows as
        // inactive (`guild_id: None`) so admins can see, reassign, or clear them.
        assert_eq!(
            store.interaction_modes("discord").unwrap(),
            vec![
                InteractionOverride {
                    scope: InteractionScope::Channel,
                    guild_id: None,
                    target_id: "c1".into(),
                    mode: InteractionMode::Always,
                },
                InteractionOverride {
                    scope: InteractionScope::User,
                    guild_id: None,
                    target_id: "u1".into(),
                    mode: InteractionMode::Always,
                },
            ]
        );
        // Legacy inactive user row can be cleared with `guild_id: None`.
        assert!(
            store
                .clear_interaction_mode("discord", InteractionScope::User, None, "u1")
                .unwrap()
        );
        assert_eq!(store.interaction_modes("discord").unwrap().len(), 1);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reply_modes_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "blob-1-reply-modes-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();

        assert!(store.reply_modes("discord").unwrap().is_empty());
        store
            .set_reply_mode("discord", "c2", ReplyMode::Direct)
            .unwrap();
        store
            .set_reply_mode("discord", "c1", ReplyMode::Thread)
            .unwrap();
        store
            .set_reply_mode("discord", "c2", ReplyMode::Decision)
            .unwrap();
        store
            .set_reply_mode("slack", "c9", ReplyMode::Direct)
            .unwrap();

        assert_eq!(
            store.reply_modes("discord").unwrap(),
            vec![
                ReplyOverride {
                    channel_id: "c1".into(),
                    mode: ReplyMode::Thread,
                },
                ReplyOverride {
                    channel_id: "c2".into(),
                    mode: ReplyMode::Decision,
                },
            ]
        );
        assert!(store.clear_reply_mode("discord", "c1").unwrap());
        assert!(!store.clear_reply_mode("discord", "c1").unwrap());
        assert_eq!(store.reply_modes("discord").unwrap().len(), 1);

        // Pending thread state persists across reopens and clears cleanly.
        let dest = store
            .destination(
                "discord",
                &Place {
                    channel: "c2".into(),
                    thread: Some("m1".into()),
                    dm: false,
                },
            )
            .unwrap();
        assert!(!store.is_pending_thread(dest).unwrap());
        store.mark_pending_thread(dest).unwrap();
        assert!(store.is_pending_thread(dest).unwrap());
        drop(store);
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();
        assert!(store.is_pending_thread(dest).unwrap());
        store.clear_pending_thread(dest).unwrap();
        assert!(!store.is_pending_thread(dest).unwrap());
        store.mark_pending_thread(dest).unwrap();
        store
            .clear_pending_thread_by_place("discord", "c2", "m1")
            .unwrap();
        assert!(!store.is_pending_thread(dest).unwrap());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backfills_pending_threads_for_legacy_db() {
        let dir = std::env::temp_dir().join(format!(
            "blob-1-pending-backfill-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("agent.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE destinations (id INTEGER PRIMARY KEY, messenger TEXT NOT NULL, channel TEXT NOT NULL, thread TEXT NOT NULL, dm INTEGER NOT NULL CHECK (dm IN (0,1)), UNIQUE (messenger, channel, thread)) STRICT;
                 CREATE TABLE memory_messages (i INTEGER PRIMARY KEY, kind TEXT NOT NULL, text TEXT NOT NULL, size INTEGER NOT NULL, date TEXT NOT NULL, more INTEGER NOT NULL, meta TEXT) STRICT;
                 INSERT INTO destinations VALUES (1, 'discord', 'c1', 'm_pending', 0);
                 INSERT INTO destinations VALUES (2, 'discord', 'c1', 'm_replied', 0);
                 INSERT INTO destinations VALUES (3, 'discord', 'c1', 'm_seen_thread', 0);
                 INSERT INTO destinations VALUES (4, 'discord', 'c1', 'm_ambiguous', 0);",
            )
            .unwrap();
            let mk_meta = |to: u64, msg: &str, thread: Option<&str>| {
                serde_json::to_string(&storage::Meta::Message {
                    messenger: "discord".into(),
                    place: Place {
                        channel: "c1".into(),
                        thread: thread.map(Into::into),
                        dm: false,
                    },
                    message: msg.into(),
                    author: storage::Person {
                        id: "u1".into(),
                        name: "u1".into(),
                    },
                    to,
                    inline: false,
                    unaddressed: false,
                    location: "#c1".into(),
                    earlier: None,
                    quote: None,
                    attachments: Vec::new(),
                })
                .unwrap()
            };
            // Dest 1: plain-channel message, never replied/sent, never seen as thread -> pending.
            conn.execute(
                "INSERT INTO memory_messages VALUES (10, 'user', 'remind me', 9, '2026-01-01', 0, ?1)",
                [mk_meta(1, "m_pending", None)],
            )
            .unwrap();
            // Dest 2: plain-channel message, but bot sent reply m20 -> NOT pending.
            conn.execute(
                "INSERT INTO memory_messages VALUES (20, 'user', 'hi', 2, '2026-01-01', 0, ?1)",
                [mk_meta(2, "m_replied", None)],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memory_messages VALUES (21, 'tool', 'send {\"reply\":\"m20\",\"text\":\"ok\"}', 30, '2026-01-01', 0, NULL)",
                [],
            )
            .unwrap();
            // Dest 3: plain-channel message, then user replied inside thread -> NOT pending.
            conn.execute(
                "INSERT INTO memory_messages VALUES (30, 'user', 'hi', 2, '2026-01-01', 0, ?1)",
                [mk_meta(3, "m_seen_thread", None)],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memory_messages VALUES (31, 'user', 'in thread', 9, '2026-01-01', 0, ?1)",
                [mk_meta(3, "m_in_thread", Some("m_seen_thread"))],
            )
            .unwrap();
            // Dest 4: no originating plain-channel message in history -> ambiguous, NOT pending.
        }
        let store = SqliteStorage::open(&db, None).unwrap();
        assert!(store.is_pending_thread(1).unwrap());
        assert!(!store.is_pending_thread(2).unwrap());
        assert!(!store.is_pending_thread(3).unwrap());
        assert!(!store.is_pending_thread(4).unwrap());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
