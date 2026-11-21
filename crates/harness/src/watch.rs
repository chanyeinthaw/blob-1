use std::borrow::Cow;

use crate::schedule::{parse_time, target, token, Result};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use environment::ToolSpec;
use serde::Deserialize;
use storage::{ConfigStore, MemoryStore, MessengerStore, Watch, WatchStore};

pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: Cow::Borrowed("watch"),
        description: Cow::Borrowed("Keep an eye on something."),
        input_schema: Cow::Borrowed(
            r#"{"type":"object","properties":{"text":{"type":"string"},"to":{"type":"string"},"reply":{"type":"string"},"until":{"type":"string"}},"required":["text"]}"#,
        ),
    },
    ToolSpec {
        name: Cow::Borrowed("unwatch"),
        description: Cow::Borrowed("Remove a watch."),
        input_schema: Cow::Borrowed(
            r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#,
        ),
    },
    ToolSpec {
        name: Cow::Borrowed("watches"),
        description: Cow::Borrowed("List watches."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{}}"#),
    },
];

#[derive(Deserialize)]
struct Add {
    text: String,
    to: Option<String>,
    reply: Option<String>,
    until: Option<String>,
}
#[derive(Deserialize)]
struct Id {
    id: String,
}

pub fn list<S: WatchStore>(store: &S, config: &dyn ConfigStore) -> Result<String> {
    let watches = store.watches()?;
    if watches.is_empty() {
        return Ok("no watches".into());
    }
    let tz: Tz = config.timezone()?.parse()?;
    watches
        .iter()
        .map(|w| {
            let until = w
                .until
                .map(|value| -> Result<String> {
                    let time = DateTime::from_timestamp(value, 0)
                        .ok_or("invalid watch timestamp")?
                        .with_timezone(&tz);
                    Ok(format!(" until {} {tz}", time.format("%Y-%m-%d %H:%M")))
                })
                .transpose()?
                .unwrap_or_default();
            let target = w
                .to_dest
                .map(|id| format!(" to d{id}"))
                .or_else(|| w.reply.as_ref().map(|id| format!(" reply {id}")))
                .unwrap_or_default();
            Ok(format!(
                "w{}{until}{target} {}",
                w.id,
                w.text.escape_default()
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

pub fn remove<S: WatchStore>(store: &S, id: &str) -> Result<String> {
    if !store.remove_watch(token(id, 'w')?)? {
        return Err("watch not found".into());
    }
    Ok(format!("removed {id}"))
}

pub fn execute<S: WatchStore + MessengerStore + MemoryStore>(
    store: &S,
    config: &dyn ConfigStore,
    name: &str,
    arguments: &str,
) -> Result<String> {
    match name {
        "watch" => {
            let args: Add = serde_json::from_str(arguments)?;
            if args.text.trim().is_empty() {
                return Err("watch text must not be blank".into());
            }
            if storage::escape(&args.text).chars().count() > 1000 {
                return Err("watch text must not exceed 1000 characters".into());
            }
            let now = Utc::now().timestamp();
            if store
                .watches()?
                .iter()
                .filter(|watch| watch.until.is_none_or(|until| until > now))
                .count()
                >= 20
            {
                return Err("at most 20 active watches are allowed".into());
            }
            let to_dest = target(store, args.to.as_deref(), args.reply.as_deref())?;
            let until = args
                .until
                .as_deref()
                .map(|value| -> Result<i64> {
                    let tz: Tz = config.timezone()?.parse()?;
                    let time = parse_time(value, tz)?;
                    if time <= Utc::now() {
                        return Err("until must be in the future".into());
                    }
                    Ok(time.timestamp() + i64::from(time.timestamp_subsec_nanos() > 0))
                })
                .transpose()?;
            let id = store.add_watch(&Watch {
                id: 0,
                text: args.text,
                to_dest,
                reply: args.reply,
                until,
            })?;
            Ok(format!("watching w{id}"))
        }
        "unwatch" => remove(store, &serde_json::from_str::<Id>(arguments)?.id),
        "watches" => {
            let _: serde_json::Value = serde_json::from_str(arguments)?;
            list(store, config)
        }
        _ => Err("unknown watch tool".into()),
    }
}
