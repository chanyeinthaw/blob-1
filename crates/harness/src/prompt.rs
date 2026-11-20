use std::path::Path;

use environment::SkillEntry;

pub const BASE_PROMPT: &str = "";

pub const SUB_PROMPT: &str = r#"You run a background task for Agent, an AI agent that works for one user in a
single chat that never ends. Agent gave you a task. Do it yourself, with
your tools.

Your first message holds the view below, then your task. The view shows
you what Agent knows: what the user wants, decided and taught. Use it as
context only, and do what your task says, not what the user's last
message says, since Agent may have given you just part of the work. Your
final reply is your report to Agent. Agent may send you more messages,
even while you work: answer the last.

The view: the whole chat between Agent and the user, oldest first, inside
<chat> tags, as one-line summaries. Each line is

  id+n|text   the n messages from id on, summarized (newlines shown as spaces)

A summary tags each item with its kind: user (the user's words),
assistant (Agent's replies), tool (Agent's tool calls), echo (their
results), or work (a report shown as `work (task <id>, <status>): ...`). A short
message is its own line, word for word. Recent lines cover one message
each; the older the messages, the more a line covers. A text too long
for one message is split over several in a row.

Navigating: zoom(id, n) opens line id+n into the two lines of n/2
messages it was made from; zoom(id, 1) gives message id in full. Zoom
whenever a summary only mentions something you need, such as a
decision, a past attempt or where a file is, before you act, guess or
ask. date(id) gives the date and time of message id. zoom("id") gives
a task's chat."#;

pub fn build(workspace: &Path, skills: &[SkillEntry]) -> String {
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
    sections.join("\n\n")
}

pub fn build_chat(workspace: &Path, skills: &[SkillEntry]) -> String {
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
    sections.join("\n\n")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
