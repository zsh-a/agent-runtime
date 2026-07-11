use agent_core::{AgentErrorKind, AgentErrorRecord, ToolError};
use serde_json::{Value, json};

pub(crate) fn tool_error(code: &str, message: impl Into<String>) -> ToolError {
    ToolError {
        record: Box::new(AgentErrorRecord {
            kind: AgentErrorKind::ToolError,
            code: code.to_owned(),
            message: message.into(),
            retryable: false,
            details: json!({}),
        }),
    }
}

pub(crate) fn retryable_tool_error(
    code: &str,
    message: impl Into<String>,
    details: Value,
) -> ToolError {
    ToolError {
        record: Box::new(AgentErrorRecord {
            kind: AgentErrorKind::ToolError,
            code: code.to_owned(),
            message: message.into(),
            retryable: true,
            details,
        }),
    }
}

pub(crate) fn tool_error_with_details(
    code: &str,
    message: impl Into<String>,
    details: Value,
) -> ToolError {
    ToolError {
        record: Box::new(AgentErrorRecord {
            kind: AgentErrorKind::ToolError,
            code: code.to_owned(),
            message: message.into(),
            retryable: false,
            details,
        }),
    }
}

pub(crate) fn tool_error_from_json(default_code: &str, error: &Value) -> ToolError {
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .map(|code| format!("json_rpc_{code}"))
        .unwrap_or_else(|| default_code.to_owned());
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string());
    let retryable = error
        .get("data")
        .and_then(|data| data.get("retryable"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    ToolError {
        record: Box::new(AgentErrorRecord {
            kind: AgentErrorKind::ToolError,
            code,
            message,
            retryable,
            details: error.get("data").cloned().unwrap_or_else(|| json!({})),
        }),
    }
}
