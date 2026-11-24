use std::{borrow::Cow, collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

pub fn valid_skill_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains('/')
}

pub const SKILL_FILE: &str = "SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
}

pub fn parse_frontmatter(content: &str) -> Option<Frontmatter> {
    let body = content
        .strip_prefix("---")?
        .trim_start_matches(['\r', '\n']);
    let header = &body[..body.find("\n---")?];
    let mut name = None;
    let mut description = None;
    for line in header.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']).to_owned();
        match key.trim() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            _ => {}
        }
    }
    Some(Frontmatter {
        name: name.filter(|name| !name.is_empty())?,
        description: description.filter(|description| !description.is_empty())?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFile {
    pub path: String,
    pub mode: u32,
    #[serde(with = "base64_bytes")]
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBundle {
    pub name: String,
    pub hash: String,
    pub files: Vec<SkillFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub location: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "name",
    content = "arguments",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ToolInput {
    Read {
        path: PathBuf,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    Write {
        path: PathBuf,
        content: String,
    },
    Edit {
        path: PathBuf,
        edits: Vec<Replacement>,
    },
    Bash {
        command: String,
        timeout: Option<f64>,
    },
}

impl ToolInput {
    pub fn parse(name: &str, arguments: &str) -> Result<Self, ToolError> {
        let invalid = |error: serde_json::Error| {
            ToolError::Invalid(format!("Invalid arguments for tool \"{name}\": {error}"))
        };
        let arguments: serde_json::Value = serde_json::from_str(arguments).map_err(invalid)?;
        serde_json::from_value(serde_json::json!({ "name": name, "arguments": arguments }))
            .map_err(invalid)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Read { .. } => "read",
            Self::Write { .. } => "write",
            Self::Edit { .. } => "edit",
            Self::Bash { .. } => "bash",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Replacement {
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub input: ToolInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub mime: String,
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

/// Byte fields cross the `/ws/node` wire as base64 strings; serde's default
/// JSON number array would roughly quadruple image and skill payloads.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
    pub images: Vec<Image>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: Cow<'static, str>,
    pub description: Cow<'static, str>,
    pub input_schema: Cow<'static, str>,
}

impl ToolSpec {
    pub const fn new(
        name: &'static str,
        description: &'static str,
        input_schema: &'static str,
    ) -> Self {
        Self {
            name: Cow::Borrowed(name),
            description: Cow::Borrowed(description),
            input_schema: Cow::Borrowed(input_schema),
        }
    }
}

pub const CATALOG: &[ToolSpec] = &[
    ToolSpec {
        name: Cow::Borrowed("read"),
        description: Cow::Borrowed("Read a text or image file. Text output is truncated to 2000 lines or 50KB (whichever is hit first). Use offset/limit for large text files. When you need the full file, continue with offset until complete."),
        input_schema: Cow::Borrowed(r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string","description":"Path to the file to read (relative or absolute)"},"offset":{"type":"integer","minimum":1,"description":"Line number to start reading from (1-indexed)"},"limit":{"type":"integer","minimum":0,"description":"Maximum number of lines to read"}}}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("write"),
        description: Cow::Borrowed("Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories."),
        input_schema: Cow::Borrowed(r#"{"type":"object","required":["path","content"],"properties":{"path":{"type":"string","description":"Path to the file to write (relative or absolute)"},"content":{"type":"string","description":"Content to write to the file"}}}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("edit"),
        description: Cow::Borrowed("Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes."),
        input_schema: Cow::Borrowed(r#"{"type":"object","required":["path","edits"],"properties":{"path":{"type":"string","description":"Path to the file to edit (relative or absolute)"},"edits":{"type":"array","description":"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.","items":{"type":"object","required":["oldText","newText"],"properties":{"oldText":{"type":"string","description":"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},"newText":{"type":"string","description":"Replacement text for this targeted edit."}}}}}}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("bash"),
        description: Cow::Borrowed("Execute a bash command in the workspace directory. Returns stdout and stderr. Output is truncated to last 2000 lines or 50KB (whichever is hit first). Optionally provide a timeout in seconds."),
        input_schema: Cow::Borrowed(r#"{"type":"object","required":["command"],"properties":{"command":{"type":"string","description":"Shell command to execute"},"timeout":{"type":"number","description":"Timeout in seconds (optional, no default timeout)"}}}"#),
    },
];

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("File not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(String),
}

#[async_trait::async_trait]
pub trait Environment: Send + Sync {
    async fn managed_skills(&self) -> Result<BTreeMap<String, (String, bool)>, ToolError>;
    async fn apply_skills(
        &self,
        put: Vec<SkillBundle>,
        remove: Vec<String>,
    ) -> Result<Vec<String>, ToolError>;
    async fn skill_catalog(&self) -> Result<Vec<SkillEntry>, ToolError>;
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, ToolError>;
}

#[async_trait::async_trait]
impl Environment for std::sync::Arc<dyn Environment> {
    async fn managed_skills(&self) -> Result<BTreeMap<String, (String, bool)>, ToolError> {
        (**self).managed_skills().await
    }
    async fn apply_skills(
        &self,
        put: Vec<SkillBundle>,
        remove: Vec<String>,
    ) -> Result<Vec<String>, ToolError> {
        (**self).apply_skills(put, remove).await
    }
    async fn skill_catalog(&self) -> Result<Vec<SkillEntry>, ToolError> {
        (**self).skill_catalog().await
    }
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, ToolError> {
        (**self).execute(call).await
    }
}

pub mod node;
