use super::*;

#[derive(Debug, Serialize)]
pub(super) struct AnthropicMessagesRequest {
    pub(super) model: String,
    pub(super) max_tokens: u32,
    pub(super) messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<AnthropicTool>,
    pub(super) stream: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct AnthropicMessage {
    pub(super) role: String,
    pub(super) content: Value,
}

#[derive(Debug, Serialize)]
pub(super) struct AnthropicTool {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) input_schema: Value,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicMessagesResponse {
    #[serde(default)]
    pub(super) content: Vec<Value>,
    #[serde(default)]
    pub(super) stop_reason: Option<String>,
    #[serde(default)]
    pub(super) usage: Option<AnthropicUsage>,
    #[serde(default)]
    pub(super) error: Option<AnthropicErrorBody>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicUsage {
    #[serde(default)]
    pub(super) input_tokens: u32,
    #[serde(default)]
    pub(super) output_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicErrorBody {
    #[serde(default)]
    pub(super) r#type: Option<String>,
    #[serde(default)]
    pub(super) message: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    pub(super) event_type: String,
    #[serde(default)]
    pub(super) index: Option<i64>,
    #[serde(default)]
    pub(super) content_block: Option<Value>,
    #[serde(default)]
    pub(super) delta: Option<Value>,
    #[serde(default)]
    pub(super) usage: Option<AnthropicUsage>,
    #[serde(default)]
    pub(super) message: Option<AnthropicStreamMessage>,
    #[serde(default)]
    pub(super) error: Option<AnthropicErrorBody>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicStreamMessage {
    #[serde(default)]
    pub(super) usage: Option<AnthropicUsage>,
}

pub(super) struct AnthropicSseState {
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) anthropic_version: String,
    pub(super) chunks: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    pub(super) buffer: String,
    pub(super) pending: VecDeque<Result<LlmEvent, LlmError>>,
    pub(super) content: String,
    pub(super) finish_reason: Option<LlmFinishReason>,
    pub(super) input_tokens: u32,
    pub(super) output_tokens: u32,
    pub(super) raw_blocks: Vec<Value>,
    pub(super) blocks: BTreeMap<i64, AnthropicBlockState>,
    pub(super) response_format: Option<LlmResponseFormat>,
    pub(super) finished: bool,
}

#[derive(Debug, Default)]
pub(super) struct AnthropicBlockState {
    pub(super) block_type: String,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: Option<Value>,
    pub(super) partial_input_json: String,
}
