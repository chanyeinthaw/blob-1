use rusqlite::{params, types::Type, OptionalExtension};
use serde::{Deserialize, Serialize};
use storage::{Message, Session, SessionId, SessionStore, SessionUsage, StoreError, TaskChoice};

use crate::{backend, SqliteStorage};

#[derive(Serialize, Deserialize)]
struct SummaryDetails {
    first_kept: usize,
    read_files: Vec<String>,
    modified_files: Vec<String>,
}

impl SqliteStorage {
    fn load_session(&self, id: &SessionId) -> rusqlite::Result<Option<Session>> {
        let connection = self.lock();

        let usage = connection
            .query_row(
                "SELECT usage_tokens, usage_boundary FROM sessions WHERE id = ?1",
                [&id.0],
                |row| {
                    let tokens: Option<i64> = row.get(0)?;
                    let boundary: Option<i64> = row.get(1)?;
                    tokens
                        .zip(boundary)
                        .map(|(tokens, boundary)| {
                            Ok(SessionUsage {
                                tokens: u64::try_from(tokens).map_err(|e| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        0,
                                        Type::Integer,
                                        Box::new(e),
                                    )
                                })?,
                                view_boundary: usize::try_from(boundary).map_err(|e| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        1,
                                        Type::Integer,
                                        Box::new(e),
                                    )
                                })?,
                            })
                        })
                        .transpose()
                },
            )
            .optional()?;
        let Some(usage) = usage else { return Ok(None) };

        let mut statement = connection.prepare(
            "SELECT role, content, tool_call_id, tool_name, details FROM messages
             WHERE session_id = ?1 ORDER BY position",
        )?;
        let messages = statement
            .query_map([&id.0], |row| {
                let role: String = row.get(0)?;
                let content: String = row.get(1)?;
                Ok(match role.as_str() {
                    "summary" => {
                        let json: String = row.get(4)?;
                        let details: SummaryDetails =
                            serde_json::from_str(&json).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    Type::Text,
                                    Box::new(error),
                                )
                            })?;
                        Message::Summary {
                            text: content,
                            first_kept: details.first_kept,
                            read_files: details.read_files,
                            modified_files: details.modified_files,
                        }
                    }
                    "user" => Message::User(content),
                    "assistant" => Message::Assistant(content),
                    "tool_call" => Message::ToolCall {
                        id: row.get(2)?,
                        name: row.get(3)?,
                        arguments: content,
                    },
                    "tool_result" => Message::ToolResult {
                        id: row.get(2)?,
                        name: row.get(3)?,
                        content,
                        images: Vec::new(),
                    },
                    _ => {
                        return Err(rusqlite::Error::FromSqlConversionFailure(
                            0,
                            Type::Text,
                            format!("unknown message role: {role}").into(),
                        ));
                    }
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut messages = messages;
        let mut images = connection.prepare("SELECT position, mime, data FROM message_images WHERE session_id = ?1 ORDER BY position, idx")?;
        let rows = images.query_map([&id.0], |row| {
            Ok((
                row.get::<_, u32>(0)? as usize,
                storage::Image {
                    mime: row.get(1)?,
                    data: row.get(2)?,
                },
            ))
        })?;
        for row in rows {
            let (position, image) = row?;
            match messages.get_mut(position) {
                Some(Message::ToolResult { images, .. }) => images.push(image),
                _ => return Err(rusqlite::Error::InvalidQuery),
            }
        }
        Ok(Some(Session {
            id: id.clone(),
            messages,
            usage,
        }))
    }

    fn save_session(&self, session: &Session) -> rusqlite::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let tokens = session
            .usage
            .map(|u| i64::try_from(u.tokens))
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let boundary = session
            .usage
            .map(|u| i64::try_from(u.view_boundary))
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        transaction.execute(
            "INSERT INTO sessions (id, usage_tokens, usage_boundary) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET usage_tokens = excluded.usage_tokens, usage_boundary = excluded.usage_boundary",
            params![session.id.0, tokens, boundary],
        )?;
        let stored: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM messages WHERE session_id = ?1",
            [&session.id.0],
            |row| row.get(0),
        )?;
        let stored = usize::try_from(stored).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, Type::Integer, Box::new(e))
        })?;
        if stored > session.messages.len() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        {
            let mut insert = transaction.prepare(
                "INSERT INTO messages (session_id, position, role, content, tool_call_id, tool_name, details)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (position, message) in session.messages.iter().enumerate().skip(stored) {
                let (role, content, tool_call_id, tool_name, details) = match message {
                    Message::Summary {
                        text,
                        first_kept,
                        read_files,
                        modified_files,
                    } => {
                        let details = serde_json::to_string(&SummaryDetails {
                            first_kept: *first_kept,
                            read_files: read_files.clone(),
                            modified_files: modified_files.clone(),
                        })
                        .map_err(|error| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                        })?;
                        ("summary", text, None, None, Some(details))
                    }
                    Message::User(content) => ("user", content, None, None, None),
                    Message::Assistant(content) => ("assistant", content, None, None, None),
                    Message::ToolCall {
                        id,
                        name,
                        arguments,
                    } => ("tool_call", arguments, Some(id), Some(name), None),
                    Message::ToolResult {
                        id, name, content, ..
                    } => ("tool_result", content, Some(id), Some(name), None),
                };
                let content = if role == "tool_call" {
                    storage::redact_json_str(content)
                } else {
                    storage::redact_callback_tokens(content)
                };
                insert.execute(params![
                    session.id.0,
                    position as i64,
                    role,
                    content.as_ref(),
                    tool_call_id,
                    tool_name,
                    details
                ])?;
                if let Message::ToolResult { images, .. } = message {
                    for (idx, image) in images.iter().enumerate() {
                        transaction.execute("INSERT INTO message_images (session_id, position, idx, mime, data) VALUES (?1, ?2, ?3, ?4, ?5)", params![session.id.0, position as i64, idx as i64, image.mime, image.data])?;
                    }
                }
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

impl SessionStore for SqliteStorage {
    fn create_task_session(
        &self,
        name: &str,
        text: &str,
        choice: &TaskChoice,
    ) -> Result<bool, StoreError> {
        let mut connection = self.lock();
        let tx = connection.transaction().map_err(backend)?;
        let id = format!("task:{name}");
        let inserted = tx
            .execute(
                "INSERT INTO sessions (id) VALUES (?1) ON CONFLICT(id) DO NOTHING",
                [&id],
            )
            .map_err(backend)?;
        if inserted == 0 {
            return Ok(false);
        }
        let redacted = storage::redact_callback_tokens(text);
        tx.execute(
            "INSERT INTO messages (session_id, position, role, content) VALUES (?1, 0, 'user', ?2)",
            params![id, redacted.as_ref()],
        )
        .map_err(backend)?;
        tx.execute(
            "INSERT INTO task_choices (id, provider, model, reasoning) VALUES (?1, ?2, ?3, ?4)",
            params![name, choice.provider, choice.model, choice.reasoning],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;
        Ok(true)
    }

    fn task_choice(&self, name: &str) -> Result<Option<TaskChoice>, StoreError> {
        self.lock()
            .query_row(
                "SELECT provider, model, reasoning FROM task_choices WHERE id = ?1",
                [name],
                |row| {
                    Ok(TaskChoice {
                        provider: row.get(0)?,
                        model: row.get(1)?,
                        reasoning: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(backend)
    }

    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        self.load_session(id).map_err(backend)
    }

    fn save(&self, session: Session) -> Result<(), StoreError> {
        self.save_session(&session).map_err(backend)
    }

    fn session_ids(&self, prefix: &str) -> Result<Vec<SessionId>, StoreError> {
        let mut upper = prefix.to_owned();
        let successor = loop {
            match upper.pop() {
                Some(ch) => {
                    if let Some(next) = (ch as u32 + 1..=0x10ffff).find_map(char::from_u32) {
                        upper.push(next);
                        break Some(upper);
                    }
                }
                None => break None,
            }
        };
        let connection = self.lock();
        let mut statement = connection
            .prepare(
                "SELECT id FROM sessions WHERE id >= ?1 AND (?2 IS NULL OR id < ?2) ORDER BY id",
            )
            .map_err(backend)?;
        let ids = statement
            .query_map(params![prefix, successor], |row| {
                row.get::<_, String>(0).map(SessionId)
            })
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        Ok(ids)
    }
}
