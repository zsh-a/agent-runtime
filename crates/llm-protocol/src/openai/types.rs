use super::*;

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum OpenAiResponseFormat {
    JsonObject,
    JsonSchema { json_schema: OpenAiJsonSchema },
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiJsonSchema {
    pub(crate) name: String,
    pub(crate) schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) strict: Option<bool>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiMessage {
    pub(crate) role: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub(crate) content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tool_calls: Vec<OpenAiToolCall>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiTool {
    pub(crate) r#type: String,
    pub(crate) function: OpenAiToolFunction,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiToolFunction {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: Value,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiToolCall {
    pub(crate) id: String,
    pub(crate) r#type: String,
    pub(crate) function: OpenAiToolCallFunction,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiToolCallFunction {
    pub(crate) name: String,
    pub(crate) arguments: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiChatCompletionResponse {
    #[serde(default)]
    pub(crate) choices: Vec<OpenAiChoice>,
    #[serde(default)]
    pub(crate) usage: Option<OpenAiUsage>,
    #[serde(default)]
    pub(crate) error: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiChoice {
    #[serde(default)]
    pub(crate) delta: Option<OpenAiMessageResponse>,
    #[serde(default)]
    pub(crate) finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiMessageResponse {
    #[serde(default)]
    pub(crate) refusal: Option<String>,
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) reasoning_content: Option<String>,
    #[serde(default)]
    pub(crate) reasoning: Option<String>,
    #[serde(default)]
    pub(crate) tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiStreamToolCall {
    #[serde(default)]
    pub(crate) index: Option<i64>,
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) function: Option<OpenAiStreamToolFunction>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiStreamToolFunction {
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiUsage {
    #[serde(default)]
    pub(crate) prompt_tokens: u32,
    #[serde(default)]
    pub(crate) completion_tokens: u32,
    #[serde(default)]
    pub(crate) total_tokens: u32,
}

#[derive(Debug, Default)]
pub(crate) struct OpenAiToolCallState {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: String,
    pub(crate) started: bool,
    pub(crate) ended: bool,
}

pub(crate) struct OpenAiSseState {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) buffer: String,
    pub(crate) pending: VecDeque<Result<LlmEvent, LlmError>>,
    pub(crate) content: String,
    pub(crate) finish_reason: Option<LlmFinishReason>,
    pub(crate) raw_finish_reason: Option<String>,
    pub(crate) terminal_signal: Option<String>,
    pub(crate) usage: Option<LlmUsage>,
    pub(crate) tools: BTreeMap<i64, OpenAiToolCallState>,
    pub(crate) response_format: Option<LlmResponseFormat>,
    pub(crate) finished: bool,
    pub(crate) refused: bool,
}
