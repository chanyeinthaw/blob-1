use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use harness::skills::{parse_frontmatter, SKILL_FILE};
use storage::{NodeId, NodeKind, Tree, WorkspaceError, WorkspaceStore};
use storage_sqlite::SqliteStorage;

use crate::Result;

pub fn add(store: &SqliteStorage, dir: &Path) -> Result<String> {
    let content = std::fs::read_to_string(dir.join(SKILL_FILE))?;
    let frontmatter = parse_frontmatter(&content)
        .ok_or_else(|| format!("{SKILL_FILE} needs name and description frontmatter"))?;
    let name = frontmatter.name;
    if name == "." || name == ".." || name.contains('/') {
        return Err(format!("invalid skill name: {name}").into());
    }
    let root = store.root(Tree::Skills)?;
    if store.lookup(root, &name).is_ok() {
        return Err(format!("skill {name} already exists; remove it first").into());
    }
    let target = store.create(root, &name, NodeKind::Directory, 0o755)?;
    if let Err(error) = copy(store, dir, target.id) {
        remove_tree(store, root, &name)?;
        return Err(error);
    }
    Ok(name)
}

pub fn remove(store: &SqliteStorage, name: &str) -> Result<()> {
    let root = store.root(Tree::Skills)?;
    remove_tree(store, root, name)
}

fn copy(store: &SqliteStorage, source: &Path, parent: NodeId) -> Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| format!("non UTF-8 file name: {name:?}"))?;
        let file_type = entry.file_type()?;
        let mode = entry.metadata()?.permissions().mode();
        if file_type.is_dir() {
            let node = store.create(parent, &name, NodeKind::Directory, mode)?;
            copy(store, &entry.path(), node.id)?;
        } else if file_type.is_file() {
            let node = store.create(parent, &name, NodeKind::File, mode)?;
            store.write(node.id, 0, &std::fs::read(entry.path())?)?;
        } else {
            return Err(format!("unsupported file type: {}", entry.path().display()).into());
        }
    }
    Ok(())
}

fn remove_tree(store: &SqliteStorage, parent: NodeId, name: &str) -> Result<()> {
    let node = store.lookup(parent, name).map_err(|error| match error {
        WorkspaceError::NotFound => format!("no skill named {name}").into(),
        error => Box::<dyn std::error::Error + Send + Sync>::from(error),
    })?;
    if node.kind == NodeKind::Directory {
        for child in store.children(node.id)? {
            remove_tree(store, node.id, &child.name)?;
        }
    }
    store.remove(parent, name, node.kind)?;
    store.forget(node.id)?;
    Ok(())
}
