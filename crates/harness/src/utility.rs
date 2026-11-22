use crate::{CompletionRequest, Llm, LlmError};
use storage::Message;

#[derive(Clone)]
pub struct Utility {
    llm: std::sync::Arc<dyn Llm + Send + Sync>,
}

impl Utility {
    pub fn new(llm: impl Llm + Send + Sync + 'static) -> Self {
        Self {
            llm: std::sync::Arc::new(llm),
        }
    }

    pub async fn generate(&self, system: &str, prompt: &str) -> Result<String, LlmError> {
        let messages = [Message::User(prompt.to_owned())];
        let response = self
            .llm
            .complete(CompletionRequest {
                system,
                messages: &messages,
                tools: &[],
            })
            .await?;
        Ok(response.text.trim().to_owned())
    }
}

pub fn parse_inline(answer: &str) -> bool {
    let answer = answer.trim();
    answer
        .strip_suffix('.')
        .unwrap_or(answer)
        .eq_ignore_ascii_case("inline")
}

pub fn parse_respond(answer: &str) -> bool {
    let answer = answer.trim().trim_end_matches(['.', '!']);
    answer.eq_ignore_ascii_case("respond")
}

const CHANNEL_TRIAGE_PROMPT: &str = "You are a fast response gate deciding whether an AI assistant in a group chat channel should respond to the latest incoming message(s). Reply with one word: RESPOND or IGNORE. Reply RESPOND only when any incoming message is clearly talking to the assistant (addressing it by name, asking it a direct question/request, or continuing an active conversation with it). Do not be proactive or chime in uninvited. If people are talking to each other or it is ambiguous, reply IGNORE.";

const THREAD_TRIAGE_PROMPT: &str = "You are a fast response gate deciding whether an AI assistant in a chat thread should respond to the latest incoming message(s). Reply with one word: RESPOND or IGNORE. Because this is a thread, lean toward RESPOND whenever any incoming message is a follow-up, question, feedback, or request that could plausibly be meant for the assistant. Reply IGNORE only when people are clearly talking purely among themselves or no reply is needed.";

/// Shared pre-messenger response gate powered by the utility model.
/// Decides whether an unaddressed group/thread message (or burst of messages)
/// is meant for the assistant before typing indicators, reactions, or waking
/// the main model.
pub async fn decide_respond(
    utility: &Utility,
    identity: Option<&str>,
    bot_id: Option<&str>,
    history: &[messenger::Fetched],
    burst: &[messenger::Inbound],
) -> bool {
    let Some(first) = burst.first() else {
        return false;
    };
    let mut sections = Vec::new();
    if let Some(identity) = identity.map(str::trim).filter(|s| !s.is_empty()) {
        let snippet: String = identity.chars().take(300).collect();
        sections.push(format!("Assistant identity:\n{snippet}"));
    }
    sections.push(format!("Where: {}", first.location));
    if !history.is_empty() {
        let recent = history
            .iter()
            .skip(history.len().saturating_sub(10))
            .map(|m| {
                if bot_id.is_some_and(|id| id == m.author.id) {
                    format!("[assistant] {}: {}", m.author.name, m.text)
                } else {
                    format!("{}: {}", m.author.name, m.text)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Recent messages:\n{recent}"));
    }
    if burst.len() == 1 {
        let inbound = first;
        if let Some(quote) = &inbound.quote {
            sections.push(format!("Quoted ({}): {}", quote.author.name, quote.text));
        }
        if !inbound.attachments.is_empty() {
            let names = inbound
                .attachments
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            sections.push(format!("Attachments: {names}"));
        }
        sections.push(format!(
            "Latest message ({}): {}",
            inbound.author.name, inbound.text
        ));
    } else {
        let lines = burst
            .iter()
            .map(|inbound| {
                let mut line = format!("{}: {}", inbound.author.name, inbound.text);
                if let Some(quote) = &inbound.quote {
                    line.push_str(&format!(" [quoting {}: {}]", quote.author.name, quote.text));
                }
                if !inbound.attachments.is_empty() {
                    let names = inbound
                        .attachments
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    line.push_str(&format!(" [attachments: {names}]"));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Incoming burst (oldest to newest):\n{lines}"));
    }
    let prompt = sections.join("\n\n");
    let system = if first.place.thread.is_some() {
        THREAD_TRIAGE_PROMPT
    } else {
        CHANNEL_TRIAGE_PROMPT
    };
    utility
        .generate(system, &prompt)
        .await
        .is_ok_and(|answer| parse_respond(&answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_respond_accepts_only_respond() {
        assert!(parse_respond("RESPOND"));
        assert!(parse_respond("respond."));
        assert!(parse_respond("  Respond! \n"));
        assert!(!parse_respond("IGNORE"));
        assert!(!parse_respond("ignore."));
        assert!(!parse_respond("maybe respond"));
        assert!(!parse_respond(""));
    }
}
