use agent_core::{
    AgentRuntimeCatalog, AgentSpec, EmbeddedEffectResponse, EmbeddedRunLimits, EmbeddedRunSnapshot,
    EmbeddedRunStepStatus, EmbeddedTerminalReason, RunId, RunRequest, ScheduleSpec, ToolRisk,
    ToolSpec, catalog_version, protocol_version,
};
use serde_json::{Value, json};
use time::OffsetDateTime;

use super::EffectStepLoop;

fn catalog() -> AgentRuntimeCatalog {
    AgentRuntimeCatalog {
        protocol_version: protocol_version(),
        catalog_version: catalog_version(),
        generated_at: OffsetDateTime::now_utc(),
        active_domains: vec!["test".to_owned()],
        agents: vec![
            AgentSpec {
                protocol_version: protocol_version(),
                id: "parent".to_owned(),
                name: "Parent".to_owned(),
                description: None,
                version: "1.0.0".to_owned(),
                schedule: ScheduleSpec::Manual,
                capabilities: vec!["read_first".to_owned()],
                metadata: json!({}),
            },
            AgentSpec {
                protocol_version: protocol_version(),
                id: "child".to_owned(),
                name: "Child".to_owned(),
                description: None,
                version: "1.0.0".to_owned(),
                schedule: ScheduleSpec::Manual,
                capabilities: vec!["read_first".to_owned()],
                metadata: json!({}),
            },
        ],
        tools: vec![ToolSpec {
            name: "read_first".to_owned(),
            description: "Read first".to_owned(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
            risk: ToolRisk::ReadOnly,
            metadata: json!({}),
        }],
        proposal_kinds: Vec::new(),
        prompt_blocks: Vec::new(),
    }
}

fn request(run_id: &str, effects: Value) -> RunRequest {
    RunRequest {
        protocol_version: protocol_version(),
        run_id: Some(RunId(run_id.to_owned())),
        input: json!({"effects": effects}),
        user: None,
        scope: None,
        trigger: agent_core::TriggerKind::Manual,
        trigger_envelope: None,
        workflow: None,
        metadata: json!({}),
    }
}

fn ok_response(snapshot: &EmbeddedRunSnapshot, value: Value) -> EmbeddedEffectResponse {
    EmbeddedEffectResponse {
        jsonrpc: "2.0".to_owned(),
        id: snapshot
            .requested_effect()
            .expect("requested effect")
            .effect_id()
            .to_owned(),
        result: Some(value),
        error: None,
    }
}

#[test]
fn typed_snapshot_runs_tool_plan_to_completion() {
    let catalog = catalog();
    let first = EffectStepLoop::start_snapshot(
        &catalog,
        request(
            "run_tool",
            json!([{"kind": "tool", "name": "read_first", "input": {}}]),
        ),
        "parent",
        EmbeddedRunLimits::default(),
    )
    .expect("snapshot starts");
    assert_eq!(first.step.status, EmbeddedRunStepStatus::EffectRequested);
    let terminal = EffectStepLoop::continue_snapshot(
        &catalog,
        first.clone(),
        ok_response(&first, json!({"ok": true})),
        "parent",
    )
    .expect("snapshot completes");
    assert_eq!(terminal.step.status, EmbeddedRunStepStatus::Completed);
    assert_eq!(terminal.progress.dispatched_effect_count, 1);
    assert_eq!(
        terminal.step.run_state.terminal_reason,
        Some(EmbeddedTerminalReason::Done)
    );
}

#[test]
fn typed_snapshot_preserves_continuation_without_json_roundtrip() {
    let catalog = catalog();
    let first = EffectStepLoop::start_snapshot(
        &catalog,
        request(
            "run_multi",
            json!([
                {"kind": "tool", "name": "read_first", "input": {"index": 1}},
                {"kind": "tool", "name": "read_first", "input": {"index": 2}}
            ]),
        ),
        "parent",
        EmbeddedRunLimits::default(),
    )
    .expect("snapshot starts");
    let second = EffectStepLoop::continue_snapshot(
        &catalog,
        first.clone(),
        ok_response(&first, json!({"index": 1})),
        "parent",
    )
    .expect("second effect requested");
    assert_eq!(second.step.step_index, 1);
    assert_eq!(second.step.run_state.effect_result_count, 1);
    let terminal = EffectStepLoop::continue_snapshot(
        &catalog,
        second.clone(),
        ok_response(&second, json!({"index": 2})),
        "parent",
    )
    .expect("snapshot completes");
    assert_eq!(terminal.step.effect_results.len(), 2);
}

#[test]
fn snapshot_cancellation_is_terminal_without_consuming_budget() {
    let catalog = catalog();
    let snapshot = EffectStepLoop::start_snapshot(
        &catalog,
        request(
            "run_cancel",
            json!([{"kind": "tool", "name": "read_first", "input": {}}]),
        ),
        "parent",
        EmbeddedRunLimits::default(),
    )
    .expect("snapshot starts");
    let cancelled =
        EffectStepLoop::cancel_snapshot(&catalog, snapshot, "parent", "user stopped the run")
            .expect("snapshot cancels");
    assert_eq!(cancelled.step.status, EmbeddedRunStepStatus::Cancelled);
    assert_eq!(cancelled.progress.dispatched_effect_count, 0);
}

#[test]
fn snapshot_closes_at_effect_budget() {
    let catalog = catalog();
    let snapshot = EffectStepLoop::start_snapshot(
        &catalog,
        request(
            "run_budget",
            json!([{"kind": "tool", "name": "read_first", "input": {}}]),
        ),
        "parent",
        EmbeddedRunLimits {
            max_effect_steps: 0,
            max_subagent_depth: 1,
        },
    )
    .expect("snapshot closes");
    assert_eq!(snapshot.step.status, EmbeddedRunStepStatus::ClosedEarly);
    assert!(snapshot.progress.effect_budget_exhausted);
}

#[test]
fn subagent_inherits_shared_budget() {
    let catalog = catalog();
    let parent = EffectStepLoop::start_snapshot(
        &catalog,
        request(
            "run_parent",
            json!([{
                "kind": "subagent",
                "agent_id": "child",
                "input": {"effects": [{"kind": "tool", "name": "read_first", "input": {}}]},
                "metadata": {}
            }]),
        ),
        "parent",
        EmbeddedRunLimits::default(),
    )
    .expect("parent starts");
    let child = EffectStepLoop::start_requested_subagent(&catalog, &parent).expect("child starts");
    assert_eq!(child.progress.dispatched_effect_count, 1);
    assert_eq!(child.progress.subagent_depth, 1);
    let child = EffectStepLoop::continue_snapshot(
        &catalog,
        child.clone(),
        ok_response(&child, json!({"ok": true})),
        "child",
    )
    .expect("child completes");
    let parent = EffectStepLoop::resume_parent_from_subagent(&catalog, parent, child)
        .expect("parent resumes");
    assert_eq!(parent.step.status, EmbeddedRunStepStatus::Completed);
    assert_eq!(parent.progress.dispatched_effect_count, 2);
}

#[test]
fn tampered_derived_state_is_rejected() {
    let catalog = catalog();
    let mut snapshot = EffectStepLoop::start_snapshot(
        &catalog,
        request(
            "run_tampered",
            json!([{"kind": "tool", "name": "read_first", "input": {}}]),
        ),
        "parent",
        EmbeddedRunLimits::default(),
    )
    .expect("snapshot starts");
    snapshot.step.run_state.step_index = 99;
    let error = EffectStepLoop::continue_snapshot(
        &catalog,
        snapshot.clone(),
        ok_response(&snapshot, json!({})),
        "parent",
    )
    .expect_err("tampering is rejected");
    assert!(error.record.message.contains("run_state"));
}
