use std::{collections::BTreeMap, sync::Mutex};

use base64::Engine;
use environment::{Image, ToolSpec};
use rmcp::model::*;

use crate::{spec_for, Mcp, REQUEST_TIMEOUT};

/// One model-visible MCP tool loaded into a session by `mcp_search`.
struct Loaded {
    server: String,
    tool: String,
    spec: ToolSpec,
}

/// Result of an MCP call converted to the harness's text-plus-images shape.
pub struct Outcome {
    pub content: String,
    pub images: Vec<Image>,
}

impl Outcome {
    fn text(content: String) -> Self {
        Self {
            content,
            images: Vec::new(),
        }
    }
}

/// Per-session tool set: the `mcp_*` built-ins plus tools loaded by
/// `mcp_search`. A fresh SessionTools per session is what scopes loaded
/// tools to the chat compaction boundary or task lifetime.
pub struct SessionTools {
    mcp: Mcp,
    loaded: Mutex<BTreeMap<String, Loaded>>,
    advertised: Mutex<BTreeMap<String, (String, String)>>,
}

const BUILTINS: &[ToolSpec] = &[
    ToolSpec::new(
        "mcp_search",
        "Search tools provided by the configured MCP servers. Matches are returned as server__tool with a description and their full schemas are loaded into this session; afterwards call them by that name.",
        r#"{"type":"object","properties":{"query":{"type":"string","description":"Keywords matched against tool names and descriptions"}},"required":["query"]}"#,
    ),
    ToolSpec::new(
        "mcp_resources",
        "List resources and resource templates exposed by MCP servers. Filter by server id and/or query words.",
        r#"{"type":"object","properties":{"server":{"type":"string","description":"MCP server id; omit to search all enabled servers"},"query":{"type":"string"}}}"#,
    ),
    ToolSpec::new(
        "mcp_read",
        "Read an MCP resource by URI.",
        r#"{"type":"object","properties":{"server":{"type":"string","description":"MCP server id"},"uri":{"type":"string","description":"Resource URI"}},"required":["server","uri"]}"#,
    ),
    ToolSpec::new(
        "mcp_prompts",
        "List prompts exposed by MCP servers with their arguments. Filter by server id and/or query words.",
        r#"{"type":"object","properties":{"server":{"type":"string","description":"MCP server id; omit to search all enabled servers"},"query":{"type":"string"}}}"#,
    ),
    ToolSpec::new(
        "mcp_prompt",
        "Fetch an MCP prompt. Returns its messages as text.",
        r#"{"type":"object","properties":{"server":{"type":"string","description":"MCP server id"},"name":{"type":"string","description":"Prompt name"},"arguments":{"type":"object","description":"Prompt arguments as a JSON object"}},"required":["server","name"]}"#,
    ),
];

fn terms(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect()
}

fn arg<'a>(arguments: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(|value| value.as_str())
}

fn required<'a>(arguments: &'a serde_json::Value, name: &str) -> Result<&'a str, String> {
    arg(arguments, name).ok_or_else(|| format!("missing '{name}'"))
}

fn push_image(images: &mut Vec<Image>, mime: &str, base64_data: &str) -> Option<String> {
    if !mime.starts_with("image/") {
        return None;
    }
    match base64::prelude::BASE64_STANDARD.decode(base64_data) {
        Ok(data) => {
            let note = format!("[image {mime}, {} bytes]", data.len());
            if matches!(
                mime,
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            ) {
                images.push(Image {
                    mime: mime.to_string(),
                    data,
                });
            }
            Some(note)
        }
        Err(_) => Some(format!("[image {mime}, undecodable]")),
    }
}

fn blocks_outcome(blocks: &[ContentBlock]) -> Outcome {
    let mut text = Vec::new();
    let mut images = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(content) => text.push(content.text.clone()),
            ContentBlock::Image(content) => {
                text.push(
                    push_image(&mut images, &content.mime_type, &content.data)
                        .unwrap_or_else(|| format!("[image {}]", content.mime_type)),
                );
            }
            ContentBlock::Audio(content) => {
                text.push(format!("[audio {}]", content.mime_type));
            }
            ContentBlock::ResourceLink(link) => {
                text.push(format!(
                    "[resource {}: {}{}]",
                    link.name,
                    link.uri,
                    link.description
                        .as_deref()
                        .map(|d| format!(" — {d}"))
                        .unwrap_or_default()
                ));
            }
            ContentBlock::Resource(embedded) => match &embedded.resource {
                ResourceContents::TextResourceContents {
                    uri, text: body, ..
                } => {
                    text.push(format!("--- {uri} ---\n{body}"));
                }
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => {
                    let mime = mime_type.as_deref().unwrap_or("application/octet-stream");
                    text.push(push_image(&mut images, mime, blob).unwrap_or_else(|| {
                        format!(
                            "[binary resource {uri} {mime}, {} bytes]",
                            blob.len() * 3 / 4
                        )
                    }));
                }
                _ => text.push("[unsupported resource content]".to_string()),
            },
            _ => text.push("[unsupported content]".to_string()),
        }
    }
    Outcome {
        content: text.join("\n"),
        images,
    }
}

/// Flatten a CallToolResult into text lines plus extracted images.
/// structuredContent often repeats the content blocks for machine readers, so it
/// is appended only when there is no text block and it doesn't repeat an image.
fn outcome_of(result: &CallToolResult) -> Outcome {
    let mut outcome = blocks_outcome(&result.content);
    let has_text = result
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Text(_)));
    if !has_text {
        if let Some(structured) = &result.structured_content {
            let json = serde_json::to_string(structured).unwrap_or_default();
            let repeats_image = result.content.iter().any(|block| match block {
                ContentBlock::Image(image) => json.contains(image.data.as_str()),
                _ => false,
            });
            if !repeats_image {
                if !outcome.content.is_empty() {
                    outcome.content.push('\n');
                }
                outcome.content.push_str(&json);
            }
        }
    }
    outcome
}

impl SessionTools {
    pub fn new(mcp: Mcp) -> Self {
        Self {
            mcp,
            loaded: Mutex::new(BTreeMap::new()),
            advertised: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn mcp(&self) -> &Mcp {
        &self.mcp
    }

    /// Drop every loaded tool; called when the session's context resets.
    pub fn clear(&self) {
        self.loaded.lock().expect("loaded tools").clear();
        self.advertised.lock().expect("advertised tools").clear();
    }

    /// Built-ins (when at least one server is enabled) plus every tool
    /// loaded via mcp_search, with schemas refreshed from the server. A
    /// wire name never changes once assigned; tools the server dropped are
    /// unloaded here. One tools/list per server per refresh, refreshed
    /// concurrently with a per-server deadline covering connection.
    pub async fn specs(&self) -> Vec<ToolSpec> {
        if !self.mcp.any_enabled() {
            self.advertised.lock().expect("advertised tools").clear();
            return Vec::new();
        }
        let enabled = self.mcp.enabled_ids().await;
        let mut specs: Vec<ToolSpec> = BUILTINS.to_vec();
        let mut by_server: BTreeMap<String, Vec<(String, String, ToolSpec)>> = BTreeMap::new();
        {
            let loaded = self.loaded.lock().expect("loaded tools");
            for (wire, l) in loaded.iter() {
                by_server.entry(l.server.clone()).or_default().push((
                    wire.clone(),
                    l.tool.clone(),
                    l.spec.clone(),
                ));
            }
        }
        let mut join = tokio::task::JoinSet::new();
        let mut dropped = Vec::new();
        for (server, tools) in by_server {
            if !enabled.iter().any(|id| id == &server) {
                dropped.extend(tools.into_iter().map(|(wire, _, _)| wire));
                continue;
            }
            let mcp = self.mcp.clone();
            join.spawn(async move {
                let refreshed = mcp.tool_list(&server, REQUEST_TIMEOUT).await;
                (server, tools, refreshed)
            });
        }
        let mut refreshed_servers = Vec::new();
        while let Some(Ok(result)) = join.join_next().await {
            refreshed_servers.push(result);
        }
        refreshed_servers.sort_by(|a, b| a.0.cmp(&b.0));
        let mut advertised = BTreeMap::new();
        for (server, tools, refreshed) in refreshed_servers {
            match refreshed {
                Some(list) => {
                    for (wire, tool, _) in tools {
                        match list
                            .iter()
                            .find(|candidate| candidate.name.as_ref() == tool)
                        {
                            Some(found) => {
                                specs.push(spec_for(&wire, found));
                                advertised.insert(wire, (server.clone(), tool));
                            }
                            None => dropped.push(wire),
                        }
                    }
                }
                None => {
                    for (wire, tool, fallback) in tools {
                        specs.push(fallback);
                        advertised.insert(wire, (server.clone(), tool));
                    }
                }
            }
        }
        if !dropped.is_empty() {
            let mut loaded = self.loaded.lock().expect("loaded tools");
            for wire in dropped {
                loaded.remove(&wire);
            }
        }
        *self.advertised.lock().expect("advertised tools") = advertised;
        specs
    }

    /// Dispatch a tool call; `None` means the name isn't ours and the caller
    /// should try the environment tools.
    pub async fn execute(&self, name: &str, arguments: &str) -> Option<Result<Outcome, String>> {
        let parsed: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(value) => value,
            Err(error) => return Some(Err(format!("arguments must be JSON: {error}"))),
        };
        match name {
            "mcp_search" => Some(self.search(&parsed).await),
            "mcp_resources" => Some(self.resources(&parsed).await),
            "mcp_read" => Some(self.read(&parsed).await),
            "mcp_prompts" => Some(self.prompts(&parsed).await),
            "mcp_prompt" => Some(self.prompt(&parsed).await),
            _ => {
                let target = self
                    .advertised
                    .lock()
                    .expect("advertised tools")
                    .get(name)
                    .cloned();
                match target {
                    Some((server, tool)) => {
                        match self.mcp.call_tool(&server, &tool, arguments).await {
                            Ok(result) if result.is_error == Some(true) => {
                                Some(Err(outcome_of(&result).content))
                            }
                            Ok(result) => Some(Ok(outcome_of(&result))),
                            Err(error) => Some(Err(error)),
                        }
                    }
                    None => None,
                }
            }
        }
    }

    async fn enabled_ids(&self) -> Vec<String> {
        self.mcp.enabled_ids().await
    }

    async fn search(&self, arguments: &serde_json::Value) -> Result<Outcome, String> {
        let query = match required(arguments, "query") {
            Ok(query) => query,
            Err(error) => return Err(error),
        };
        let terms = terms(query);
        let mut scored: Vec<(usize, String, String, Tool)> = Vec::new();
        let mut notes = Vec::new();
        for id in self.enabled_ids().await {
            match self
                .mcp
                .request(&id, REQUEST_TIMEOUT, |peer| async move {
                    peer.list_all_tools().await
                })
                .await
            {
                Ok(tools) => {
                    for (wire, tool) in Mcp::wire_names(&id, &tools) {
                        let text = format!(
                            "{} {}",
                            tool.name,
                            tool.description.as_deref().unwrap_or_default()
                        )
                        .to_lowercase();
                        let score = terms
                            .iter()
                            .filter(|term| text.contains(term.as_str()))
                            .count();
                        if score > 0 || terms.is_empty() {
                            scored.push((score, id.clone(), wire, tool));
                        }
                    }
                }
                Err(error) => notes.push(format!("{id}: {error}")),
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
        scored.truncate(10);
        let mut lines = Vec::new();
        {
            let mut loaded = self.loaded.lock().expect("loaded tools");
            for (_, server, wire, tool) in scored {
                lines.push(format!(
                    "{wire} — {}",
                    tool.description.as_deref().unwrap_or("(no description)")
                ));
                loaded.insert(
                    wire.clone(),
                    Loaded {
                        server,
                        tool: tool.name.to_string(),
                        spec: spec_for(&wire, &tool),
                    },
                );
            }
        }
        lines.extend(notes);
        if lines.is_empty() {
            lines.push("no matching tools".to_string());
        }
        Ok(Outcome::text(format!(
            "loaded tools:\n{}",
            lines.join("\n")
        )))
    }

    async fn resources(&self, arguments: &serde_json::Value) -> Result<Outcome, String> {
        let filter = arg(arguments, "server");
        let query = terms(arg(arguments, "query").unwrap_or_default());
        let ids: Vec<String> = match filter {
            Some(id) => vec![id.to_string()],
            None => self.enabled_ids().await,
        };
        let mut lines = Vec::new();
        for id in ids {
            match self
                .mcp
                .request(&id, REQUEST_TIMEOUT, |peer| async move {
                    peer.list_all_resources().await
                })
                .await
            {
                Ok(resources) => {
                    for resource in resources {
                        let line = format!(
                            "{id}: {} — {} {}",
                            resource.uri,
                            resource.name,
                            resource.description.as_deref().unwrap_or_default()
                        );
                        if query
                            .iter()
                            .all(|term| line.to_lowercase().contains(term.as_str()))
                        {
                            lines.push(line);
                        }
                    }
                }
                Err(error) => lines.push(format!("{id}: resources: {error}")),
            }
            match self
                .mcp
                .request(&id, REQUEST_TIMEOUT, |peer| async move {
                    peer.list_all_resource_templates().await
                })
                .await
            {
                Ok(templates) => {
                    for template in templates {
                        let line = format!(
                            "{id}: template {} — {} {}",
                            template.uri_template,
                            template.name,
                            template.description.as_deref().unwrap_or_default()
                        );
                        if query
                            .iter()
                            .all(|term| line.to_lowercase().contains(term.as_str()))
                        {
                            lines.push(line);
                        }
                    }
                }
                Err(error) => lines.push(format!("{id}: templates: {error}")),
            }
        }
        if lines.is_empty() {
            lines.push("no resources".to_string());
        }
        Ok(Outcome::text(lines.join("\n")))
    }

    async fn read(&self, arguments: &serde_json::Value) -> Result<Outcome, String> {
        let (server, uri) = match (required(arguments, "server"), required(arguments, "uri")) {
            (Ok(server), Ok(uri)) => (server, uri),
            (Err(error), _) | (_, Err(error)) => return Err(error),
        };
        let result = self
            .mcp
            .request(server, REQUEST_TIMEOUT, |peer| async move {
                peer.read_resource(ReadResourceRequestParams::new(uri.to_string()))
                    .await
            })
            .await
            .map_err(|error| error.to_string())?;
        let mut text = Vec::new();
        let mut images = Vec::new();
        for contents in result.contents {
            match contents {
                ResourceContents::TextResourceContents { text: body, .. } => text.push(body),
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => {
                    let mime = mime_type.unwrap_or_else(|| "application/octet-stream".into());
                    text.push(push_image(&mut images, &mime, &blob).unwrap_or_else(|| {
                        format!("[binary {uri} {mime}, {} bytes]", blob.len() * 3 / 4)
                    }));
                }
                _ => text.push("[unsupported resource]".to_string()),
            }
        }
        Ok(Outcome {
            content: text.join("\n"),
            images,
        })
    }

    async fn prompts(&self, arguments: &serde_json::Value) -> Result<Outcome, String> {
        let filter = arg(arguments, "server");
        let query = terms(arg(arguments, "query").unwrap_or_default());
        let ids: Vec<String> = match filter {
            Some(id) => vec![id.to_string()],
            None => self.enabled_ids().await,
        };
        let mut lines = Vec::new();
        for id in ids {
            match self
                .mcp
                .request(&id, REQUEST_TIMEOUT, |peer| async move {
                    peer.list_all_prompts().await
                })
                .await
            {
                Ok(prompts) => {
                    for prompt in prompts {
                        let args = prompt
                            .arguments
                            .unwrap_or_default()
                            .iter()
                            .map(|a| {
                                format!(
                                    "{}{}{}",
                                    a.name,
                                    if a.required.unwrap_or(false) { "*" } else { "" },
                                    a.description
                                        .as_deref()
                                        .map(|d| format!(" — {d}"))
                                        .unwrap_or_default()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        let line = format!(
                            "{id}: {} — {}{}",
                            prompt.name,
                            prompt.description.as_deref().unwrap_or_default(),
                            if args.is_empty() {
                                String::new()
                            } else {
                                format!(" (args: {args})")
                            }
                        );
                        if query
                            .iter()
                            .all(|term| line.to_lowercase().contains(term.as_str()))
                        {
                            lines.push(line);
                        }
                    }
                }
                Err(error) => lines.push(format!("{id}: {error}")),
            }
        }
        if lines.is_empty() {
            lines.push("no prompts".to_string());
        }
        Ok(Outcome::text(lines.join("\n")))
    }

    async fn prompt(&self, arguments: &serde_json::Value) -> Result<Outcome, String> {
        let (server, name) = match (required(arguments, "server"), required(arguments, "name")) {
            (Ok(server), Ok(name)) => (server, name),
            (Err(error), _) | (_, Err(error)) => return Err(error),
        };
        let args = arguments
            .get("arguments")
            .and_then(|value| value.as_object().cloned());
        let mut params = GetPromptRequestParams::new(name.to_string());
        if let Some(args) = args {
            params = params.with_arguments(args);
        }
        let result = self
            .mcp
            .request(server, REQUEST_TIMEOUT, |peer| async move {
                peer.get_prompt(params).await
            })
            .await
            .map_err(|error| error.to_string())?;
        let mut text = Vec::new();
        let mut images = Vec::new();
        for message in result.messages {
            let role = match message.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let outcome = blocks_outcome(std::slice::from_ref(&message.content));
            images.extend(outcome.images);
            text.push(format!("{role}: {}", outcome.content));
        }
        Ok(Outcome {
            content: text.join("\n"),
            images,
        })
    }
}
