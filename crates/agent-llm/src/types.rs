use std::pin::Pin;

use agent_core::{PROTOCOL_VERSION, ToolSpec};
use async_trait::async_trait;
use futures::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub type LlmEventStream = Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LlmRequest {
    pub protocol_version: String,
    pub provider: String,
    pub model: String,
    pub messages: Vec<LlmMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<LlmResponseFormat>,
    #[serde(default)]
    pub metadata: Value,
}

impl LlmRequest {
    pub fn validate_protocol(&self) -> Result<(), LlmError> {
        if self.protocol_version == PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(LlmError::validation(format!(
                "protocol_version '{}' is not supported; expected '{PROTOCOL_VERSION}'",
                self.protocol_version
            )))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmResponseFormat {
    JsonObject,
    JsonSchema {
        name: String,
        schema: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strict: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LlmMessage {
    pub role: LlmRole,
    pub content: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LlmResponse {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub provider: String,
    pub model: String,
    pub content: String,
    pub finish_reason: LlmFinishReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsage>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmFinishReason {
    Stop,
    Length,
    ToolCall,
    ContentFilter,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LlmUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LlmEvent {
    pub kind: LlmEventKind,
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
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmEventKind {
    Started,
    Delta,
    ThinkingDelta,
    ThinkingSignatureDelta,
    ToolCallStart,
    ToolCallDelta,
    ToolCallEnd,
    Finished,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LlmErrorRecord {
    pub kind: LlmErrorKind,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default)]
    pub details: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmErrorKind {
    ValidationError,
    ProviderError,
    TransientProviderError,
    RateLimited,
    Timeout,
    InternalError,
}

#[derive(Debug, Error)]
#[error("{record:?}")]
pub struct LlmError {
    pub record: Box<LlmErrorRecord>,
}

impl LlmError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            record: Box::new(LlmErrorRecord {
                kind: LlmErrorKind::ValidationError,
                code: "validation_error".to_owned(),
                message: message.into(),
                retryable: false,
                details: json!({}),
            }),
        }
    }

    pub(crate) fn provider(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        details: Value,
    ) -> Self {
        Self {
            record: Box::new(LlmErrorRecord {
                kind: if retryable {
                    LlmErrorKind::TransientProviderError
                } else {
                    LlmErrorKind::ProviderError
                },
                code: code.into(),
                message: message.into(),
                retryable,
                details,
            }),
        }
    }

    pub(crate) fn rate_limited(message: impl Into<String>, details: Value) -> Self {
        Self {
            record: Box::new(LlmErrorRecord {
                kind: LlmErrorKind::RateLimited,
                code: "rate_limited".to_owned(),
                message: message.into(),
                retryable: true,
                details,
            }),
        }
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    async fn stream(&self, request: LlmRequest) -> Result<LlmEventStream, LlmError>;
}

pub fn user_message(content: impl Into<String>) -> LlmMessage {
    LlmMessage {
        role: LlmRole::User,
        content: Value::String(content.into()),
        name: None,
        metadata: json!({}),
    }
}

fn protocol_version() -> String {
    PROTOCOL_VERSION.to_owned()
}
