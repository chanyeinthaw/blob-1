use std::sync::Arc;

use crate::{CompletionRequest, Llm, LlmError};
use llm::{ChoiceAnswer, ChoiceQuestion, DecisionModel};
use storage::Message;

/// Utility backend supporting a fast dedicated decision model (preferred for
/// Auto-respond & inline-vs-thread gates, falling back to `llm` when absent or
/// on error) and an optional text LLM (used for thread titles and fallback).
#[derive(Clone)]
pub struct Utility {
    llm: Option<Arc<dyn Llm + Send + Sync>>,
    decision: Option<Arc<dyn DecisionModel>>,
}

impl Utility {
    pub fn new(llm: impl Llm + Send + Sync + 'static) -> Self {
        Self {
            llm: Some(Arc::new(llm)),
            decision: None,
        }
    }

    pub fn from_parts(
        llm: Option<Arc<dyn Llm + Send + Sync>>,
        decision: Option<Arc<dyn DecisionModel>>,
    ) -> Option<Self> {
        if llm.is_none() && decision.is_none() {
            return None;
        }
        Some(Self { llm, decision })
    }

    pub fn has_llm(&self) -> bool {
        self.llm.is_some()
    }

    pub async fn generate(&self, system: &str, prompt: &str) -> Result<String, LlmError> {
        let Some(llm) = &self.llm else {
            return Err(LlmError::Request(
                "no utility text model is configured".into(),
            ));
        };
        let messages = [Message::User(prompt.to_owned())];
        let response = llm
            .complete(CompletionRequest {
                system,
                messages: &messages,
                tools: &[],
                cache_key: None,
                kind: Some(storage::UsageKind::Utility),
            })
            .await?;
        Ok(response.text.trim().to_owned())
    }

    pub async fn choose(
        &self,
        state: &str,
        question: ChoiceQuestion<'_>,
    ) -> Result<Option<ChoiceAnswer>, LlmError> {
        let Some(decision) = &self.decision else {
            return Ok(None);
        };
        decision.choose(state, question).await.map(Some)
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
    instructions: Option<&str>,
    bot_id: Option<&str>,
    history: &[messenger::Fetched],
    burst: &[messenger::Inbound],
) -> bool {
    let Some(first) = burst.first() else {
        return false;
    };
    let mut sections = Vec::new();
    if let Some(instructions) = instructions.map(str::trim).filter(|s| !s.is_empty()) {
        let snippet: String = instructions.chars().take(300).collect();
        sections.push(format!("Assistant instructions:\n{snippet}"));
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
    let is_thread = first.place.thread.is_some();
    let system = if is_thread {
        THREAD_TRIAGE_PROMPT
    } else {
        CHANNEL_TRIAGE_PROMPT
    };
    let criteria: &[(&str, &str)] = if is_thread {
        &[
            (
                "RESPOND",
                "Any incoming message in the burst is a follow-up, question, feedback, or request plausibly meant for the assistant in this thread.",
            ),
            (
                "IGNORE",
                "All incoming messages in the burst are people clearly talking purely among themselves or no reply from the assistant is needed.",
            ),
        ]
    } else {
        &[
            (
                "RESPOND",
                "Any incoming message in the burst is clearly addressed to the assistant, asks it a direct question/request, or continues an active conversation with it.",
            ),
            (
                "IGNORE",
                "All incoming messages in the burst are people talking to each other, talking about the assistant in third person, or ambiguous chatter not directed at the assistant.",
            ),
        ]
    };
    // Prefer the fast decision model with calibrated probability thresholds;
    // fall back to the utility LLM if no decision model is configured or it errors.
    match utility
        .choose(
            &prompt,
            ChoiceQuestion {
                instructions: system,
                criteria,
            },
        )
        .await
    {
        Ok(Some(answer)) => {
            let threshold = if is_thread { 0.50 } else { 0.65 };
            return answer.choice.eq_ignore_ascii_case("RESPOND") && answer.probability >= threshold;
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("decision model respond check failed, falling back to utility LLM: {error}");
        }
    }
    utility
        .generate(system, &prompt)
        .await
        .is_ok_and(|answer| parse_respond(&answer))
}

const INLINE_DECISION_PROMPT: &str = "Decide how a team chat bot should answer this message. Reply 'inline' for a quick question, greeting or short answer; reply 'thread' if it will need work, several messages, files or a longer discussion. Reply with one word.";

/// Decides whether a plain-channel Discord message should be answered inline
/// (`true`) or in a new thread (`false`). Prefers the Decision model and falls
/// back to the Utility LLM.
pub async fn decide_inline(
    utility: &Utility,
    history: &[messenger::Fetched],
    inbound: &messenger::Inbound,
) -> bool {
    let history = history
        .iter()
        .map(|m| format!("{}: {}", m.author.name, m.text))
        .collect::<Vec<_>>()
        .join("\n");
    let quote = inbound.quote.as_ref().map_or("", |q| q.text.as_str());
    let attachments = inbound
        .attachments
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let prompt = format!(
        "Recent messages:\n{history}\nMessage: {}\nQuote: {quote}\nAttachments: {attachments}",
        inbound.text
    );
    match utility
        .choose(
            &prompt,
            ChoiceQuestion {
                instructions: INLINE_DECISION_PROMPT,
                criteria: &[
                    (
                        "INLINE",
                        "Quick question, greeting, casual acknowledgement, or short factual answer.",
                    ),
                    (
                        "THREAD",
                        "Multi-step work, debugging, files, code changes, or a longer explanation/discussion.",
                    ),
                ],
            },
        )
        .await
    {
        Ok(Some(answer)) => return answer.choice.eq_ignore_ascii_case("INLINE"),
        Ok(None) => {}
        Err(error) => {
            eprintln!("decision model inline check failed, falling back to utility LLM: {error}");
        }
    }
    utility
        .generate(INLINE_DECISION_PROMPT, &prompt)
        .await
        .is_ok_and(|answer| parse_inline(&answer))
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

    struct StubDecision(std::result::Result<ChoiceAnswer, String>);

    #[async_trait::async_trait]
    impl DecisionModel for StubDecision {
        async fn choose(
            &self,
            _state: &str,
            _question: ChoiceQuestion<'_>,
        ) -> Result<ChoiceAnswer, LlmError> {
            match &self.0 {
                Ok(ans) => Ok(ans.clone()),
                Err(msg) => Err(LlmError::Request(msg.clone())),
            }
        }
    }

    struct StubLlm(&'static str);

    #[async_trait::async_trait]
    impl Llm for StubLlm {
        async fn complete(
            &self,
            _request: CompletionRequest<'_>,
        ) -> Result<crate::CompletionResponse, LlmError> {
            Ok(crate::CompletionResponse {
                text: self.0.into(),
                tool_calls: Vec::new(),
                usage: None,
            })
        }
    }

    fn sample_inbound(thread: Option<&str>) -> messenger::Inbound {
        messenger::Inbound {
            place: storage::Place {
                channel: "c1".into(),
                thread: thread.map(Into::into),
                dm: false,
            },
            guild: Some("g1".into()),
            message: "m1".into(),
            author: messenger::Person {
                id: "u1".into(),
                name: "Chan".into(),
            },
            location: "#dev".into(),
            text: "hey".into(),
            quote: None,
            attachments: Vec::new(),
            earlier: None,
            addressed: false,
        }
    }

    #[tokio::test]
    async fn decision_thresholds_and_fallback_to_utility_llm() {
        // Channel requires P(RESPOND) >= 0.65; thread accepts >= 0.50.
        let borderline = Utility::from_parts(
            Some(Arc::new(StubLlm("RESPOND"))),
            Some(Arc::new(StubDecision(Ok(ChoiceAnswer {
                choice: "RESPOND".into(),
                probability: 0.58,
                usage: None,
            })))),
        )
        .unwrap();
        assert!(
            !decide_respond(&borderline, None, None, &[], &[sample_inbound(None)]).await
        );
        assert!(
            decide_respond(&borderline, None, None, &[], &[sample_inbound(Some("t1"))]).await
        );

        // When Decision model errors, falls back cleanly to Utility LLM.
        let failing_with_fallback = Utility::from_parts(
            Some(Arc::new(StubLlm("RESPOND"))),
            Some(Arc::new(StubDecision(Err("cf 500".into())))),
        )
        .unwrap();
        assert!(
            decide_respond(
                &failing_with_fallback,
                None,
                None,
                &[],
                &[sample_inbound(None)]
            )
            .await
        );
        let inline_fallback = Utility::from_parts(
            Some(Arc::new(StubLlm("inline"))),
            Some(Arc::new(StubDecision(Err("cf 500".into())))),
        )
        .unwrap();
        assert!(decide_inline(&inline_fallback, &[], &sample_inbound(None)).await);
    }
}
