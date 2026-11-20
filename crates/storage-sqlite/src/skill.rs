use rusqlite::params;
use storage::{SkillFile, SkillInfo, SkillStore, StoreError};

use crate::{backend, SqliteStorage};

impl SkillStore for SqliteStorage {
    fn skills(&self) -> Result<Vec<SkillInfo>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT name, description, hash FROM skills ORDER BY name")
            .map_err(backend)?;
        let rows = statement
            .query_map([], |row| {
                Ok(SkillInfo {
                    name: row.get(0)?,
                    description: row.get(1)?,
                    hash: row.get(2)?,
                })
            })
            .map_err(backend)?;
        rows.collect::<Result<_, _>>().map_err(backend)
    }

    fn skill_files(&self, name: &str) -> Result<Vec<SkillFile>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT path, mode, content FROM skill_files WHERE skill = ?1 ORDER BY path")
            .map_err(backend)?;
        let rows = statement
            .query_map([name], |row| {
                Ok(SkillFile {
                    path: row.get(0)?,
                    mode: row.get(1)?,
                    content: row.get(2)?,
                })
            })
            .map_err(backend)?;
        rows.collect::<Result<_, _>>().map_err(backend)
    }

    fn put_skill(&self, info: SkillInfo, files: Vec<SkillFile>) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction().map_err(backend)?;
        transaction
            .execute(
                "INSERT INTO skills (name, description, hash) VALUES (?1, ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET description = excluded.description, hash = excluded.hash",
                params![info.name, info.description, info.hash],
            )
            .map_err(backend)?;
        transaction
            .execute("DELETE FROM skill_files WHERE skill = ?1", [&info.name])
            .map_err(backend)?;
        for file in files {
            transaction
                .execute(
                    "INSERT INTO skill_files (skill, path, mode, content) VALUES (?1, ?2, ?3, ?4)",
                    params![info.name, file.path, file.mode, file.content],
                )
                .map_err(backend)?;
        }
        transaction.commit().map_err(backend)
    }

    fn remove_skill(&self, name: &str) -> Result<bool, StoreError> {
        let connection = self.lock();
        connection
            .execute("DELETE FROM skills WHERE name = ?1", [name])
            .map(|count| count > 0)
            .map_err(backend)
    }
}
