use crate::{backend, SqliteStorage};
use rusqlite::{params, OptionalExtension};
use storage::{McpServer, McpStore, McpTransport, StoreError};

fn map_json<T: serde::de::DeserializeOwned + Default>(text: &str) -> T {
    serde_json::from_str(text).unwrap_or_default()
}

impl McpStore for SqliteStorage {
    fn mcp_servers(&self) -> Result<Vec<McpServer>, StoreError> {
        let connection = self.lock();
        let rows = connection
            .prepare(
                "SELECT id, transport, command, args, env, url, headers, enabled FROM mcp_servers ORDER BY id",
            )
            .map_err(backend)?
            .query_map([], |row| {
                let transport = match (row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(5)?) {
                    (_, Some(command), _) => McpTransport::Stdio {
                        command,
                        args: map_json(&row.get::<_, String>(3)?),
                        env: map_json(&row.get::<_, String>(4)?),
                    },
                    (_, _, Some(url)) => McpTransport::Http {
                        url,
                        headers: map_json(&row.get::<_, String>(6)?),
                    },
                    _ => unreachable!("CHECK forces a transport"),
                };
                Ok(McpServer {
                    id: row.get(0)?,
                    transport,
                    enabled: row.get::<_, i64>(7)? == 1,
                })
            })
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend);
        rows
    }

    fn save_mcp_server(&self, server: &McpServer) -> Result<(), StoreError> {
        let (transport, command, args, env, url, headers) = match &server.transport {
            McpTransport::Stdio {
                command,
                args,
                env,
            } => (
                "stdio",
                Some(command.as_str()),
                serde_json::to_string(args).unwrap(),
                serde_json::to_string(env).unwrap(),
                None,
                "{}".to_string(),
            ),
            McpTransport::Http { url, headers } => (
                "http",
                None,
                "[]".to_string(),
                "{}".to_string(),
                Some(url.as_str()),
                serde_json::to_string(headers).unwrap(),
            ),
        };
        let connection = self.lock();
        let replaced = connection
            .query_row(
                "SELECT transport, command, args, env, url, headers FROM mcp_servers WHERE id = ?1",
                params![server.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(backend)?
            .is_some_and(|old| {
                old != (
                    transport.to_string(),
                    command.map(str::to_string),
                    args.clone(),
                    env.clone(),
                    url.map(str::to_string),
                    headers.clone(),
                )
            });
        connection
            .execute(
                "INSERT INTO mcp_servers (id, transport, command, args, env, url, headers, enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (id) DO UPDATE SET transport = excluded.transport, command = excluded.command, args = excluded.args, env = excluded.env, url = excluded.url, headers = excluded.headers, enabled = excluded.enabled",
                params![
                    server.id,
                    transport,
                    command,
                    args,
                    env,
                    url,
                    headers,
                    server.enabled as i64
                ],
            )
            .map_err(backend)?;
        if replaced {
            connection
                .execute("DELETE FROM mcp_tokens WHERE server = ?1", params![server.id])
                .map_err(backend)?;
            connection
                .execute(
                    "DELETE FROM mcp_oauth_states WHERE server = ?1",
                    params![server.id],
                )
                .map_err(backend)?;
        }
        Ok(())
    }

    fn remove_mcp_server(&self, id: &str) -> Result<bool, StoreError> {
        Ok(self
            .lock()
            .execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])
            .map_err(backend)?
            > 0)
    }

    fn mcp_oauth(
        &self,
        server: &str,
    ) -> Result<(Option<serde_json::Value>, Option<serde_json::Value>), StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT credentials, registration FROM mcp_tokens WHERE server = ?1",
                params![server],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?
                            .map(|s| serde_json::from_str(&s).unwrap_or_default()),
                        row.get::<_, Option<String>>(1)?
                            .map(|s| serde_json::from_str(&s).unwrap_or_default()),
                    ))
                },
            )
            .optional()
            .map_err(backend)?
            .unwrap_or_default())
    }

    fn save_mcp_credentials(
        &self,
        server: &str,
        credentials: Option<&serde_json::Value>,
    ) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO mcp_tokens (server, credentials) VALUES (?1, ?2)
                 ON CONFLICT (server) DO UPDATE SET credentials = excluded.credentials",
                params![server, credentials.map(|c| serde_json::to_string(c).unwrap())],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn save_mcp_registration(
        &self,
        server: &str,
        registration: &serde_json::Value,
    ) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO mcp_tokens (server, registration) VALUES (?1, ?2)
                 ON CONFLICT (server) DO UPDATE SET registration = excluded.registration",
                params![server, serde_json::to_string(registration).unwrap()],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn mcp_oauth_state(
        &self,
        csrf: &str,
    ) -> Result<Option<(String, serde_json::Value)>, StoreError> {
        self.lock()
            .query_row(
                "SELECT server, state FROM mcp_oauth_states WHERE csrf = ?1",
                params![csrf],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        serde_json::from_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                    ))
                },
            )
            .optional()
            .map_err(backend)
    }

    fn save_mcp_oauth_state(
        &self,
        server: &str,
        csrf: &str,
        state: &serde_json::Value,
        created: i64,
    ) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO mcp_oauth_states (csrf, server, state, created) VALUES (?1, ?2, ?3, ?4)",
                params![csrf, server, serde_json::to_string(state).unwrap(), created],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn delete_mcp_oauth_state(&self, csrf: &str) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "DELETE FROM mcp_oauth_states WHERE csrf = ?1",
                params![csrf],
            )
            .map_err(backend)?;
        Ok(())
    }
}
