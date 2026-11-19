use std::collections::BTreeSet;

use environment::ToolInput;
use storage::Message;

use crate::{CompletionRequest, HarnessError, Llm, Usage};

const SYSTEM: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";
const INITIAL: &str = r#"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;
const UPDATE: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;
const TURN_PREFIX: &str = "The messages above are earlier context from an ongoing conversation. Later messages are stored separately and do not need to be reconstructed.\n\nCreate a concise checkpoint of the user's request and the progress shown above. This checkpoint will be placed before the later messages so the conversation can continue with the necessary context.\n\n## Original Request\n[What did the user ask for?]\n\n## Progress So Far\n- [Key decisions and work completed in these messages]\n\n## Context Needed to Continue\n- [Information from these messages needed to understand the later work]\n\nOnly summarize information explicitly present above. Do not infer or recreate later messages.";

fn estimate_text(text: &str) -> u64 {
    (text.encode_utf16().count() as u64).div_ceil(4)
}

fn estimate(message: &Message) -> u64 {
    match message {
        Message::User(text) | Message::Assistant(text) | Message::Summary { text, .. } => {
            estimate_text(text)
        }
        Message::ToolCall {
            name, arguments, ..
        } => ((name.encode_utf16().count() + arguments.encode_utf16().count()) as u64).div_ceil(4),
        Message::ToolResult { content, .. } => estimate_text(content),
    }
}

pub(crate) fn context_tokens(
    system: &str,
    messages: &[Message],
    usage: Option<(Usage, usize)>,
) -> u64 {
    match usage {
        Some((usage, index)) => {
            let total = if usage.total > 0 {
                usage.total
            } else {
                usage.input + usage.output + usage.cache_read + usage.cache_write
            };
            total + messages[index..].iter().map(estimate).sum::<u64>()
        }
        None => estimate_text(system) + messages.iter().map(estimate).sum::<u64>(),
    }
}

fn render(messages: &[Message]) -> String {
    let mut parts = Vec::new();
    let mut calls = Vec::new();
    for message in messages {
        if let Message::ToolCall {
            name, arguments, ..
        } = message
        {
            calls.push(format!("{name}({arguments})"));
            continue;
        }
        if !calls.is_empty() {
            parts.push(format!("[Assistant tool calls]: {}", calls.join("; ")));
            calls.clear();
        }
        match message {
            Message::User(text) if text.is_empty() => {}
            Message::User(text) => parts.push(format!("[User]: {text}")),
            Message::Assistant(text) => parts.push(format!("[Assistant]: {text}")),
            Message::Summary { text, .. } => parts.push(format!("[Summary]: {text}")),
            Message::ToolResult { content, .. } => {
                let count = content.chars().count();
                let prefix: String = content.chars().take(2000).collect();
                let suffix = if count > 2000 {
                    format!("\n\n[... {} more characters truncated]", count - 2000)
                } else {
                    String::new()
                };
                parts.push(format!("[Tool result]: {prefix}{suffix}"));
            }
            Message::ToolCall { .. } => unreachable!(),
        }
    }
    if !calls.is_empty() {
        parts.push(format!("[Assistant tool calls]: {}", calls.join("; ")));
    }
    parts.join("\n\n")
}

async fn summarize<L: Llm>(llm: &L, prompt: String) -> Result<String, HarnessError> {
    let response = llm
        .complete(CompletionRequest {
            system: SYSTEM,
            messages: &[Message::User(prompt)],
            tools: &[],
        })
        .await?;
    if !response.tool_calls.is_empty() {
        return Err(HarnessError::Compaction(
            "Summarization attempted to call a tool".into(),
        ));
    }
    if response.text.trim().is_empty() {
        return Err(HarnessError::Compaction(
            "Summarization returned empty text".into(),
        ));
    }
    Ok(response.text)
}

pub(crate) async fn compact<L: Llm>(
    llm: &L,
    messages: &[Message],
    keep: u32,
) -> Result<Option<Vec<Message>>, HarnessError> {
    let previous = match messages.first() {
        Some(Message::Summary {
            text,
            read_files,
            modified_files,
        }) => Some((text, read_files, modified_files)),
        _ => None,
    };
    let boundary = usize::from(previous.is_some());
    let valid: Vec<usize> = (boundary..messages.len())
        .filter(|&i| match &messages[i] {
            Message::User(_) | Message::Assistant(_) => true,
            Message::ToolCall { .. } => {
                i == 0 || !matches!(messages[i - 1], Message::ToolCall { .. })
            }
            _ => false,
        })
        .collect();
    let Some(&first) = valid.first() else {
        return Ok(None);
    };
    let mut accumulated = 0;
    let mut cut = first;
    for i in (boundary..messages.len()).rev() {
        let tokens = estimate(&messages[i]);
        if tokens == 0 {
            continue;
        }
        accumulated += tokens;
        if accumulated >= u64::from(keep) {
            cut = valid
                .iter()
                .copied()
                .find(|&v| v >= i)
                .unwrap_or(*valid.last().unwrap());
            break;
        }
    }
    let turn_start = if matches!(messages[cut], Message::User(_)) {
        None
    } else {
        (boundary..cut)
            .rev()
            .find(|&i| matches!(messages[i], Message::User(_)))
    };
    let history_end = turn_start.unwrap_or(cut);
    let history = &messages[boundary..history_end];
    let prefix = &messages[history_end..cut];
    if history.is_empty() && prefix.is_empty() {
        return Ok(None);
    }
    let mut read = BTreeSet::new();
    let mut modified = BTreeSet::new();
    if let Some((_, old_read, old_modified)) = previous {
        read.extend(old_read.iter().cloned());
        modified.extend(old_modified.iter().cloned());
    }
    for message in &messages[boundary..cut] {
        if let Message::ToolCall {
            name, arguments, ..
        } = message
        {
            match ToolInput::parse(name, arguments) {
                Ok(ToolInput::Read { path, .. }) => {
                    read.insert(path.display().to_string());
                }
                Ok(ToolInput::Write { path, .. } | ToolInput::Edit { path, .. }) => {
                    modified.insert(path.display().to_string());
                }
                _ => {}
            }
        }
    }
    read.retain(|path| !modified.contains(path));
    let mut text = if history.is_empty() {
        previous
            .map_or("No prior history.", |(text, _, _)| text)
            .to_owned()
    } else {
        let mut prompt = format!("<conversation>\n{}\n</conversation>\n\n", render(history));
        if let Some((text, _, _)) = previous {
            prompt.push_str(&format!(
                "<previous-summary>\n{text}\n</previous-summary>\n\n"
            ));
        }
        prompt.push_str(if previous.is_some() { UPDATE } else { INITIAL });
        summarize(llm, prompt).await?
    };
    if !prefix.is_empty() {
        let prompt = format!(
            "# Conversation\n{}\n\n# Instructions\n{TURN_PREFIX}",
            render(prefix)
        );
        let prefix_summary = summarize(llm, prompt).await?;
        text.push_str(&format!(
            "\n\n---\n\n**Turn Context (split turn):**\n\n{prefix_summary}"
        ));
    }
    let mut result = Vec::with_capacity(1 + messages.len() - cut);
    result.push(Message::Summary {
        text,
        read_files: read.into_iter().collect(),
        modified_files: modified.into_iter().collect(),
    });
    result.extend_from_slice(&messages[cut..]);
    Ok(Some(result))
}
