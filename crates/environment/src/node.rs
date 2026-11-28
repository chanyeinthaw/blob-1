//! Wire protocol between a primary `blob-1 serve` and a `blob-1 node`
//! (remote execution environment) over `/ws/node`. Nodes dial out, present a
//! `hello`, then sit pending until an admin accepts them in the web UI or CLI.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{SkillBundle, SkillEntry, ToolCall, ToolResult};

pub const NODE_PROTOCOL_VERSION: u32 = 1;

/// Frame and message cap on both ends of `/ws/node`: fits a 20 MiB image
/// read (base64 inside a JSON reply), above tungstenite's 16 MiB frame default.
pub const MAX_FRAME_BYTES: usize = 64 << 20;

/// The primary sends `Ping` this often and the node answers `Pong`.
pub const PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Either end drops a socket that has been silent this long, so a half-open
/// connection can't keep an environment "online". Pongs queue behind a reply
/// being sent, so this covers the 120s execute deadline plus one ping.
pub const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(150);

/// First frame a node sends after the socket opens. `node_id` and `node_key`
/// are generated once and persisted in the node's `node.toml`; the primary
/// stores `sha256(node_key)` as the environment credential at accept time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub node_id: String,
    pub hostname: String,
    pub version: String,
    pub protocol: u32,
    pub node_key: String,
    /// Absolute workspaces root on the node (`<workspaces>/<workspace>` is a
    /// bot's remote working directory) — reported so prompts can name the
    /// real remote path. Empty for v1 hellos that predate the field.
    #[serde(default)]
    pub workspaces: String,
}

/// One environment method invocation routed to the node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum NodeRequest {
    Execute {
        workspace: String,
        call: ToolCall,
    },
    ManagedSkills {
        workspace: String,
    },
    ApplySkills {
        workspace: String,
        put: Vec<SkillBundle>,
        remove: Vec<String>,
    },
    SkillCatalog {
        workspace: String,
    },
    RemoveDetectedSkill {
        workspace: String,
        folder: String,
    },
    ReadDetectedSkill {
        workspace: String,
        folder: String,
    },
    /// Writes a secret to a private file the node deletes after `ttl_secs`.
    WriteSecret {
        workspace: String,
        name: String,
        value: String,
        ttl_secs: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NodeReply {
    Execute {
        result: ToolResult,
    },
    ManagedSkills {
        manifest: BTreeMap<String, (String, bool)>,
    },
    ApplySkills {
        skipped: Vec<String>,
    },
    SkillCatalog {
        entries: Vec<SkillEntry>,
    },
    RemoveDetectedSkill,
    ReadDetectedSkill {
        files: Vec<crate::SkillFile>,
    },
    WriteSecret {
        path: String,
    },
    Error {
        error: String,
    },
}

/// Primary → node frames after `hello`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PrimaryFrame {
    /// The node is connected but not yet an accepted environment; it must
    /// only listen for `invited`.
    Pending,
    /// An admin accepted this node: it is now the environment `name`.
    Invited {
        name: String,
    },
    /// Permanent refusal (protocol mismatch, revoked credential).
    Rejected {
        reason: String,
    },
    Request {
        id: u64,
        request: NodeRequest,
    },
    /// Aborts an in-flight `Request` (timeout, caller went away).
    Cancel {
        id: u64,
    },
    Ping,
}

/// Node → primary frames after `hello`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NodeFrame {
    Result { id: u64, reply: NodeReply },
    Pong,
}
