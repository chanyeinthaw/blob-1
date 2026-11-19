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
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
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
    #[error("storage failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

pub trait SessionStore: Send + Sync {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError>;
    fn save(&self, session: Session) -> Result<(), StoreError>;
}

impl<T: SessionStore + ?Sized> SessionStore for std::sync::Arc<T> {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        (**self).load(id)
    }

    fn save(&self, session: Session) -> Result<(), StoreError> {
        (**self).save(session)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);

impl NodeId {
    pub const ROOT: Self = Self(1);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub mode: u32,
    pub size: u64,
    pub modified: std::time::SystemTime,
    pub linked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub node: Node,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("no such file or directory")]
    NotFound,
    #[error("file exists")]
    AlreadyExists,
    #[error("not a directory")]
    NotADirectory,
    #[error("is a directory")]
    IsADirectory,
    #[error("directory not empty")]
    NotEmpty,
    #[error("invalid argument")]
    Invalid,
    #[error("file too large")]
    TooLarge,
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub trait WorkspaceStore: Send + Sync {
    fn node(&self, id: NodeId) -> Result<Node, WorkspaceError>;
    fn lookup(&self, parent: NodeId, name: &str) -> Result<Node, WorkspaceError>;
    fn children(&self, parent: NodeId) -> Result<Vec<DirEntry>, WorkspaceError>;
    fn create(
        &self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        mode: u32,
    ) -> Result<Node, WorkspaceError>;
    fn read(&self, id: NodeId, offset: u64, size: u32) -> Result<Vec<u8>, WorkspaceError>;
    fn write(&self, id: NodeId, offset: u64, data: &[u8]) -> Result<Node, WorkspaceError>;
    fn truncate(&self, id: NodeId, size: u64) -> Result<Node, WorkspaceError>;
    fn set_mode(&self, id: NodeId, mode: u32) -> Result<Node, WorkspaceError>;
    fn set_modified(
        &self,
        id: NodeId,
        modified: std::time::SystemTime,
    ) -> Result<Node, WorkspaceError>;
    fn remove(&self, parent: NodeId, name: &str, kind: NodeKind) -> Result<(), WorkspaceError>;
    fn rename(
        &self,
        parent: NodeId,
        name: &str,
        new_parent: NodeId,
        new_name: &str,
        replace: bool,
    ) -> Result<(), WorkspaceError>;
    fn forget(&self, id: NodeId) -> Result<(), WorkspaceError>;
    fn sync(&self) -> Result<(), WorkspaceError>;
}

impl<T: WorkspaceStore + ?Sized> WorkspaceStore for std::sync::Arc<T> {
    fn node(&self, id: NodeId) -> Result<Node, WorkspaceError> {
        (**self).node(id)
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<Node, WorkspaceError> {
        (**self).lookup(parent, name)
    }

    fn children(&self, parent: NodeId) -> Result<Vec<DirEntry>, WorkspaceError> {
        (**self).children(parent)
    }

    fn create(
        &self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        mode: u32,
    ) -> Result<Node, WorkspaceError> {
        (**self).create(parent, name, kind, mode)
    }

    fn read(&self, id: NodeId, offset: u64, size: u32) -> Result<Vec<u8>, WorkspaceError> {
        (**self).read(id, offset, size)
    }

    fn write(&self, id: NodeId, offset: u64, data: &[u8]) -> Result<Node, WorkspaceError> {
        (**self).write(id, offset, data)
    }

    fn truncate(&self, id: NodeId, size: u64) -> Result<Node, WorkspaceError> {
        (**self).truncate(id, size)
    }

    fn set_mode(&self, id: NodeId, mode: u32) -> Result<Node, WorkspaceError> {
        (**self).set_mode(id, mode)
    }

    fn set_modified(
        &self,
        id: NodeId,
        modified: std::time::SystemTime,
    ) -> Result<Node, WorkspaceError> {
        (**self).set_modified(id, modified)
    }

    fn remove(&self, parent: NodeId, name: &str, kind: NodeKind) -> Result<(), WorkspaceError> {
        (**self).remove(parent, name, kind)
    }

    fn rename(
        &self,
        parent: NodeId,
        name: &str,
        new_parent: NodeId,
        new_name: &str,
        replace: bool,
    ) -> Result<(), WorkspaceError> {
        (**self).rename(parent, name, new_parent, new_name, replace)
    }

    fn forget(&self, id: NodeId) -> Result<(), WorkspaceError> {
        (**self).forget(id)
    }

    fn sync(&self) -> Result<(), WorkspaceError> {
        (**self).sync()
    }
}
