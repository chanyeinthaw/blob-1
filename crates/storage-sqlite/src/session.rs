use rusqlite::{params, types::Type, OptionalExtension};
use storage::{Message, Session, SessionId, SessionStore, StoreError};

use crate::{backend, SqliteStorage};

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
            "SELECT role, content, tool_call_id, tool_name FROM messages
             WHERE session_id = ?1 ORDER BY position",
        )?;
        let messages = statement
            .query_map([&id.0], |row| {
                let role: String = row.get(0)?;
                let content: String = row.get(1)?;
                Ok(match role.as_str() {
                    "system" => Message::System(content),
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
                "INSERT INTO messages (session_id, position, role, content, tool_call_id, tool_name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (position, message) in session.messages.iter().enumerate() {
                let (role, content, tool_call_id, tool_name) = match message {
                    Message::System(content) => ("system", content, None, None),
                    Message::User(content) => ("user", content, None, None),
                    Message::Assistant(content) => ("assistant", content, None, None),
                    Message::ToolCall {
                        id,
                        name,
                        arguments,
                    } => ("tool_call", arguments, Some(id), Some(name)),
                    Message::ToolResult { id, name, content } => {
                        ("tool_result", content, Some(id), Some(name))
                    }
                };
                insert.execute(params![
                    session.id.0,
                    position as i64,
                    role,
                    content,
                    tool_call_id,
                    tool_name
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
}
