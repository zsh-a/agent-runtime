use super::*;

#[derive(Debug, Serialize)]
pub(super) struct OpenAiChatCompletionRequest {
    pub(super) model: String,
    pub(super) messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<OpenAiTool>,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream_options: Option<OpenAiStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) response_format: Option<OpenAiResponseFormat>,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiStreamOptions {
    pub(super) include_usage: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum OpenAiResponseFormat {
    JsonObject,
    JsonSchema { json_schema: OpenAiJsonSchema },
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiJsonSchema {
    pub(super) name: String,
    pub(super) schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) strict: Option<bool>,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiMessage {
    pub(super) role: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub(super) content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_calls: Vec<OpenAiToolCall>,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiTool {
    pub(super) r#type: String,
    pub(super) function: OpenAiToolFunction,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiToolFunction {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) parameters: Value,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiToolCall {
    pub(super) id: String,
    pub(super) r#type: String,
    pub(super) function: OpenAiToolCallFunction,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiToolCallFunction {
    pub(super) name: String,
    pub(super) arguments: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiChatCompletionResponse {
    #[serde(default)]
    pub(super) choices: Vec<OpenAiChoice>,
    #[serde(default)]
    pub(super) usage: Option<OpenAiUsage>,
    #[serde(default)]
    pub(super) error: Option<OpenAiErrorBody>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiChoice {
    pub(super) message: Option<OpenAiMessageResponse>,
    #[serde(default)]
    pub(super) delta: Option<OpenAiMessageResponse>,
    #[serde(default)]
    pub(super) finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiMessageResponse {
    #[serde(default)]
    pub(super) content: Option<String>,
    #[serde(default)]
    pub(super) reasoning_content: Option<String>,
    #[serde(default)]
    pub(super) reasoning: Option<String>,
    #[serde(default)]
    pub(super) tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiStreamToolCall {
    #[serde(default)]
    pub(super) index: Option<i64>,
    #[serde(default)]
    pub(super) id: Option<String>,
    #[serde(default)]
    pub(super) function: Option<OpenAiStreamToolFunction>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiStreamToolFunction {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiUsage {
    #[serde(default)]
    pub(super) prompt_tokens: u32,
    #[serde(default)]
    pub(super) completion_tokens: u32,
    #[serde(default)]
    pub(super) total_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenAiErrorBody {
    #[serde(default)]
    pub(super) message: String,
    #[serde(default)]
    pub(super) r#type: Option<String>,
    #[serde(default)]
    pub(super) code: Option<Value>,
}

pub(super) struct OpenAiSseState {
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) chunks: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    pub(super) buffer: String,
    pub(super) pending: VecDeque<Result<LlmEvent, LlmError>>,
    pub(super) content: String,
    pub(super) finish_reason: Option<LlmFinishReason>,
    pub(super) usage: Option<LlmUsage>,
    pub(super) tools: BTreeMap<i64, OpenAiToolCallState>,
    pub(super) response_format: Option<LlmResponseFormat>,
    pub(super) finished: bool,
}

#[derive(Debug, Default)]
pub(super) struct OpenAiToolCallState {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) arguments: String,
    pub(super) started: bool,
    pub(super) ended: bool,
}
