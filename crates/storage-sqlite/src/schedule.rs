use rusqlite::params;
use storage::{Schedule, ScheduleStore, StoreError};

use crate::{backend, SqliteStorage};

fn schedule(row: &rusqlite::Row<'_>) -> rusqlite::Result<Schedule> {
    Ok(Schedule {
        id: row.get::<_, i64>(0)? as u64,
        text: row.get(1)?,
        at: row.get(2)?,
        cron: row.get(3)?,
        timezone: row.get(4)?,
        to_dest: row.get::<_, Option<i64>>(5)?.map(|id| id as u64),
        reply: row.get(6)?,
        next_due: row.get(7)?,
    })
}

impl ScheduleStore for SqliteStorage {
    fn add_schedule(&self, s: &Schedule) -> Result<u64, StoreError> {
        let connection = self.lock();
        connection.execute("INSERT INTO schedules (text, at, cron, timezone, to_dest, reply, next_due) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![s.text, s.at, s.cron, s.timezone, s.to_dest.map(|id| id as i64), s.reply, s.next_due]).map_err(backend)?;
        Ok(connection.last_insert_rowid() as u64)
    }

    fn schedules(&self) -> Result<Vec<Schedule>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT id, text, at, cron, timezone, to_dest, reply, next_due FROM schedules ORDER BY next_due, id").map_err(backend)?;
        let rows = statement.query_map([], schedule).map_err(backend)?;
        rows.collect::<Result<_, _>>().map_err(backend)
    }

    fn due_schedules(&self, now: i64) -> Result<Vec<Schedule>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT id, text, at, cron, timezone, to_dest, reply, next_due FROM schedules WHERE next_due <= ?1 ORDER BY next_due, id").map_err(backend)?;
        let rows = statement.query_map([now], schedule).map_err(backend)?;
        rows.collect::<Result<_, _>>().map_err(backend)
    }

    fn remove_schedule(&self, id: u64) -> Result<bool, StoreError> {
        self.lock()
            .execute("DELETE FROM schedules WHERE id = ?1", [id as i64])
            .map(|n| n > 0)
            .map_err(backend)
    }

    fn claim_schedule(&self, id: u64, due: i64, next_due: Option<i64>) -> Result<bool, StoreError> {
        let connection = self.lock();
        let changed = match next_due {
            Some(next) => connection.execute(
                "UPDATE schedules SET next_due = ?3 WHERE id = ?1 AND next_due = ?2",
                params![id as i64, due, next],
            ),
            None => connection.execute(
                "DELETE FROM schedules WHERE id = ?1 AND next_due = ?2",
                params![id as i64, due],
            ),
        }
        .map_err(backend)?;
        Ok(changed > 0)
    }
}
