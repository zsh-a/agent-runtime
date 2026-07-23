use std::pin::Pin;

use agent_core::{
    CompactionRecord, ContextBlock, ContextPolicy, ContextSnapshot, InteractionEnvelope,
    InteractionResponse, PROTOCOL_VERSION, ToolOutcome, ToolSpec, infer_tool_outcome,
};
use agent_llm::{LlmMessage, LlmResponse, LlmUsage};
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ChatError;

pub type ChatEventStream = Pin<Box<dyn Stream<Item = Result<ChatTurnEvent, ChatError>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatTurnRequest {
    pub protocol_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub messages: Vec<LlmMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// Host-provided context that is independent from the conversational
    /// transcript. The runtime validates, budgets, snapshots, and renders
    /// these blocks; hosts retain ownership of retrieval and business policy.
    #[serde(default)]
    pub context_blocks: Vec<ContextBlock>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub context_policy: ContextPolicy,
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: u32,
    #[serde(default)]
    pub tool_execution: ChatToolExecution,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatTurnState {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub messages: Vec<LlmMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub context_blocks: Vec<ContextBlock>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub context_policy: ContextPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_snapshot: Option<ContextSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionRecord>,
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: u32,
    #[serde(default)]
    pub round: u32,
    #[serde(default)]
    pub pending_tool_calls: Vec<ChatToolCall>,
    /// A durable human-in-the-loop boundary. It is mutually exclusive with
    /// pending tool calls and is resolved through [ChatResumeRequest].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_interaction: Option<InteractionEnvelope>,
    #[serde(default)]
    pub tool_execution: ChatToolExecution,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub output: Value,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ToolOutcome>,
}

impl ChatToolResult {
    pub fn effective_outcome(&self) -> ToolOutcome {
        self.outcome
            .clone()
            .unwrap_or_else(|| infer_tool_outcome(&self.output, self.is_error))
    }

    pub fn effective_is_error(&self) -> bool {
        self.effective_outcome().is_error()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatToolExecution {
    #[default]
    Runtime,
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatResumeRequest {
    pub protocol_version: String,
    pub state: ChatTurnState,
    #[serde(default)]
    pub tool_results: Vec<ChatToolResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_response: Option<InteractionResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatTurnAdvance {
    Completed {
        state: ChatTurnState,
        stop_reason: String,
    },
    RequiresToolResults {
        state: ChatTurnState,
        tool_calls: Vec<ChatToolCall>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatTurnEvent {
    pub kind: ChatTurnEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<LlmResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_input_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsage>,
    pub round: u32,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatTurnEventKind {
    Started,
    LlmStarted,
    Delta,
    ThinkingDelta,
    ThinkingSignatureDelta,
    ToolCallStart,
    ToolCallDelta,
    ToolCallEnd,
    ToolResult,
    Usage,
    ContextSnapshot,
    InteractionRequired,
    InteractionResolved,
    RoundFinished,
    Error,
    Done,
}

pub(crate) fn default_max_tool_rounds() -> u32 {
    4
}

fn protocol_version() -> String {
    PROTOCOL_VERSION.to_owned()
}
