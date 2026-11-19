use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    System(String),
    User(String),
    Assistant(String),
    Tool {
        id: String,
        name: String,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub messages: Vec<Message>,
}

impl Session {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            messages: Vec::new(),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("storage failed: {0}")]
    Io(String),
}

pub trait SessionStore: Send + Sync {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError>;
    fn save(&self, session: Session) -> Result<(), StoreError>;
}

#[derive(Default)]
pub struct MemorySessionStore {
    sessions: Mutex<HashMap<SessionId, Session>>,
}

impl MemorySessionStore {
    fn lock(&self) -> Result<MutexGuard<'_, HashMap<SessionId, Session>>, StoreError> {
        self.sessions
            .lock()
            .map_err(|_| StoreError::Io("session store mutex poisoned".into()))
    }
}

impl SessionStore for MemorySessionStore {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        Ok(self.lock()?.get(id).cloned())
    }

    fn save(&self, session: Session) -> Result<(), StoreError> {
        self.lock()?.insert(session.id.clone(), session);
        Ok(())
    }
}
