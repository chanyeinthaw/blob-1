use std::collections::BTreeSet;

use environment::ToolInput;
use storage::{Message, SessionUsage};

use crate::{CompletionRequest, HarnessError, Llm, LlmError};

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
    usage: Option<SessionUsage>,
) -> u64 {
    match usage {
        Some(usage) => usage.tokens + messages[usage.boundary..].iter().map(estimate).sum::<u64>(),
        None => estimate_text(system) + messages.iter().map(estimate).sum::<u64>(),
    }
}

fn fit_prompt(
    messages: &[Message],
    parts: &[Option<String>],
    prefix: &str,
    suffix: &str,
    window: Option<u32>,
    reserve: u32,
    start: &mut usize,
) -> Result<String, HarnessError> {
    let budget = window.map(|window| u64::from(window.saturating_sub(reserve)));
    let fixed = format!("{prefix}{suffix}");
    let fits = |length: u64| {
        budget.is_none_or(|budget| estimate_text(SYSTEM) + length.div_ceil(4) <= budget)
    };
    if !fits(fixed.encode_utf16().count() as u64) {
        return Err(HarnessError::Compaction(
            "Summarization instructions and previous summary exceed the summarizer context window"
                .into(),
        ));
    }
    let marker = "[Earlier messages omitted]\n\n";
    let fixed_len = fixed.encode_utf16().count() as u64;
    let mut length = 0;
    let mut next_call = false;
    let mut selected = messages.len();
    for i in (*start..messages.len()).rev() {
        let is_call = matches!(messages[i], Message::ToolCall { .. });
        if let Some(part) = &parts[i] {
            length += part.encode_utf16().count() as u64;
            if is_call && next_call {
                length += 2;
            } else {
                if length > part.encode_utf16().count() as u64 {
                    length += 2;
                }
                if is_call {
                    length += "[Assistant tool calls]: ".len() as u64;
                }
            }
        }
        next_call = is_call;
        let marker_len = if i > 0 { marker.len() as u64 } else { 0 };
        if fits(fixed_len + marker_len + length) {
            selected = i;
        }
    }
    *start = selected;
    if selected == messages.len() {
        return Ok(if selected > 0 && fits(fixed_len + marker.len() as u64) {
            format!("{prefix}{marker}{suffix}")
        } else {
            fixed
        });
    }
    let mut rendered = String::new();
    let mut in_calls = false;
    for i in selected..messages.len() {
        let is_call = matches!(messages[i], Message::ToolCall { .. });
        if let Some(part) = &parts[i] {
            if is_call && in_calls {
                rendered.push_str("; ");
            } else {
                if !rendered.is_empty() {
                    rendered.push_str("\n\n");
                }
                if is_call {
                    rendered.push_str("[Assistant tool calls]: ");
                }
            }
            rendered.push_str(part);
        }
        in_calls = is_call;
    }
    let marker = if selected > 0 { marker } else { "" };
    Ok(format!("{prefix}{marker}{rendered}{suffix}"))
}

async fn summarize<L: Llm>(
    llm: &L,
    messages: &[Message],
    prefix: &str,
    suffix: &str,
    window: Option<u32>,
    reserve: u32,
    on_model: &(dyn Fn() + Sync),
) -> Result<String, HarnessError> {
    let parts: Vec<_> = messages
        .iter()
        .map(|message| match message {
            Message::User(text) if text.is_empty() => None,
            Message::User(text) => Some(format!("[User]: {text}")),
            Message::Assistant(text) => Some(format!("[Assistant]: {text}")),
            Message::Summary { text, .. } => Some(format!("[Summary]: {text}")),
            Message::ToolCall {
                name, arguments, ..
            } => Some(format!("{name}({arguments})")),
            Message::ToolResult { content, .. } => {
                let count = content.chars().count();
                let prefix: String = content.chars().take(2000).collect();
                let suffix = if count > 2000 {
                    format!("\n\n[... {} more characters truncated]", count - 2000)
                } else {
                    String::new()
                };
                Some(format!("[Tool result]: {prefix}{suffix}"))
            }
        })
        .collect();
    let mut start = 0;
    let response = loop {
        let prompt = fit_prompt(
            messages, &parts, prefix, suffix, window, reserve, &mut start,
        )?;
        on_model();
        match llm
            .complete(CompletionRequest {
                system: SYSTEM,
                messages: &[Message::User(prompt)],
                tools: &[],
            })
            .await
        {
            Err(error @ LlmError::ContextOverflow(_)) => {
                let remaining = messages.len() - start;
                if remaining <= 1 {
                    return Err(error.into());
                }
                start += remaining.div_ceil(2);
            }
            result => break result?,
        }
    };
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
    window: Option<u32>,
    reserve: u32,
    on_model: &(dyn Fn() + Sync),
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
        let suffix = format!(
            "\n</conversation>\n\n{}{}",
            previous.map_or_else(String::new, |(text, _, _)| format!(
                "<previous-summary>\n{text}\n</previous-summary>\n\n"
            )),
            if previous.is_some() { UPDATE } else { INITIAL }
        );
        summarize(
            llm,
            history,
            "<conversation>\n",
            &suffix,
            window,
            reserve,
            on_model,
        )
        .await?
    };
    if !prefix.is_empty() {
        let prefix_summary = summarize(
            llm,
            prefix,
            "# Conversation\n",
            &format!("\n\n# Instructions\n{TURN_PREFIX}"),
            window,
            reserve,
            on_model,
        )
        .await?;
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
