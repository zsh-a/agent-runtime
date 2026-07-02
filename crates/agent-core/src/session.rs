use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{PROTOCOL_VERSION, RunId, SessionId, StepId, ThreadId, protocol_version};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionRecord {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub session_id: SessionId,
    pub title: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(default)]
    pub metadata: Value,
}

impl SessionRecord {
    pub fn new(title: impl Into<String>, metadata: Value) -> Self {
        let now = OffsetDateTime::now_utc();
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            session_id: SessionId::new_v7(),
            title: title.into(),
            created_at: now,
            updated_at: now,
            metadata,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ThreadRecord {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub thread_id: ThreadId,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default)]
    pub metadata: Value,
}

impl ThreadRecord {
    pub fn root(session_id: SessionId, title: Option<String>, metadata: Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            thread_id: ThreadId::new_v7(),
            session_id,
            parent_thread_id: None,
            title,
            created_at: OffsetDateTime::now_utc(),
            metadata,
        }
    }

    pub fn fork(
        session_id: SessionId,
        parent_thread_id: ThreadId,
        title: Option<String>,
        metadata: Value,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            thread_id: ThreadId::new_v7(),
            session_id,
            parent_thread_id: Some(parent_thread_id),
            title,
            created_at: OffsetDateTime::now_utc(),
            metadata,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    AgentRun,
    LlmRound,
    ToolCall,
    Proposal,
    Approval,
    StateUpdate,
    Note,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StepRecord {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub step_id: StepId,
    pub thread_id: ThreadId,
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub payload: Value,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl StepRecord {
    pub fn new(
        thread_id: ThreadId,
        kind: StepKind,
        run_id: Option<RunId>,
        summary: Option<String>,
        payload: Value,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            step_id: StepId::new_v7(),
            thread_id,
            kind,
            run_id,
            summary,
            payload,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    pub fn agent_run(
        thread_id: ThreadId,
        run_id: RunId,
        summary: Option<String>,
        payload: Value,
    ) -> Self {
        Self::new(
            thread_id,
            StepKind::AgentRun,
            Some(run_id),
            summary,
            payload,
        )
    }
}
