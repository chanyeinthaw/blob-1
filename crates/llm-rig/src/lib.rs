use environment::ToolSpec;
use harness::{CompletionRequest, CompletionResponse, Llm, LlmError, ToolCallRequest};
use rig_core::{
    completion::{self, AssistantContent, ToolDefinition},
    message::{CallId, ToolCall, ToolFunction, ToolName, ToolResultContent, UserContent},
    operation::Completion,
    DynModel,
};
use storage::Message;

pub struct RigLlm {
    model: DynModel<Completion>,
}

impl RigLlm {
    pub fn new(model: impl Into<DynModel<Completion>>) -> Self {
        Self {
            model: model.into(),
        }
    }

    async fn send(
        &self,
        request: completion::CompletionRequest,
    ) -> Result<completion::CompletionResponse, LlmError> {
        let response = self.model.call(request).await.map_err(request_error)?;
        match response.error {
            Some(error) => Err(LlmError::Request(error)),
            None => Ok(response),
        }
    }
}

#[async_trait::async_trait]
impl Llm for RigLlm {
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<CompletionResponse, LlmError> {
        let response = self.send(to_rig_request(request)?).await?;
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
        })
    }

    async fn summarize(&self, messages: &[Message]) -> Result<String, LlmError> {
        let transcript = messages.iter().map(render).collect::<Vec<_>>().join("\n\n");
        let prompt = format!(
            "Summarize the conversation below so it can be continued from the summary alone. Keep the user's goals, decisions made, important facts and file paths, and any unfinished work.\n\n{transcript}"
        );
        let response = self
            .send(completion::CompletionRequest::new(
                completion::Message::user(prompt),
            ))
            .await?;
        Ok(response.text())
    }
}

fn to_rig_request(
    request: CompletionRequest<'_>,
) -> Result<completion::CompletionRequest, LlmError> {
    let mut system = Vec::new();
    let mut history: Vec<completion::Message> = Vec::new();

    for message in request.messages {
        match message {
            Message::System(text) => system.push(text.as_str()),
            Message::User(text) => history.push(completion::Message::user(text)),
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
            Message::ToolResult { id, name, content } => {
                let result = UserContent::tool_result(
                    CallId::from_wire(id.as_str()),
                    tool_name(name)?,
                    vec![ToolResultContent::text(content)],
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
    Ok(match system.is_empty() {
        true => rig_request,
        false => rig_request.preamble(system.join("\n\n")),
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

fn render(message: &Message) -> String {
    match message {
        Message::System(text) => format!("System: {text}"),
        Message::User(text) => format!("User: {text}"),
        Message::Assistant(text) => format!("Assistant: {text}"),
        Message::ToolCall {
            name, arguments, ..
        } => format!("Assistant called tool {name} with {arguments}"),
        Message::ToolResult { name, content, .. } => {
            format!("Tool {name} returned: {content}")
        }
    }
}
