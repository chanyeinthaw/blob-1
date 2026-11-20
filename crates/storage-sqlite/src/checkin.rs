use crate::{backend, SqliteStorage};
use rusqlite::{params, OptionalExtension};
use storage::{Checkin, CheckinStore, StoreError};

impl CheckinStore for SqliteStorage {
    fn checkin(&self) -> Result<Option<Checkin>, StoreError> {
        self.lock()
            .query_row(
                "SELECT every, days, start_hour, end_hour FROM checkin WHERE id = 1",
                [],
                |row| {
                    Ok(Checkin {
                        every: row.get(0)?,
                        days: row.get(1)?,
                        start_hour: row.get(2)?,
                        end_hour: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(backend)
    }
    fn set_checkin(&self, checkin: &Checkin) -> Result<(), StoreError> {
        self.lock().execute("INSERT OR REPLACE INTO checkin (id, every, days, start_hour, end_hour) VALUES (1, ?1, ?2, ?3, ?4)", params![checkin.every, checkin.days, checkin.start_hour, checkin.end_hour]).map_err(backend)?;
        Ok(())
    }
    fn clear_checkin(&self) -> Result<(), StoreError> {
        self.lock()
            .execute("DELETE FROM checkin WHERE id = 1", [])
            .map_err(backend)?;
        Ok(())
    }
}
