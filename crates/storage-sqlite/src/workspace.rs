use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, OptionalExtension, Row, Transaction};
use storage::{DirEntry, Node, NodeId, NodeKind, Tree, WorkspaceError, WorkspaceStore};

use crate::{backend, SqliteStorage};

const CHUNK_SIZE: u64 = 64 * 1024;
const MAX_FILE_SIZE: u64 = i64::MAX as u64;

const NODE_COLUMNS: &str = "n.id, n.kind, n.mode, n.size, n.modified_ns,
    EXISTS (SELECT 1 FROM workspace_roots root WHERE root.node = n.id)
    OR EXISTS (SELECT 1 FROM workspace_entries linked WHERE linked.node = n.id)";

enum Failure {
    Workspace(WorkspaceError),
    Sqlite(rusqlite::Error),
}

impl From<WorkspaceError> for Failure {
    fn from(error: WorkspaceError) -> Self {
        Self::Workspace(error)
    }
}

impl From<rusqlite::Error> for Failure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<Failure> for WorkspaceError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Workspace(error) => error,
            Failure::Sqlite(error) => WorkspaceError::Store(backend(error)),
        }
    }
}

impl SqliteStorage {
    fn in_transaction<T>(
        &self,
        work: impl FnOnce(&Transaction) -> Result<T, Failure>,
    ) -> Result<T, WorkspaceError> {
        let mut connection = self.lock();
        let transaction = connection.transaction().map_err(Failure::from)?;
        let value = work(&transaction)?;
        transaction.commit().map_err(Failure::from)?;
        Ok(value)
    }
}

impl WorkspaceStore for SqliteStorage {
    fn root(&self, tree: Tree) -> Result<NodeId, WorkspaceError> {
        let name = match tree {
            Tree::Workspace => "workspace",
            Tree::Skills => "skills",
        };
        self.in_transaction(|tx| {
            Ok(NodeId(tx.query_row(
                "SELECT node FROM workspace_roots WHERE tree = ?1",
                [name],
                |row| row.get::<_, i64>(0),
            )? as u64))
        })
    }

    fn node(&self, id: NodeId) -> Result<Node, WorkspaceError> {
        self.in_transaction(|tx| node(tx, id))
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<Node, WorkspaceError> {
        self.in_transaction(|tx| {
            child(tx, parent, name)?.ok_or_else(|| WorkspaceError::NotFound.into())
        })
    }

    fn children(&self, parent: NodeId) -> Result<Vec<DirEntry>, WorkspaceError> {
        self.in_transaction(|tx| {
            directory(tx, parent)?;
            let mut statement = tx.prepare(&format!(
                "SELECT {NODE_COLUMNS}, e.name FROM workspace_entries e
                 JOIN workspace_nodes n ON n.id = e.node
                 WHERE e.parent = ?1 ORDER BY e.name"
            ))?;
            let entries = statement
                .query_map([parent.0 as i64], |row| {
                    Ok(DirEntry {
                        node: node_from_row(row)?,
                        name: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(entries)
        })
    }

    fn create(
        &self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        mode: u32,
    ) -> Result<Node, WorkspaceError> {
        self.in_transaction(|tx| {
            directory(tx, parent)?;
            if child(tx, parent, name)?.is_some() {
                return Err(WorkspaceError::AlreadyExists.into());
            }
            let kind_name = match kind {
                NodeKind::File => "file",
                NodeKind::Directory => "directory",
            };
            tx.execute(
                "INSERT INTO workspace_nodes (kind, mode, size, modified_ns)
                 VALUES (?1, ?2, 0, ?3)",
                params![kind_name, mode & 0o7777, now()],
            )?;
            let id = NodeId(tx.last_insert_rowid() as u64);
            tx.execute(
                "INSERT INTO workspace_entries (parent, name, node) VALUES (?1, ?2, ?3)",
                params![parent.0 as i64, name, id.0 as i64],
            )?;
            touch(tx, parent)?;
            node(tx, id)
        })
    }

    fn read(&self, id: NodeId, offset: u64, size: u32) -> Result<Vec<u8>, WorkspaceError> {
        self.in_transaction(|tx| {
            let target = file(tx, id)?;
            let end = target.size.min(offset.saturating_add(u64::from(size)));
            if offset >= end {
                return Ok(Vec::new());
            }
            let mut buffer = vec![0; (end - offset) as usize];
            let mut statement = tx.prepare(
                "SELECT idx, data FROM workspace_chunks
                 WHERE node = ?1 AND idx BETWEEN ?2 AND ?3",
            )?;
            let chunks = statement.query_map(
                params![
                    id.0 as i64,
                    (offset / CHUNK_SIZE) as i64,
                    ((end - 1) / CHUNK_SIZE) as i64
                ],
                |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, Vec<u8>>(1)?)),
            )?;
            for chunk in chunks {
                let (index, data) = chunk?;
                let chunk_start = index * CHUNK_SIZE;
                let from = offset.max(chunk_start);
                let to = end.min(chunk_start + data.len() as u64);
                if from < to {
                    buffer[(from - offset) as usize..(to - offset) as usize].copy_from_slice(
                        &data[(from - chunk_start) as usize..(to - chunk_start) as usize],
                    );
                }
            }
            Ok(buffer)
        })
    }

    fn write(&self, id: NodeId, offset: u64, data: &[u8]) -> Result<Node, WorkspaceError> {
        self.in_transaction(|tx| {
            let target = file(tx, id)?;
            if data.is_empty() {
                return Ok(target);
            }
            let end = offset
                .checked_add(data.len() as u64)
                .filter(|end| *end <= MAX_FILE_SIZE)
                .ok_or(WorkspaceError::TooLarge)?;

            let mut position = offset;
            while position < end {
                let index = position / CHUNK_SIZE;
                let chunk_start = index * CHUNK_SIZE;
                let from = (position - chunk_start) as usize;
                let to = (end.min(chunk_start + CHUNK_SIZE) - chunk_start) as usize;
                let source = (position - offset) as usize;

                let mut chunk: Vec<u8> = tx
                    .query_row(
                        "SELECT data FROM workspace_chunks WHERE node = ?1 AND idx = ?2",
                        params![id.0 as i64, index as i64],
                        |row| row.get(0),
                    )
                    .optional()?
                    .unwrap_or_default();
                if chunk.len() < to {
                    chunk.resize(to, 0);
                }
                chunk[from..to].copy_from_slice(&data[source..source + (to - from)]);
                tx.execute(
                    "INSERT OR REPLACE INTO workspace_chunks (node, idx, data) VALUES (?1, ?2, ?3)",
                    params![id.0 as i64, index as i64, chunk],
                )?;
                position = chunk_start + to as u64;
            }

            tx.execute(
                "UPDATE workspace_nodes SET size = MAX(size, ?2), modified_ns = ?3 WHERE id = ?1",
                params![id.0 as i64, end as i64, now()],
            )?;
            node(tx, id)
        })
    }

    fn truncate(&self, id: NodeId, size: u64) -> Result<Node, WorkspaceError> {
        self.in_transaction(|tx| {
            file(tx, id)?;
            if size > MAX_FILE_SIZE {
                return Err(WorkspaceError::TooLarge.into());
            }
            let kept = size.div_ceil(CHUNK_SIZE);
            tx.execute(
                "DELETE FROM workspace_chunks WHERE node = ?1 AND idx >= ?2",
                params![id.0 as i64, kept as i64],
            )?;
            let tail = size % CHUNK_SIZE;
            if tail != 0 {
                tx.execute(
                    "UPDATE workspace_chunks SET data = substr(data, 1, ?3)
                     WHERE node = ?1 AND idx = ?2 AND length(data) > ?3",
                    params![id.0 as i64, (kept - 1) as i64, tail as i64],
                )?;
            }
            tx.execute(
                "UPDATE workspace_nodes SET size = ?2, modified_ns = ?3 WHERE id = ?1",
                params![id.0 as i64, size as i64, now()],
            )?;
            node(tx, id)
        })
    }

    fn set_mode(&self, id: NodeId, mode: u32) -> Result<Node, WorkspaceError> {
        self.in_transaction(|tx| {
            node(tx, id)?;
            tx.execute(
                "UPDATE workspace_nodes SET mode = ?2 WHERE id = ?1",
                params![id.0 as i64, mode & 0o7777],
            )?;
            node(tx, id)
        })
    }

    fn set_modified(&self, id: NodeId, modified: SystemTime) -> Result<Node, WorkspaceError> {
        self.in_transaction(|tx| {
            node(tx, id)?;
            tx.execute(
                "UPDATE workspace_nodes SET modified_ns = ?2 WHERE id = ?1",
                params![id.0 as i64, nanos(modified)],
            )?;
            node(tx, id)
        })
    }

    fn remove(&self, parent: NodeId, name: &str, kind: NodeKind) -> Result<(), WorkspaceError> {
        self.in_transaction(|tx| {
            let target = child(tx, parent, name)?.ok_or(WorkspaceError::NotFound)?;
            match (kind, target.kind) {
                (NodeKind::File, NodeKind::Directory) => {
                    return Err(WorkspaceError::IsADirectory.into())
                }
                (NodeKind::Directory, NodeKind::File) => {
                    return Err(WorkspaceError::NotADirectory.into())
                }
                _ => {}
            }
            unlink(tx, &target)?;
            touch(tx, parent)
        })
    }

    fn rename(
        &self,
        parent: NodeId,
        name: &str,
        new_parent: NodeId,
        new_name: &str,
        replace: bool,
    ) -> Result<(), WorkspaceError> {
        self.in_transaction(|tx| {
            let source = child(tx, parent, name)?.ok_or(WorkspaceError::NotFound)?;
            directory(tx, new_parent)?;

            if source.kind == NodeKind::Directory {
                let mut ancestor = Some(new_parent);
                while let Some(id) = ancestor {
                    if id == source.id {
                        return Err(WorkspaceError::Invalid.into());
                    }
                    ancestor = tx
                        .query_row(
                            "SELECT parent FROM workspace_entries WHERE node = ?1",
                            [id.0 as i64],
                            |row| Ok(NodeId(row.get::<_, i64>(0)? as u64)),
                        )
                        .optional()?;
                }
            }

            if let Some(existing) = child(tx, new_parent, new_name)? {
                if !replace {
                    return Err(WorkspaceError::AlreadyExists.into());
                }
                if existing.id == source.id {
                    return Ok(());
                }
                match (source.kind, existing.kind) {
                    (NodeKind::Directory, NodeKind::File) => {
                        return Err(WorkspaceError::NotADirectory.into())
                    }
                    (NodeKind::File, NodeKind::Directory) => {
                        return Err(WorkspaceError::IsADirectory.into())
                    }
                    _ => {}
                }
                unlink(tx, &existing)?;
            }

            tx.execute(
                "UPDATE workspace_entries SET parent = ?2, name = ?3 WHERE node = ?1",
                params![source.id.0 as i64, new_parent.0 as i64, new_name],
            )?;
            touch(tx, parent)?;
            touch(tx, new_parent)
        })
    }

    fn forget(&self, id: NodeId) -> Result<(), WorkspaceError> {
        self.in_transaction(|tx| {
            if node(tx, id)?.linked {
                return Ok(());
            }
            tx.execute(
                "DELETE FROM workspace_chunks WHERE node = ?1",
                [id.0 as i64],
            )?;
            tx.execute("DELETE FROM workspace_nodes WHERE id = ?1", [id.0 as i64])?;
            Ok(())
        })
    }

    fn sync(&self) -> Result<(), WorkspaceError> {
        self.lock()
            .query_row("PRAGMA wal_checkpoint(FULL)", [], |_| Ok(()))
            .map_err(|error| WorkspaceError::Store(backend(error)))
    }
}

fn node_from_row(row: &Row) -> rusqlite::Result<Node> {
    let kind: String = row.get(1)?;
    Ok(Node {
        id: NodeId(row.get::<_, i64>(0)? as u64),
        kind: if kind == "directory" {
            NodeKind::Directory
        } else {
            NodeKind::File
        },
        mode: row.get(2)?,
        size: row.get::<_, i64>(3)? as u64,
        modified: time(row.get(4)?),
        linked: row.get(5)?,
    })
}

fn node(tx: &Transaction, id: NodeId) -> Result<Node, Failure> {
    tx.query_row(
        &format!("SELECT {NODE_COLUMNS} FROM workspace_nodes n WHERE n.id = ?1"),
        [id.0 as i64],
        node_from_row,
    )
    .optional()?
    .ok_or_else(|| WorkspaceError::NotFound.into())
}

fn child(tx: &Transaction, parent: NodeId, name: &str) -> Result<Option<Node>, Failure> {
    Ok(tx
        .query_row(
            &format!(
                "SELECT {NODE_COLUMNS} FROM workspace_entries e
                 JOIN workspace_nodes n ON n.id = e.node
                 WHERE e.parent = ?1 AND e.name = ?2"
            ),
            params![parent.0 as i64, name],
            node_from_row,
        )
        .optional()?)
}

fn directory(tx: &Transaction, id: NodeId) -> Result<Node, Failure> {
    let target = node(tx, id)?;
    match target.kind {
        NodeKind::Directory => Ok(target),
        NodeKind::File => Err(WorkspaceError::NotADirectory.into()),
    }
}

fn file(tx: &Transaction, id: NodeId) -> Result<Node, Failure> {
    let target = node(tx, id)?;
    match target.kind {
        NodeKind::File => Ok(target),
        NodeKind::Directory => Err(WorkspaceError::IsADirectory.into()),
    }
}

fn unlink(tx: &Transaction, target: &Node) -> Result<(), Failure> {
    if target.kind == NodeKind::Directory {
        let has_children = tx
            .query_row(
                "SELECT 1 FROM workspace_entries WHERE parent = ?1 LIMIT 1",
                [target.id.0 as i64],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if has_children {
            return Err(WorkspaceError::NotEmpty.into());
        }
    }
    tx.execute(
        "DELETE FROM workspace_entries WHERE node = ?1",
        [target.id.0 as i64],
    )?;
    Ok(())
}

fn touch(tx: &Transaction, id: NodeId) -> Result<(), Failure> {
    tx.execute(
        "UPDATE workspace_nodes SET modified_ns = ?2 WHERE id = ?1",
        params![id.0 as i64, now()],
    )?;
    Ok(())
}

fn now() -> i64 {
    nanos(SystemTime::now())
}

fn nanos(time: SystemTime) -> i64 {
    let nanos = match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_nanos() as i128,
        Err(before) => -(before.duration().as_nanos() as i128),
    };
    nanos.clamp(i64::MIN.into(), i64::MAX.into()) as i64
}

fn time(nanos: i64) -> SystemTime {
    let offset = Duration::from_nanos(nanos.unsigned_abs());
    if nanos >= 0 {
        UNIX_EPOCH + offset
    } else {
        UNIX_EPOCH - offset
    }
}
