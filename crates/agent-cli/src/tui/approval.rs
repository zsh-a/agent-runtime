use miette::{Result, miette};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::{
    chat::{
        ChatApprovalDecision, TuiTaskHandle, resume_chat_approval, resume_chat_approval_with_emit,
    },
    data::{
        TuiActivityItem, TuiActivityKind, TuiApprovalSelection, TuiOptions, TuiPendingApproval,
        TuiPendingApprovalAction, TuiState, TuiUpdate,
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
    approve_pending_tool_with_display(state, "/approve").await
}

pub(super) fn start_pending_approval_task(
    state: &mut TuiState,
    selection: TuiApprovalSelection,
    display: impl Into<String>,
    sender: UnboundedSender<TuiUpdate>,
) -> Result<TuiTaskHandle> {
    let approval = state
        .take_pending_approval()
        .ok_or_else(|| miette!("no pending high-risk tool call to decide"))?;
    let summary = approval.summary();
    state.push_user_message(display.into());
    state.push_activity(TuiActivityItem::with_detail(
        TuiActivityKind::Approval,
        match selection {
            TuiApprovalSelection::Approve => "approval granted",
            TuiApprovalSelection::Deny => "approval denied",
        },
        summary.clone(),
    ));
    state.set_busy(true);

    let options = state.options.clone();
    let cancellation = CancellationToken::new();
    let join = tokio::spawn({
        let cancellation = cancellation.clone();
        let sender = sender.clone();
        async move {
            if let Err(error) = run_pending_approval_task(
                options,
                approval.clone(),
                selection,
                sender.clone(),
                cancellation.clone(),
            )
            .await
            {
                let _ = sender.send(TuiUpdate::PendingApproval(Some(approval)));
                let _ = sender.send(TuiUpdate::Activity(TuiActivityItem::with_detail(
                    TuiActivityKind::Approval,
                    "approval still pending",
                    format!("{summary}; decision failed"),
                )));
                let _ = sender.send(TuiUpdate::SystemMessage(format!(
                    "Approval failed: {error}"
                )));
                let _ = sender.send(TuiUpdate::Activity(TuiActivityItem::with_detail(
                    TuiActivityKind::Error,
                    "approval failed",
                    error.to_string(),
                )));
            }
            let _ = sender.send(TuiUpdate::Busy(false));
        }
    });

    Ok(TuiTaskHandle { join, cancellation })
}

pub(super) async fn approve_pending_tool_with_display(
    state: &mut TuiState,
    display: impl Into<String>,
) -> Result<()> {
    let approval = state
        .take_pending_approval()
        .ok_or_else(|| miette!("no pending high-risk tool call to approve"))?;
    let summary = approval.summary();
    state.push_user_message(display.into());
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
    deny_pending_tool_with_display(state, "/deny").await
}

pub(super) async fn deny_pending_tool_with_display(
    state: &mut TuiState,
    display: impl Into<String>,
) -> Result<()> {
    match state.take_pending_approval() {
        Some(approval) => {
            let summary = approval.summary();
            state.push_user_message(display.into());
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
            "a high-risk tool call is already pending approval; use the approval card or type yes/no"
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
        "Approval required for high-risk tool '{tool_name}'. Use the approval card: Tab selects, Enter confirms. You can also type yes/no.\nInput: {input_preview}"
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

async fn run_pending_approval_task(
    options: TuiOptions,
    approval: TuiPendingApproval,
    selection: TuiApprovalSelection,
    sender: UnboundedSender<TuiUpdate>,
    cancellation: CancellationToken,
) -> Result<()> {
    let risk = approval.risk.clone();
    match (selection, approval.action) {
        (
            TuiApprovalSelection::Approve,
            TuiPendingApprovalAction::SlashTool { tool_name, input },
        ) => {
            approve_slash_tool_with_updates(options, tool_name, input, sender, cancellation)
                .await?;
        }
        (TuiApprovalSelection::Deny, TuiPendingApprovalAction::SlashTool { tool_name, .. }) => {
            let _ = sender.send(TuiUpdate::SystemMessage(format!(
                "Denied high-risk tool '{tool_name}'."
            )));
        }
        (
            TuiApprovalSelection::Approve,
            TuiPendingApprovalAction::ChatTools {
                agent_id,
                state: chat_state,
                tool_calls,
                surface_messages,
            },
        ) => {
            let sender_for_emit = sender.clone();
            let mut emit = move |update| {
                let _ = sender_for_emit.send(update);
            };
            resume_chat_approval_with_emit(
                options,
                ChatApprovalDecision::Approve,
                agent_id,
                chat_state,
                tool_calls,
                surface_messages,
                cancellation,
                &mut emit,
            )
            .await?;
            let _ = sender.send(TuiUpdate::Activity(TuiActivityItem::with_detail(
                TuiActivityKind::Approval,
                "approved chat tools resumed",
                risk.label(),
            )));
        }
        (
            TuiApprovalSelection::Deny,
            TuiPendingApprovalAction::ChatTools {
                agent_id,
                state: chat_state,
                tool_calls,
                surface_messages,
            },
        ) => {
            let sender_for_emit = sender.clone();
            let mut emit = move |update| {
                let _ = sender_for_emit.send(update);
            };
            resume_chat_approval_with_emit(
                options,
                ChatApprovalDecision::Deny,
                agent_id,
                chat_state,
                tool_calls,
                surface_messages,
                cancellation,
                &mut emit,
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

async fn approve_slash_tool_with_updates(
    options: TuiOptions,
    tool_name: String,
    input: Value,
    sender: UnboundedSender<TuiUpdate>,
    cancellation: CancellationToken,
) -> Result<()> {
    let runtime = TuiRuntime::load_with_cancellation(&options, cancellation.clone()).await?;
    let decision = runtime.tool_policy_decision(&tool_name);
    if !decision.allowed {
        return Err(miette!(
            "tool '{}' is blocked by the current TUI tool policy",
            tool_name
        ));
    }
    let services = runtime.tool_services(None);
    let output = services
        .call_tool_with_cancellation(&tool_name, input, cancellation)
        .await
        .map_err(|err| miette!(err.record.message))?;
    let _ = sender.send(TuiUpdate::ToolMessage {
        title: Some(tool_name),
        content: pretty_json(&output),
    });
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
