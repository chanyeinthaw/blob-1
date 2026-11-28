use rusqlite::{params, types::Type, OptionalExtension};
use storage::{
    Bot, BotModels, BotStore, ConfigStore, Environment, EnvironmentStore, ModelRef, ProviderId,
    ReasoningLevel, StoreError, DEFAULT_BOT_ID,
};

use crate::{backend, SqliteStorage};

fn parse_opt_reasoning(
    value: Option<String>,
    col: usize,
) -> rusqlite::Result<Option<ReasoningLevel>> {
    value
        .map(|s| {
            s.parse().map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(col, Type::Text, Box::new(err))
            })
        })
        .transpose()
}

fn bot_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Bot> {
    Ok(Bot {
        id: row.get(0)?,
        name: row.get(1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        created_at: row.get(3)?,
        generation: row.get(4)?,
        environment: row.get(5)?,
    })
}

impl BotStore for SqliteStorage {
    fn bots(&self) -> Result<Vec<Bot>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, name, enabled, created_at, generation, environment
                 FROM bots
                 ORDER BY CASE WHEN id = 'default' THEN 0 ELSE 1 END, created_at ASC, id ASC",
            )
            .map_err(backend)?;
        let rows = stmt
            .query_map([], bot_from_row)
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        Ok(rows)
    }

    fn bot(&self, id: &str) -> Result<Option<Bot>, StoreError> {
        self.lock()
            .query_row(
                "SELECT id, name, enabled, created_at, generation, environment FROM bots WHERE id = ?1",
                [id],
                bot_from_row,
            )
            .optional()
            .map_err(backend)
    }

    fn create_bot(&self, id: &str, name: &str, created_at: i64) -> Result<Bot, StoreError> {
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        let generation: i64 = tx
            .query_row(
                "UPDATE bot_sequence SET next_gen = next_gen + 1 WHERE id = 1 RETURNING next_gen - 1",
                [],
                |row| row.get(0),
            )
            .map_err(backend)?;
        tx.execute(
            "INSERT INTO bots (id, name, enabled, created_at, generation) VALUES (?1, ?2, 1, ?3, ?4)",
            params![id, name, created_at, generation],
        )
        .map_err(backend)?;
        tx.execute(
            "INSERT OR IGNORE INTO bot_models (bot_id) VALUES (?1)",
            [id],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(Bot {
            id: id.to_owned(),
            name: name.to_owned(),
            enabled: true,
            created_at,
            generation,
            environment: storage::LOCAL_ENVIRONMENT.to_owned(),
        })
    }

    fn update_bot(
        &self,
        id: &str,
        name: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<Option<Bot>, StoreError> {
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend)?;
        let Some(mut existing) = tx
            .query_row(
                "SELECT id, name, enabled, created_at, generation, environment FROM bots WHERE id = ?1",
                [id],
                bot_from_row,
            )
            .optional()
            .map_err(backend)?
        else {
            return Ok(None);
        };
        if let Some(new_name) = name {
            existing.name = new_name.to_owned();
        }
        if let Some(new_enabled) = enabled {
            existing.enabled = new_enabled;
        }
        tx.execute(
            "UPDATE bots SET name = ?2, enabled = ?3 WHERE id = ?1",
            params![id, &existing.name, i64::from(existing.enabled)],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(Some(existing))
    }

    fn remove_bot(&self, id: &str) -> Result<bool, StoreError> {
        if id == DEFAULT_BOT_ID {
            return Err(StoreError::Backend("cannot remove the default bot".into()));
        }
        let deleted = self
            .lock()
            .execute("DELETE FROM bots WHERE id = ?1", [id])
            .map_err(backend)?;
        Ok(deleted > 0)
    }

    fn set_bot_avatar(
        &self,
        bot_id: &str,
        data: &[u8],
        content_type: &str,
        updated_at: i64,
    ) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO bot_avatars (bot_id, data, content_type, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![bot_id, data, content_type, updated_at],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn bot_avatar(&self, bot_id: &str) -> Result<Option<(Vec<u8>, String)>, StoreError> {
        self.lock()
            .query_row(
                "SELECT data, content_type FROM bot_avatars WHERE bot_id = ?1",
                [bot_id],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(backend)
    }

    fn clear_bot_avatar(&self, bot_id: &str) -> Result<(), StoreError> {
        self.lock()
            .execute("DELETE FROM bot_avatars WHERE bot_id = ?1", [bot_id])
            .map_err(backend)?;
        Ok(())
    }

    fn bot_avatar_versions(&self) -> Result<Vec<(String, i64)>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT bot_id, updated_at FROM bot_avatars")
            .map_err(backend)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        Ok(rows)
    }

    fn bot_models(&self, bot_id: &str) -> Result<BotModels, StoreError> {
        if bot_id == DEFAULT_BOT_ID {
            let s = self.settings()?;
            return Ok(BotModels {
                main: s.default_model,
                main_reasoning: s.reasoning,
                compaction: s.compaction_model,
                compaction_reasoning: s.compaction_reasoning,
                utility: self.utility_model()?,
                decision: self.decision_model()?,
            });
        }
        let row = self
            .lock()
            .query_row(
                "SELECT default_provider, default_model, reasoning,
                        compaction_provider, compaction_model, compaction_reasoning,
                        utility_provider, utility_model, utility_reasoning,
                        decision_provider, decision_model
                 FROM bot_models
                 WHERE bot_id = ?1",
                [bot_id],
                |row| {
                    let main_p: Option<String> = row.get(0)?;
                    let main_m: Option<String> = row.get(1)?;
                    let main_r_str: String = row.get(2)?;
                    let main_reasoning: ReasoningLevel = main_r_str.parse().map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(2, Type::Text, Box::new(err))
                    })?;
                    let comp_p: Option<String> = row.get(3)?;
                    let comp_m: Option<String> = row.get(4)?;
                    let compaction_reasoning = parse_opt_reasoning(row.get(5)?, 5)?;
                    let util_p: Option<String> = row.get(6)?;
                    let util_m: Option<String> = row.get(7)?;
                    let utility_reasoning = parse_opt_reasoning(row.get(8)?, 8)?;
                    let dec_p: Option<String> = row.get(9)?;
                    let dec_m: Option<String> = row.get(10)?;
                    Ok(BotModels {
                        main: main_p.zip(main_m).map(|(p, m)| ModelRef {
                            provider: ProviderId(p),
                            model: m,
                        }),
                        main_reasoning,
                        compaction: comp_p.zip(comp_m).map(|(p, m)| ModelRef {
                            provider: ProviderId(p),
                            model: m,
                        }),
                        compaction_reasoning,
                        utility: util_p.zip(util_m).map(|(p, m)| {
                            (
                                ModelRef {
                                    provider: ProviderId(p),
                                    model: m,
                                },
                                utility_reasoning,
                            )
                        }),
                        decision: dec_p.zip(dec_m).map(|(p, m)| ModelRef {
                            provider: ProviderId(p),
                            model: m,
                        }),
                    })
                },
            )
            .optional()
            .map_err(backend)?;
        Ok(row.unwrap_or_default())
    }

    fn set_bot_main_model(
        &self,
        bot_id: &str,
        model: Option<&ModelRef>,
        reasoning: ReasoningLevel,
    ) -> Result<(), StoreError> {
        if bot_id == DEFAULT_BOT_ID {
            let mut s = self.settings()?;
            s.default_model = model.cloned();
            s.reasoning = reasoning;
            return self.save_settings(s);
        }
        let (p, m) = model
            .map(|r| (Some(r.provider.0.as_str()), Some(r.model.as_str())))
            .unwrap_or((None, None));
        self.lock()
            .execute(
                "INSERT INTO bot_models (bot_id, default_provider, default_model, reasoning)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (bot_id) DO UPDATE SET
                     default_provider = excluded.default_provider,
                     default_model = excluded.default_model,
                     reasoning = excluded.reasoning",
                params![bot_id, p, m, reasoning.as_str()],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn set_bot_compaction_model(
        &self,
        bot_id: &str,
        model: Option<&ModelRef>,
        reasoning: Option<ReasoningLevel>,
    ) -> Result<(), StoreError> {
        if bot_id == DEFAULT_BOT_ID {
            let mut s = self.settings()?;
            s.compaction_model = model.cloned();
            s.compaction_reasoning = if model.is_some() { reasoning } else { None };
            return self.save_settings(s);
        }
        let (p, m, r) = match model {
            Some(r_ref) => (
                Some(r_ref.provider.0.as_str()),
                Some(r_ref.model.as_str()),
                reasoning.map(ReasoningLevel::as_str),
            ),
            None => (None, None, None),
        };
        self.lock()
            .execute(
                "INSERT INTO bot_models (bot_id, compaction_provider, compaction_model, compaction_reasoning)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (bot_id) DO UPDATE SET
                     compaction_provider = excluded.compaction_provider,
                     compaction_model = excluded.compaction_model,
                     compaction_reasoning = excluded.compaction_reasoning",
                params![bot_id, p, m, r],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn set_bot_utility_model(
        &self,
        bot_id: &str,
        model: Option<(&ModelRef, Option<ReasoningLevel>)>,
    ) -> Result<(), StoreError> {
        if bot_id == DEFAULT_BOT_ID {
            return self.set_utility_model(model.map(|(m, r)| (m.clone(), r)));
        }
        let (p, m, r) = match model {
            Some((r_ref, reasoning)) => (
                Some(r_ref.provider.0.as_str()),
                Some(r_ref.model.as_str()),
                reasoning.map(ReasoningLevel::as_str),
            ),
            None => (None, None, None),
        };
        self.lock()
            .execute(
                "INSERT INTO bot_models (bot_id, utility_provider, utility_model, utility_reasoning)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (bot_id) DO UPDATE SET
                     utility_provider = excluded.utility_provider,
                     utility_model = excluded.utility_model,
                     utility_reasoning = excluded.utility_reasoning",
                params![bot_id, p, m, r],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn set_bot_decision_model(
        &self,
        bot_id: &str,
        model: Option<&ModelRef>,
    ) -> Result<(), StoreError> {
        if bot_id == DEFAULT_BOT_ID {
            return self.set_decision_model(model);
        }
        let (p, m) = model
            .map(|r| (Some(r.provider.0.as_str()), Some(r.model.as_str())))
            .unwrap_or((None, None));
        self.lock()
            .execute(
                "INSERT INTO bot_models (bot_id, decision_provider, decision_model)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (bot_id) DO UPDATE SET
                     decision_provider = excluded.decision_provider,
                     decision_model = excluded.decision_model",
                params![bot_id, p, m],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn provider_in_use_by_bot(&self, provider: &ProviderId) -> Result<Option<String>, StoreError> {
        let def = self.bot_models(DEFAULT_BOT_ID)?;
        let uses_default = def.main.as_ref().is_some_and(|m| &m.provider == provider)
            || def
                .compaction
                .as_ref()
                .is_some_and(|m| &m.provider == provider)
            || def
                .utility
                .as_ref()
                .is_some_and(|(m, _)| &m.provider == provider)
            || def
                .decision
                .as_ref()
                .is_some_and(|m| &m.provider == provider);
        if uses_default {
            return Ok(Some(DEFAULT_BOT_ID.to_owned()));
        }
        self.lock()
            .query_row(
                "SELECT bot_id FROM bot_models
                 WHERE default_provider = ?1
                    OR compaction_provider = ?1
                    OR utility_provider = ?1
                    OR decision_provider = ?1
                 LIMIT 1",
                [&provider.0],
                |row| row.get(0),
            )
            .optional()
            .map_err(backend)
    }

    fn bot_disabled_mcp_servers(&self, bot_id: &str) -> Result<Vec<String>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT server_id FROM bot_mcp_disabled WHERE bot_id = ?1 ORDER BY server_id")
            .map_err(backend)?;
        let ids = stmt
            .query_map([bot_id], |row| row.get(0))
            .map_err(backend)?
            .collect::<Result<Vec<String>, _>>()
            .map_err(backend)?;
        Ok(ids)
    }

    fn bot_skill_modes(
        &self,
        bot_id: &str,
    ) -> Result<std::collections::BTreeMap<String, storage::SkillMode>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT name, mode FROM bot_skill_modes WHERE bot_id = ?1")
            .map_err(backend)?;
        let rows = stmt
            .query_map([bot_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        rows.into_iter()
            .map(|(name, mode)| {
                storage::SkillMode::parse(&mode)
                    .map(|mode| (name, mode))
                    .ok_or_else(|| StoreError::Invalid(format!("invalid skill mode '{mode}'")))
            })
            .collect()
    }

    fn set_bot_skill_mode(
        &self,
        bot_id: &str,
        name: &str,
        mode: storage::SkillMode,
    ) -> Result<(), StoreError> {
        let conn = self.lock();
        match mode {
            storage::SkillMode::Both => conn.execute(
                "DELETE FROM bot_skill_modes WHERE bot_id = ?1 AND name = ?2",
                params![bot_id, name],
            ),
            mode => conn.execute(
                "INSERT INTO bot_skill_modes (bot_id, name, mode) VALUES (?1, ?2, ?3)
                 ON CONFLICT (bot_id, name) DO UPDATE SET mode = excluded.mode",
                params![bot_id, name, mode.as_str()],
            ),
        }
        .map_err(backend)?;
        Ok(())
    }

    fn clear_skill_modes(&self, name: &str, bot_id: Option<&str>) -> Result<(), StoreError> {
        let conn = self.lock();
        match bot_id {
            Some(bot_id) => conn.execute(
                "DELETE FROM bot_skill_modes WHERE bot_id = ?1 AND name = ?2",
                params![bot_id, name],
            ),
            None => conn.execute("DELETE FROM bot_skill_modes WHERE name = ?1", [name]),
        }
        .map_err(backend)?;
        Ok(())
    }

    fn set_bot_environment(
        &self,
        bot_id: &str,
        environment: Option<&str>,
    ) -> Result<bool, StoreError> {
        let environment = environment.unwrap_or(storage::LOCAL_ENVIRONMENT);
        let mut conn = self.lock();
        // Target validation and the binding share one transaction so a
        // revoke between check and write cannot strand the bot on a plain
        // name a future enrollment would silently inherit.
        let tx = conn.transaction().map_err(backend)?;
        if environment != storage::LOCAL_ENVIRONMENT {
            let accepted = tx
                .query_row(
                    "SELECT 1 FROM environments WHERE name = ?1 AND credential != x''",
                    [environment],
                    |_| Ok(()),
                )
                .optional()
                .map_err(backend)?
                .is_some();
            if !accepted {
                return Err(StoreError::Invalid(format!(
                    "unknown environment '{environment}'"
                )));
            }
        }
        let changed = tx
            .execute(
                "UPDATE bots SET environment = ?2 WHERE id = ?1",
                params![bot_id, environment],
            )
            .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(changed > 0)
    }

    fn bot_env_binding(
        &self,
        bot_id: &str,
    ) -> Result<Option<(String, Option<Vec<u8>>)>, StoreError> {
        // Binding name and the bound env's enrollment in one row — a
        // revoke+re-accept between separate reads can't split them.
        self.lock()
            .query_row(
                "SELECT b.environment, e.enrollment
                 FROM bots b
                 LEFT JOIN environments e
                   ON e.name = b.environment AND length(e.credential) > 0
                 WHERE b.id = ?1",
                [bot_id],
                |row| Ok((row.get::<_, String>(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(backend)
    }

    fn set_bot_mcp_server_enabled(
        &self,
        bot_id: &str,
        server_id: &str,
        enabled: bool,
    ) -> Result<(), StoreError> {
        let conn = self.lock();
        if enabled {
            conn.execute(
                "DELETE FROM bot_mcp_disabled WHERE bot_id = ?1 AND server_id = ?2",
                params![bot_id, server_id],
            )
            .map_err(backend)?;
        } else {
            conn.execute(
                "INSERT OR IGNORE INTO bot_mcp_disabled (bot_id, server_id) VALUES (?1, ?2)",
                params![bot_id, server_id],
            )
            .map_err(backend)?;
        }
        Ok(())
    }
}

fn environment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Environment> {
    Ok(Environment {
        name: row.get(0)?,
        node_id: row.get(1)?,
        credential: row.get(2)?,
        pending_key: row.get(3)?,
        enrollment: row.get::<_, Option<Vec<u8>>>(4)?.unwrap_or_default(),
        hostname: row.get(5)?,
        version: row.get(6)?,
        last_seen: row.get(7)?,
        created_at: row.get(8)?,
    })
}

const ENVIRONMENT_COLS: &str =
    "name, node_id, credential, pending_key, enrollment, hostname, version, last_seen, created_at";

impl EnvironmentStore for SqliteStorage {
    fn environments(&self) -> Result<Vec<Environment>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {ENVIRONMENT_COLS} FROM environments ORDER BY name"
            ))
            .map_err(backend)?;
        let rows = stmt
            .query_map([], environment_from_row)
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        Ok(rows)
    }

    fn environment(&self, name: &str) -> Result<Option<Environment>, StoreError> {
        self.lock()
            .query_row(
                &format!("SELECT {ENVIRONMENT_COLS} FROM environments WHERE name = ?1"),
                [name],
                environment_from_row,
            )
            .optional()
            .map_err(backend)
    }

    fn environment_for_node(&self, node_id: &str) -> Result<Option<Environment>, StoreError> {
        self.lock()
            .query_row(
                &format!("SELECT {ENVIRONMENT_COLS} FROM environments WHERE node_id = ?1"),
                [node_id],
                environment_from_row,
            )
            .optional()
            .map_err(backend)
    }

    fn environment_for_key(&self, key_hash: &[u8]) -> Result<Option<Environment>, StoreError> {
        self.lock()
            .query_row(
                &format!("SELECT {ENVIRONMENT_COLS} FROM environments WHERE credential = ?1 AND length(credential) > 0"),
                [key_hash],
                environment_from_row,
            )
            .optional()
            .map_err(backend)
    }

    fn rename_environment(
        &self,
        from: &str,
        to: &str,
        expected_enrollment: &[u8],
    ) -> Result<Option<Environment>, StoreError> {
        if !storage::environment_name_valid(to) {
            return Err(StoreError::Invalid(format!(
                "'{to}' is a reserved environment name"
            )));
        }
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(backend)?;
        // RETURNING gives back the row actually renamed — a later lookup by
        // `to` could resolve a different node's recycled name. Accepted
        // rows only, and only the one carrying `expected_enrollment`: a
        // `pending:<id>` rename would strand the pending node, and a name
        // recycled for another node's row must not move.
        let renamed = tx
            .query_row(
                &format!(
                    "UPDATE environments SET name = ?2 WHERE name = ?1 AND length(credential) > 0 AND COALESCE(enrollment, x'') = ?3 RETURNING {ENVIRONMENT_COLS}"
                ),
                params![from, to, expected_enrollment],
                environment_from_row,
            )
            .optional()
            .map_err(backend)?;
        if renamed.is_some() {
            // Bound bots follow the rename so they rebind rather than pause on
            // a dangling name.
            tx.execute(
                "UPDATE bots SET environment = ?2 WHERE environment = ?1",
                params![from, to],
            )
            .map_err(backend)?;
        }
        tx.commit().map_err(backend)?;
        Ok(renamed)
    }

    fn remove_environment(
        &self,
        name: &str,
        expected_enrollment: &[u8],
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(backend)?;
        // `enrollment` pins the delete to the row the caller read — a
        // recycled name under another node doesn't match.
        let changed = tx
            .execute(
                "DELETE FROM environments WHERE name = ?1 AND COALESCE(enrollment, x'') = ?2",
                params![name, expected_enrollment],
            )
            .map_err(backend)?;
        if changed > 0 {
            // Tombstone bound bots so a re-enrolled node under the same name
            // can't silently inherit them — the operator rebinds explicitly.
            tx.execute(
                "UPDATE bots SET environment = ?2 WHERE environment = ?1",
                params![name, storage::revoked_environment(name)],
            )
            .map_err(backend)?;
        }
        tx.commit().map_err(backend)?;
        Ok(changed > 0)
    }

    fn upsert_pending_node(
        &self,
        node_id: &str,
        hostname: &str,
        version: &str,
        key_hash: &[u8],
        seen_at: i64,
    ) -> Result<bool, StoreError> {
        // `pending_key = ?5` in the WHERE pins the whole row to the first key:
        // a reconnect with a different key changes nothing the operator saw.
        let changed = self
            .lock()
            .execute(
                "INSERT INTO environments (name, node_id, credential, pending_key, hostname, version, last_seen, created_at)                  VALUES (?1, ?2, x'', ?5, ?6, ?3, ?4, ?4)                  ON CONFLICT(node_id) DO UPDATE SET version = ?3, last_seen = ?4, hostname = ?6                  WHERE credential = x'' AND pending_key = ?5",
                params![format!("pending:{node_id}"), node_id, version, seen_at, key_hash, hostname],
            )
            .map_err(backend)?;
        Ok(changed > 0)
    }

    fn drop_pending_node(
        &self,
        node_id: &str,
        expected_pending_key: &[u8],
    ) -> Result<bool, StoreError> {
        // `pending_key = ?2` pins the delete to the row the caller
        // reviewed — a re-registration under a different key survives.
        let changed = self
            .lock()
            .execute(
                "DELETE FROM environments WHERE node_id = ?1 AND credential = x'' AND pending_key = ?2",
                params![node_id, expected_pending_key],
            )
            .map_err(backend)?;
        Ok(changed > 0)
    }

    fn accept_pending_node(
        &self,
        node_id: &str,
        name: &str,
        expected_pending_key: &[u8],
    ) -> Result<Option<Environment>, StoreError> {
        if !storage::environment_name_valid(name) {
            return Err(StoreError::Invalid(format!(
                "'{name}' is a reserved environment name"
            )));
        }
        // `pending_key = ?3` pins the promotion to the row the caller
        // reviewed — a deny + different-key re-register can't be adopted.
        // RETURNING hands back the promoted row itself; a follow-up SELECT
        // could miss or read a replacement after a concurrent revoke.
        self.lock()
            .query_row(
                &format!(
                    "UPDATE environments SET name = ?2, credential = pending_key, pending_key = NULL, enrollment = randomblob(16)                     WHERE node_id = ?1 AND credential = x'' AND pending_key = ?3                     RETURNING {ENVIRONMENT_COLS}"
                ),
                params![node_id, name, expected_pending_key],
                environment_from_row,
            )
            .optional()
            .map_err(backend)
    }

    fn expire_pending_nodes(&self, before: i64) -> Result<Vec<String>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("DELETE FROM environments WHERE credential = x'' AND last_seen < ?1 RETURNING node_id")
            .map_err(backend)?;
        let ids = stmt
            .query_map([before], |row| row.get(0))
            .map_err(backend)?
            .collect::<Result<Vec<String>, _>>()
            .map_err(backend)?;
        Ok(ids)
    }

    fn touch_environment(
        &self,
        node_id: &str,
        version: &str,
        last_seen: i64,
    ) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "UPDATE environments SET version = ?2, last_seen = ?3 WHERE node_id = ?1",
                params![node_id, version, last_seen],
            )
            .map_err(backend)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage::{DiscoveredModel, Provider, ProviderApi};

    #[test]
    fn bot_crud_models_and_mcp_toggles() {
        let dir = std::env::temp_dir().join(format!("blob-1-bot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();

        // Default bot is seeded automatically.
        let bots = store.bots().unwrap();
        assert_eq!(bots.len(), 1);
        assert_eq!(bots[0].id, DEFAULT_BOT_ID);
        assert!(store.remove_bot(DEFAULT_BOT_ID).is_err());

        // Create second bot, assign models from shared catalog, verify provider_in_use.
        let sunday = store.create_bot("sunday", "Sunday", 100).unwrap();
        assert_eq!(sunday.name, "Sunday");
        assert!(sunday.enabled);
        let pid = ProviderId("cpa1".into());
        store
            .save_provider(
                Provider {
                    id: pid.clone(),
                    api: ProviderApi::OpenAiResponses,
                    base_url: "http://127.0.0.1:8320/v1".into(),
                    has_api_key: false,
                },
                None,
            )
            .unwrap();
        store
            .replace_models(
                &pid,
                vec![DiscoveredModel {
                    id: "gpt-6.1-terra".into(),
                    context_window: Some(128_000),
                }],
            )
            .unwrap();
        let mref = ModelRef {
            provider: pid.clone(),
            model: "gpt-6.1-terra".into(),
        };
        store
            .set_bot_main_model("sunday", Some(&mref), ReasoningLevel::High)
            .unwrap();
        let bm = store.bot_models("sunday").unwrap();
        assert_eq!(bm.main, Some(mref));
        assert_eq!(bm.main_reasoning, ReasoningLevel::High);
        assert_eq!(
            store.provider_in_use_by_bot(&pid).unwrap().as_deref(),
            Some("sunday")
        );
        // Atomic remove_provider guard rejects deletion while any bot references it (CLI + web).
        assert!(store.remove_provider(&pid).is_err());

        // Update name / enabled and remove bot.
        let updated = store
            .update_bot("sunday", Some("Sunday Dev"), Some(false))
            .unwrap()
            .unwrap();
        assert_eq!(updated.name, "Sunday Dev");
        assert!(!updated.enabled);
        assert!(store.remove_bot("sunday").unwrap());
        assert_eq!(store.provider_in_use_by_bot(&pid).unwrap(), None);
        store.remove_provider(&pid).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bot_avatar_roundtrip_and_cascade() {
        let dir = std::env::temp_dir().join(format!("blob-1-avatar-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();

        // None until set; versions list empty.
        assert_eq!(store.bot_avatar("sunday").unwrap(), None);
        assert!(store.bot_avatar_versions().unwrap().is_empty());

        store.create_bot("sunday", "Sunday", 100).unwrap();
        store
            .set_bot_avatar("sunday", b"png-bytes", "image/png", 1000)
            .unwrap();
        assert_eq!(
            store.bot_avatar("sunday").unwrap(),
            Some((b"png-bytes".to_vec(), "image/png".to_owned()))
        );
        assert_eq!(
            store.bot_avatar_versions().unwrap(),
            vec![("sunday".to_owned(), 1000)]
        );

        // Replace bumps the version; clear removes the row.
        store
            .set_bot_avatar("sunday", b"webp-bytes", "image/webp", 2000)
            .unwrap();
        assert_eq!(
            store.bot_avatar_versions().unwrap(),
            vec![("sunday".to_owned(), 2000)]
        );
        store.clear_bot_avatar("sunday").unwrap();
        assert_eq!(store.bot_avatar("sunday").unwrap(), None);

        // Removing the bot cascades its avatar row away.
        store
            .set_bot_avatar("sunday", b"png-bytes", "image/png", 3000)
            .unwrap();
        assert!(store.remove_bot("sunday").unwrap());
        assert_eq!(store.bot_avatar("sunday").unwrap(), None);
        assert!(store.bot_avatar_versions().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_enabled_defaults_disabled_seeds_assigned_and_toggles() {
        let dir = std::env::temp_dir().join(format!("blob-1-enabled-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.db");
        let store = SqliteStorage::open(&path, None).unwrap();
        let pid = ProviderId("cpa1".into());
        store
            .save_provider(
                Provider {
                    id: pid.clone(),
                    api: ProviderApi::OpenAiResponses,
                    base_url: "http://127.0.0.1:8320/v1".into(),
                    has_api_key: false,
                },
                None,
            )
            .unwrap();
        store
            .replace_models(
                &pid,
                vec![
                    DiscoveredModel {
                        id: "gpt-6.1-terra".into(),
                        context_window: Some(128_000),
                    },
                    DiscoveredModel {
                        id: "gpt-6-luna".into(),
                        context_window: None,
                    },
                    DiscoveredModel {
                        id: "gpt-5-terra".into(),
                        context_window: None,
                    },
                ],
            )
            .unwrap();

        // Fresh models are disabled until explicitly enabled; unknown models error.
        assert!(store.models(&pid).unwrap().iter().all(|m| !m.enabled));
        store.set_enabled(&pid, "gpt-6-luna", true).unwrap();
        assert!(store
            .models(&pid)
            .unwrap()
            .iter()
            .any(|m| m.id == "gpt-6-luna" && m.enabled));
        store.set_enabled(&pid, "gpt-6-luna", false).unwrap();
        assert!(store.models(&pid).unwrap().iter().all(|m| !m.enabled));
        assert!(store.set_enabled(&pid, "nope", true).is_err());

        // Assign models to slots across both store paths, then drop the flag
        // table to simulate a database created before `model_enabled` existed.
        store
            .set_bot_main_model(
                DEFAULT_BOT_ID,
                Some(&ModelRef {
                    provider: pid.clone(),
                    model: "gpt-6.1-terra".into(),
                }),
                ReasoningLevel::Medium,
            )
            .unwrap();
        store.create_bot("sunday", "Sunday", 100).unwrap();
        store
            .set_bot_utility_model(
                "sunday",
                Some((
                    &ModelRef {
                        provider: pid.clone(),
                        model: "gpt-6-luna".into(),
                    },
                    None,
                )),
            )
            .unwrap();
        {
            let conn = store.lock();
            conn.execute_batch("DROP TABLE model_enabled").unwrap();
        }
        drop(store);

        // Reopen seeds only the slot-assigned models; unassigned stay disabled.
        let store = SqliteStorage::open(&path, None).unwrap();
        let models = store.models(&pid).unwrap();
        let enabled = |id: &str| models.iter().find(|m| m.id == id).unwrap().enabled;
        assert!(enabled("gpt-6.1-terra"));
        assert!(enabled("gpt-6-luna"));
        assert!(!enabled("gpt-5-terra"));

        // With the table present the seed must not rerun: a later explicit
        // disable of a slot-assigned model survives reopening.
        store.set_enabled(&pid, "gpt-6-luna", false).unwrap();
        drop(store);
        let store = SqliteStorage::open(&path, None).unwrap();
        let models = store.models(&pid).unwrap();
        let enabled = |id: &str| models.iter().find(|m| m.id == id).unwrap().enabled;
        assert!(enabled("gpt-6.1-terra"));
        assert!(!enabled("gpt-6-luna"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
