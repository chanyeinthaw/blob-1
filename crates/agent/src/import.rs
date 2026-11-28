use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use environment::{parse_frontmatter, valid_skill_name, SKILL_FILE};
use sha2::{Digest, Sha256};
use storage::{SkillFile, SkillInfo, SkillStore};
use storage_sqlite::SqliteStorage;

use crate::Result;

pub fn add(store: &SqliteStorage, dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect(dir, dir, &mut files)?;
    store_bundle(store, files)
}

/// Validates a skill's files (paths relative to its folder) and stores them as
/// a managed skill, replacing one with the same name; returns the skill name.
pub fn store_bundle(store: &SqliteStorage, mut files: Vec<SkillFile>) -> Result<String> {
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let skill = files
        .iter()
        .find(|file| file.path == SKILL_FILE)
        .ok_or("missing SKILL.md")?;
    let frontmatter = parse_frontmatter(std::str::from_utf8(&skill.content)?)
        .ok_or("invalid SKILL.md frontmatter")?;
    if !valid_skill_name(&frontmatter.name) {
        return Err("invalid skill name".into());
    }
    let mut hash = Sha256::new();
    for file in &files {
        hash.update(file.path.as_bytes());
        hash.update(b"\0");
        hash.update(file.mode.to_string().as_bytes());
        hash.update(b"\0");
        hash.update(file.content.len().to_string().as_bytes());
        hash.update(b"\0");
        hash.update(&file.content);
    }
    let name = frontmatter.name;
    store.put_skill(
        SkillInfo {
            name: name.clone(),
            description: frontmatter.description,
            hash: format!("{:x}", hash.finalize()),
        },
        files,
    )?;
    Ok(name)
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<SkillFile>) -> Result<()> {
    let metadata = std::fs::symlink_metadata(dir)?;
    if !metadata.is_dir() {
        return Err(format!(
            "not a directory or unsupported file type: {}",
            dir.display()
        )
        .into());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            collect(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("skill path is not UTF-8")?
                .to_owned();
            files.push(SkillFile {
                path: relative,
                mode: metadata.permissions().mode() & 0o777,
                content: std::fs::read(&path)?,
            });
        } else {
            return Err(format!(
                "unsupported file type (including symlinks): {}",
                path.display()
            )
            .into());
        }
    }
    Ok(())
}

pub fn remove(store: &SqliteStorage, name: &str) -> Result<()> {
    if !store.remove_skill(name)? {
        return Err(format!("skill not found: {name}").into());
    }
    Ok(())
}
