use super::*;
use crate::tui::{
    data::{
        TuiAgentSummary, TuiApprovalSelection, TuiPendingApproval, TuiProposalListSummary,
        TuiProposalSummary,
    },
    policy::TuiToolRisk,
    test_support::test_state,
};
use agent_core::{AgentRunRecord, AgentRunStatus, PROTOCOL_VERSION, RunId, RunScope};
use serde_json::json;
use time::OffsetDateTime;

#[path = "terminal/approval.rs"]
mod approval;
#[path = "terminal/completion.rs"]
mod completion;
#[path = "terminal/mouse.rs"]
mod mouse;

async fn apply_updates_until_idle(
    state: &mut TuiState,
    receiver: &mut UnboundedReceiver<TuiUpdate>,
) {
    loop {
        let update = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
            .await
            .expect("update arrives")
            .expect("update exists");
        state.apply_update(update);
        if !state.busy {
            break;
        }
    }
}

fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn test_run(run_id: &str, status: AgentRunStatus) -> AgentRunRecord {
    let now = OffsetDateTime::now_utc();
    AgentRunRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        version: 1,
        run_id: RunId(run_id.to_owned()),
        idempotency_key: None,
        agent_id: "echo_agent".to_owned(),
        status: status.clone(),
        scope: RunScope::Global,
        started_at: now,
        finished_at: (status != AgentRunStatus::Running).then_some(now),
        input: json!({}),
        output: json!({}),
        error: None,
        workflow: None,
        metadata: json!({}),
    }
}
