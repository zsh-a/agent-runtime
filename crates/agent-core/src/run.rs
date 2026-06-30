use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::{AgentErrorRecord, PROTOCOL_VERSION, RunId, protocol_version};

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
    #[serde(default = "default_trigger")]
    pub trigger: TriggerKind,
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
    #[serde(default)]
    pub metadata: Value,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum RunScope {
    Global,
    User(String),
    Tenant(String),
}

fn default_trigger() -> TriggerKind {
    TriggerKind::Manual
}
