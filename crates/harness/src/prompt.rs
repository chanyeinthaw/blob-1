use std::path::Path;

use crate::skills::Skill;

pub const BASE_PROMPT: &str = "";

pub fn build(workspace: &Path, skills: &[Skill]) -> String {
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

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
