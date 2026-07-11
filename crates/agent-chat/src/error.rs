use agent_core::{AgentErrorKind, AgentErrorRecord};
use agent_llm::LlmError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatErrorRecord {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default)]
    pub details: Value,
}

#[derive(Debug, Error)]
#[error("{record:?}")]
pub struct ChatError {
    pub record: Box<ChatErrorRecord>,
}

impl ChatError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            record: Box::new(ChatErrorRecord {
                code: "validation_error".to_owned(),
                message: message.into(),
                retryable: false,
                details: json!({}),
            }),
        }
    }

    pub(crate) fn llm(error: LlmError) -> Self {
        let record = error.record;
        Self {
            record: Box::new(ChatErrorRecord {
                code: record.code,
                message: record.message,
                retryable: record.retryable,
                details: record.details,
            }),
        }
    }
}

impl From<ChatError> for agent_core::AgentError {
    fn from(error: ChatError) -> Self {
        agent_core::AgentError {
            record: Box::new(AgentErrorRecord {
                kind: AgentErrorKind::LlmError,
                code: error.record.code,
                message: error.record.message,
                retryable: error.record.retryable,
                details: error.record.details,
            }),
        }
    }
}
