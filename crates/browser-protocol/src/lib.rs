//! Wire protocol between `blob-1 serve` and the `blob-1-browser` companion.
//!
//! The companion listens on a Unix socket (`0600`, inside the serve data dir's
//! `runtime/`) and serve connects as the only client. Each frame is one line
//! of JSON (`serde_json` never emits raw newlines). Serve sends
//! [`ClientFrame`]s; the companion answers every `Request` with exactly one
//! `Reply` carrying the same `id`, in any order.

use serde::{Deserialize, Serialize};

/// Bumped on any incompatible change; the companion refuses other versions.
pub const PROTOCOL_VERSION: u32 = 1;

/// Biggest frame either side accepts (a full-page PNG, base64-encoded).
pub const MAX_FRAME_BYTES: usize = 64 << 20;

/// Serve → companion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientFrame {
    /// First frame after connecting.
    Hello { protocol: u32 },
    Request { id: u64, request: Request },
    /// Abandons an in-flight request (timeout, caller went away). The
    /// companion may still finish it; its reply is then ignored.
    Cancel { id: u64 },
    Ping,
}

/// Companion → serve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerFrame {
    /// Answer to `Hello`: the companion's version, for logs.
    Ready { version: String },
    Reply {
        id: u64,
        #[serde(flatten)]
        result: ReplyResult,
    },
    Pong,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReplyResult {
    Ok(Reply),
    Error(String),
}

/// One browser operation. Tabs belong to an `owner` (a task id) inside a
/// named `profile`; a profile is one persistent cookie/storage jar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Request {
    /// Opens a new tab, optionally loading `url`, and waits for the load.
    Open {
        profile: String,
        owner: String,
        url: Option<String>,
    },
    /// Loads `url` in the tab and waits for the load.
    Navigate { tab: String, url: String },
    /// The tab's interactive elements as an indented text tree; each
    /// actionable node carries a `ref` that `Act` accepts. Refs are only
    /// valid until the next `Snapshot` of the same tab.
    Snapshot { tab: String },
    Act {
        tab: String,
        #[serde(rename = "ref")]
        element: String,
        action: Action,
    },
    /// Runs `expression` in the page's main frame; promises are awaited.
    Evaluate { tab: String, expression: String },
    Screenshot { tab: String, full_page: bool },
    /// The owner's open tabs.
    Tabs { owner: String },
    Close { tab: String },
    /// Closes every tab of `owner` (its task ended).
    CloseOwner { owner: String },
    /// Closes the profile's tabs and deletes its stored data.
    DeleteProfile { profile: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Action {
    Click,
    /// Focuses the element, replaces its value with `text` as real key
    /// input, and presses Enter afterwards when `submit` is set.
    Type { text: String, submit: bool },
    /// A key such as `Enter`, `Tab`, `Escape`, `ArrowDown`.
    Press { key: String },
    /// Picks the `<option>` whose value or label is `value`.
    Select { value: String },
    Hover,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Reply {
    Opened { tab: String, page: Page },
    Navigated { page: Page },
    Snapshot { page: Page, tree: String },
    Acted { page: Page },
    Evaluated { value: serde_json::Value },
    Screenshot {
        #[serde(with = "base64_bytes")]
        png: Vec<u8>,
    },
    Tabs { tabs: Vec<TabInfo> },
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabInfo {
    pub tab: String,
    pub profile: String,
    pub page: Page,
}

/// Profile names are path components on the companion's disk.
pub fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
}

mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}
