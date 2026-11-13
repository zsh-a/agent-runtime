use super::*;

#[derive(Debug, Serialize)]
pub(crate) struct AnthropicMessage {
    pub(crate) role: String,
    pub(crate) content: Value,
}

#[derive(Debug, Serialize)]
pub(crate) struct AnthropicTool {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_schema: Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicUsage {
    #[serde(default)]
    pub(crate) input_tokens: u32,
    #[serde(default)]
    pub(crate) output_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    pub(crate) event_type: String,
    #[serde(default)]
    pub(crate) index: Option<i64>,
    #[serde(default)]
    pub(crate) content_block: Option<Value>,
    #[serde(default)]
    pub(crate) delta: Option<Value>,
    #[serde(default)]
    pub(crate) usage: Option<AnthropicUsage>,
    #[serde(default)]
    pub(crate) message: Option<AnthropicStreamMessage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicStreamMessage {
    #[serde(default)]
    pub(crate) usage: Option<AnthropicUsage>,
}

pub(crate) struct AnthropicSseState {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) anthropic_version: String,
    pub(crate) buffer: String,
    pub(crate) pending: VecDeque<Result<LlmEvent, LlmError>>,
    pub(crate) content: String,
    pub(crate) finish_reason: Option<LlmFinishReason>,
    pub(crate) raw_finish_reason: Option<String>,
    pub(crate) terminal_signal: Option<String>,
    pub(crate) input_tokens: u32,
    pub(crate) output_tokens: u32,
    pub(crate) raw_blocks: Vec<Value>,
    pub(crate) blocks: BTreeMap<i64, AnthropicBlockState>,
    pub(crate) response_format: Option<LlmResponseFormat>,
    pub(crate) finished: bool,
}

#[derive(Debug, Default)]
pub(crate) struct AnthropicBlockState {
    pub(crate) block_type: String,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) input: Option<Value>,
    pub(crate) partial_input_json: String,
}
