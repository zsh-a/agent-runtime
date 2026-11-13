//! Pure model protocol: no network client, runtime, clock, or timers.
mod anthropic;
mod content;
mod decoder;
mod endpoint;
mod openai;
mod sse;
mod structured;
mod types;
mod wire;
pub use content::*;
pub use decoder::StreamDecoder;
pub use endpoint::*;
pub use structured::{structured_output_from_content, structured_output_instruction};
pub use types::*;
pub use wire::{StructuredMode, decode, encode, http_error, rejects_structured_output};
fn llm_content_as_text<'a>(
    content: &'a MessageContent,
    provider: &str,
) -> Result<&'a str, LlmError> {
    content
        .as_str()
        .ok_or_else(|| LlmError::validation(format!("{provider} requires text content")))
}

pub use agent_core::PROTOCOL_VERSION;

#[cfg(test)]
mod tests;
