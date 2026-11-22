use std::path::Path;

use environment::SkillEntry;

pub const BASE_PROMPT: &str = "";

pub const SUB_PROMPT: &str = r#"You run a background task for the main agent, an AI agent that works for one user in a
single chat that never ends. The main agent gave you a task. Do it yourself, with
your tools.

Your first message holds the view below, then your task. The view shows
you what the main agent knows: what the user wants, decided and taught. Use it as
context only, and do what your task says, not what the user's last
message says, since the main agent may have given you just part of the work. Your
final reply is your report to the main agent. The main agent may send you more messages,
even while you work: answer the last.

The view: the whole chat between the main agent and the user, oldest first, inside
<chat> tags, as one-line summaries. Each line is

  id+n|text   the n messages from id on, summarized (newlines shown as spaces)

A summary tags each item with its kind: user (the user's words),
assistant (the main agent's replies), tool (the main agent's tool calls), echo (their
results), or work (a report shown as `work (task <id>, <status>): ...`). A short
message is its own line, word for word. Recent lines cover one message
each; the older the messages, the more a line covers. A text too long
for one message is split over several in a row.

Navigating: zoom(id, n) opens line id+n into the two lines of n/2
messages it was made from; zoom(id, 1) gives message id in full. Zoom
whenever a summary only mentions something you need, such as a
decision, a past attempt or where a file is, before you act, guess or
ask. date(id) gives the date and time of message id. zoom("id") gives
a task's chat.

Sharing long outputs: when your task produces a long report, document,
or generated file meant for the user, write it to the workspace (prefer
Markdown unless another format was asked for), call share to get a
public link, and include that link in your report. If share is
unavailable (for example, no public URL is configured), include the key
findings directly in your report instead."#;

/// `mcp` holds `(id, one-line description)` for each enabled MCP server;
/// empty when no server is configured.
fn mcp_section(mcp: &[(String, String)]) -> Option<String> {
    if mcp.is_empty() {
        return None;
    }
    let servers = mcp
        .iter()
        .map(|(id, description)| format!("- {id}: {description}"))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("MCP servers provide extra tools, resources and prompts. Call mcp_search {{query}} to find a server's tools and load them into this session, then call them by name. mcp_resources {{server?, query?}} and mcp_read {{server, uri}} cover resources; mcp_prompts {{server?, query?}} and mcp_prompt {{server, name, arguments?}} cover prompts.\n\n{servers}"))
}

pub fn build(workspace: &Path, skills: &[SkillEntry], mcp: &[(String, String)]) -> String {
    let mut sections = Vec::new();
    if !BASE_PROMPT.is_empty() {
        sections.push(BASE_PROMPT.to_owned());
    }
    sections.push(format!("Working directory: {}", workspace.display()));
    if !skills.is_empty() {
        let entries = skills
            .iter()
            .map(|skill| {
                format!(
                    "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}</location>\n  </skill>",
                    escape(&skill.name),
                    escape(&skill.description),
                    escape(&skill.location.display().to_string()),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "The following skills provide specialized instructions for specific tasks.\nUse the read tool to load a skill's file when the task matches its description.\n\n<available_skills>\n{entries}\n</available_skills>"
        ));
    }
    if let Some(section) = mcp_section(mcp) {
        sections.push(section);
    }
    sections.join("\n\n")
}

pub fn build_chat(workspace: &Path, skills: &[SkillEntry], mcp: &[(String, String)]) -> String {
    let mut sections = vec![format!("Working directory: {}", workspace.display())];
    if !skills.is_empty() {
        let entries = skills
            .iter()
            .map(|skill| {
                format!(
                    "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n  </skill>",
                    escape(&skill.name),
                    escape(&skill.description)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Tasks can load these skills; name the skill in the task when it fits.\n\n<available_skills>\n{entries}\n</available_skills>"));
    }
    if let Some(section) = mcp_section(mcp) {
        sections.push(section);
    }
    sections.join("\n\n")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
