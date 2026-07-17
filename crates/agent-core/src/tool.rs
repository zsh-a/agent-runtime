use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Whether an interrupted host tool call may be dispatched again.
///
/// `at_most_once` is the compatibility default because older catalogs did not
/// declare replay safety. Hosts may opt read-only calls into `safe_retry`, or
/// declare writes `idempotent` when they honor the runtime effect id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolReplayPolicy {
    SafeRetry,
    Idempotent,
    #[default]
    AtMostOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcomeStatus {
    Ok,
    Error,
    PolicyDenied,
    ApprovalRequired,
    Cancelled,
}

impl ToolOutcomeStatus {
    pub fn is_error(self) -> bool {
        self != Self::Ok
    }
}

/// Semantic result of a tool call, separate from JSON-RPC transport success.
///
/// A host can successfully return a JSON-RPC response while the tool itself is
/// denied by policy or requires approval. Keeping that distinction explicit
/// prevents traces and recovery code from treating a denied side effect as a
/// successful execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolOutcome {
    pub status: ToolOutcomeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub details: Value,
}

impl ToolOutcome {
    pub fn ok() -> Self {
        Self {
            status: ToolOutcomeStatus::Ok,
            code: None,
            message: None,
            retryable: false,
            details: json!({}),
        }
    }

    pub fn is_error(&self) -> bool {
        self.status.is_error()
    }
}

/// Derive a semantic outcome from a legacy tool payload.
///
/// This keeps `agent.v1` hosts compatible while they migrate to emitting the
/// explicit `outcome` object. It recognizes the error envelopes already used
/// by NaviWealth and by the CLI tool adapters.
pub fn infer_tool_outcome(output: &Value, transport_error: bool) -> ToolOutcome {
    let nested_error = output.get("error");
    let code = output
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| {
            nested_error
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned);
    let message = output
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| {
            nested_error
                .and_then(|value| value.get("message"))
                .and_then(Value::as_str)
        })
        .or_else(|| nested_error.and_then(Value::as_str))
        .map(str::to_owned);
    let retryable = output
        .get("retryable")
        .and_then(Value::as_bool)
        .or_else(|| {
            nested_error
                .and_then(|value| value.get("retryable"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);
    let status = match code.as_deref() {
        Some("policy_denied" | "runtime_not_allowed") => ToolOutcomeStatus::PolicyDenied,
        Some("approval_required" | "confirmation_required") => ToolOutcomeStatus::ApprovalRequired,
        Some("user_cancel" | "user_cancelled" | "cancelled") => ToolOutcomeStatus::Cancelled,
        Some(_) if transport_error || nested_error.is_some() => ToolOutcomeStatus::Error,
        _ if output.get("policy_denied").and_then(Value::as_bool) == Some(true) => {
            ToolOutcomeStatus::PolicyDenied
        }
        _ if transport_error || nested_error.is_some() => ToolOutcomeStatus::Error,
        _ => ToolOutcomeStatus::Ok,
    };
    ToolOutcome {
        status,
        code,
        message,
        retryable,
        details: output
            .get("details")
            .cloned()
            .or_else(|| nested_error.and_then(|value| value.get("details")).cloned())
            .unwrap_or_else(|| json!({})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_nested_policy_denial_from_successful_transport() {
        let outcome = infer_tool_outcome(
            &json!({
                "error": {
                    "code": "policy_denied",
                    "message": "external calls are blocked"
                },
                "policy_denied": true
            }),
            false,
        );
        assert_eq!(outcome.status, ToolOutcomeStatus::PolicyDenied);
        assert!(outcome.is_error());
    }

    #[test]
    fn ordinary_business_payload_remains_successful() {
        let outcome = infer_tool_outcome(&json!({"code": "USD", "value": 12}), false);
        assert_eq!(outcome.status, ToolOutcomeStatus::Ok);
    }
}
