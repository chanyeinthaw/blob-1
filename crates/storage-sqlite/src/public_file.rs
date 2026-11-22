use rusqlite::{params, OptionalExtension};
use storage::{PublicFile, PublicFileStore, StoreError};

use crate::{backend, SqliteStorage};

impl PublicFileStore for SqliteStorage {
    fn create_public_file(&self, file: &PublicFile) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO public_files (id, workspace, path, content_type, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    file.id,
                    file.workspace,
                    file.path,
                    file.content_type,
                    file.created_at
                ],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn public_file(&self, id: &str) -> Result<Option<PublicFile>, StoreError> {
        self.lock()
            .query_row(
                "SELECT id, workspace, path, content_type, created_at FROM public_files WHERE id = ?1",
                [id],
                |row| {
                    Ok(PublicFile {
                        id: row.get(0)?,
                        workspace: row.get(1)?,
                        path: row.get(2)?,
                        content_type: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(backend)
    }
}
