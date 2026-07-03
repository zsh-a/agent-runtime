use miette::{Result, miette};
use serde_json::Value;

use super::{
    chat::{ChatApprovalDecision, resume_chat_approval},
    data::{
        TuiActivityItem, TuiActivityKind, TuiPendingApproval, TuiPendingApprovalAction, TuiState,
    },
    format::{compact_json, pretty_json},
    policy::TuiToolRisk,
    runtime::TuiRuntime,
};

pub(super) async fn call_tool_or_request_approval(
    state: &mut TuiState,
    name: String,
    input: Value,
) -> Result<()> {
    let runtime = TuiRuntime::load(&state.options).await?;
    let decision = runtime.tool_policy_decision(&name);
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Policy,
        "tool policy",
        format!(
            "{name} risk={} allowed={}",
            decision.risk.label(),
            decision.allowed
        ),
    ));
    if !decision.allowed {
        return Err(miette!(
            "tool '{name}' is blocked by the current TUI tool policy"
        ));
    }
    if decision.risk == TuiToolRisk::High {
        request_slash_tool_approval(state, name, decision.risk, input)?;
        return Ok(());
    }
    let output = runtime.call_tool(&name, input).await?;
    state.push_tool_message(Some(name), pretty_json(&output));
    Ok(())
}

pub(super) async fn approve_pending_tool(state: &mut TuiState) -> Result<()> {
    let approval = state
        .take_pending_approval()
        .ok_or_else(|| miette!("no pending high-risk tool call to approve"))?;
    let summary = approval.summary();
    state.push_user_message("/approve");
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Approval,
        "approval granted",
        summary,
    ));
    if let Err(error) = approve_pending_approval(state, approval.clone()).await {
        restore_pending_approval(state, approval, "approve failed");
        return Err(error);
    }
    Ok(())
}

pub(super) async fn deny_pending_tool(state: &mut TuiState) -> Result<()> {
    match state.take_pending_approval() {
        Some(approval) => {
            let summary = approval.summary();
            state.push_user_message("/deny");
            state.push_activity(TuiActivityItem::with_detail(
                TuiActivityKind::Approval,
                "approval denied",
                summary,
            ));
            if let Err(error) = deny_pending_approval(state, approval.clone()).await {
                restore_pending_approval(state, approval, "deny failed");
                return Err(error);
            }
        }
        None => state.push_system_message("No pending high-risk tool call to deny."),
    }
    Ok(())
}

fn request_slash_tool_approval(
    state: &mut TuiState,
    tool_name: String,
    risk: TuiToolRisk,
    input: Value,
) -> Result<()> {
    if state.pending_approval.is_some() {
        return Err(miette!(
            "a high-risk tool call is already pending approval; use /approve or /deny"
        ));
    }
    let input_preview = compact_json(&input);
    state.set_pending_approval(TuiPendingApproval::tool_call(
        tool_name.clone(),
        risk,
        input,
    ));
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Approval,
        "approval required",
        format!("{} ({})", tool_name, risk.label()),
    ));
    state.push_system_message(format!(
        "Approval required for high-risk tool '{tool_name}'. Use /approve to run or /deny to cancel.\nInput: {input_preview}"
    ));
    Ok(())
}

async fn approve_pending_approval(
    state: &mut TuiState,
    approval: TuiPendingApproval,
) -> Result<()> {
    let risk = approval.risk.clone();
    match approval.action {
        TuiPendingApprovalAction::SlashTool { tool_name, input } => {
            approve_slash_tool(state, tool_name, input).await?;
        }
        TuiPendingApprovalAction::ChatTools {
            agent_id,
            state: chat_state,
            tool_calls,
            surface_messages,
        } => {
            resume_chat_approval(
                state,
                ChatApprovalDecision::Approve,
                agent_id,
                chat_state,
                tool_calls,
                surface_messages,
            )
            .await?;
            state.push_activity(TuiActivityItem::with_detail(
                TuiActivityKind::Approval,
                "approved chat tools resumed",
                risk.label(),
            ));
        }
    };
    Ok(())
}

async fn deny_pending_approval(state: &mut TuiState, approval: TuiPendingApproval) -> Result<()> {
    match approval.action {
        TuiPendingApprovalAction::SlashTool { tool_name, .. } => {
            state.push_system_message(format!("Denied high-risk tool '{tool_name}'."));
        }
        TuiPendingApprovalAction::ChatTools {
            agent_id,
            state: chat_state,
            tool_calls,
            surface_messages,
        } => {
            resume_chat_approval(
                state,
                ChatApprovalDecision::Deny,
                agent_id,
                chat_state,
                tool_calls,
                surface_messages,
            )
            .await?;
        }
    }
    Ok(())
}

fn restore_pending_approval(
    state: &mut TuiState,
    approval: TuiPendingApproval,
    reason: &'static str,
) {
    let summary = approval.summary();
    state.set_pending_approval(approval);
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Approval,
        "approval still pending",
        format!("{summary}; {reason}"),
    ));
}

async fn approve_slash_tool(state: &mut TuiState, tool_name: String, input: Value) -> Result<()> {
    let runtime = TuiRuntime::load(&state.options).await?;
    let decision = runtime.tool_policy_decision(&tool_name);
    if !decision.allowed {
        return Err(miette!(
            "tool '{}' is blocked by the current TUI tool policy",
            tool_name
        ));
    }
    let output = runtime.call_tool(&tool_name, input).await?;
    state.push_tool_message(Some(tool_name), pretty_json(&output));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::test_state_with_policy;
    use agent_runtime::AGENT_RUN_TOOL_NAME;
    use serde_json::json;

    #[tokio::test]
    async fn approve_failure_restores_pending_tool_call() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state_with_policy(&dir, "mock response", false).await;
        state.set_pending_approval(TuiPendingApproval::tool_call(
            AGENT_RUN_TOOL_NAME,
            TuiToolRisk::High,
            json!({
                "agent_id": "echo_agent",
                "input": {"message": "retry later"}
            }),
        ));

        let error = approve_pending_tool(&mut state)
            .await
            .expect_err("approval fails under deny policy");

        assert!(
            error
                .to_string()
                .contains("blocked by the current TUI tool policy")
        );
        assert!(state.pending_approval.is_some());
        assert_eq!(
            state
                .pending_approval
                .as_ref()
                .map(TuiPendingApproval::summary),
            Some("agent.run (high)".to_owned())
        );
        assert!(state.activity.iter().any(|activity| {
            activity.kind == TuiActivityKind::Approval && activity.title == "approval still pending"
        }));
    }
}
