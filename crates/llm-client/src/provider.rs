use agent_core::bounds::{MaybeBoxStream, MaybeSend, MaybeSync};
use llm_protocol::*;
pub type LlmEventStream = MaybeBoxStream<'static, Result<LlmEvent, LlmError>>;
#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(
    not(all(target_arch = "wasm32", target_os = "unknown")),
    async_trait::async_trait
)]
pub trait LlmProvider: MaybeSend + MaybeSync {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    async fn stream(&self, request: LlmRequest) -> Result<LlmEventStream, LlmError>;
}
