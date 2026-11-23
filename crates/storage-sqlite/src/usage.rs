use rusqlite::params;
use storage::{
    StoreError, UsageDayBucket, UsageEvent, UsageKind, UsageKindBucket, UsageModelBucket,
    UsageStore, UsageSummary, UsageTotals,
};

use crate::{backend, SqliteStorage};

fn totals_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<UsageTotals> {
    Ok(UsageTotals {
        calls: row.get::<_, i64>(offset)? as u64,
        input_tokens: row.get::<_, i64>(offset + 1)? as u64,
        cached_tokens: row.get::<_, i64>(offset + 2)? as u64,
        output_tokens: row.get::<_, i64>(offset + 3)? as u64,
    })
}

impl UsageStore for SqliteStorage {
    fn record_usage(&self, event: &UsageEvent) -> Result<(), StoreError> {
        self.lock()
            .execute(
                "INSERT INTO usage_events (ts, kind, provider, model, input_tokens, cached_tokens, output_tokens)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    event.ts,
                    event.kind.as_str(),
                    event.provider,
                    event.model,
                    event.input_tokens as i64,
                    event.cached_tokens as i64,
                    event.output_tokens as i64,
                ],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn usage_summary(&self, since_ts: i64) -> Result<UsageSummary, StoreError> {
        let connection = self.lock();

        let totals = connection
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(input_tokens), 0),
                        COALESCE(SUM(cached_tokens), 0),
                        COALESCE(SUM(output_tokens), 0)
                 FROM usage_events
                 WHERE ts >= ?1",
                [since_ts],
                |row| totals_from_row(row, 0),
            )
            .map_err(backend)?;

        let mut kind_stmt = connection
            .prepare(
                "SELECT kind,
                        COUNT(*),
                        COALESCE(SUM(input_tokens), 0),
                        COALESCE(SUM(cached_tokens), 0),
                        COALESCE(SUM(output_tokens), 0)
                 FROM usage_events
                 WHERE ts >= ?1
                 GROUP BY kind
                 ORDER BY SUM(input_tokens) DESC, kind ASC",
            )
            .map_err(backend)?;
        let kind_rows = kind_stmt
            .query_map([since_ts], |row| {
                Ok((row.get::<_, String>(0)?, totals_from_row(row, 1)?))
            })
            .map_err(backend)?;
        let mut by_kind = Vec::new();
        for row in kind_rows {
            let (raw, totals) = row.map_err(backend)?;
            let kind = UsageKind::parse(&raw).ok_or_else(|| {
                StoreError::Backend(format!("invalid kind in usage_events: {raw}").into())
            })?;
            by_kind.push(UsageKindBucket { kind, totals });
        }

        let mut model_stmt = connection
            .prepare(
                "SELECT provider,
                        model,
                        COUNT(*),
                        COALESCE(SUM(input_tokens), 0),
                        COALESCE(SUM(cached_tokens), 0),
                        COALESCE(SUM(output_tokens), 0)
                 FROM usage_events
                 WHERE ts >= ?1
                 GROUP BY provider, model
                 ORDER BY (SUM(input_tokens) + SUM(output_tokens)) DESC, provider ASC, model ASC",
            )
            .map_err(backend)?;
        let by_model = model_stmt
            .query_map([since_ts], |row| {
                Ok(UsageModelBucket {
                    provider: row.get(0)?,
                    model: row.get(1)?,
                    totals: totals_from_row(row, 2)?,
                })
            })
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;

        let mut daily_stmt = connection
            .prepare(
                "SELECT strftime('%Y-%m-%d', ts, 'unixepoch') AS day,
                        COUNT(*),
                        COALESCE(SUM(input_tokens), 0),
                        COALESCE(SUM(cached_tokens), 0),
                        COALESCE(SUM(output_tokens), 0)
                 FROM usage_events
                 WHERE ts >= ?1
                 GROUP BY day
                 ORDER BY day ASC",
            )
            .map_err(backend)?;
        let daily = daily_stmt
            .query_map([since_ts], |row| {
                Ok(UsageDayBucket {
                    day: row.get(0)?,
                    totals: totals_from_row(row, 1)?,
                })
            })
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;

        Ok(UsageSummary {
            totals,
            by_kind,
            by_model,
            daily,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_summary_aggregates_and_filters_since_ts() {
        let dir = std::env::temp_dir().join(format!("blob-1-usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStorage::open(dir.join("agent.db"), None).unwrap();

        // Empty summary starts at zero.
        let empty = store.usage_summary(0).unwrap();
        assert_eq!(empty, UsageSummary::default());

        // 2025-01-01T00:00:00Z = 1735689600
        let day1 = 1_735_689_600;
        // 2025-01-02T12:00:00Z
        let day2 = day1 + 86_400 + 43_200;
        // 2025-01-03T06:00:00Z
        let day3 = day1 + 2 * 86_400 + 21_600;

        // Excluded when querying since day2.
        store
            .record_usage(&UsageEvent {
                ts: day1,
                kind: UsageKind::Utility,
                provider: "openai".into(),
                model: "gpt-4o-mini".into(),
                input_tokens: 500,
                cached_tokens: 100,
                output_tokens: 50,
            })
            .unwrap();
        store
            .record_usage(&UsageEvent {
                ts: day2,
                kind: UsageKind::Chat,
                provider: "openai".into(),
                model: "gpt-5".into(),
                input_tokens: 1000,
                cached_tokens: 400,
                output_tokens: 200,
            })
            .unwrap();
        store
            .record_usage(&UsageEvent {
                ts: day2 + 60,
                kind: UsageKind::Task,
                provider: "anthropic".into(),
                model: "claude-sonnet".into(),
                input_tokens: 2000,
                cached_tokens: 1500,
                output_tokens: 500,
            })
            .unwrap();
        store
            .record_usage(&UsageEvent {
                ts: day3,
                kind: UsageKind::Chat,
                provider: "openai".into(),
                model: "gpt-5".into(),
                input_tokens: 600,
                cached_tokens: 200,
                output_tokens: 100,
            })
            .unwrap();

        let summary = store.usage_summary(day2).unwrap();
        assert_eq!(
            summary.totals,
            UsageTotals {
                calls: 3,
                input_tokens: 3600,
                cached_tokens: 2100,
                output_tokens: 800,
            }
        );

        // by_kind ordered by input_tokens DESC: Task (2000) then Chat (1600).
        assert_eq!(
            summary.by_kind,
            vec![
                UsageKindBucket {
                    kind: UsageKind::Task,
                    totals: UsageTotals {
                        calls: 1,
                        input_tokens: 2000,
                        cached_tokens: 1500,
                        output_tokens: 500,
                    },
                },
                UsageKindBucket {
                    kind: UsageKind::Chat,
                    totals: UsageTotals {
                        calls: 2,
                        input_tokens: 1600,
                        cached_tokens: 600,
                        output_tokens: 300,
                    },
                },
            ]
        );

        // by_model ordered by input_tokens + output_tokens DESC:
        // claude-sonnet (2500) then gpt-5 (1900).
        assert_eq!(
            summary.by_model,
            vec![
                UsageModelBucket {
                    provider: "anthropic".into(),
                    model: "claude-sonnet".into(),
                    totals: UsageTotals {
                        calls: 1,
                        input_tokens: 2000,
                        cached_tokens: 1500,
                        output_tokens: 500,
                    },
                },
                UsageModelBucket {
                    provider: "openai".into(),
                    model: "gpt-5".into(),
                    totals: UsageTotals {
                        calls: 2,
                        input_tokens: 1600,
                        cached_tokens: 600,
                        output_tokens: 300,
                    },
                },
            ]
        );

        // daily ordered chronologically ascending.
        assert_eq!(
            summary.daily,
            vec![
                UsageDayBucket {
                    day: "2025-01-02".into(),
                    totals: UsageTotals {
                        calls: 2,
                        input_tokens: 3000,
                        cached_tokens: 1900,
                        output_tokens: 700,
                    },
                },
                UsageDayBucket {
                    day: "2025-01-03".into(),
                    totals: UsageTotals {
                        calls: 1,
                        input_tokens: 600,
                        cached_tokens: 200,
                        output_tokens: 100,
                    },
                },
            ]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
