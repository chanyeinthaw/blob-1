use std::collections::{BTreeMap, HashSet};

use rusqlite::{params, types::Type, Connection, OptionalExtension, Row};
use storage::{
    ConfigStore, DiscoveredModel, Model, ModelRef, Provider, ProviderApi, ProviderId,
    ReasoningLevel, Settings, StoreError,
};

use crate::{backend, SqliteStorage};

impl ConfigStore for SqliteStorage {
    fn timezone(&self) -> Result<String, StoreError> {
        Ok(self
            .lock()
            .query_row("SELECT timezone FROM clock WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(backend)?
            .unwrap_or_else(|| "UTC".into()))
    }

    fn set_timezone(&self, timezone: &str) -> Result<(), StoreError> {
        timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| StoreError::Backend(format!("invalid timezone: {timezone}").into()))?;
        self.lock().execute("INSERT INTO clock (id, timezone) VALUES (1, ?1) ON CONFLICT (id) DO UPDATE SET timezone = excluded.timezone", [timezone]).map_err(backend)?;
        Ok(())
    }

    fn home(&self) -> Result<Option<storage::Home>, StoreError> {
        self.lock()
            .query_row(
                "SELECT messenger, target FROM home WHERE id = 1",
                [],
                |row| {
                    Ok(storage::Home {
                        messenger: row.get(0)?,
                        target: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(backend)
    }

    fn set_home(&self, home: Option<&storage::Home>) -> Result<(), StoreError> {
        let connection = self.lock();
        match home {
            Some(home) => connection.execute("INSERT INTO home (id, messenger, target) VALUES (1, ?1, ?2) ON CONFLICT (id) DO UPDATE SET messenger = excluded.messenger, target = excluded.target", params![home.messenger, home.target]),
            None => connection.execute("DELETE FROM home WHERE id = 1", []),
        }.map_err(backend)?;
        Ok(())
    }

    fn identity(&self) -> Result<Option<String>, StoreError> {
        self.lock()
            .query_row("SELECT text FROM identity WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(backend)
    }

    fn set_identity(&self, text: Option<&str>) -> Result<(), StoreError> {
        let connection = self.lock();
        match text {
            Some(text) => connection.execute("INSERT INTO identity (id, text) VALUES (1, ?1) ON CONFLICT (id) DO UPDATE SET text = excluded.text", [text]),
            None => connection.execute("DELETE FROM identity WHERE id = 1", []),
        }.map_err(backend)?;
        Ok(())
    }
    fn providers(&self) -> Result<Vec<Provider>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT id, api, base_url, api_key_env FROM providers ORDER BY id")
            .map_err(backend)?;
        let providers = statement
            .query_map([], provider_from_row)
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        Ok(providers)
    }

    fn save_provider(&self, provider: Provider) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO providers (id, api, base_url, api_key_env) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET
                     api = excluded.api,
                     base_url = excluded.base_url,
                     api_key_env = excluded.api_key_env",
                params![
                    provider.id.0,
                    api_name(provider.api),
                    provider.base_url,
                    provider.api_key_env
                ],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn remove_provider(&self, id: &ProviderId) -> Result<(), StoreError> {
        self.lock()
            .execute("DELETE FROM providers WHERE id = ?1", [&id.0])
            .map_err(backend)?;
        Ok(())
    }

    fn models(&self, provider: &ProviderId) -> Result<Vec<Model>, StoreError> {
        let connection = self.lock();
        let listed = connection
            .prepare(
                "SELECT id, context_window, context_limit, EXISTS(SELECT 1 FROM model_images WHERE model_images.provider = models.provider AND model_images.model = models.id) FROM models
                 WHERE provider = ?1 ORDER BY id",
            )
            .map_err(backend)?
            .query_map([&provider.0], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<u32>>(1)?,
                    row.get::<_, Option<u32>>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        let mut models = Vec::with_capacity(listed.len());
        for (id, context_window, context_limit, images) in listed {
            let reasoning_map = reasoning_map(&connection, provider, &id)?;
            models.push(Model {
                images,
                provider: provider.clone(),
                id,
                context_window,
                context_limit,
                reasoning_map,
            });
        }
        Ok(models)
    }

    fn set_images(
        &self,
        provider: &ProviderId,
        model: &str,
        images: bool,
    ) -> Result<(), StoreError> {
        let connection = self.lock();
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM models WHERE provider = ?1 AND id = ?2)",
                params![provider.0, model],
                |row| row.get(0),
            )
            .map_err(backend)?;
        if !exists {
            return Err(StoreError::Backend(
                format!("unknown model {}/{model}", provider.0).into(),
            ));
        }
        if images {
            connection
                .execute(
                    "INSERT OR IGNORE INTO model_images (provider, model) VALUES (?1, ?2)",
                    params![provider.0, model],
                )
                .map_err(backend)?;
        } else {
            connection
                .execute(
                    "DELETE FROM model_images WHERE provider = ?1 AND model = ?2",
                    params![provider.0, model],
                )
                .map_err(backend)?;
        }
        Ok(())
    }

    fn replace_models(
        &self,
        provider: &ProviderId,
        models: Vec<DiscoveredModel>,
    ) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction().map_err(backend)?;
        let kept: HashSet<&str> = models.iter().map(|model| model.id.as_str()).collect();
        let existing = transaction
            .prepare("SELECT id FROM models WHERE provider = ?1")
            .map_err(backend)?
            .query_map([&provider.0], |row| row.get::<_, String>(0))
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        for id in existing.iter().filter(|id| !kept.contains(id.as_str())) {
            transaction
                .execute(
                    "DELETE FROM models WHERE provider = ?1 AND id = ?2",
                    params![provider.0, id],
                )
                .map_err(backend)?;
        }
        for model in &models {
            transaction
                .execute(
                    "INSERT INTO models (provider, id, context_window) VALUES (?1, ?2, ?3)
                     ON CONFLICT (provider, id) DO UPDATE SET
                         context_window = excluded.context_window",
                    params![provider.0, model.id, model.context_window],
                )
                .map_err(backend)?;
        }
        transaction.commit().map_err(backend)?;
        Ok(())
    }

    fn set_context_limit(
        &self,
        provider: &ProviderId,
        model: &str,
        limit: Option<u32>,
    ) -> Result<(), StoreError> {
        let updated = self
            .lock()
            .execute(
                "UPDATE models SET context_limit = ?3 WHERE provider = ?1 AND id = ?2",
                params![provider.0, model, limit],
            )
            .map_err(backend)?;
        if updated == 0 {
            return Err(StoreError::Backend(
                format!("unknown model {}/{model}", provider.0).into(),
            ));
        }
        Ok(())
    }

    fn set_reasoning_map(
        &self,
        provider: &ProviderId,
        model: &str,
        map: BTreeMap<ReasoningLevel, Option<String>>,
    ) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction().map_err(backend)?;
        transaction
            .execute(
                "DELETE FROM model_reasoning WHERE provider = ?1 AND model = ?2",
                params![provider.0, model],
            )
            .map_err(backend)?;
        for (level, value) in map {
            transaction
                .execute(
                    "INSERT INTO model_reasoning (provider, model, level, value)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![provider.0, model, level.as_str(), value],
                )
                .map_err(backend)?;
        }
        transaction.commit().map_err(backend)?;
        Ok(())
    }

    fn utility_model(&self) -> Result<Option<(ModelRef, Option<ReasoningLevel>)>, StoreError> {
        self.lock()
            .query_row(
                "SELECT provider, model, reasoning FROM utility_model WHERE id = 1",
                [],
                |row| {
                    let reasoning = row
                        .get::<_, Option<String>>(2)?
                        .map(|value| {
                            value.parse().map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    2,
                                    Type::Text,
                                    Box::new(error),
                                )
                            })
                        })
                        .transpose()?;
                    Ok((
                        ModelRef {
                            provider: ProviderId(row.get(0)?),
                            model: row.get(1)?,
                        },
                        reasoning,
                    ))
                },
            )
            .optional()
            .map_err(backend)
    }

    fn set_utility_model(
        &self,
        model: Option<(ModelRef, Option<ReasoningLevel>)>,
    ) -> Result<(), StoreError> {
        let connection = self.lock();
        match model {
            Some((model, reasoning)) => connection.execute(
                "INSERT INTO utility_model (id, provider, model, reasoning) VALUES (1, ?1, ?2, ?3) ON CONFLICT (id) DO UPDATE SET provider = excluded.provider, model = excluded.model, reasoning = excluded.reasoning",
                params![model.provider.0, model.model, reasoning.map(ReasoningLevel::as_str)],
            ),
            None => connection.execute("DELETE FROM utility_model WHERE id = 1", []),
        }.map_err(backend)?;
        Ok(())
    }

    fn settings(&self) -> Result<Settings, StoreError> {
        let (provider, model, reasoning, compaction_provider, compaction_model, compaction_reasoning) = self
            .lock()
            .query_row(
                "SELECT default_provider, default_model, reasoning, compaction_provider, compaction_model, compaction_reasoning FROM settings WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        reasoning_from_column(row, 2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?.map(|level| level.parse().map_err(|error| rusqlite::Error::FromSqlConversionFailure(5, Type::Text, Box::new(error)))).transpose()?,
                    ))
                },
            )
            .map_err(backend)?;
        Ok(Settings {
            default_model: provider.zip(model).map(|(provider, model)| ModelRef {
                provider: ProviderId(provider),
                model,
            }),
            reasoning,
            compaction_model: compaction_provider
                .zip(compaction_model)
                .map(|(provider, model)| ModelRef {
                    provider: ProviderId(provider),
                    model,
                }),
            compaction_reasoning,
        })
    }

    fn save_settings(&self, settings: Settings) -> Result<(), StoreError> {
        let (provider, model) = settings
            .default_model
            .map(|model| (model.provider.0, model.model))
            .unzip();
        let (compaction_provider, compaction_model) = settings
            .compaction_model
            .map(|model| (model.provider.0, model.model))
            .unzip();
        self.lock()
            .execute(
                "UPDATE settings
                 SET default_provider = ?1, default_model = ?2, reasoning = ?3, compaction_provider = ?4, compaction_model = ?5, compaction_reasoning = ?6
                 WHERE id = 1",
                params![provider, model, settings.reasoning.as_str(), compaction_provider, compaction_model, settings.compaction_reasoning.map(ReasoningLevel::as_str)],
            )
            .map_err(backend)?;
        Ok(())
    }
}

fn reasoning_map(
    connection: &Connection,
    provider: &ProviderId,
    model: &str,
) -> Result<BTreeMap<ReasoningLevel, Option<String>>, StoreError> {
    connection
        .prepare_cached(
            "SELECT level, value FROM model_reasoning WHERE provider = ?1 AND model = ?2",
        )
        .map_err(backend)?
        .query_map(params![provider.0, model], |row| {
            Ok((
                reasoning_from_column(row, 0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .map_err(backend)?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(backend)
}

fn reasoning_from_column(row: &Row, column: usize) -> rusqlite::Result<ReasoningLevel> {
    let level: String = row.get(column)?;
    level.parse().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
    })
}

fn provider_from_row(row: &Row) -> rusqlite::Result<Provider> {
    let api: String = row.get(1)?;
    Ok(Provider {
        id: ProviderId(row.get(0)?),
        api: match api.as_str() {
            "openai-responses" => ProviderApi::OpenAiResponses,
            "openai-completions" => ProviderApi::OpenAiCompletions,
            _ => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    1,
                    Type::Text,
                    format!("unknown provider api: {api}").into(),
                ));
            }
        },
        base_url: row.get(2)?,
        api_key_env: row.get(3)?,
    })
}

fn api_name(api: ProviderApi) -> &'static str {
    match api {
        ProviderApi::OpenAiResponses => "openai-responses",
        ProviderApi::OpenAiCompletions => "openai-completions",
    }
}
