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
    #[serde(default)]
    pub required_approval_level: ApprovalLevel,
    #[serde(default = "default_required_approver_count")]
    pub required_approver_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_after_seconds: Option<u64>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalLevel {
    None,
    #[default]
    SingleUser,
    MultiApprover,
    Admin,
}

impl ApprovalLevel {
    pub fn satisfies(self, required: Self) -> bool {
        self.rank() >= required.rank()
    }

    fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::SingleUser => 1,
            Self::MultiApprover => 2,
            Self::Admin => 3,
        }
    }
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
    #[serde(default)]
    pub version: u64,
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
    #[serde(default)]
    pub required_approval_level: ApprovalLevel,
    #[serde(default = "default_required_approver_count")]
    pub required_approver_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_decisions: Vec<ApprovalDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<ProposalDiff>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ProposalWarning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_version: Option<String>,
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
            version: 0,
            run_id,
            agent_id: agent_id.into(),
            kind: kind.into(),
            summary: summary.into(),
            payload,
            risk: default_proposal_risk(),
            approval_policy: ProposalApprovalPolicy::Manual,
            approval_required: true,
            required_approval_level: ApprovalLevel::SingleUser,
            required_approver_count: default_required_approver_count(),
            approval_decisions: Vec::new(),
            diffs: Vec::new(),
            warnings: Vec::new(),
            policy_id: None,
            policy_version: None,
            status: ProposalStatus::PendingApproval,
            created_at: OffsetDateTime::now_utc(),
            expires_at: None,
        }
    }

    pub fn with_kind_policy(mut self, kind: &ProposalKindSpec) -> Self {
        self.risk = kind.risk.clone();
        self.approval_policy = kind.approval_policy;
        self.approval_required = kind.approval_required();
        self.required_approval_level = if self.approval_required {
            kind.required_approval_level
        } else {
            ApprovalLevel::None
        };
        self.required_approver_count = if self.approval_required {
            normalized_required_approver_count(
                self.required_approval_level,
                kind.required_approver_count,
            )
        } else {
            0
        };
        self.policy_id = kind.policy_id.clone();
        self.policy_version = kind.policy_version.clone();
        if let Some(seconds) = kind.expires_after_seconds {
            let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
            self.expires_at = Some(self.created_at + time::Duration::seconds(seconds));
        }
        if !self.approval_required && self.status == ProposalStatus::PendingApproval {
            self.status = ProposalStatus::Approved;
        }
        self
    }

    pub fn is_expired_at(&self, now: OffsetDateTime) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    pub fn mark_expired_if_needed(&mut self, now: OffsetDateTime) -> bool {
        if self.is_expirable() && self.is_expired_at(now) {
            self.status = ProposalStatus::Expired;
            return true;
        }
        false
    }

    fn is_expirable(&self) -> bool {
        matches!(
            self.status,
            ProposalStatus::Created | ProposalStatus::PendingApproval | ProposalStatus::Approved
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposalDiff {
    pub path: String,
    #[serde(default)]
    pub operation: ProposalDiffOperation,
    #[serde(default)]
    pub before: Value,
    #[serde(default)]
    pub after: Value,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProposalDiffOperation {
    Add,
    Remove,
    #[default]
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposalWarning {
    #[serde(default)]
    pub severity: ProposalWarningSeverity,
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProposalWarningSeverity {
    Info,
    #[default]
    Warning,
    Danger,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalDecision {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub proposal_id: ProposalId,
    pub decision: ApprovalDecisionKind,
    #[serde(default)]
    pub approval_level: ApprovalLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

fn default_required_approver_count() -> u32 {
    1
}

pub fn normalized_required_approver_count(level: ApprovalLevel, count: u32) -> u32 {
    match level {
        ApprovalLevel::None => 0,
        ApprovalLevel::MultiApprover => count.max(2),
        ApprovalLevel::SingleUser | ApprovalLevel::Admin => count.max(1),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn proposal_kind(
        approval_policy: ProposalApprovalPolicy,
        required_approval_level: ApprovalLevel,
    ) -> ProposalKindSpec {
        ProposalKindSpec {
            kind: "fake".to_owned(),
            tool_name: "propose_fake".to_owned(),
            risk: ToolRisk::High,
            approval_policy,
            required_approval_level,
            required_approver_count: 1,
            policy_id: Some("finance.proposal.default".to_owned()),
            policy_version: Some("2026-06-28".to_owned()),
            expires_after_seconds: Some(60),
        }
    }

    #[test]
    fn with_kind_policy_records_manual_approval_policy() {
        let proposal = ProposalEnvelope::new(
            RunId("run_test".to_owned()),
            "execution_review",
            "fake",
            "Review fake proposal",
            json!({"value": 7}),
        );
        let created_at = proposal.created_at;
        let proposal = proposal.with_kind_policy(&proposal_kind(
            ProposalApprovalPolicy::Manual,
            ApprovalLevel::Admin,
        ));

        assert_eq!(proposal.risk, ToolRisk::High);
        assert_eq!(proposal.approval_policy, ProposalApprovalPolicy::Manual);
        assert!(proposal.approval_required);
        assert_eq!(proposal.required_approval_level, ApprovalLevel::Admin);
        assert_eq!(proposal.required_approver_count, 1);
        assert_eq!(
            proposal.policy_id.as_deref(),
            Some("finance.proposal.default")
        );
        assert_eq!(proposal.policy_version.as_deref(), Some("2026-06-28"));
        assert_eq!(
            proposal.expires_at,
            Some(created_at + time::Duration::seconds(60))
        );
        assert_eq!(proposal.status, ProposalStatus::PendingApproval);
    }

    #[test]
    fn with_kind_policy_auto_approve_clears_required_approval_level() {
        let proposal = ProposalEnvelope::new(
            RunId("run_test".to_owned()),
            "execution_review",
            "fake",
            "Auto fake proposal",
            json!({"value": 7}),
        )
        .with_kind_policy(&proposal_kind(
            ProposalApprovalPolicy::AutoApprove,
            ApprovalLevel::Admin,
        ));

        assert_eq!(
            proposal.approval_policy,
            ProposalApprovalPolicy::AutoApprove
        );
        assert!(!proposal.approval_required);
        assert_eq!(proposal.required_approval_level, ApprovalLevel::None);
        assert_eq!(proposal.required_approver_count, 0);
        assert_eq!(proposal.status, ProposalStatus::Approved);
    }

    #[test]
    fn with_kind_policy_normalizes_multi_approver_count() {
        let proposal = ProposalEnvelope::new(
            RunId("run_test".to_owned()),
            "execution_review",
            "fake",
            "Multi fake proposal",
            json!({"value": 7}),
        )
        .with_kind_policy(&proposal_kind(
            ProposalApprovalPolicy::Manual,
            ApprovalLevel::MultiApprover,
        ));

        assert_eq!(
            proposal.required_approval_level,
            ApprovalLevel::MultiApprover
        );
        assert_eq!(proposal.required_approver_count, 2);
    }

    #[test]
    fn mark_expired_if_needed_only_expires_open_proposals() {
        let now = OffsetDateTime::now_utc();
        let mut proposal = ProposalEnvelope::new(
            RunId("run_test".to_owned()),
            "execution_review",
            "fake",
            "Expired fake proposal",
            json!({}),
        );
        proposal.status = ProposalStatus::Approved;
        proposal.expires_at = Some(now - time::Duration::seconds(1));

        assert!(proposal.mark_expired_if_needed(now));
        assert_eq!(proposal.status, ProposalStatus::Expired);

        let mut applied = ProposalEnvelope::new(
            RunId("run_test".to_owned()),
            "execution_review",
            "fake",
            "Applied fake proposal",
            json!({}),
        );
        applied.status = ProposalStatus::Applied;
        applied.expires_at = Some(now - time::Duration::seconds(1));

        assert!(!applied.mark_expired_if_needed(now));
        assert_eq!(applied.status, ProposalStatus::Applied);
    }

    #[test]
    fn approval_level_satisfies_required_level_hierarchy() {
        assert!(ApprovalLevel::SingleUser.satisfies(ApprovalLevel::None));
        assert!(ApprovalLevel::MultiApprover.satisfies(ApprovalLevel::SingleUser));
        assert!(ApprovalLevel::Admin.satisfies(ApprovalLevel::MultiApprover));
        assert!(!ApprovalLevel::SingleUser.satisfies(ApprovalLevel::MultiApprover));
        assert!(!ApprovalLevel::None.satisfies(ApprovalLevel::SingleUser));
    }
}
