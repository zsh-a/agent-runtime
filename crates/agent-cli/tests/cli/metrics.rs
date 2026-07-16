use super::*;

#[test]
fn metrics_summary_counts_runs_tools_and_proposals() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("metrics-store");
    let input_path = dir.path().join("input.json");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&serde_json::json!({
            "tool_call": {
                "name": "echo",
                "input": {"value": 23}
            },
            "proposal": {
                "kind": "fake",
                "summary": "Metrics fake proposal",
                "payload": {"value": 24}
            }
        }))
        .expect("input encodes"),
    )
    .expect("input writes");

    let run_output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_path.to_str().expect("utf8 input path"),
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run_result: Value = serde_json::from_slice(&run_output).expect("run result is JSON");
    let run_id = run_result["run_id"].as_str().expect("run id is string");
    let trace_path = store.join("traces").join(format!("{run_id}.trace.json"));
    let mut trace = read_json(&trace_path);
    let events = trace["events"]
        .as_array_mut()
        .expect("trace events are array");
    events.push(serde_json::json!({
        "kind": "llm_response",
        "occurred_at": "2026-06-28T09:12:31Z",
        "payload": {
            "provider": "openai",
            "model": "gpt-test",
            "duration_ms": 25,
            "usage": {
                "input_tokens": 5,
                "output_tokens": 3,
                "total_tokens": 8,
                "cost_micros": 42,
                "cost_currency": "USD"
            }
        }
    }));
    events.push(serde_json::json!({
        "kind": "proposal_decided",
        "occurred_at": "2026-06-28T09:12:32Z",
        "payload": {
            "proposal_id": "proposal_metrics",
            "decision": "approve",
            "status": "pending_approval"
        }
    }));
    events.push(serde_json::json!({
        "kind": "proposal_decided",
        "occurred_at": "2026-06-28T09:12:33Z",
        "payload": {
            "proposal_id": "proposal_metrics",
            "decision": "approve",
            "status": "approved"
        }
    }));
    std::fs::write(
        &trace_path,
        serde_json::to_vec_pretty(&trace).expect("trace encodes"),
    )
    .expect("trace writes");

    let output = agent_cmd()
        .args([
            "metrics",
            "summary",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let metrics: Value = serde_json::from_slice(&output).expect("metrics are JSON");
    assert_eq!(metrics["protocol_version"], "agent.v1");
    assert_eq!(metrics["run_count"], 1);
    assert_eq!(metrics["successful_run_count"], 1);
    assert_eq!(metrics["runs_by_status"]["completed"], 1);
    assert_eq!(metrics["tool_call_count"], 1);
    assert_eq!(metrics["failed_tool_call_count"], 0);
    assert_eq!(metrics["runs_by_agent"]["ai_chat"]["run_count"], 1);
    assert_eq!(
        metrics["runs_by_agent"]["ai_chat"]["successful_run_count"],
        1
    );
    assert_eq!(
        metrics["runs_by_agent"]["ai_chat"]["runs_by_status"]["completed"],
        1
    );
    assert_eq!(metrics["tool_calls_by_tool"]["echo"]["tool_call_count"], 1);
    assert_eq!(
        metrics["tool_calls_by_tool"]["echo"]["failed_tool_call_count"],
        0
    );
    assert!(metrics["average_run_latency_ms"].is_number());
    assert!(metrics["average_tool_call_latency_ms"].is_number());
    assert_eq!(metrics["proposal_count"], 1);
    assert_eq!(metrics["proposal_created_count"], 1);
    assert_eq!(metrics["proposal_approved_count"], 1);
    assert_eq!(metrics["proposals_by_status"]["pending_approval"], 1);
    assert_eq!(metrics["artifact_ref_count"], 0);
    assert_eq!(metrics["replay_count"], 0);
    assert_eq!(metrics["llm_total_tokens"], 8);
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["request_count"],
        1
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["input_tokens"],
        5
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["output_tokens"],
        3
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["total_tokens"],
        8
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["cost_micros_by_currency"]["USD"],
        42
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["total_latency_ms"],
        25
    );
    assert!(metrics["llm_usage_by_provider"]["openai"]["average_latency_ms"].is_number());
}
