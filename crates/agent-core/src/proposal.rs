use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{PROTOCOL_VERSION, ProposalId, RunId, protocol_version};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposalKindSpec {
    pub kind: String,
    pub tool_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Created,
    PendingApproval,
    Approved,
    Denied,
    Expired,
    Applying,
    Applied,
    ApplyFailed,
    Undoing,
    Undone,
    UndoFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposalEnvelope {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub proposal_id: ProposalId,
    pub run_id: RunId,
    pub agent_id: String,
    pub kind: String,
    pub summary: String,
    #[serde(default)]
    pub payload: Value,
    pub status: ProposalStatus,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[schemars(with = "Option<String>")]
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

impl ProposalEnvelope {
    pub fn new(
        run_id: RunId,
        agent_id: impl Into<String>,
        kind: impl Into<String>,
        summary: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            proposal_id: ProposalId::new_v7(),
            run_id,
            agent_id: agent_id.into(),
            kind: kind.into(),
            summary: summary.into(),
            payload,
            status: ProposalStatus::PendingApproval,
            created_at: OffsetDateTime::now_utc(),
            expires_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalDecision {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub proposal_id: ProposalId,
    pub decision: ApprovalDecisionKind,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionKind {
    Approve,
    Deny,
}
