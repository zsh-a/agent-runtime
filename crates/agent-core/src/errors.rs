use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorKind {
    ValidationError,
    ToolError,
    LlmError,
    Timeout,
    Cancelled,
    ApprovalRequired,
    TransientExternalError,
    InternalError,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentErrorRecord {
    pub kind: AgentErrorKind,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default)]
    pub details: Value,
}

#[derive(Debug, Error)]
#[error("{record:?}")]
pub struct AgentError {
    pub record: AgentErrorRecord,
}

impl AgentError {
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            record: AgentErrorRecord {
                kind: AgentErrorKind::InternalError,
                code: "internal_error".to_owned(),
                message: message.into(),
                retryable: false,
                details: json!({}),
            },
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            record: AgentErrorRecord {
                kind: AgentErrorKind::ValidationError,
                code: "validation_error".to_owned(),
                message: message.into(),
                retryable: false,
                details: json!({}),
            },
        }
    }

    pub fn timeout(duration: Duration) -> Self {
        Self {
            record: AgentErrorRecord {
                kind: AgentErrorKind::Timeout,
                code: "timeout".to_owned(),
                message: format!("agent run timed out after {}ms", duration.as_millis()),
                retryable: true,
                details: json!({"timeout_ms": duration.as_millis()}),
            },
        }
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self {
            record: AgentErrorRecord {
                kind: AgentErrorKind::Cancelled,
                code: "cancelled".to_owned(),
                message: message.into(),
                retryable: false,
                details: json!({}),
            },
        }
    }

    pub fn policy_denied(message: impl Into<String>, details: Value) -> Self {
        Self {
            record: AgentErrorRecord {
                kind: AgentErrorKind::ApprovalRequired,
                code: "policy_denied".to_owned(),
                message: message.into(),
                retryable: false,
                details,
            },
        }
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct StoreError {
    pub message: String,
}

impl StoreError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
#[error("{record:?}")]
pub struct ToolError {
    pub record: AgentErrorRecord,
}

impl ToolError {
    pub fn from_agent_error(error: AgentError) -> Self {
        Self {
            record: error.record,
        }
    }

    pub fn policy_denied(message: impl Into<String>, details: Value) -> Self {
        Self {
            record: AgentError::policy_denied(message, details).record,
        }
    }
}
