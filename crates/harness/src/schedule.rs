use std::borrow::Cow;

use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use environment::ToolSpec;
use serde::Deserialize;
use storage::{
    Checkin, CheckinStore, ConfigStore, MemoryStore, MessengerStore, Meta, Schedule, ScheduleStore,
    WatchStore,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: Cow::Borrowed("schedule"),
        description: Cow::Borrowed("Schedule a message at a time or on a cron schedule."),
        input_schema: Cow::Borrowed(
            r#"{"type":"object","properties":{"text":{"type":"string"},"at":{"type":"string"},"cron":{"type":"string"},"tz":{"type":"string"},"to":{"type":"string"},"reply":{"type":"string"}},"required":["text"]}"#,
        ),
    },
    ToolSpec {
        name: Cow::Borrowed("unschedule"),
        description: Cow::Borrowed("Remove a schedule by id."),
        input_schema: Cow::Borrowed(
            r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#,
        ),
    },
    ToolSpec {
        name: Cow::Borrowed("schedules"),
        description: Cow::Borrowed("List schedules."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{}}"#),
    },
];

#[derive(Deserialize)]
pub struct Add {
    pub text: String,
    pub at: Option<String>,
    pub cron: Option<String>,
    pub tz: Option<String>,
    pub to: Option<String>,
    pub reply: Option<String>,
}

pub(crate) fn token(value: &str, prefix: char) -> Result<u64> {
    let number = value
        .strip_prefix(prefix)
        .ok_or_else(|| format!("expected {prefix} id"))?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid {prefix} id").into());
    }
    Ok(number.parse()?)
}

pub fn next_cron(cron: &str, tz: Tz, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    if cron.split_whitespace().count() != 5 {
        return Err("cron must have five fields".into());
    }
    let cron: croner::Cron = cron.parse()?;
    Ok(cron
        .find_next_occurrence(&after.with_timezone(&tz), false)?
        .with_timezone(&Utc))
}

pub fn next_checkin(config: &Checkin, tz: Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if config.every <= 0 || config.start_hour >= config.end_hour || config.end_hour > 24 {
        return None;
    }
    let date = after.with_timezone(&tz).date_naive();
    let mut next = None;
    for day in 0..=8 {
        let Some(date) = date.checked_add_days(chrono::Days::new(day)) else {
            break;
        };
        let weekday = date.weekday().to_string().to_lowercase();
        if !config.days.split(',').any(|day| day == weekday) {
            continue;
        }
        let mut seconds = i64::from(config.start_hour) * 3600;
        let end = i64::from(config.end_hour) * 3600;
        while seconds < end {
            let local = date.and_hms_opt(0, 0, 0)? + chrono::Duration::seconds(seconds);
            let candidates = tz.from_local_datetime(&local);
            for candidate in [candidates.earliest(), candidates.latest()]
                .into_iter()
                .flatten()
            {
                let candidate = candidate.with_timezone(&Utc);
                if candidate > after && next.is_none_or(|current| candidate < current) {
                    next = Some(candidate);
                }
            }
            let Some(value) = seconds.checked_add(config.every) else {
                break;
            };
            seconds = value;
        }
    }
    next
}

pub(crate) fn target<S: MessengerStore + MemoryStore>(
    store: &S,
    to: Option<&str>,
    reply: Option<&str>,
) -> Result<Option<u64>> {
    if to.is_some() && reply.is_some() {
        return Err("specify at most one of to or reply".into());
    }
    let to_dest = if let Some(to) = to {
        let id = token(to, 'd')?;
        store.place(id)?.ok_or("destination not found")?;
        Some(id)
    } else {
        None
    };
    if let Some(reply) = reply {
        let id = token(reply, 'm')?;
        if !matches!(
            store.message(id)?.and_then(|m| m.meta),
            Some(Meta::Message { .. })
        ) {
            return Err("reply message not found".into());
        }
    }
    Ok(to_dest)
}

pub(crate) fn parse_time(value: &str, tz: Tz) -> Result<DateTime<Utc>> {
    Ok(match DateTime::parse_from_rfc3339(value) {
        Ok(value) => value.with_timezone(&Utc),
        Err(_) => {
            let local = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M")
                .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M"))?;
            tz.from_local_datetime(&local)
                .single()
                .ok_or("local time is ambiguous or does not exist")?
                .with_timezone(&Utc)
        }
    })
}

/// Validates an IANA timezone name (e.g. `Asia/Bangkok`, `UTC`).
pub fn validate_timezone(value: &str) -> Result<String> {
    let tz: Tz = value.parse()?;
    Ok(tz.name().to_string())
}

/// Resolves the configured `clock.timezone` (falling back to `UTC`).
pub(crate) fn current_timezone(config: &dyn ConfigStore) -> Tz {
    config
        .timezone()
        .ok()
        .and_then(|tz| tz.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

/// Formats a UTC RFC 3339 timestamp in `tz` as `Mon 2026-11-23 21:45 Asia/Tokyo`.
pub(crate) fn format_local_time(utc_rfc3339: &str, tz: Tz) -> Option<String> {
    let dt = DateTime::parse_from_rfc3339(utc_rfc3339).ok()?;
    let local = dt.with_timezone(&tz);
    Some(format!("{} {tz}", local.format("%a %Y-%m-%d %H:%M")))
}

/// Computes the next check-in unix timestamp (seconds) from now, if valid.
pub fn next_checkin_timestamp(config: &Checkin, timezone: &str) -> Option<i64> {
    let tz: Tz = timezone.parse().ok()?;
    next_checkin(config, tz, Utc::now()).map(|dt| dt.timestamp())
}

/// Expands a schedule's firing timestamps (unix seconds) within `[start, end]`,
/// capped at `limit` occurrences so high-frequency crons stay bounded.
pub fn occurrences(schedule: &Schedule, start: i64, end: i64, limit: usize) -> Vec<i64> {
    if end < start || limit == 0 {
        return Vec::new();
    }
    if schedule.at.is_some() {
        return if schedule.next_due >= start && schedule.next_due <= end {
            vec![schedule.next_due]
        } else {
            Vec::new()
        };
    }
    let Some(cron_expr) = schedule.cron.as_deref() else {
        return Vec::new();
    };
    let Ok(tz) = schedule.timezone.parse::<Tz>() else {
        return Vec::new();
    };
    let Ok(cron) = cron_expr.parse::<croner::Cron>() else {
        return Vec::new();
    };
    let Some(mut cursor) = DateTime::from_timestamp(start.saturating_sub(1), 0) else {
        return Vec::new();
    };
    let Some(end_dt) = DateTime::from_timestamp(end, 0) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while out.len() < limit {
        let Ok(next) = cron.find_next_occurrence(&cursor.with_timezone(&tz), false) else {
            break;
        };
        let next_utc = next.with_timezone(&Utc);
        if next_utc > end_dt {
            break;
        }
        out.push(next_utc.timestamp());
        cursor = next_utc;
    }
    out
}

pub fn add<S: ScheduleStore + MessengerStore + MemoryStore>(
    store: &S,
    config: &dyn ConfigStore,
    args: Add,
) -> Result<String> {
    if args.at.is_some() == args.cron.is_some() {
        return Err("specify exactly one of at or cron".into());
    }
    if args.to.is_some() && args.reply.is_some() {
        return Err("specify at most one of to or reply".into());
    }
    let timezone = match args.tz {
        Some(tz) => tz,
        None => config.timezone()?,
    };
    let tz: Tz = timezone.parse()?;
    let now = Utc::now();
    let due = if let Some(at) = &args.at {
        let due = parse_time(at, tz)?;
        if due <= now {
            return Err("at must be in the future".into());
        }
        due
    } else {
        next_cron(args.cron.as_deref().ok_or("missing cron")?, tz, now)?
    };
    let to_dest = target(store, args.to.as_deref(), args.reply.as_deref())?;
    let mut schedule = Schedule {
        id: 0,
        text: args.text,
        at: args.at,
        cron: args.cron,
        timezone,
        to_dest,
        reply: args.reply,
        next_due: due.timestamp() + i64::from(due.timestamp_subsec_nanos() > 0),
    };
    schedule.id = store.add_schedule(&schedule)?;
    Ok(format!(
        "scheduled s{}, next {} {}",
        schedule.id,
        due.with_timezone(&tz).format("%Y-%m-%d %H:%M"),
        tz
    ))
}

pub fn list<S: ScheduleStore>(store: &S) -> Result<String> {
    let schedules = store.schedules()?;
    if schedules.is_empty() {
        return Ok("no schedules".into());
    }
    schedules
        .iter()
        .map(|s| {
            let tz: Tz = s.timezone.parse()?;
            let due = DateTime::from_timestamp(s.next_due, 0)
                .ok_or("invalid schedule timestamp")?
                .with_timezone(&tz);
            let when = match (&s.at, &s.cron) {
                (Some(at), _) => format!("at {at}"),
                (_, Some(cron)) => format!("cron {cron}"),
                _ => return Err("invalid schedule".into()),
            };
            let target = s
                .to_dest
                .map(|id| format!(" to d{id}"))
                .or_else(|| s.reply.as_ref().map(|id| format!(" reply {id}")))
                .unwrap_or_default();
            Ok(format!(
                "s{} next {} {} {when} tz {}{target} {}",
                s.id,
                due.format("%Y-%m-%d %H:%M"),
                tz,
                s.timezone,
                s.text.escape_default()
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

pub fn remove<S: ScheduleStore>(store: &S, id: &str) -> Result<String> {
    if !store.remove_schedule(token(id, 's')?)? {
        return Err("schedule not found".into());
    }
    Ok(format!("removed {id}"))
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

pub fn execute<S: ScheduleStore + MessengerStore + MemoryStore>(
    store: &S,
    config: &dyn ConfigStore,
    name: &str,
    arguments: &str,
) -> Result<String> {
    match name {
        "schedule" => add(store, config, serde_json::from_str(arguments)?),
        "unschedule" => remove(store, &serde_json::from_str::<Id>(arguments)?.id),
        "schedules" => {
            let _: serde_json::Value = serde_json::from_str(arguments)?;
            list(store)
        }
        _ => Err("unknown schedule tool".into()),
    }
}

async fn home<S: MessengerStore>(
    store: &S,
    config: &dyn ConfigStore,
    messengers: Option<&crate::Messengers>,
    label: &str,
) -> Option<u64> {
    let result: Result<Option<u64>> = async {
        let Some(messengers) = messengers else {
            return Ok(None);
        };
        let Some(home) = config.home()? else {
            return Ok(None);
        };
        let messenger = messengers.find(&home.messenger)?;
        let place = if let Some(user) = home.target.strip_prefix("dm:") {
            messenger.direct(user).await?
        } else {
            storage::Place {
                channel: home.target,
                thread: None,
                dm: false,
            }
        };
        Ok(Some(store.destination(&home.messenger, &place)?))
    }
    .await;
    match result {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{label}: {error}");
            None
        }
    }
}

impl<E, S, M, F: crate::Models> crate::ChatHarness<E, S, M, F>
where
    S: ScheduleStore + MessengerStore + CheckinStore + WatchStore,
{
    pub async fn run_schedules(&self) {
        let mut active = None;
        let mut next_checkin_at = None;
        let mut last_slot: Option<DateTime<Utc>> = None;
        loop {
            if self
                .inner
                .schedule_gate
                .as_ref()
                .is_some_and(|ready| !ready())
            {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
            let now = Utc::now();
            let result: Result<()> = async {
                let config = self.inner.store.checkin()?;
                let tz: Tz = self.config.timezone()?.parse()?;
                if active.as_ref().map(|(config, tz)| (config, *tz))
                    != config.as_ref().map(|config| (config, tz))
                {
                    let after =
                        (now - chrono::Duration::minutes(2) - chrono::Duration::nanoseconds(1))
                            .max(last_slot.unwrap_or(DateTime::<Utc>::MIN_UTC));
                    next_checkin_at = config
                        .as_ref()
                        .and_then(|config| next_checkin(config, tz, after));
                    active = config.clone().map(|config| (config, tz));
                }
                if next_checkin_at.is_some_and(|due| now >= due) {
                    last_slot = next_checkin_at;
                    if next_checkin_at.is_some_and(|due| now - due <= chrono::Duration::minutes(2))
                    {
                        let mut watches = self.inner.store.watches()?;
                        for watch in &watches {
                            if watch.until.is_some_and(|until| until <= now.timestamp()) {
                                self.inner.store.remove_watch(watch.id)?;
                            }
                        }
                        watches.retain(|watch| {
                            watch.until.is_none_or(|until| until > now.timestamp())
                        });
                        if !watches.is_empty() {
                            let time = format!(
                                "{} {}",
                                now.with_timezone(&tz).format("%Y-%m-%d %H:%M"),
                                tz
                            );
                            let home = if watches
                                .iter()
                                .any(|watch| watch.to_dest.is_none() && watch.reply.is_none())
                            {
                                home(
                                    &self.inner.store,
                                    self.config.as_ref(),
                                    self.inner.messengers.as_ref(),
                                    "checkin",
                                )
                                .await
                            } else {
                                None
                            };
                            let report = crate::tasks::Report {
                                meta: Meta::Checkin { time, home },
                                text: storage::render_checkin_body(&watches, |until| {
                                    chrono::DateTime::from_timestamp(until, 0).map_or_else(
                                        || until.to_string(),
                                        |until| {
                                            format!(
                                                "{} {}",
                                                until.with_timezone(&tz).format("%Y-%m-%d %H:%M"),
                                                tz
                                            )
                                        },
                                    )
                                }),
                                name: String::new(),
                                run_id: 0,
                            };
                            self.inner
                                .state
                                .lock()
                                .expect("chat state mutex poisoned")
                                .reports
                                .push_back(report);
                            self.inner.notify.notify_one();
                        }
                    }
                    next_checkin_at = config
                        .as_ref()
                        .and_then(|config| next_checkin(config, tz, now));
                }
                for job in self.inner.store.due_schedules(now.timestamp())? {
                    let due = DateTime::from_timestamp(job.next_due, 0)
                        .ok_or("invalid schedule timestamp")?;
                    let tz: Tz = job.timezone.parse()?;
                    let home = if job.to_dest.is_none() && job.reply.is_none() {
                        home(
                            &self.inner.store,
                            self.config.as_ref(),
                            self.inner.messengers.as_ref(),
                            &format!("schedule s{}", job.id),
                        )
                        .await
                    } else {
                        None
                    };
                    let now = Utc::now();
                    let next = job
                        .cron
                        .as_deref()
                        .map(|cron| next_cron(cron, tz, now).map(|next| next.timestamp()))
                        .transpose()?;
                    let minutes = (now - due).num_minutes();
                    let late = if now - due > chrono::Duration::minutes(1) {
                        Some(if minutes >= 60 {
                            if minutes % 60 == 0 {
                                format!("{}h", minutes / 60)
                            } else {
                                format!("{}h{}m", minutes / 60, minutes % 60)
                            }
                        } else {
                            format!("{minutes}m")
                        })
                    } else {
                        None
                    };
                    if !self
                        .inner
                        .store
                        .claim_schedule(job.id, job.next_due, next)?
                    {
                        continue;
                    }
                    let report = crate::tasks::Report {
                        meta: Meta::Scheduled {
                            id: format!("s{}", job.id),
                            due: due.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string(),
                            late,
                            timezone: job.timezone,
                            to: job.to_dest,
                            for_message: job.reply,
                            home,
                        },
                        text: job.text,
                        name: String::new(),
                        run_id: 0,
                    };
                    self.inner
                        .state
                        .lock()
                        .expect("chat state mutex poisoned")
                        .reports
                        .push_back(report);
                    self.inner.notify.notify_one();
                }
                Ok(())
            }
            .await;
            let failed = result.is_err();
            let wait = match result {
                Err(error) => {
                    eprintln!("{error}");
                    std::time::Duration::from_secs(60)
                }
                Ok(()) => match self
                    .inner
                    .store
                    .schedules()
                    .map(|jobs| jobs.first().map(|job| job.next_due))
                {
                    Ok(Some(due)) => match DateTime::from_timestamp(due, 0) {
                        Some(due) => (due - Utc::now())
                            .to_std()
                            .unwrap_or_default()
                            .min(std::time::Duration::from_secs(60)),
                        None => {
                            eprintln!("invalid schedule timestamp");
                            std::time::Duration::from_secs(60)
                        }
                    },
                    Ok(None) => std::time::Duration::from_secs(60),
                    Err(error) => {
                        eprintln!("{error}");
                        std::time::Duration::from_secs(60)
                    }
                },
            };
            let wait = if failed {
                wait
            } else {
                next_checkin_at.map_or(wait, |due| {
                    wait.min((due - Utc::now()).to_std().unwrap_or_default())
                })
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage::{Meta, Person, Place};

    #[test]
    fn formats_local_time_and_attaches_to_rendered_inputs() {
        let tokyo: Tz = "Asia/Tokyo".parse().unwrap();
        let formatted = format_local_time("2026-11-23T12:45:09Z", tokyo).unwrap();
        assert_eq!(formatted, "Mon 2026-11-23 21:45 Asia/Tokyo");

        let utc_formatted = format_local_time("2026-11-23T12:45:09Z", chrono_tz::UTC).unwrap();
        assert_eq!(utc_formatted, "Mon 2026-11-23 12:45 UTC");

        // Plain CLI user message gets time="..."
        let cli = storage::render_input(None, 1, "hi", Some(&formatted));
        assert_eq!(
            cli,
            "<message time=\"Mon 2026-11-23 21:45 Asia/Tokyo\">\nhi\n</message>"
        );

        // Messenger <message> gets time="..." right after where="..."
        let msg_meta = Meta::Message {
            messenger: "discord".into(),
            place: Place {
                channel: "c1".into(),
                thread: None,
                dm: false,
            },
            message: "m1".into(),
            author: Person {
                id: "u1".into(),
                name: "Chan".into(),
            },
            to: 7,
            location: "#dev".into(),
            earlier: None,
            quote: None,
            attachments: Vec::new(),
            inline: false,
            unaddressed: false,
        };
        let rendered_msg = storage::render_input(Some(&msg_meta), 42, "yo", Some(&formatted));
        assert!(rendered_msg.starts_with(
            "<message messenger=\"discord\" reply=\"m42\" to=\"d7\" author=\"u1\" author_name=\"Chan\" where=\"#dev\" time=\"Mon 2026-11-23 21:45 Asia/Tokyo\">"
        ));

        // <event> gets time="..." after for="..."
        let ev_meta = Meta::Event {
            source: "callback".into(),
            name: "build".into(),
            for_message: Some("m42".into()),
        };
        let rendered_ev = storage::render_input(Some(&ev_meta), 43, "ok", Some(&formatted));
        assert_eq!(
            rendered_ev,
            "<event source=\"callback\" name=\"build\" for=\"m42\" time=\"Mon 2026-11-23 21:45 Asia/Tokyo\">\nok\n</event>"
        );
    }
}
