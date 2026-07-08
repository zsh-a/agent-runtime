use agent_core::{
    AgentError, AgentProposalStore, AgentRuntimeCatalog, AgentServices, AgentTraceStore,
    ApprovalDecision, ApprovalDecisionKind, ApprovalLevel, HookEventName, PROTOCOL_VERSION,
    ProposalEnvelope, ProposalKindSpec, ProposalStatus, RunId, TraceEvent, TraceSink,
    normalized_required_approver_count,
};
use agent_runtime::HookManager;
use async_trait::async_trait;
use miette::{IntoDiagnostic, Result, miette};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, miette::Diagnostic, thiserror::Error)]
#[error("{message}")]
pub(crate) struct PolicyDeniedError {
    pub(crate) message: String,
    pub(crate) details: Value,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProposalDecisionResponse {
    pub(crate) decision: ApprovalDecision,
    pub(crate) proposal: ProposalEnvelope,
}

#[derive(Debug)]
pub(crate) struct ProposalDecisionInput {
    pub(crate) decision: ApprovalDecisionKind,
    pub(crate) approval_level: Option<ApprovalLevel>,
    pub(crate) decided_by: Option<String>,
    pub(crate) comment: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProposalActionResponse {
    pub(crate) action: String,
    pub(crate) tool: String,
    pub(crate) tool_output: Value,
    pub(crate) proposal: ProposalEnvelope,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ProposalAction {
    Apply,
    Undo,
}

impl ProposalAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Undo => "undo",
        }
    }

    fn required_status(self) -> ProposalStatus {
        match self {
            Self::Apply => ProposalStatus::Approved,
            Self::Undo => ProposalStatus::Applied,
        }
    }

    fn in_progress_status(self) -> ProposalStatus {
        match self {
            Self::Apply => ProposalStatus::Applying,
            Self::Undo => ProposalStatus::Undoing,
        }
    }

    fn success_status(self) -> ProposalStatus {
        match self {
            Self::Apply => ProposalStatus::Applied,
            Self::Undo => ProposalStatus::Undone,
        }
    }

    fn failure_status(self) -> ProposalStatus {
        match self {
            Self::Apply => ProposalStatus::ApplyFailed,
            Self::Undo => ProposalStatus::UndoFailed,
        }
    }
}

pub(crate) async fn decide_proposal_with_store(
    store: &dyn AgentProposalStore,
    proposal: &mut ProposalEnvelope,
    input: ProposalDecisionInput,
) -> Result<ProposalDecisionResponse> {
    let now = time::OffsetDateTime::now_utc();
    if proposal.mark_expired_if_needed(now) {
        store
            .update_proposal(proposal.clone())
            .await
            .into_diagnostic()?;
        return Err(miette!(
            "proposal '{}' expired before decision and was marked expired",
            proposal.proposal_id.0
        ));
    }

    if !matches!(
        proposal.status,
        ProposalStatus::Created | ProposalStatus::PendingApproval
    ) {
        return Err(miette!(
            "proposal '{}' must be pending approval before decision but is {:?}",
            proposal.proposal_id.0,
            proposal.status
        ));
    }

    proposal.required_approver_count = normalized_required_approver_count(
        proposal.required_approval_level,
        proposal.required_approver_count,
    );
    let approval_level = input.approval_level.unwrap_or_else(|| {
        if proposal.approval_required {
            ApprovalLevel::SingleUser
        } else {
            ApprovalLevel::None
        }
    });
    let decision = ApprovalDecision {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        proposal_id: proposal.proposal_id.clone(),
        decision: input.decision,
        approval_level,
        decided_by: input.decided_by,
        decided_at: now,
        comment: input.comment,
    };
    apply_approval_decision(proposal, decision.clone())?;
    store
        .update_proposal(proposal.clone())
        .await
        .into_diagnostic()?;

    Ok(ProposalDecisionResponse {
        decision,
        proposal: proposal.clone(),
    })
}

fn apply_approval_decision(
    proposal: &mut ProposalEnvelope,
    decision: ApprovalDecision,
) -> Result<()> {
    match decision.decision {
        ApprovalDecisionKind::Deny => {
            proposal.approval_decisions.push(decision);
            proposal.status = ProposalStatus::Denied;
            Ok(())
        }
        ApprovalDecisionKind::Approve => apply_approval(proposal, decision),
    }
}

fn apply_approval(proposal: &mut ProposalEnvelope, decision: ApprovalDecision) -> Result<()> {
    if proposal.required_approval_level == ApprovalLevel::MultiApprover
        && !decision
            .approval_level
            .satisfies(ApprovalLevel::MultiApprover)
    {
        if !decision.approval_level.satisfies(ApprovalLevel::SingleUser) {
            return Err(insufficient_approval_level_error(
                proposal,
                decision.approval_level,
            ));
        }
        let actor = decision.decided_by.as_deref().ok_or_else(|| {
            miette!(
                "multi-approver proposal '{}' requires decided_by for single-user approvals",
                proposal.proposal_id.0
            )
        })?;
        if proposal.approval_decisions.iter().any(|existing| {
            existing.decision == ApprovalDecisionKind::Approve
                && existing.decided_by.as_deref() == Some(actor)
        }) {
            return Err(miette!(
                "actor '{}' has already approved proposal '{}'",
                actor,
                proposal.proposal_id.0
            ));
        }
        proposal.approval_decisions.push(decision);
        let approved_count = distinct_approver_count(&proposal.approval_decisions);
        proposal.status = if approved_count >= proposal.required_approver_count as usize {
            ProposalStatus::Approved
        } else {
            ProposalStatus::PendingApproval
        };
        return Ok(());
    }

    if !decision
        .approval_level
        .satisfies(proposal.required_approval_level)
    {
        return Err(insufficient_approval_level_error(
            proposal,
            decision.approval_level,
        ));
    }
    proposal.approval_decisions.push(decision);
    proposal.status = ProposalStatus::Approved;
    Ok(())
}

fn insufficient_approval_level_error(
    proposal: &ProposalEnvelope,
    approval_level: ApprovalLevel,
) -> miette::Report {
    miette!(
        "approval level {:?} does not satisfy required level {:?} for proposal '{}'",
        approval_level,
        proposal.required_approval_level,
        proposal.proposal_id.0
    )
}

fn distinct_approver_count(decisions: &[ApprovalDecision]) -> usize {
    let mut actors = std::collections::BTreeSet::new();
    for decision in decisions {
        if decision.decision == ApprovalDecisionKind::Approve
            && let Some(actor) = decision.decided_by.as_deref()
        {
            actors.insert(actor.to_owned());
        }
    }
    actors.len()
}

pub(crate) async fn authorize_proposal_apply_policy(
    hooks: &HookManager,
    trace_store: &dyn AgentTraceStore,
    proposal: &ProposalEnvelope,
    tool: &str,
    action: ProposalAction,
) -> Result<()> {
    if !matches!(action, ProposalAction::Apply)
        || proposal.status != ProposalStatus::Approved
        || proposal.is_expired_at(time::OffsetDateTime::now_utc())
    {
        return Ok(());
    }

    let trace = StoreTraceSink {
        trace_store,
        run_id: proposal.run_id.clone(),
    };
    let input = json!({
        "run_id": proposal.run_id.0.clone(),
        "agent_id": proposal.agent_id.clone(),
        "proposal_id": proposal.proposal_id.0.clone(),
        "kind": proposal.kind.clone(),
        "action": action.as_str(),
        "tool": tool,
        "proposal": proposal.clone(),
    });
    let decision = hooks
        .authorize(
            HookEventName::BeforeProposalApply,
            Some(proposal.run_id.clone()),
            Some(proposal.agent_id.clone()),
            input.clone(),
            &trace,
        )
        .await
        .into_diagnostic()?;
    if decision.is_denied() {
        let message = decision.reason.clone().unwrap_or_else(|| {
            format!(
                "proposal '{}' apply denied by policy hook",
                proposal.proposal_id.0
            )
        });
        return Err(PolicyDeniedError {
            message,
            details: json!({
                "decision": decision,
                "event": "BeforeProposalApply",
                "proposal_id": proposal.proposal_id.0.clone(),
                "run_id": proposal.run_id.0.clone(),
                "agent_id": proposal.agent_id.clone(),
                "kind": proposal.kind.clone(),
                "action": action.as_str(),
                "tool": tool,
            }),
        }
        .into());
    }
    hooks
        .observe(
            HookEventName::BeforeProposalApply,
            Some(proposal.run_id.clone()),
            Some(proposal.agent_id.clone()),
            input,
            &trace,
        )
        .await
        .into_diagnostic()
}

pub(crate) async fn authorize_proposal_create_policy(
    hooks: &HookManager,
    trace_store: &dyn AgentTraceStore,
    proposal: &ProposalEnvelope,
) -> Result<()> {
    let trace = StoreTraceSink {
        trace_store,
        run_id: proposal.run_id.clone(),
    };
    let input = json!({
        "run_id": proposal.run_id.0.clone(),
        "agent_id": proposal.agent_id.clone(),
        "proposal_id": proposal.proposal_id.0.clone(),
        "kind": proposal.kind.clone(),
        "proposal": proposal.clone(),
    });
    let decision = hooks
        .authorize(
            HookEventName::BeforeProposalCreate,
            Some(proposal.run_id.clone()),
            Some(proposal.agent_id.clone()),
            input.clone(),
            &trace,
        )
        .await
        .into_diagnostic()?;
    if decision.is_denied() {
        let message = decision.reason.clone().unwrap_or_else(|| {
            format!(
                "proposal '{}' creation denied by policy hook",
                proposal.proposal_id.0
            )
        });
        return Err(PolicyDeniedError {
            message,
            details: json!({
                "decision": decision,
                "event": "BeforeProposalCreate",
                "proposal_id": proposal.proposal_id.0.clone(),
                "run_id": proposal.run_id.0.clone(),
                "agent_id": proposal.agent_id.clone(),
                "kind": proposal.kind.clone(),
            }),
        }
        .into());
    }
    hooks
        .observe(
            HookEventName::BeforeProposalCreate,
            Some(proposal.run_id.clone()),
            Some(proposal.agent_id.clone()),
            input,
            &trace,
        )
        .await
        .into_diagnostic()
}

pub(crate) async fn execute_proposal_action_with_store(
    store: &dyn AgentProposalStore,
    services: &dyn AgentServices,
    proposal: &mut ProposalEnvelope,
    tool: String,
    action: ProposalAction,
) -> Result<ProposalActionResponse> {
    if matches!(action, ProposalAction::Apply)
        && proposal.mark_expired_if_needed(time::OffsetDateTime::now_utc())
    {
        store
            .update_proposal(proposal.clone())
            .await
            .into_diagnostic()?;
        return Err(miette!(
            "proposal '{}' expired before apply and was marked expired",
            proposal.proposal_id.0
        ));
    }

    let required = action.required_status();
    if proposal.status != required {
        return Err(miette!(
            "proposal '{}' must be {:?} before {} but is {:?}",
            proposal.proposal_id.0,
            required,
            action.as_str(),
            proposal.status
        ));
    }

    proposal.status = action.in_progress_status();
    store
        .update_proposal(proposal.clone())
        .await
        .into_diagnostic()?;

    let tool_input = json!({
        "action": action.as_str(),
        "proposal": proposal.clone(),
    });
    let tool_output = match services.call_tool(&tool, tool_input).await {
        Ok(output) => output,
        Err(err) => {
            proposal.status = action.failure_status();
            store
                .update_proposal(proposal.clone())
                .await
                .into_diagnostic()?;
            return Err(miette!(err.record.message));
        }
    };

    proposal.status = action.success_status();
    store
        .update_proposal(proposal.clone())
        .await
        .into_diagnostic()?;
    Ok(ProposalActionResponse {
        action: action.as_str().to_owned(),
        tool,
        tool_output,
        proposal: proposal.clone(),
    })
}

struct StoreTraceSink<'a> {
    trace_store: &'a dyn AgentTraceStore,
    run_id: RunId,
}

#[async_trait]
impl TraceSink for StoreTraceSink<'_> {
    async fn emit(&self, event: TraceEvent) -> Result<(), AgentError> {
        append_store_trace_event(self.trace_store, &self.run_id, event)
            .await
            .map_err(|error| AgentError::internal(error.to_string()))
    }
}

pub(crate) fn proposal_action_tool(catalog: &AgentRuntimeCatalog, kind: &str) -> Result<String> {
    Ok(proposal_kind_spec(catalog, kind)?.tool_name.clone())
}

pub(crate) fn proposal_kind_spec<'a>(
    catalog: &'a AgentRuntimeCatalog,
    kind: &str,
) -> Result<&'a ProposalKindSpec> {
    catalog
        .proposal_kinds
        .iter()
        .find(|spec| spec.kind == kind)
        .ok_or_else(|| miette!("proposal kind '{kind}' is not present in the active catalog"))
}

pub(crate) async fn append_proposal_created_trace_event(
    trace_store: &dyn AgentTraceStore,
    proposal: &ProposalEnvelope,
) -> Result<()> {
    append_store_trace_event(
        trace_store,
        &proposal.run_id,
        TraceEvent::new(
            "proposal_created",
            json!({
                "proposal_id": proposal.proposal_id.0.clone(),
                "run_id": proposal.run_id.0.clone(),
                "agent_id": proposal.agent_id.clone(),
                "kind": proposal.kind.clone(),
                "summary": proposal.summary.clone(),
                "risk": proposal.risk.clone(),
                "approval_policy": proposal.approval_policy,
                "approval_required": proposal.approval_required,
                "required_approval_level": proposal.required_approval_level,
                "required_approver_count": proposal.required_approver_count,
                "approval_decision_count": proposal.approval_decisions.len(),
                "diff_count": proposal.diffs.len(),
                "warning_count": proposal.warnings.len(),
                "policy_id": proposal.policy_id.clone(),
                "policy_version": proposal.policy_version.clone(),
                "status": proposal.status.clone(),
            }),
        ),
    )
    .await
}

pub(crate) async fn append_proposal_decision_trace_event(
    trace_store: &dyn AgentTraceStore,
    response: &ProposalDecisionResponse,
) -> Result<()> {
    append_store_trace_event(
        trace_store,
        &response.proposal.run_id,
        TraceEvent::new(
            "proposal_decided",
            json!({
                "proposal_id": response.proposal.proposal_id.0.clone(),
                "run_id": response.proposal.run_id.0.clone(),
                "agent_id": response.proposal.agent_id.clone(),
                "kind": response.proposal.kind.clone(),
                "risk": response.proposal.risk.clone(),
                "approval_policy": response.proposal.approval_policy,
                "approval_required": response.proposal.approval_required,
                "required_approval_level": response.proposal.required_approval_level,
                "required_approver_count": response.proposal.required_approver_count,
                "approval_decision_count": response.proposal.approval_decisions.len(),
                "diff_count": response.proposal.diffs.len(),
                "warning_count": response.proposal.warnings.len(),
                "policy_id": response.proposal.policy_id.clone(),
                "policy_version": response.proposal.policy_version.clone(),
                "decision": response.decision.decision.clone(),
                "approval_level": response.decision.approval_level,
                "decided_by": response.decision.decided_by.clone(),
                "status": response.proposal.status.clone(),
                "comment": response.decision.comment.clone(),
            }),
        ),
    )
    .await
}

pub(crate) async fn append_proposal_action_trace_event(
    trace_store: &dyn AgentTraceStore,
    response: &ProposalActionResponse,
) -> Result<()> {
    let event_kind = match response.action.as_str() {
        "apply" => "proposal_applied",
        "undo" => "proposal_undone",
        _ => "proposal_action_finished",
    };
    append_store_trace_event(
        trace_store,
        &response.proposal.run_id,
        TraceEvent::new(
            event_kind,
            json!({
                "proposal_id": response.proposal.proposal_id.0.clone(),
                "run_id": response.proposal.run_id.0.clone(),
                "agent_id": response.proposal.agent_id.clone(),
                "kind": response.proposal.kind.clone(),
                "risk": response.proposal.risk.clone(),
                "approval_policy": response.proposal.approval_policy,
                "approval_required": response.proposal.approval_required,
                "required_approval_level": response.proposal.required_approval_level,
                "required_approver_count": response.proposal.required_approver_count,
                "approval_decision_count": response.proposal.approval_decisions.len(),
                "diff_count": response.proposal.diffs.len(),
                "warning_count": response.proposal.warnings.len(),
                "policy_id": response.proposal.policy_id.clone(),
                "policy_version": response.proposal.policy_version.clone(),
                "action": response.action,
                "status": response.proposal.status.clone(),
                "tool": response.tool.clone(),
                "tool_output": response.tool_output.clone(),
            }),
        ),
    )
    .await
}

pub(crate) fn parse_approval_decision(value: &str) -> Result<ApprovalDecisionKind> {
    match value {
        "approve" | "approved" => Ok(ApprovalDecisionKind::Approve),
        "deny" | "denied" => Ok(ApprovalDecisionKind::Deny),
        other => Err(miette!(
            "unsupported approval decision '{other}', expected approve or deny"
        )),
    }
}

pub(crate) fn parse_approval_level(value: &str) -> Result<ApprovalLevel> {
    match value {
        "none" => Ok(ApprovalLevel::None),
        "single_user" | "single-user" | "single" => Ok(ApprovalLevel::SingleUser),
        "multi_approver" | "multi-approver" | "multi" => Ok(ApprovalLevel::MultiApprover),
        "admin" => Ok(ApprovalLevel::Admin),
        other => Err(miette!(
            "unsupported approval level '{other}', expected none, single_user, multi_approver, or admin"
        )),
    }
}

async fn append_store_trace_event(
    trace_store: &dyn AgentTraceStore,
    run_id: &RunId,
    event: TraceEvent,
) -> Result<()> {
    let Some(mut trace) = trace_store.read_trace(run_id).await.into_diagnostic()? else {
        return Ok(());
    };
    trace.events.push(event);
    trace_store.write_trace(trace).await.into_diagnostic()
}
