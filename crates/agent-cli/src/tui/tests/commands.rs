use super::*;
use crate::tui::{
    data::{TranscriptRole, TuiOptions, TuiPendingApproval},
    policy::TuiToolRisk,
    test_support::{test_options, test_state},
};
use crate::{config::RuntimeStoreBackend, runtime_stores::RuntimeStores};
use agent_core::{
    AgentProposalStore, AgentRunRecord, AgentRunStore, AgentTrace, AgentTraceStore,
    PROTOCOL_VERSION, RunScope, ToolRisk, ToolSpec, TraceEvent,
};
use agent_store::{FileProposalStore, FileRunStore, FileTraceStore};
use camino::Utf8PathBuf;
use time::OffsetDateTime;

async fn multi_agent_state(dir: &tempfile::TempDir) -> TuiState {
    let registry_path = dir.path().join("multi-agents.yaml");
    fs_err::write(
        &registry_path,
        r#"agents:
  - protocol_version: agent.v1
    id: echo_agent
    name: Echo Agent
    description: First test agent.
    version: 0.1.0
    runner: echo
    schedule:
      type: manual
    capabilities: []
    metadata: {}
  - protocol_version: agent.v1
    id: review_agent
    name: Review Agent
    description: Second test agent.
    version: 0.1.0
    runner: echo
    schedule:
      type: manual
    capabilities: []
    metadata: {}
"#,
    )
    .expect("registry writes");
    let mut options = test_options(dir, "mock response", true);
    options.runtime_sources.registry =
        Utf8PathBuf::from_path_buf(registry_path).expect("registry path is utf8");
    TuiState::load(options).await.expect("state loads")
}

fn add_high_risk_echo_tool(options: &mut TuiOptions) {
    options.tool_overrides.source_specs.push(ToolSpec {
        name: "echo".to_owned(),
        description: "High-risk echo test tool.".to_owned(),
        input_schema: json!({"type": "object"}),
        output_schema: Some(json!({"type": "object"})),
        risk: ToolRisk::High,
        replay_policy: agent_core::ToolReplayPolicy::AtMostOnce,
        metadata: json!({"source": "test_high_risk"}),
    });
}

async fn high_risk_echo_state(dir: &tempfile::TempDir, allow_high_risk_tools: bool) -> TuiState {
    let mut options = test_options(dir, "mock response", allow_high_risk_tools);
    add_high_risk_echo_tool(&mut options);
    TuiState::load(options).await.expect("state loads")
}

async fn write_test_trace(store_path: &Utf8PathBuf, trace: &AgentTrace) {
    let trace_store = FileTraceStore::new(store_path.clone())
        .await
        .expect("trace store loads");
    trace_store
        .write_trace(trace.clone())
        .await
        .expect("trace writes");
}

#[path = "commands/agents.rs"]
mod agents;
#[path = "commands/events.rs"]
mod events;
#[path = "commands/inventory.rs"]
mod inventory;
#[path = "commands/proposals.rs"]
mod proposals;
#[path = "commands/runs.rs"]
mod runs;
#[path = "commands/shell.rs"]
mod shell;
#[path = "commands/tools.rs"]
mod tools;
#[path = "commands/workflow.rs"]
mod workflow;

async fn create_test_proposal(
    store_path: &Utf8PathBuf,
    run_id: &str,
    agent_id: &str,
    kind: &str,
    summary: &str,
    status: ProposalStatus,
) -> ProposalEnvelope {
    let store = FileProposalStore::new(store_path.clone())
        .await
        .expect("proposal store opens");
    let mut proposal = ProposalEnvelope::new(
        RunId(run_id.to_owned()),
        agent_id.to_owned(),
        kind.to_owned(),
        summary.to_owned(),
        json!({}),
    );
    proposal.status = status;
    store
        .create_proposal(proposal.clone())
        .await
        .expect("proposal writes");
    proposal
}

async fn create_test_run(
    store_path: &Utf8PathBuf,
    run_id: RunId,
    status: AgentRunStatus,
) -> AgentRunRecord {
    let store = FileRunStore::new(store_path.clone())
        .await
        .expect("run store opens");
    let now = OffsetDateTime::now_utc();
    let record = AgentRunRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        version: 1,
        run_id,
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
    };
    store.create_run(record.clone()).await.expect("run writes");
    record
}
