use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{RunId, RunScope, RunWorkflow, protocol_version};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentEvent {
    pub kind: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceEvent {
    pub kind: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    #[serde(default)]
    pub payload: Value,
}

impl TraceEvent {
    pub fn new(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            occurred_at: OffsetDateTime::now_utc(),
            payload,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Blob,
    Document,
    Image,
    Dataset,
    Log,
    Other,
}

fn default_artifact_kind() -> ArtifactKind {
    ArtifactKind::Blob
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RedactionClassification {
    Public,
    Internal,
    Confidential,
    Secret,
}

fn default_redaction_classification() -> RedactionClassification {
    RedactionClassification::Internal
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactStoreRef {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactRef {
    pub artifact_id: String,
    #[serde(default = "default_artifact_kind")]
    pub kind: ArtifactKind,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default = "default_redaction_classification")]
    pub redaction_classification: RedactionClassification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<ArtifactStoreRef>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceSpan {
    pub span_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub name: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub finished_at: OffsetDateTime,
    pub duration_ms: u64,
    pub status: String,
    #[serde(default)]
    pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceUsageProviderSummary {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cost_micros_by_currency: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceUsageSummary {
    pub llm_request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cost_micros_by_currency: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_provider: Vec<TraceUsageProviderSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub enum HookEventName {
    SessionStart,
    SessionStop,
    RunStart,
    RunStop,
    BeforeAgentStep,
    AfterAgentStep,
    SubagentStart,
    SubagentStop,
    BeforeToolCall,
    AfterToolCall,
    BeforeProposalCreate,
    BeforeProposalApply,
    AfterProposalDecision,
    BeforeStateSave,
    AfterStateSave,
    BeforeCompact,
    AfterCompact,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    NativeRust,
    Process,
    Server,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookInvocationStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HookEvent {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub hook_event: HookEventName,
    pub hook_kind: HookKind,
    pub hook_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub status: HookInvocationStatus,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub finished_at: OffsetDateTime,
    pub duration_ms: u64,
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentTrace {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub runtime_version: String,
    pub run_id: RunId,
    pub agent_id: String,
    pub agent_version: String,
    #[serde(default = "default_trace_scope")]
    pub scope: RunScope,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub finished_at: OffsetDateTime,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<RunWorkflow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_summary: Option<TraceUsageSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<TraceSpan>,
    #[serde(default)]
    pub events: Vec<TraceEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ArtifactRef>,
}

fn default_trace_scope() -> RunScope {
    RunScope::Global
}
