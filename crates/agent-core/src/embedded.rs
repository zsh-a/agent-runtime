use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{RunId, RunScope, RunWorkflow, ToolRisk, protocol_version};

pub const EMBEDDED_SNAPSHOT_VERSION: u32 = 1;

fn empty_json_object() -> Value {
    Value::Object(Default::default())
}

/// A serializable checkpoint produced by the embedded host-effect loop.
///
/// Embedded hosts persist this value as protocol JSON and return it unchanged
/// when supplying the next host-effect response. Product persistence remains
/// host-owned; this contract only describes runtime execution state.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedRunStep {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub run_id: RunId,
    pub agent_id: String,
    pub agent_version: String,
    pub step_index: u64,
    pub status: EmbeddedRunStepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<EmbeddedHostEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_response: Option<EmbeddedEffectResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_result: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effect_results: Vec<EmbeddedEffectResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<EmbeddedRunContinuation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Value>,
    pub run_state: EmbeddedRunState,
    pub trace_event: EmbeddedStepTraceEvent,
    /// Forward-compatible extension fields. Stable consumers should not need
    /// these for control flow.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl EmbeddedRunStep {
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    pub fn requested_effect(&self) -> Option<&EmbeddedHostEffect> {
        (self.status == EmbeddedRunStepStatus::EffectRequested)
            .then_some(self.effect.as_ref())
            .flatten()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedRunLimits {
    pub max_effect_steps: u32,
    pub max_subagent_depth: u32,
}

impl Default for EmbeddedRunLimits {
    fn default() -> Self {
        Self {
            max_effect_steps: 4,
            max_subagent_depth: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedRunProgress {
    pub dispatched_effect_count: u32,
    pub subagent_depth: u32,
    #[serde(default)]
    pub effect_budget_exhausted: bool,
    #[serde(default)]
    pub subagent_depth_exceeded: bool,
}

impl EmbeddedRunProgress {
    pub fn remaining_effect_steps(self, limits: EmbeddedRunLimits) -> u32 {
        limits
            .max_effect_steps
            .saturating_sub(self.dispatched_effect_count)
    }
}

/// Versioned, host-persistable state for an embedded run.
///
/// Hosts must treat the snapshot as opaque protocol data apart from inspecting
/// the current requested effect and terminal status. The runtime validates it
/// again before every continuation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedRunSnapshot {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub snapshot_version: u32,
    pub step: EmbeddedRunStep,
    pub limits: EmbeddedRunLimits,
    pub progress: EmbeddedRunProgress,
}

impl EmbeddedRunSnapshot {
    pub fn is_terminal(&self) -> bool {
        self.step.is_terminal()
    }

    pub fn requested_effect(&self) -> Option<&EmbeddedHostEffect> {
        self.step.requested_effect()
    }

    pub fn remaining_effect_steps(&self) -> u32 {
        self.progress.remaining_effect_steps(self.limits)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddedRunStepStatus {
    EffectRequested,
    Completed,
    Failed,
    Cancelled,
    PolicyDenied,
    ClosedEarly,
    TimedOut,
}

impl EmbeddedRunStepStatus {
    pub fn is_terminal(self) -> bool {
        self != Self::EffectRequested
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddedTerminalReason {
    Done,
    StreamError,
    UserCancel,
    PolicyDenied,
    ClosedEarly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedRunState {
    pub status: EmbeddedRunStepStatus,
    pub step_index: u64,
    pub remaining_effect_count: usize,
    pub effect_result_count: usize,
    #[serde(default)]
    pub terminal_reason: Option<EmbeddedTerminalReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmbeddedHostEffect {
    Tool {
        effect_id: String,
        name: String,
        #[serde(default = "empty_json_object")]
        input: Value,
        risk: ToolRisk,
        #[serde(default = "empty_json_object")]
        metadata: Value,
    },
    Subagent {
        effect_id: String,
        agent_id: String,
        #[serde(default = "empty_json_object")]
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<RunScope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow: Option<Box<RunWorkflow>>,
        #[serde(default = "empty_json_object")]
        metadata: Value,
    },
}

/// An effect that has not yet been materialized as the current host request.
/// The runtime assigns `effect_id` only when it promotes this value into an
/// [`EmbeddedHostEffect`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmbeddedPendingHostEffect {
    Tool {
        name: String,
        #[serde(default = "empty_json_object")]
        input: Value,
    },
    Subagent {
        agent_id: String,
        #[serde(default = "empty_json_object")]
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<RunScope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow: Option<Box<RunWorkflow>>,
        #[serde(default = "empty_json_object")]
        metadata: Value,
    },
}

impl EmbeddedHostEffect {
    pub fn effect_id(&self) -> &str {
        match self {
            Self::Tool { effect_id, .. } | Self::Subagent { effect_id, .. } => effect_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedEffectResponse {
    pub jsonrpc: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EmbeddedEffectError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedEffectError {
    /// JSON-RPC transport error code. Stable agent error codes belong in
    /// `data.code` or in a successful result envelope interpreted by the
    /// embedded runtime.
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedEffectResult {
    pub kind: EmbeddedEffectKind,
    pub effect: EmbeddedHostEffect,
    pub effect_response: EmbeddedEffectResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddedEffectKind {
    Tool,
    Subagent,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedRunContinuation {
    #[serde(default)]
    pub effects: Vec<EmbeddedPendingHostEffect>,
    #[serde(default)]
    pub effect_results: Vec<EmbeddedEffectResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_response: Option<Value>,
    pub next_step_index: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedStepTraceEvent {
    pub kind: String,
    pub run_id: RunId,
    pub agent_id: String,
    pub status: EmbeddedRunStepStatus,
    pub step_index: u64,
    #[serde(default)]
    pub effect_id: Option<String>,
    #[serde(default)]
    pub effect_kind: Option<EmbeddedEffectKind>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub subagent_id: Option<String>,
    pub run_state: EmbeddedRunState,
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}
