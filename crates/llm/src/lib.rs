use environment::ToolSpec;
use storage::Message;

#[derive(Debug, Clone)]
pub struct CompletionRequest<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    /// Optional stable routing key for upstream prompt caching (sent as
    /// `prompt_cache_key` on OpenAI Responses).
    pub cache_key: Option<&'a str>,
    /// Explicit usage bucket (`chat`, `task`, `utility`, `compaction`) so
    /// telemetry attributes fallback compaction calls accurately.
    pub kind: Option<storage::UsageKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCallRequest>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("LLM request failed: {0}")]
    Request(String),
    #[error("context overflow: {0}")]
    ContextOverflow(String),
    #[error("rate limited: {message}")]
    RateLimited {
        message: String,
        retry_after: Option<std::time::Duration>,
    },
    #[error("quota exhausted: {0}")]
    QuotaExhausted(String),
}

pub struct Fallback<A, B> {
    primary: A,
    secondary: B,
    switched: std::sync::atomic::AtomicBool,
}

impl<A, B> Fallback<A, B> {
    pub fn new(primary: A, secondary: B) -> Self {
        Self {
            primary,
            secondary,
            switched: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl<A: Llm + Sync, B: Llm + Sync> Llm for Fallback<A, B> {
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<CompletionResponse, LlmError> {
        use std::sync::atomic::Ordering;
        if !self.switched.load(Ordering::Acquire) {
            match self.primary.complete(request.clone()).await {
                Err(LlmError::QuotaExhausted(_)) => self.switched.store(true, Ordering::Release),
                result => return result,
            }
        }
        self.secondary.complete(request).await
    }
}

#[async_trait::async_trait]
pub trait Llm {
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<CompletionResponse, LlmError>;
}

/// One multiple-choice classification question for a System One / Jev-compatible
/// decision model (such as Cloudflare Workers AI Clef / Clef-flash).
#[derive(Debug, Clone, Copy)]
pub struct ChoiceQuestion<'a> {
    pub instructions: &'a str,
    pub criteria: &'a [(&'a str, &'a str)],
}

#[derive(Debug, Clone)]
pub struct ChoiceAnswer {
    pub choice: String,
    pub probability: f64,
    pub usage: Option<Usage>,
}

#[async_trait::async_trait]
pub trait DecisionModel: Send + Sync {
    async fn choose(
        &self,
        state: &str,
        question: ChoiceQuestion<'_>,
    ) -> Result<ChoiceAnswer, LlmError>;
}

/// Resolves the model a role should use now. Callers resolve once at the
/// start of a unit of work (a turn, a compaction pass, a one-shot call) and
/// keep the returned client until it finishes; the next unit resolves again.
pub trait Source: Send + Sync {
    type Llm: Llm + Send;
    fn current(&self) -> Result<Self::Llm, LlmError>;
}
