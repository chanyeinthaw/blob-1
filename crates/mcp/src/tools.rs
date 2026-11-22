use base64::Engine;
use environment::{Image, ToolSpec};
use rmcp::model::*;
use serde::Deserialize;

use crate::{Mcp, REQUEST_TIMEOUT};

const SEARCH_PAGE_SIZE: usize = 10;
const SEARCH_PAGE_MAX_BYTES: usize = 32 * 1024;

#[derive(Deserialize)]
struct SearchArgs {
    server: Option<String>,
    query: Option<String>,
    #[serde(default)]
    offset: usize,
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

/// Static MCP tool interface (`mcp_search`, `mcp_execute`, resources, and
/// prompts). Keeping the tool catalog static preserves prompt-cache prefixes
/// across turns and tool calls instead of mutating the `tools` array mid-turn.
pub struct SessionTools {
    mcp: Mcp,
}

const BUILTINS: &[ToolSpec] = &[
    ToolSpec::new(
        "mcp_search",
        "Search tools provided by enabled MCP servers. Returns up to 10 matching tools per page with server, tool name, description, and JSON input schema (use offset to view additional pages); call tools with mcp_execute.",
        r#"{"type":"object","properties":{"server":{"type":"string","description":"MCP server id; omit to search all enabled servers"},"query":{"type":"string","description":"Keywords matched against tool names and descriptions; omit to list tools"},"offset":{"type":"integer","minimum":0,"description":"Zero-based match offset for pagination (default 0)"}}}"#,
    ),
    ToolSpec::new(
        "mcp_execute",
        "Execute a tool exposed by an enabled MCP server. Use mcp_search first to discover server id, tool name, and required arguments.",
        r#"{"type":"object","properties":{"server":{"type":"string","description":"MCP server id"},"tool":{"type":"string","description":"Exact MCP tool name from mcp_search"},"arguments":{"type":"object","description":"Tool input arguments as a JSON object"}},"required":["server","tool"]}"#,
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
        Self { mcp }
    }

    pub fn mcp(&self) -> &Mcp {
        &self.mcp
    }

    /// Static `mcp_*` built-ins when at least one MCP server is enabled.
    pub fn specs(&self) -> Vec<ToolSpec> {
        if self.mcp.any_enabled() {
            BUILTINS.to_vec()
        } else {
            Vec::new()
        }
    }

    /// Dispatch an `mcp_*` tool call; `None` means the name isn't ours and the
    /// caller should try the other tools.
    pub async fn execute(&self, name: &str, arguments: &str) -> Option<Result<Outcome, String>> {
        if !matches!(
            name,
            "mcp_search"
                | "mcp_execute"
                | "mcp_resources"
                | "mcp_read"
                | "mcp_prompts"
                | "mcp_prompt"
        ) {
            return None;
        }
        let parsed: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(value) => value,
            Err(error) => return Some(Err(format!("arguments must be JSON: {error}"))),
        };
        Some(match name {
            "mcp_search" => self.search(&parsed).await,
            "mcp_execute" => self.call(&parsed).await,
            "mcp_resources" => self.resources(&parsed).await,
            "mcp_read" => self.read(&parsed).await,
            "mcp_prompts" => self.prompts(&parsed).await,
            "mcp_prompt" => self.prompt(&parsed).await,
            _ => unreachable!(),
        })
    }

    async fn enabled_ids(&self) -> Vec<String> {
        self.mcp.enabled_ids().await
    }

    async fn search(&self, arguments: &serde_json::Value) -> Result<Outcome, String> {
        let args = parse_search_args(arguments)?;
        let terms = terms(args.query.as_deref().unwrap_or_default());
        let ids: Vec<String> = match args.server {
            Some(id) => vec![id],
            None => self.enabled_ids().await,
        };
        let mut scored: Vec<(usize, String, Tool)> = Vec::new();
        let mut notes = Vec::new();
        for id in ids {
            match self
                .mcp
                .request(&id, REQUEST_TIMEOUT, |peer| async move {
                    peer.list_all_tools().await
                })
                .await
            {
                Ok(tools) => {
                    for tool in tools {
                        let text = format!(
                            "{} {} {}",
                            id,
                            tool.name,
                            tool.description.as_deref().unwrap_or_default()
                        )
                        .to_lowercase();
                        let score = terms
                            .iter()
                            .filter(|term| text.contains(term.as_str()))
                            .count();
                        if score > 0 || terms.is_empty() {
                            scored.push((score, id.clone(), tool));
                        }
                    }
                }
                Err(error) => notes.push(format!("{id}: {error}")),
            }
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.name.cmp(&b.2.name))
        });
        let entries: Vec<(String, String, String, String)> = scored
            .into_iter()
            .map(|(_, server, tool)| {
                let desc = tool
                    .description
                    .as_deref()
                    .or(tool.title.as_deref())
                    .unwrap_or("(no description)")
                    .to_string();
                let schema =
                    serde_json::to_string(&tool.input_schema).unwrap_or_else(|_| "{}".to_string());
                (server, tool.name.to_string(), desc, schema)
            })
            .collect();
        let output = format_search_page(
            &entries,
            &notes,
            args.offset,
            SEARCH_PAGE_SIZE,
            SEARCH_PAGE_MAX_BYTES,
        );
        Ok(Outcome::text(output))
    }

    async fn call(&self, arguments: &serde_json::Value) -> Result<Outcome, String> {
        let server = required(arguments, "server")?;
        let tool = required(arguments, "tool")?;
        let args_json = match arguments.get("arguments") {
            None | Some(serde_json::Value::Null) => "{}".to_string(),
            Some(value @ serde_json::Value::Object(_)) => {
                serde_json::to_string(value).map_err(|error| error.to_string())?
            }
            Some(_) => return Err("'arguments' must be a JSON object".to_string()),
        };
        match self.mcp.call_tool(server, tool, &args_json).await {
            Ok(result) if result.is_error == Some(true) => Err(outcome_of(&result).content),
            Ok(result) => Ok(outcome_of(&result)),
            Err(error) => Err(error),
        }
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

fn parse_search_args(arguments: &serde_json::Value) -> Result<SearchArgs, String> {
    let Some(obj) = arguments.as_object() else {
        return Err("invalid mcp_search arguments: expected a JSON object".to_string());
    };
    if matches!(obj.get("server"), Some(serde_json::Value::Null)) {
        return Err("'server' must be a non-empty string".to_string());
    }
    let args: SearchArgs = serde_json::from_value(arguments.clone())
        .map_err(|error| format!("invalid mcp_search arguments: {error}"))?;
    if let Some(server) = args.server.as_deref() {
        if server.is_empty() {
            return Err("'server' must not be empty".to_string());
        }
    }
    Ok(args)
}

fn clip_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Formats one page of `mcp_search` results (entries, pagination footer, and
/// server error notes) strictly within `max_bytes`, keeping each returned tool
/// schema intact and omitting any single schema that exceeds the remaining
/// entry budget.
fn format_search_page(
    entries: &[(String, String, String, String)],
    notes: &[String],
    offset: usize,
    page_size: usize,
    max_bytes: usize,
) -> String {
    let total = entries.len();
    let max_footer_len = format!(
        "showing {total}..={total} of {total} matching tools ({total} more; call mcp_search with offset={total} for the next page)"
    )
    .len()
    .max(format!("no more matching tools (offset={offset}, total={total})").len());

    // Cap server error notes to at most a quarter of the page budget (and <= 2 KiB).
    let notes_cap = (max_bytes / 4).min(2048);
    let mut note_lines = Vec::new();
    let mut notes_bytes = 0usize;
    for note in notes {
        let clipped = clip_utf8(note, 256);
        let sep = usize::from(!note_lines.is_empty());
        if notes_bytes + sep + clipped.len() > notes_cap {
            let omitted = notes.len() - note_lines.len();
            let suffix = format!("({omitted} more server error(s) omitted)");
            let sep = usize::from(!note_lines.is_empty());
            if notes_bytes + sep + suffix.len() <= notes_cap {
                notes_bytes += sep + suffix.len();
                note_lines.push(suffix);
            }
            break;
        }
        notes_bytes += sep + clipped.len();
        note_lines.push(clipped.to_string());
    }

    let footer_reserve = if total > 0 { max_footer_len + 1 } else { 0 };
    let notes_reserve = if note_lines.is_empty() {
        0
    } else {
        notes_bytes + 1
    };
    let entry_budget = max_bytes.saturating_sub(footer_reserve + notes_reserve);

    let mut lines = Vec::new();
    let mut used_bytes = 0usize;
    let mut shown = 0usize;
    for (server, tool, desc, schema) in entries.iter().skip(offset).take(page_size) {
        // Shorten the description first so a verbose description never causes a
        // small schema to be omitted.
        let short_desc: String = desc.chars().take(200).collect();
        let full = format!("server={server} tool={tool} — {short_desc}\n  schema: {schema}");
        let line = if full.len() > entry_budget {
            let fallback = format!(
                "server={server} tool={tool} — {short_desc}\n  schema: (omitted: {} bytes exceeds {entry_budget}-byte search budget)",
                schema.len()
            );
            if fallback.len() > entry_budget {
                clip_utf8(&fallback, entry_budget).to_string()
            } else {
                fallback
            }
        } else {
            full
        };
        let sep = usize::from(!lines.is_empty());
        if !lines.is_empty() && used_bytes + sep + line.len() > entry_budget {
            break;
        }
        if lines.is_empty() && line.len() > entry_budget {
            break;
        }
        used_bytes += sep + line.len();
        lines.push(line);
        shown += 1;
    }

    if total > 0 && shown == 0 {
        lines.push(format!(
            "no more matching tools (offset={offset}, total={total})"
        ));
    } else if offset + shown < total {
        let next_offset = offset + shown;
        let remaining = total - next_offset;
        lines.push(format!(
            "showing {}..={} of {total} matching tools ({remaining} more; call mcp_search with offset={next_offset} for the next page)",
            offset + 1,
            next_offset
        ));
    }
    lines.extend(note_lines);
    if lines.is_empty() {
        lines.push("no matching tools".to_string());
    }
    let output = lines.join("\n");
    if output.len() > max_bytes {
        clip_utf8(&output, max_bytes).to_string()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_args_reject_malformed_and_null_server_filters() {
        assert!(parse_search_args(&serde_json::json!(42)).is_err());
        assert!(parse_search_args(&serde_json::json!({"server": 42})).is_err());
        assert!(parse_search_args(&serde_json::json!({"server": null})).is_err());
        assert!(parse_search_args(&serde_json::json!({"server": ""})).is_err());
        assert!(parse_search_args(&serde_json::json!({"offset": -1})).is_err());
        let ok = parse_search_args(&serde_json::json!({"server": "linear", "offset": 10})).unwrap();
        assert_eq!(ok.server.as_deref(), Some("linear"));
        assert_eq!(ok.offset, 10);
    }

    #[test]
    fn format_search_page_enforces_total_budget_and_preserves_small_schemas() {
        let long_desc = "d".repeat(1000);
        let big_schema = "x".repeat(800);
        let entries = vec![
            ("s".into(), "t1".into(), long_desc, r#"{"a":1}"#.into()),
            ("s".into(), "t2".into(), "d2".into(), r#"{"b":2}"#.into()),
            ("s".into(), "t3".into(), "d3".into(), big_schema),
        ];
        let notes = vec!["broken: ".to_string() + &"e".repeat(500)];

        // Long description is trimmed before checking schema fit, so t1's small schema is kept,
        // and the full response including footer + server error stays strictly within max_bytes.
        let budget = 400;
        let p1 = format_search_page(&entries, &notes, 0, 10, budget);
        assert!(p1.len() <= budget, "len {} > {budget}", p1.len());
        assert!(p1.contains("tool=t1"));
        assert!(p1.contains(r#"schema: {"a":1}"#));
        assert!(p1.contains("offset=1"));

        // Oversized single schema at offset 2 is explicitly omitted within budget.
        let p3 = format_search_page(&entries, &notes, 2, 10, budget);
        assert!(p3.len() <= budget, "len {} > {budget}", p3.len());
        assert!(p3.contains("tool=t3"));
        assert!(p3.contains("omitted: 800 bytes exceeds"));
    }
}
