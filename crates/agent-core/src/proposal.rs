use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{PROTOCOL_VERSION, ProposalId, RunId, ToolRisk, protocol_version};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposalKindSpec {
    pub kind: String,
    pub tool_name: String,
    #[serde(default = "default_proposal_risk")]
    pub risk: ToolRisk,
    #[serde(default)]
    pub approval_policy: ProposalApprovalPolicy,
}

impl ProposalKindSpec {
    pub fn approval_required(&self) -> bool {
        matches!(self.approval_policy, ProposalApprovalPolicy::Manual)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProposalApprovalPolicy {
    #[default]
    Manual,
    AutoApprove,
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
    #[serde(default = "default_proposal_risk")]
    pub risk: ToolRisk,
    #[serde(default)]
    pub approval_policy: ProposalApprovalPolicy,
    #[serde(default = "default_approval_required")]
    pub approval_required: bool,
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
            risk: default_proposal_risk(),
            approval_policy: ProposalApprovalPolicy::Manual,
            approval_required: true,
            status: ProposalStatus::PendingApproval,
            created_at: OffsetDateTime::now_utc(),
            expires_at: None,
        }
    }

    pub fn with_kind_policy(mut self, kind: &ProposalKindSpec) -> Self {
        self.risk = kind.risk.clone();
        self.approval_policy = kind.approval_policy;
        self.approval_required = kind.approval_required();
        if !self.approval_required && self.status == ProposalStatus::PendingApproval {
            self.status = ProposalStatus::Approved;
        }
        self
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

fn default_proposal_risk() -> ToolRisk {
    ToolRisk::Medium
}

fn default_approval_required() -> bool {
    true
}
