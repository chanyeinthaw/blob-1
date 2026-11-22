use rusqlite::{params, OptionalExtension};
use storage::{
    AccessEntry, AccessList, AccessMode, AccessScope, InteractionMode, InteractionOverride,
    InteractionScope, MessengerDefaults, MessengerStore, Place, ReplyMode, ReplyOverride,
    ReplyScope, StoreError,
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
            .prepare(
                "SELECT user_id FROM access_entries WHERE messenger = ?1 AND list = 'whitelist' AND scope = 'global' AND scope_id = '' ORDER BY user_id",
            )
            .map_err(backend)?;
        let rows = statement
            .query_map([messenger], |row| row.get(0))
            .map_err(backend)?;
        rows.collect::<Result<_, _>>().map_err(backend)
    }

    fn allow_user(&self, messenger: &str, user: &str) -> Result<(), StoreError> {
        self.add_access_entry(
            messenger,
            AccessList::Whitelist,
            AccessScope::Global,
            None,
            user,
        )
    }

    fn disallow_user(&self, messenger: &str, user: &str) -> Result<bool, StoreError> {
        self.remove_access_entry(
            messenger,
            AccessList::Whitelist,
            AccessScope::Global,
            None,
            user,
        )
    }

    fn access_mode(&self, messenger: &str) -> Result<AccessMode, StoreError> {
        let mode: Option<String> = self
            .lock()
            .query_row(
                "SELECT mode FROM access_modes WHERE messenger = ?1",
                [messenger],
                |row| row.get(0),
            )
            .optional()
            .map_err(backend)?;
        match mode {
            None => Ok(AccessMode::Whitelist),
            Some(value) => AccessMode::parse(&value).ok_or_else(|| {
                StoreError::Backend(format!("invalid mode in access_modes: {value}").into())
            }),
        }
    }

    fn set_access_mode(&self, messenger: &str, mode: AccessMode) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO access_modes (messenger, mode) VALUES (?1, ?2) ON CONFLICT (messenger) DO UPDATE SET mode = excluded.mode",
                params![messenger, mode.as_str()],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn access_entries(&self, messenger: &str) -> Result<Vec<AccessEntry>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare(
                "SELECT list, scope, scope_id, user_id FROM access_entries WHERE messenger = ?1 ORDER BY list, scope, scope_id, user_id",
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
            let (list, scope, scope_id, user_id) = row.map_err(backend)?;
            let invalid = |kind: &str, value: &str| {
                StoreError::Backend(format!("invalid {kind} in access_entries: {value}").into())
            };
            Ok(AccessEntry {
                list: AccessList::parse(&list).ok_or_else(|| invalid("list", &list))?,
                scope: AccessScope::parse(&scope).ok_or_else(|| invalid("scope", &scope))?,
                scope_id: (!scope_id.is_empty()).then_some(scope_id),
                user_id,
            })
        })
        .collect()
    }

    fn add_access_entry(
        &self,
        messenger: &str,
        list: AccessList,
        scope: AccessScope,
        scope_id: Option<&str>,
        user_id: &str,
    ) -> Result<(), StoreError> {
        let sid = normalize_access_scope(scope, scope_id)?;
        self.lock()
            .execute(
                "INSERT OR IGNORE INTO access_entries (messenger, list, scope, scope_id, user_id) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![messenger, list.as_str(), scope.as_str(), sid, user_id],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn remove_access_entry(
        &self,
        messenger: &str,
        list: AccessList,
        scope: AccessScope,
        scope_id: Option<&str>,
        user_id: &str,
    ) -> Result<bool, StoreError> {
        let sid = normalize_access_scope(scope, scope_id)?;
        self.lock()
            .execute(
                "DELETE FROM access_entries WHERE messenger = ?1 AND list = ?2 AND scope = ?3 AND scope_id = ?4 AND user_id = ?5",
                params![messenger, list.as_str(), scope.as_str(), sid, user_id],
            )
            .map(|n| n > 0)
            .map_err(backend)
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
                scope: InteractionScope::parse(&scope).ok_or_else(|| invalid("scope", &scope))?,
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
                "SELECT scope, target_id, mode FROM reply_modes WHERE messenger = ?1 ORDER BY scope, target_id",
            )
            .map_err(backend)?;
        let rows = statement
            .query_map([messenger], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(backend)?;
        rows.map(|row| {
            let (scope, target_id, mode) = row.map_err(backend)?;
            let invalid = |kind: &str, value: &str| {
                StoreError::Backend(format!("invalid {kind} in reply_modes: {value}").into())
            };
            Ok(ReplyOverride {
                scope: ReplyScope::parse(&scope).ok_or_else(|| invalid("scope", &scope))?,
                target_id,
                mode: ReplyMode::parse(&mode).ok_or_else(|| invalid("mode", &mode))?,
            })
        })
        .collect()
    }

    fn set_reply_mode(
        &self,
        messenger: &str,
        scope: ReplyScope,
        target_id: &str,
        mode: ReplyMode,
    ) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO reply_modes (messenger, scope, target_id, mode) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (messenger, scope, target_id) DO UPDATE SET mode = excluded.mode",
                params![messenger, scope.as_str(), target_id, mode.as_str()],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn clear_reply_mode(
        &self,
        messenger: &str,
        scope: ReplyScope,
        target_id: &str,
    ) -> Result<bool, StoreError> {
        self.lock()
            .execute(
                "DELETE FROM reply_modes WHERE messenger = ?1 AND scope = ?2 AND target_id = ?3",
                params![messenger, scope.as_str(), target_id],
            )
            .map(|n| n > 0)
            .map_err(backend)
    }

    fn messenger_defaults(&self, messenger: &str) -> Result<MessengerDefaults, StoreError> {
        let row = self
            .lock()
            .query_row(
                "SELECT interaction_mode, reply_mode FROM messenger_defaults WHERE messenger = ?1",
                [messenger],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(backend)?;
        match row {
            None => Ok(MessengerDefaults::default()),
            Some((interaction_mode, reply_mode)) => {
                let invalid = |kind: &str, value: &str| {
                    StoreError::Backend(
                        format!("invalid {kind} in messenger_defaults: {value}").into(),
                    )
                };
                Ok(MessengerDefaults {
                    interaction_mode: InteractionMode::parse(&interaction_mode)
                        .ok_or_else(|| invalid("interaction_mode", &interaction_mode))?,
                    reply_mode: ReplyMode::parse(&reply_mode)
                        .ok_or_else(|| invalid("reply_mode", &reply_mode))?,
                })
            }
        }
    }

    fn set_default_interaction_mode(
        &self,
        messenger: &str,
        mode: InteractionMode,
    ) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO messenger_defaults (messenger, interaction_mode, reply_mode) VALUES (?1, ?2, 'auto') ON CONFLICT (messenger) DO UPDATE SET interaction_mode = excluded.interaction_mode",
                params![messenger, mode.as_str()],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn set_default_reply_mode(&self, messenger: &str, mode: ReplyMode) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO messenger_defaults (messenger, interaction_mode, reply_mode) VALUES (?1, 'auto', ?2) ON CONFLICT (messenger) DO UPDATE SET reply_mode = excluded.reply_mode",
                params![messenger, mode.as_str()],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn mark_pending_thread(&self, destination: u64) -> Result<(), StoreError> {
        // Skip inserting if the thread was already recorded in `seen_threads`
        // (e.g. gateway `ThreadCreate` arrived during the reply-mode decision).
        self.lock()
            .execute(
                "INSERT OR IGNORE INTO pending_threads (destination_id)
                 SELECT id FROM destinations
                 WHERE id = ?1 AND thread != ''
                   AND NOT EXISTS (
                     SELECT 1 FROM seen_threads s
                     WHERE s.messenger = destinations.messenger
                       AND s.channel = destinations.channel
                       AND s.thread = destinations.thread
                   )",
                [destination as i64],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn clear_pending_thread(&self, destination: u64) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        tx.execute(
            "INSERT OR IGNORE INTO seen_threads (messenger, channel, thread)
             SELECT messenger, channel, thread FROM destinations WHERE id = ?1 AND thread != ''",
            [destination as i64],
        )
        .map_err(backend)?;
        tx.execute(
            "DELETE FROM pending_threads WHERE destination_id = ?1",
            [destination as i64],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(())
    }

    fn clear_pending_thread_by_place(
        &self,
        messenger: &str,
        channel: &str,
        thread: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        // Record in `seen_threads` without creating a `destinations` row for
        // threads where the agent has never admitted a message.
        tx.execute(
            "INSERT OR IGNORE INTO seen_threads (messenger, channel, thread) VALUES (?1, ?2, ?3)",
            params![messenger, channel, thread],
        )
        .map_err(backend)?;
        tx.execute(
            "DELETE FROM pending_threads WHERE destination_id IN (SELECT id FROM destinations WHERE messenger = ?1 AND channel = ?2 AND thread = ?3)",
            params![messenger, channel, thread],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
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

fn normalize_access_scope(scope: AccessScope, scope_id: Option<&str>) -> Result<&str, StoreError> {
    match (scope, scope_id) {
        (AccessScope::Global, None | Some("")) => Ok(""),
        (AccessScope::Global, Some(_)) => Err(StoreError::Backend(
            "global access entries cannot specify a scope id".into(),
        )),
        (AccessScope::Guild | AccessScope::Channel, Some(id)) if !id.is_empty() => Ok(id),
        (AccessScope::Guild, _) => Err(StoreError::Backend(
            "guild access entries require a guild id".into(),
        )),
        (AccessScope::Channel, _) => Err(StoreError::Backend(
            "channel access entries require a channel id".into(),
        )),
    }
}

fn normalize_guild(scope: InteractionScope, guild_id: Option<&str>) -> Result<&str, StoreError> {
    match (scope, guild_id) {
        (InteractionScope::Channel | InteractionScope::Server, None | Some("")) => Ok(""),
        (InteractionScope::Channel | InteractionScope::Server, Some(_)) => Err(
            StoreError::Backend("a guild id applies only to user interaction overrides".into()),
        ),
        (InteractionScope::User, Some(g)) if !g.is_empty() => Ok(g),
        (InteractionScope::User, _) => Err(StoreError::Backend(
            "user interaction overrides require a guild id".into(),
        )),
    }
}

/// Clearing allows `None` on `User` so legacy unassigned migrated rows (`guild_id = ''`)
/// can be removed cleanly.
fn normalize_clear_guild(
    scope: InteractionScope,
    guild_id: Option<&str>,
) -> Result<&str, StoreError> {
    match (scope, guild_id) {
        (InteractionScope::Channel | InteractionScope::Server, None | Some("")) => Ok(""),
        (InteractionScope::Channel | InteractionScope::Server, Some(_)) => Err(
            StoreError::Backend("a guild id applies only to user interaction overrides".into()),
        ),
        (InteractionScope::User, None | Some("")) => Ok(""),
        (InteractionScope::User, Some(g)) => Ok(g),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interaction_modes_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("blob-1-messenger-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();

        assert!(store.interaction_modes("discord").unwrap().is_empty());
        // User overrides require a guild id; channel overrides forbid one.
        assert!(store
            .set_interaction_mode(
                "discord",
                InteractionScope::User,
                None,
                "u1",
                InteractionMode::Always,
            )
            .is_err());
        assert!(store
            .set_interaction_mode(
                "discord",
                InteractionScope::Channel,
                Some("g1"),
                "c1",
                InteractionMode::Always,
            )
            .is_err());
        // Server overrides forbid a guild id too: the target id IS the guild.
        assert!(store
            .set_interaction_mode(
                "discord",
                InteractionScope::Server,
                Some("g1"),
                "g1",
                InteractionMode::Always,
            )
            .is_err());

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
        // A server override covers a whole guild; `auto` is a storable mode.
        store
            .set_interaction_mode(
                "discord",
                InteractionScope::Server,
                None,
                "g9",
                InteractionMode::Auto,
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
                    scope: InteractionScope::Server,
                    guild_id: None,
                    target_id: "g9".into(),
                    mode: InteractionMode::Auto,
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
        assert!(store
            .clear_interaction_mode("discord", InteractionScope::Server, None, "g9")
            .unwrap());
        assert!(store
            .clear_interaction_mode("discord", InteractionScope::Server, Some("g9"), "g9")
            .is_err());
        assert!(store
            .clear_interaction_mode("discord", InteractionScope::User, Some("g1"), "u1")
            .unwrap());
        assert!(!store
            .clear_interaction_mode("discord", InteractionScope::User, Some("g1"), "u1")
            .unwrap());
        // Clearing g1 leaves g2 intact.
        assert_eq!(store.interaction_modes("discord").unwrap().len(), 3);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn messenger_defaults_round_trip() {
        let dir = std::env::temp_dir().join(format!("blob-1-defaults-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();

        // Fresh database: no row, so the built-in defaults apply.
        assert_eq!(
            store.messenger_defaults("discord").unwrap(),
            MessengerDefaults {
                interaction_mode: InteractionMode::Auto,
                reply_mode: ReplyMode::Auto,
            }
        );
        // Each setter writes its own column without clobbering the other.
        store
            .set_default_interaction_mode("discord", InteractionMode::MentionOnly)
            .unwrap();
        assert_eq!(
            store.messenger_defaults("discord").unwrap(),
            MessengerDefaults {
                interaction_mode: InteractionMode::MentionOnly,
                reply_mode: ReplyMode::Auto,
            }
        );
        store
            .set_default_reply_mode("discord", ReplyMode::Direct)
            .unwrap();
        assert_eq!(
            store.messenger_defaults("discord").unwrap(),
            MessengerDefaults {
                interaction_mode: InteractionMode::MentionOnly,
                reply_mode: ReplyMode::Direct,
            }
        );
        // Rows are scoped per messenger.
        assert_eq!(
            store.messenger_defaults("slack").unwrap(),
            MessengerDefaults::default()
        );
        drop(store);

        // Defaults persist across reopens.
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();
        assert_eq!(
            store.messenger_defaults("discord").unwrap(),
            MessengerDefaults {
                interaction_mode: InteractionMode::MentionOnly,
                reply_mode: ReplyMode::Direct,
            }
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_legacy_interaction_modes_table() {
        let dir =
            std::env::temp_dir().join(format!("blob-1-messenger-migrate-{}", std::process::id()));
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
        assert!(store
            .clear_interaction_mode("discord", InteractionScope::User, None, "u1")
            .unwrap());
        assert_eq!(store.interaction_modes("discord").unwrap().len(), 1);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_legacy_reply_modes_and_defaults() {
        let dir =
            std::env::temp_dir().join(format!("blob-1-reply-modes-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("agent.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE reply_modes (
                    messenger TEXT NOT NULL,
                    channel_id TEXT NOT NULL,
                    mode TEXT NOT NULL CHECK (mode IN ('decision', 'thread', 'direct')),
                    PRIMARY KEY (messenger, channel_id)
                ) STRICT;
                INSERT INTO reply_modes VALUES ('discord', 'c1', 'decision');
                INSERT INTO reply_modes VALUES ('discord', 'c2', 'direct');
                CREATE TABLE messenger_defaults (
                    messenger TEXT PRIMARY KEY,
                    interaction_mode TEXT NOT NULL CHECK (interaction_mode IN ('mention-only', 'always', 'auto')),
                    reply_mode TEXT NOT NULL CHECK (reply_mode IN ('decision', 'thread', 'direct'))
                ) STRICT;
                INSERT INTO messenger_defaults VALUES ('discord', 'always', 'decision');",
            )
            .unwrap();
        }
        let store = SqliteStorage::open(&db, None).unwrap();
        // Channel rows keep their target and mode; `decision` rewrites to `auto`.
        assert_eq!(
            store.reply_modes("discord").unwrap(),
            vec![
                ReplyOverride {
                    scope: ReplyScope::Channel,
                    target_id: "c1".into(),
                    mode: ReplyMode::Auto,
                },
                ReplyOverride {
                    scope: ReplyScope::Channel,
                    target_id: "c2".into(),
                    mode: ReplyMode::Direct,
                },
            ]
        );
        assert_eq!(
            store.messenger_defaults("discord").unwrap(),
            MessengerDefaults {
                interaction_mode: InteractionMode::Always,
                reply_mode: ReplyMode::Auto,
            }
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reply_modes_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("blob-1-reply-modes-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();

        assert!(store.reply_modes("discord").unwrap().is_empty());
        store
            .set_reply_mode("discord", ReplyScope::Channel, "c2", ReplyMode::Direct)
            .unwrap();
        store
            .set_reply_mode("discord", ReplyScope::Channel, "c1", ReplyMode::Thread)
            .unwrap();
        store
            .set_reply_mode("discord", ReplyScope::Channel, "c2", ReplyMode::Auto)
            .unwrap();
        store
            .set_reply_mode("discord", ReplyScope::Server, "g1", ReplyMode::Thread)
            .unwrap();
        store
            .set_reply_mode("slack", ReplyScope::Channel, "c9", ReplyMode::Direct)
            .unwrap();

        // Channel and server rows are independent keys; scope sorts channels first.
        assert_eq!(
            store.reply_modes("discord").unwrap(),
            vec![
                ReplyOverride {
                    scope: ReplyScope::Channel,
                    target_id: "c1".into(),
                    mode: ReplyMode::Thread,
                },
                ReplyOverride {
                    scope: ReplyScope::Channel,
                    target_id: "c2".into(),
                    mode: ReplyMode::Auto,
                },
                ReplyOverride {
                    scope: ReplyScope::Server,
                    target_id: "g1".into(),
                    mode: ReplyMode::Thread,
                },
            ]
        );
        assert!(store
            .clear_reply_mode("discord", ReplyScope::Channel, "c1")
            .unwrap());
        assert!(!store
            .clear_reply_mode("discord", ReplyScope::Channel, "c1")
            .unwrap());
        // Clearing a channel row leaves the server row intact.
        assert_eq!(store.reply_modes("discord").unwrap().len(), 2);
        assert!(store
            .clear_reply_mode("discord", ReplyScope::Server, "g1")
            .unwrap());
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
        // Once seen, a later `mark_pending_thread` cannot re-arm pending state.
        store.mark_pending_thread(dest).unwrap();
        assert!(!store.is_pending_thread(dest).unwrap());

        // Observing a thread before its destination exists does NOT create a
        // `destinations` row, yet still blocks a later `mark_pending_thread`.
        store
            .clear_pending_thread_by_place("discord", "c2", "m_raced")
            .unwrap();
        let dest_count: i64 = store
            .lock()
            .query_row("SELECT COUNT(*) FROM destinations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(dest_count, 1);
        let raced = store
            .destination(
                "discord",
                &Place {
                    channel: "c2".into(),
                    thread: Some("m_raced".into()),
                    dm: false,
                },
            )
            .unwrap();
        store.mark_pending_thread(raced).unwrap();
        assert!(!store.is_pending_thread(raced).unwrap());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backfills_pending_threads_for_legacy_db() {
        let dir =
            std::env::temp_dir().join(format!("blob-1-pending-backfill-{}", std::process::id()));
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

    #[test]
    fn access_modes_and_legacy_migration() {
        let dir = std::env::temp_dir().join(format!("blob-1-access-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("agent.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE allowed_users (messenger TEXT NOT NULL, user_id TEXT NOT NULL, PRIMARY KEY (messenger, user_id)) STRICT;
                 INSERT INTO allowed_users VALUES ('discord', 'u_legacy');",
            )
            .unwrap();
        }
        // Opening in shared mode when no server holds the lock upgrades cleanly.
        let store = SqliteStorage::open_shared(&db, None).unwrap();
        assert_eq!(store.access_mode("discord").unwrap(), AccessMode::Whitelist);
        assert_eq!(store.allowed_users("discord").unwrap(), vec!["u_legacy"]);
        assert_eq!(
            store.access_entries("discord").unwrap(),
            vec![AccessEntry {
                list: AccessList::Whitelist,
                scope: AccessScope::Global,
                scope_id: None,
                user_id: "u_legacy".into(),
            }]
        );
        // Scope validation
        assert!(store
            .add_access_entry(
                "discord",
                AccessList::Whitelist,
                AccessScope::Global,
                Some("g1"),
                "u1"
            )
            .is_err());
        assert!(store
            .add_access_entry(
                "discord",
                AccessList::Whitelist,
                AccessScope::Guild,
                None,
                "u1"
            )
            .is_err());
        assert!(store
            .add_access_entry(
                "discord",
                AccessList::Blacklist,
                AccessScope::Channel,
                None,
                "u1"
            )
            .is_err());
        // Separate whitelist and blacklist entries coexist safely across mode changes.
        store
            .add_access_entry(
                "discord",
                AccessList::Whitelist,
                AccessScope::Guild,
                Some("g1"),
                "u2",
            )
            .unwrap();
        store
            .add_access_entry(
                "discord",
                AccessList::Blacklist,
                AccessScope::Channel,
                Some("c1"),
                "u3",
            )
            .unwrap();
        store
            .set_access_mode("discord", AccessMode::Blacklist)
            .unwrap();
        assert_eq!(store.access_mode("discord").unwrap(), AccessMode::Blacklist);
        assert_eq!(store.access_entries("discord").unwrap().len(), 3);
        assert!(store
            .remove_access_entry(
                "discord",
                AccessList::Whitelist,
                AccessScope::Guild,
                Some("g1"),
                "u2"
            )
            .unwrap());
        assert!(!store
            .remove_access_entry(
                "discord",
                AccessList::Whitelist,
                AccessScope::Guild,
                Some("g1"),
                "u2"
            )
            .unwrap());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);

        // An empty legacy `allowed_users` table still requires the owner lock
        // when upgrading, so an older running server cannot race with migration.
        let dir2 =
            std::env::temp_dir().join(format!("blob-1-access-lock-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(&dir2).unwrap();
        let db2 = dir2.join("agent.db");
        {
            let conn = rusqlite::Connection::open(&db2).unwrap();
            conn.execute_batch(
                "CREATE TABLE allowed_users (messenger TEXT NOT NULL, user_id TEXT NOT NULL, PRIMARY KEY (messenger, user_id)) STRICT;",
            )
            .unwrap();
        }
        let owner_lock = std::fs::File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(dir2.join("agent.db.lock"))
            .unwrap();
        owner_lock.try_lock().unwrap();
        assert!(SqliteStorage::open_shared(&db2, None).is_err());
        drop(owner_lock);
        assert!(SqliteStorage::open_shared(&db2, None).is_ok());
        let _ = std::fs::remove_dir_all(&dir2);
    }
}
