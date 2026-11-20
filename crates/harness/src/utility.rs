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
