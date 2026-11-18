use crate::anthropic::AnthropicSseState;
use crate::openai::OpenAiSseState;
use crate::*;
use std::collections::VecDeque;

pub struct StreamDecoder {
    state: State,
    pending_bytes: Vec<u8>,
}
enum State {
    OpenAi(OpenAiSseState),
    Anthropic(AnthropicSseState),
}
impl StreamDecoder {
    pub fn new(endpoint: &Endpoint, request: &LlmRequest) -> Result<Self, LlmError> {
        let state = match endpoint.protocol {
            Protocol::OpenAiCompatible => State::OpenAi(OpenAiSseState::new(
                endpoint.provider.clone(),
                request.model.clone(),
                request.response_format.clone(),
            )),
            Protocol::Anthropic => State::Anthropic(AnthropicSseState::new(
                endpoint.provider.clone(),
                request.model.clone(),
                endpoint.anthropic_version.clone(),
                request.response_format.clone(),
            )),
            Protocol::OpenAiResponses => {
                return Err(LlmError::validation(
                    "Responses streaming is not supported; use whole-response completion",
                ));
            }
            Protocol::Ollama => return Err(LlmError::validation("Ollama uses a whole response")),
        };
        Ok(Self {
            state,
            pending_bytes: Vec::new(),
        })
    }
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<LlmEvent, LlmError>> {
        self.pending_bytes.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.pending_bytes) {
            Ok(_) => self.pending_bytes.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => {
                return vec![Err(LlmError::provider(
                    "provider_stream_decode_failed",
                    "Invalid UTF-8 in provider stream",
                    false,
                    serde_json::json!({}),
                ))];
            }
        };
        let text = std::str::from_utf8(&self.pending_bytes[..valid]).expect("validated UTF-8");
        match &mut self.state {
            State::OpenAi(s) => {
                s.buffer.push_str(text);
                s.drain_frames();
            }
            State::Anthropic(s) => {
                s.buffer.push_str(text);
                s.drain_frames();
            }
        }
        self.pending_bytes.drain(..valid);
        self.drain()
    }
    pub fn finish(&mut self) -> Vec<Result<LlmEvent, LlmError>> {
        if !self.pending_bytes.is_empty() {
            return vec![Err(LlmError::provider(
                "provider_stream_decode_failed",
                "Truncated UTF-8 in provider stream",
                false,
                serde_json::json!({}),
            ))];
        }
        match &mut self.state {
            State::OpenAi(s) => s.finish(),
            State::Anthropic(s) => s.finish(),
        };
        self.drain()
    }
    pub fn is_finished(&self) -> bool {
        match &self.state {
            State::OpenAi(s) => s.finished,
            State::Anthropic(s) => s.finished,
        }
    }
    fn drain(&mut self) -> Vec<Result<LlmEvent, LlmError>> {
        let p: &mut VecDeque<_> = match &mut self.state {
            State::OpenAi(s) => &mut s.pending,
            State::Anthropic(s) => &mut s.pending,
        };
        p.drain(..).collect()
    }
}
