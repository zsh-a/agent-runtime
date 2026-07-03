use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{AgentErrorRecord, PROTOCOL_VERSION, RunId, protocol_version, trace::AgentTrace};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentSpec {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub version: String,
    pub schedule: ScheduleSpec,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleSpec {
    Manual,
    Interval {
        every_seconds: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preferred_hour_local: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        jitter_seconds: Option<u64>,
    },
    Cron {
        expression: String,
        timezone: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UserContext {
    pub user_id: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    Manual,
    Scheduled,
    Replay,
    Webhook,
    Queue,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TriggerEnvelope {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[schemars(with = "Option<String>")]
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub received_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunRequest {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<UserContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RunScope>,
    #[serde(default = "default_trigger")]
    pub trigger: TriggerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_envelope: Option<TriggerEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<RunWorkflow>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunWorkflow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<RunDependency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanout_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<RunCompensation>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunDependency {
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunCompensation {
    pub compensates_run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunRequest {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<UserContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RunScope>,
    #[serde(default = "default_trigger")]
    pub trigger: TriggerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_envelope: Option<TriggerEnvelope>,
    #[serde(default)]
    pub nodes: Vec<WorkflowRunNode>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunNode {
    pub node_id: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_mappings: Vec<WorkflowInputMapping>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<WorkflowRunNodeCompensation>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowInputMapping {
    pub from_node: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from_path: String,
    pub to_path: String,
    #[serde(default, skip_serializing_if = "WorkflowInputTransform::is_none")]
    pub transform: WorkflowInputTransform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInputTransform {
    #[default]
    None,
    String,
    Number,
    Integer,
    Boolean,
    JsonString,
}

impl WorkflowInputTransform {
    pub fn is_none(&self) -> bool {
        matches!(self, WorkflowInputTransform::None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunNodeCompensation {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunResult {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub workflow_id: String,
    pub status: AgentRunStatus,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub finished_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_run_id: Option<RunId>,
    #[serde(default)]
    pub nodes: Vec<WorkflowRunNodeResult>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunNodeResult {
    pub node_id: String,
    pub agent_id: String,
    pub status: AgentRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentErrorRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<AgentTrace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<WorkflowRunNodeCompensationResult>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunNodeCompensationResult {
    pub agent_id: String,
    pub status: AgentRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentErrorRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<AgentTrace>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Running,
    Completed,
    Skipped,
    Failed,
    Cancelled,
    TimedOut,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentRunResult {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub run_id: RunId,
    pub agent_id: String,
    pub status: AgentRunStatus,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub finished_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentErrorRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<RunWorkflow>,
}

impl AgentRunResult {
    pub fn completed(
        run_id: RunId,
        agent_id: impl Into<String>,
        started_at: OffsetDateTime,
        output: Value,
        summary: Option<String>,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id,
            agent_id: agent_id.into(),
            status: AgentRunStatus::Completed,
            started_at,
            finished_at: OffsetDateTime::now_utc(),
            summary,
            output,
            error: None,
            workflow: None,
        }
    }

    pub fn skipped(
        run_id: RunId,
        agent_id: impl Into<String>,
        started_at: OffsetDateTime,
        reason: Option<String>,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id,
            agent_id: agent_id.into(),
            status: AgentRunStatus::Skipped,
            started_at,
            finished_at: OffsetDateTime::now_utc(),
            summary: reason,
            output: json!({}),
            error: None,
            workflow: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentRunRecord {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub agent_id: String,
    pub status: AgentRunStatus,
    pub scope: RunScope,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[schemars(with = "Option<String>")]
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentErrorRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<RunWorkflow>,
    #[serde(default)]
    pub metadata: Value,
}

impl AgentRunRecord {
    pub fn cancellation_requested(&self) -> bool {
        self.metadata
            .get("control")
            .and_then(|control| control.get("cancel_requested"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn request_cancellation(
        &mut self,
        requested_at: OffsetDateTime,
        requested_by: impl Into<Option<String>>,
    ) {
        let metadata = ensure_object(&mut self.metadata);
        let control = metadata
            .entry("control".to_owned())
            .or_insert_with(|| json!({}));
        let control = ensure_object(control);
        control.insert("cancel_requested".to_owned(), Value::Bool(true));
        control.insert(
            "cancel_requested_at".to_owned(),
            Value::String(format_rfc3339(requested_at)),
        );
        if let Some(requested_by) = requested_by.into() {
            control.insert(
                "cancel_requested_by".to_owned(),
                Value::String(requested_by),
            );
        }
    }

    pub fn merge_control_metadata_from(&mut self, other: &AgentRunRecord) {
        let Some(control) = other.metadata.get("control").cloned() else {
            return;
        };
        let metadata = ensure_object(&mut self.metadata);
        metadata.insert("control".to_owned(), control);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunLease {
    pub key: String,
    pub owner: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub acquired_at: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum RunScope {
    Global,
    User(String),
    Tenant(String),
}

fn default_trigger() -> TriggerKind {
    TriggerKind::Manual
}

fn ensure_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value
        .as_object_mut()
        .expect("metadata was normalized to an object")
}

fn format_rfc3339(value: OffsetDateTime) -> String {
    value
        .format(&Rfc3339)
        .unwrap_or_else(|_| value.unix_timestamp().to_string())
}
