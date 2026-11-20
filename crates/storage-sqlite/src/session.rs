use rusqlite::{params, types::Type, OptionalExtension};
use serde::{Deserialize, Serialize};
use storage::{Message, Session, SessionId, SessionStore, StoreError};

use crate::{backend, SqliteStorage};

#[derive(Serialize, Deserialize)]
struct SummaryDetails {
    read_files: Vec<String>,
    modified_files: Vec<String>,
}

impl SqliteStorage {
    fn load_session(&self, id: &SessionId) -> rusqlite::Result<Option<Session>> {
        let connection = self.lock();

        let exists = connection
            .query_row("SELECT 1 FROM sessions WHERE id = ?1", [&id.0], |_| Ok(()))
            .optional()?
            .is_some();
        if !exists {
            return Ok(None);
        }

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
                    },
                    _ => {
                        return Err(rusqlite::Error::FromSqlConversionFailure(
                            0,
                            Type::Text,
                            format!("unknown message role: {role}").into(),
                        ))
                    }
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(Session {
            id: id.clone(),
            messages,
        }))
    }

    fn save_session(&self, session: &Session) -> rusqlite::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO sessions (id) VALUES (?1)",
            [&session.id.0],
        )?;
        transaction.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            [&session.id.0],
        )?;
        {
            let mut insert = transaction.prepare(
                "INSERT INTO messages (session_id, position, role, content, tool_call_id, tool_name, details)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (position, message) in session.messages.iter().enumerate() {
                let (role, content, tool_call_id, tool_name, details) = match message {
                    Message::Summary {
                        text,
                        read_files,
                        modified_files,
                    } => {
                        let details = serde_json::to_string(&SummaryDetails {
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
                    Message::ToolResult { id, name, content } => {
                        ("tool_result", content, Some(id), Some(name), None)
                    }
                };
                insert.execute(params![
                    session.id.0,
                    position as i64,
                    role,
                    content,
                    tool_call_id,
                    tool_name,
                    details
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

impl SessionStore for SqliteStorage {
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
