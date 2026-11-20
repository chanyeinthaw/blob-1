use base64::Engine as _;
use environment::ToolSpec;
use llm::{CompletionRequest, CompletionResponse, Llm, LlmError, ToolCallRequest, Usage};
use regex::Regex;
use rig_core::{
    catalog::{Catalog, ModelFacts, ModelSpec},
    completion::{self, AssistantContent, ToolDefinition},
    message::{CallId, ToolCall, ToolFunction, ToolName, ToolResultContent, UserContent},
    operation::Completion,
    providers::{
        openai::{OpenAI, OpenAIConfig},
        registry::ProviderId,
    },
    DynModel,
};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime};
use storage::{DiscoveredModel, Message, Model, Provider, ProviderApi, ReasoningLevel};
use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Clone)]
pub struct RigLlm {
    model: DynModel<Completion>,
    reasoning: Option<ReasoningSetting>,
    api: ProviderApi,
    hold: Arc<Mutex<Option<Instant>>>,
}

#[derive(Clone)]
enum ReasoningSetting {
    Typed(completion::Reasoning),
    Raw(String),
}

impl RigLlm {
    pub fn new(model: impl Into<DynModel<Completion>>) -> Self {
        Self {
            model: model.into(),
            reasoning: None,
            api: ProviderApi::OpenAiResponses,
            hold: Arc::new(Mutex::new(None)),
        }
    }

    async fn send(
        &self,
        request: completion::CompletionRequest,
    ) -> Result<completion::CompletionResponse, LlmError> {
        let response = self.model.call(request).await.map_err(|error| {
            let status = error
                .provider_response_status()
                .map(|status| status.as_u16());
            let retry_after = error.report().retry_after(SystemTime::now());
            classify_error(error.to_string(), status, retry_after)
        })?;
        match response.error {
            Some(error) => Err(classify_error(error, None, None)),
            None => Ok(response),
        }
    }
}

pub fn connect(provider: &Provider, api_key: &str, model: &Model, level: ReasoningLevel) -> RigLlm {
    let client = client(provider, api_key);
    let id = match provider.api {
        ProviderApi::OpenAiResponses => model.id.rsplit('/').next().unwrap_or(&model.id),
        ProviderApi::OpenAiCompletions => &model.id,
    };
    let catalog = Catalog::builtin();
    let provider_id = ProviderId::catalog("openai").expect("OpenAI is in rig's catalog");
    let mut spec = catalog
        .get(provider_id, id)
        .map(|entry| entry.spec.clone())
        .unwrap_or_else(|| ModelSpec::new(provider_id, id));
    spec.id = id.to_owned();
    spec.input.image = model.images;
    let facts = ModelFacts::new(spec);
    let mut llm = match provider.api {
        ProviderApi::OpenAiResponses => {
            let mut rig = client.responses(&model.id);
            rig.wire = rig.wire.with_facts(facts);
            RigLlm::new(rig)
        }
        ProviderApi::OpenAiCompletions => {
            let mut rig = client.chat(&model.id);
            rig.wire = rig.wire.with_facts(facts);
            RigLlm::new(rig)
        }
    };
    llm.reasoning = resolve_reasoning(&llm.model, model, level);
    llm.api = provider.api;
    llm
}

pub fn reasoning_levels(provider: &Provider, model: &Model) -> Option<Vec<ReasoningLevel>> {
    let client = client(provider, "unused");
    let rig = match provider.api {
        ProviderApi::OpenAiResponses => RigLlm::new(client.responses(&model.id)),
        ProviderApi::OpenAiCompletions => RigLlm::new(client.chat(&model.id)),
    };
    rig.model.spec()?;
    Some(supported_levels(&rig.model, model))
}

fn supported_levels(rig: &DynModel<Completion>, model: &Model) -> Vec<ReasoningLevel> {
    let spec = rig.spec();
    let levels = spec.and_then(|s| s.reasoning.levels());
    let off = spec.and_then(|s| s.reasoning.can_disable()).unwrap_or(true);
    ReasoningLevel::ALL
        .into_iter()
        .filter(|level| {
            model.reasoning_map.get(level).map_or_else(
                || {
                    if *level == ReasoningLevel::Off {
                        off
                    } else {
                        levels.is_none_or(|levels| {
                            levels
                                .iter()
                                .any(|effort| effort.as_str() == level.as_str())
                        })
                    }
                },
                Option::is_some,
            )
        })
        .collect()
}

fn resolve_reasoning(
    rig: &DynModel<Completion>,
    model: &Model,
    requested: ReasoningLevel,
) -> Option<ReasoningSetting> {
    let spec = rig.spec();
    let off = spec.and_then(|s| s.reasoning.can_disable()).unwrap_or(true);
    let levels = supported_levels(rig, model);
    let supported = |level: ReasoningLevel| levels.contains(&level);
    let chosen = ReasoningLevel::ALL
        .into_iter()
        .filter(|level| *level >= requested)
        .find(|level| supported(*level))
        .or_else(|| {
            ReasoningLevel::ALL
                .into_iter()
                .rev()
                .filter(|level| *level < requested)
                .find(|level| supported(*level))
        })?;
    let mapped = model.reasoning_map.get(&chosen).and_then(Option::as_deref);
    if let Some(value) = mapped {
        return Some(ReasoningSetting::Raw(value.to_owned()));
    }
    let effort = match chosen {
        ReasoningLevel::Off => {
            return off.then_some(ReasoningSetting::Typed(completion::Reasoning::Off))
        }
        ReasoningLevel::Low => completion::Effort::Low,
        ReasoningLevel::Medium => completion::Effort::Medium,
        ReasoningLevel::High => completion::Effort::High,
        ReasoningLevel::XHigh => completion::Effort::XHigh,
        ReasoningLevel::Max => completion::Effort::Max,
    };
    Some(ReasoningSetting::Typed(effort.into()))
}

pub async fn discover(
    provider: &Provider,
    api_key: &str,
) -> Result<Vec<DiscoveredModel>, LlmError> {
    let listed = client(provider, api_key)
        .list_models()
        .await
        .map_err(request_error)?;
    let mut models: Vec<DiscoveredModel> = listed
        .data
        .into_iter()
        .map(|info| DiscoveredModel {
            context_window: info.context_length.or_else(|| context_window(&info.id)),
            id: info.id,
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

fn client(provider: &Provider, api_key: &str) -> OpenAI {
    OpenAIConfig::new(api_key)
        .with_base_url(&provider.base_url)
        .client()
}

fn context_window(model: &str) -> Option<u32> {
    Catalog::builtin()
        .iter()
        .find_map(|spec| (spec.id == model).then_some(spec.context_window).flatten())
}

#[async_trait::async_trait]
impl Llm for RigLlm {
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<CompletionResponse, LlmError> {
        let mut request = to_rig_request(request)?;
        request = match &self.reasoning {
            Some(ReasoningSetting::Typed(reasoning)) => request.reasoning(*reasoning),
            Some(ReasoningSetting::Raw(value)) => request.additional_params(match self.api {
                ProviderApi::OpenAiResponses => serde_json::json!({"reasoning": {"effort": value}}),
                ProviderApi::OpenAiCompletions => serde_json::json!({"reasoning_effort": value}),
            }),
            None => request,
        };
        let mut attempt = 0;
        let response = loop {
            loop {
                let until = *self.hold.lock().await;
                match until {
                    Some(until) if until > Instant::now() => tokio::time::sleep_until(until).await,
                    _ => break,
                }
            }
            attempt += 1;
            match self.send(request.clone()).await {
                Err(LlmError::RateLimited {
                    message,
                    retry_after,
                }) => {
                    let delay = retry_after
                        .unwrap_or_else(|| Duration::from_secs((1_u64 << (attempt - 1)).min(60)))
                        .min(Duration::from_secs(300));
                    let mut hold = self.hold.lock().await;
                    let until = Instant::now() + delay;
                    *hold = Some(hold.map_or(until, |previous| previous.max(until)));
                    drop(hold);
                    if attempt >= 8 {
                        return Err(LlmError::RateLimited {
                            message,
                            retry_after,
                        });
                    }
                }
                result => break result?,
            }
        };
        let usage = Usage {
            input: response.usage.input_tokens.unwrap_or_default(),
            output: response.usage.output_tokens.unwrap_or_default(),
            cache_read: response.usage.cached_input_tokens.unwrap_or_default(),
            cache_write: response
                .usage
                .cache_creation_input_tokens
                .unwrap_or_default(),
            total: response.usage.total_tokens.unwrap_or_default(),
        };
        Ok(CompletionResponse {
            text: response.text(),
            tool_calls: response
                .tool_calls()
                .map(|call| ToolCallRequest {
                    id: call.id.wire().into_owned(),
                    name: call.function.name.to_string(),
                    arguments: call.function.raw_arguments(),
                })
                .collect(),
            usage: (usage != Usage::default()).then_some(usage),
        })
    }
}

fn to_rig_request(
    request: CompletionRequest<'_>,
) -> Result<completion::CompletionRequest, LlmError> {
    let mut history: Vec<completion::Message> = Vec::new();

    for (index, message) in request.messages.iter().enumerate() {
        match message {
            Message::Summary {
                text,
                read_files,
                modified_files,
                ..
            } => {
                let mut text = summary_text(text, read_files, modified_files);
                if matches!(
                    request.messages.get(index + 1),
                    Some(Message::User(_) | Message::Summary { .. })
                ) {
                    text.push_str("\n\n");
                }
                history.push(completion::Message::user(text));
            }
            Message::User(text) => {
                let text = if matches!(
                    request.messages.get(index + 1),
                    Some(Message::User(_) | Message::Summary { .. })
                ) {
                    format!("{text}\n\n")
                } else {
                    text.clone()
                };
                history.push(completion::Message::user(text));
            }
            Message::Assistant(text) => history.push(completion::Message::assistant(text)),
            Message::ToolCall {
                id,
                name,
                arguments,
            } => {
                let call = AssistantContent::ToolCall(ToolCall::from_wire(
                    id.as_str(),
                    ToolFunction::parse(tool_name(name)?, arguments),
                ));
                match history.last_mut() {
                    Some(completion::Message::Assistant(turn)) => turn.content.push(call),
                    _ => history.push(call.into()),
                }
            }
            Message::ToolResult {
                id,
                name,
                content,
                images,
            } => {
                let mut parts = vec![ToolResultContent::text(content)];
                for image in images {
                    let media_type = match image.mime.as_str() {
                        "image/png" => rig_core::message::ImageMediaType::PNG,
                        "image/jpeg" => rig_core::message::ImageMediaType::JPEG,
                        "image/gif" => rig_core::message::ImageMediaType::GIF,
                        "image/webp" => rig_core::message::ImageMediaType::WEBP,
                        other => {
                            return Err(LlmError::Request(format!(
                                "unsupported image type: {other}"
                            )))
                        }
                    };
                    parts.push(ToolResultContent::Image(rig_core::message::Image {
                        data: rig_core::message::DocumentSourceKind::Base64(
                            base64::engine::general_purpose::STANDARD.encode(&image.data),
                        ),
                        media_type: Some(media_type),
                        detail: None,
                        native: None,
                    }));
                }
                let result = UserContent::tool_result(
                    CallId::from_wire(id.as_str()),
                    tool_name(name)?,
                    parts,
                );
                match history.last_mut() {
                    Some(completion::Message::User { content })
                        if matches!(content.last(), Some(UserContent::ToolResult(_))) =>
                    {
                        content.push(result)
                    }
                    _ => history.push(result.into()),
                }
            }
        }
    }

    let tools = request
        .tools
        .iter()
        .map(tool_definition)
        .collect::<Result<Vec<_>, _>>()?;
    let rig_request = completion::CompletionRequest::from(history).tools(tools);
    Ok(match request.system.is_empty() {
        true => rig_request,
        false => rig_request.preamble(request.system),
    })
}

fn tool_definition(spec: &ToolSpec) -> Result<ToolDefinition, LlmError> {
    let parameters = serde_json::from_str(spec.input_schema).map_err(request_error)?;
    Ok(ToolDefinition::new(
        tool_name(spec.name)?,
        spec.description,
        parameters,
    ))
}

fn tool_name(name: &str) -> Result<ToolName, LlmError> {
    ToolName::new(name).map_err(request_error)
}

fn request_error(error: impl std::fmt::Display) -> LlmError {
    LlmError::Request(error.to_string())
}

static OVERFLOW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
    r"(?i)(?:prompt (?:is )?too long|prompt exceeds max length|request_too_large|",
    r"input is too long for requested model|exceeds the context window|",
    r"exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))|",
    r"context window exceeded|input token count.*exceeds the maximum|maximum prompt length is \d+|",
    r"reduce the length of the messages|maximum context length is \d+ tokens|",
    r"exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?|",
    r"input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)|",
    r"exceeds the limit of \d+|exceeds the available context size|greater than the context length|",
    r"context window exceeds limit|exceeded model token limit|too large for model with \d+ maximum context length|",
    r"prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?|",
    r"model_context_window_exceeded|prompt too long; exceeded (?:max )?context length|",
    r"range of input length should be|context[_ ]length[_ ]exceeded|too many tokens|token limit exceeded)"
)).unwrap()
});
static NON_OVERFLOW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:Throttling error|Service unavailable):|rate limit|too many requests")
        .unwrap()
});

fn classify_error(text: String, status: Option<u16>, retry_after: Option<Duration>) -> LlmError {
    let lower = text.to_lowercase();
    if status == Some(402)
        || [
            "insufficient_quota",
            "exceeded your current quota",
            "credit balance",
        ]
        .iter()
        .any(|term| lower.contains(term))
    {
        LlmError::QuotaExhausted(text)
    } else if status == Some(429)
        || [
            "rate limit",
            "too many requests",
            "throttl",
            "quota exceeded for requests",
        ]
        .iter()
        .any(|term| lower.contains(term))
    {
        static RETRY: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?i)(?:try again|retry)(?:\s+after|\s+in)\s+(\d+(?:\.\d+)?)\s*(milliseconds?|msecs?|ms|seconds?|secs?|s)\b").unwrap()
        });
        let retry_after = retry_after.or_else(|| {
            RETRY.captures(&text).and_then(|m| {
                let amount = m[1].parse::<f64>().ok()?;
                let unit = m[2].to_ascii_lowercase();
                let seconds = if unit.starts_with('m') {
                    amount / 1000.0
                } else {
                    amount
                };
                Duration::try_from_secs_f64(seconds).ok()
            })
        });
        LlmError::RateLimited {
            message: text,
            retry_after,
        }
    } else if status == Some(413) || (!NON_OVERFLOW.is_match(&text) && OVERFLOW.is_match(&text)) {
        LlmError::ContextOverflow(text)
    } else {
        LlmError::Request(text)
    }
}

fn summary_text(text: &str, read_files: &[String], modified_files: &[String]) -> String {
    format!(
        "The conversation history before this point was compacted into the following summary:\n\n<summary>\n{text}{}{}\n</summary>",
        file_list("read-files", read_files),
        file_list("modified-files", modified_files),
    )
}

fn file_list(tag: &str, files: &[String]) -> String {
    if files.is_empty() {
        String::new()
    } else {
        format!("\n\n<{tag}>\n{}\n</{tag}>", files.join("\n"))
    }
}
