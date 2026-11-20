use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use environment::ToolSpec;
use serde::Deserialize;
use storage::{ConfigStore, MemoryStore, MessengerStore, Meta, Schedule, ScheduleStore};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "schedule",
        description: "Schedule a message at a time or on a cron schedule.",
        input_schema: r#"{"type":"object","properties":{"text":{"type":"string"},"at":{"type":"string"},"cron":{"type":"string"},"tz":{"type":"string"},"to":{"type":"string"},"reply":{"type":"string"}},"required":["text"]}"#,
    },
    ToolSpec {
        name: "unschedule",
        description: "Remove a schedule by id.",
        input_schema: r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#,
    },
    ToolSpec {
        name: "schedules",
        description: "List schedules.",
        input_schema: r#"{"type":"object","properties":{}}"#,
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

fn token(value: &str, prefix: char) -> Result<u64> {
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
        let due = match DateTime::parse_from_rfc3339(at) {
            Ok(value) => value.with_timezone(&Utc),
            Err(_) => tz
                .from_local_datetime(&NaiveDateTime::parse_from_str(at, "%Y-%m-%d %H:%M")?)
                .single()
                .ok_or("local time is ambiguous or does not exist")?
                .with_timezone(&Utc),
        };
        if due <= now {
            return Err("at must be in the future".into());
        }
        due
    } else {
        next_cron(args.cron.as_deref().ok_or("missing cron")?, tz, now)?
    };
    let to_dest = if let Some(to) = &args.to {
        let id = token(to, 'd')?;
        store.place(id)?.ok_or("destination not found")?;
        Some(id)
    } else {
        None
    };
    if let Some(reply) = &args.reply {
        let id = token(reply, 'm')?;
        if !matches!(
            store.message(id)?.and_then(|m| m.meta),
            Some(Meta::Message { .. })
        ) {
            return Err("reply message not found".into());
        }
    }
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
    id: u64,
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
            eprintln!("schedule s{id}: {error}");
            None
        }
    }
}

impl<L, E, S, M, F: crate::Models> crate::ChatHarness<L, E, S, M, F>
where
    S: ScheduleStore + MessengerStore,
{
    pub async fn run_schedules(&self) {
        loop {
            let now = Utc::now();
            let result: Result<()> = async {
                for job in self.inner.store.due_schedules(now.timestamp())? {
                    let due = DateTime::from_timestamp(job.next_due, 0)
                        .ok_or("invalid schedule timestamp")?;
                    let tz: Tz = job.timezone.parse()?;
                    let home = if job.to_dest.is_none() && job.reply.is_none() {
                        home(
                            &self.inner.store,
                            self.config.as_ref(),
                            self.inner.messengers.as_ref(),
                            job.id,
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
            tokio::time::sleep(wait).await;
        }
    }
}
