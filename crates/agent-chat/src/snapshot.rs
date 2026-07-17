use std::collections::HashSet;

use agent_core::{PROTOCOL_VERSION, ToolReplayPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ChatError, ChatToolCall, ChatToolResult, ChatTurnState};

pub const CHAT_TURN_SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatTurnSnapshotStatus {
    ReadyForModel,
    RequiresToolResults,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatToolDispatchStatus {
    Pending,
    Dispatching,
    Completed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatToolDispatchRecord {
    pub call: ChatToolCall,
    pub replay_policy: ToolReplayPolicy,
    pub status: ChatToolDispatchStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ChatToolResult>,
}

impl ChatToolDispatchRecord {
    /// Whether a host may dispatch this call after process recovery.
    pub fn may_dispatch_after_recovery(&self) -> bool {
        match self.status {
            ChatToolDispatchStatus::Pending => true,
            ChatToolDispatchStatus::Dispatching => matches!(
                self.replay_policy,
                ToolReplayPolicy::SafeRetry | ToolReplayPolicy::Idempotent
            ),
            ChatToolDispatchStatus::Completed | ChatToolDispatchStatus::Interrupted => false,
        }
    }
}

/// Durable, host-owned boundary state for a provider-neutral chat turn.
///
/// The dispatch journal distinguishes a call that was never started from one
/// that may have crossed a side-effect boundary before an Android process was
/// reclaimed. `at_most_once` calls in `dispatching` therefore require manual
/// recovery instead of being silently replayed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatTurnSnapshot {
    pub protocol_version: String,
    pub snapshot_version: u32,
    pub status: ChatTurnSnapshotStatus,
    pub state: ChatTurnState,
    #[serde(default)]
    pub tool_dispatches: Vec<ChatToolDispatchRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

impl ChatTurnSnapshot {
    pub fn requires_tool_results(state: ChatTurnState) -> Self {
        let tool_dispatches = state
            .pending_tool_calls
            .iter()
            .cloned()
            .map(|call| {
                let replay_policy = state
                    .tools
                    .iter()
                    .find(|tool| tool.name == call.name)
                    .map(|tool| tool.replay_policy)
                    .unwrap_or(ToolReplayPolicy::AtMostOnce);
                ChatToolDispatchRecord {
                    call,
                    replay_policy,
                    status: ChatToolDispatchStatus::Pending,
                    result: None,
                }
            })
            .collect();
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            snapshot_version: CHAT_TURN_SNAPSHOT_VERSION,
            status: ChatTurnSnapshotStatus::RequiresToolResults,
            state,
            tool_dispatches,
            stop_reason: None,
            error: None,
        }
    }

    pub fn completed(state: ChatTurnState, stop_reason: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            snapshot_version: CHAT_TURN_SNAPSHOT_VERSION,
            status: ChatTurnSnapshotStatus::Completed,
            state,
            tool_dispatches: Vec::new(),
            stop_reason: Some(stop_reason.into()),
            error: None,
        }
    }

    pub fn validate(&self) -> Result<(), ChatError> {
        if self.protocol_version != PROTOCOL_VERSION
            || self.state.protocol_version != PROTOCOL_VERSION
        {
            return Err(ChatError::validation(format!(
                "chat snapshot protocol versions must be '{PROTOCOL_VERSION}'"
            )));
        }
        if self.snapshot_version != CHAT_TURN_SNAPSHOT_VERSION {
            return Err(ChatError::validation(format!(
                "chat snapshot version {} is not supported",
                self.snapshot_version
            )));
        }
        let pending_ids = self
            .state
            .pending_tool_calls
            .iter()
            .map(|call| call.id.as_str())
            .collect::<HashSet<_>>();
        let mut dispatch_ids = HashSet::new();
        for dispatch in &self.tool_dispatches {
            if !dispatch_ids.insert(dispatch.call.id.as_str()) {
                return Err(ChatError::validation(format!(
                    "chat snapshot duplicates tool call '{}'",
                    dispatch.call.id
                )));
            }
            if !pending_ids.contains(dispatch.call.id.as_str()) {
                return Err(ChatError::validation(format!(
                    "chat snapshot dispatch '{}' is not pending",
                    dispatch.call.id
                )));
            }
            match (dispatch.status, dispatch.result.as_ref()) {
                (ChatToolDispatchStatus::Completed, Some(result)) => {
                    if result.tool_call_id != dispatch.call.id
                        || result.tool_name != dispatch.call.name
                    {
                        return Err(ChatError::validation(format!(
                            "chat snapshot result does not match tool call '{}'",
                            dispatch.call.id
                        )));
                    }
                }
                (ChatToolDispatchStatus::Completed, None) => {
                    return Err(ChatError::validation(format!(
                        "completed chat dispatch '{}' is missing a result",
                        dispatch.call.id
                    )));
                }
                (_, Some(_)) => {
                    return Err(ChatError::validation(format!(
                        "unfinished chat dispatch '{}' cannot contain a result",
                        dispatch.call.id
                    )));
                }
                (_, None) => {}
            }
        }
        match self.status {
            ChatTurnSnapshotStatus::RequiresToolResults => {
                if pending_ids.is_empty() || pending_ids != dispatch_ids {
                    return Err(ChatError::validation(
                        "chat snapshot tool journal must match pending tool calls",
                    ));
                }
            }
            ChatTurnSnapshotStatus::ReadyForModel | ChatTurnSnapshotStatus::Completed => {
                if !self.state.pending_tool_calls.is_empty() || !self.tool_dispatches.is_empty() {
                    return Err(ChatError::validation(
                        "non-tool chat snapshot cannot retain pending tool calls",
                    ));
                }
            }
            ChatTurnSnapshotStatus::Cancelled | ChatTurnSnapshotStatus::Failed => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use agent_core::{ToolReplayPolicy, ToolRisk, ToolSpec};
    use serde_json::json;

    use super::*;
    use crate::{ChatToolExecution, ChatTurnRequest, chat_turn_initial_state};
    use agent_llm::user_message;

    fn pending_state(replay_policy: ToolReplayPolicy) -> ChatTurnState {
        let mut state = chat_turn_initial_state(&ChatTurnRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            turn_id: Some("turn_snapshot".to_owned()),
            surface: Some("ai_chat".to_owned()),
            mode: Some("chat".to_owned()),
            session_id: None,
            thread_id: None,
            agent_id: Some("ai_chat".to_owned()),
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            messages: vec![user_message("read")],
            temperature: None,
            max_output_tokens: None,
            tools: vec![ToolSpec {
                name: "read_task".to_owned(),
                description: "Read task".to_owned(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                risk: ToolRisk::ReadOnly,
                replay_policy,
                metadata: json!({}),
            }],
            metadata: json!({}),
            context_policy: Default::default(),
            max_tool_rounds: 4,
            tool_execution: ChatToolExecution::Client,
        })
        .expect("state");
        state.round = 1;
        state.pending_tool_calls = vec![ChatToolCall {
            id: "call_1".to_owned(),
            name: "read_task".to_owned(),
            input: json!({}),
        }];
        state
    }

    #[test]
    fn snapshot_captures_catalog_replay_policy() {
        let snapshot =
            ChatTurnSnapshot::requires_tool_results(pending_state(ToolReplayPolicy::SafeRetry));
        snapshot.validate().expect("snapshot validates");
        assert_eq!(
            snapshot.tool_dispatches[0].replay_policy,
            ToolReplayPolicy::SafeRetry
        );
    }

    #[test]
    fn interrupted_at_most_once_dispatch_cannot_be_replayed() {
        let mut snapshot =
            ChatTurnSnapshot::requires_tool_results(pending_state(ToolReplayPolicy::AtMostOnce));
        snapshot.tool_dispatches[0].status = ChatToolDispatchStatus::Dispatching;
        assert!(!snapshot.tool_dispatches[0].may_dispatch_after_recovery());
        snapshot.validate().expect("snapshot validates");
    }
}
