use std::path::{Path, PathBuf};

use storage::{NodeKind, Tree, WorkspaceStore};

use crate::HarnessError;

pub const SKILL_FILE: &str = "SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub location: PathBuf,
}

pub fn parse_frontmatter(content: &str) -> Option<Frontmatter> {
    let body = content
        .strip_prefix("---")?
        .trim_start_matches(['\r', '\n']);
    let header = &body[..body.find("\n---")?];
    let mut name = None;
    let mut description = None;
    for line in header.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']).to_owned();
        match key.trim() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            _ => {}
        }
    }
    Some(Frontmatter {
        name: name.filter(|name| !name.is_empty())?,
        description: description.filter(|description| !description.is_empty())?,
    })
}

pub fn catalog(store: &impl WorkspaceStore, skills_dir: &Path) -> Result<Vec<Skill>, HarnessError> {
    let root = store.root(Tree::Skills)?;
    let mut skills = Vec::new();
    for entry in store.children(root)? {
        if entry.node.kind != NodeKind::Directory {
            continue;
        }
        let file = store.lookup(entry.node.id, SKILL_FILE)?;
        let size = u32::try_from(file.size).unwrap_or(u32::MAX);
        let content = String::from_utf8_lossy(&store.read(file.id, 0, size)?).into_owned();
        let frontmatter = parse_frontmatter(&content).ok_or_else(|| {
            HarnessError::Skill(format!(
                "{}/{SKILL_FILE} has no name and description",
                entry.name
            ))
        })?;
        skills.push(Skill {
            name: frontmatter.name,
            description: frontmatter.description,
            location: skills_dir.join(&entry.name).join(SKILL_FILE),
        });
    }
    Ok(skills)
}
