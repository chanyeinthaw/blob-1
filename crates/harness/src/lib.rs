pub mod chat;
pub mod messenger;
pub mod schedule;
pub mod share;
pub mod watch;
pub use messenger::Messengers;
pub use share::{PinnedWorkspace, ShareTools};
mod compaction;
pub mod prompt;
pub mod session;
mod tasks;
mod utility;
pub use utility::{decide_respond, parse_inline, parse_respond, Utility};

pub use chat::{
    ChatHarness, Current, ModelSelection, Models, ZoomId, CHAT_PROMPT, DEFAULT_INSTRUCTIONS,
};
pub use llm::{CompletionRequest, CompletionResponse, Llm, LlmError, ToolCallRequest, Usage};
pub use prompt::SUB_PROMPT;
pub use session::{
    CompactionConfig, EnvironmentPaths, HarnessConfig, HarnessError, MemoryTools, SessionHarness,
    Step,
};

/// Stable FNV-1a 64-bit hash used to build compact (<= 64 char) upstream
/// `prompt_cache_key` values scoped per workspace / session.
pub(crate) fn prompt_cache_key(scope: &str, workspace: &std::path::Path, extra: &str) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut hash: u64 = 0xcbf29ce484222325;
    for chunk in [workspace.as_os_str().as_bytes(), b"\0", extra.as_bytes()] {
        for &byte in chunk {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("blob-1:{scope}:{hash:016x}")
}
