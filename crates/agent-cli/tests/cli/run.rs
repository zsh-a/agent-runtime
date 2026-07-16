use super::*;

#[test]
fn run_can_use_catalog_backed_dry_run_registry() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let trace = dir.path().join("trace.json");

    let output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            "../../fixtures/contracts/run-request.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--trace-out",
            trace.to_str().expect("utf8 trace path"),
            "--scope",
            "tenant:tenant_cli",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result: Value = serde_json::from_slice(&output).expect("result is JSON");
    assert_eq!(result["agent_id"], "ai_chat");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["summary"], "catalog dry-run completed");
    assert_eq!(result["output"]["mode"], "catalog_dry_run");
    assert_eq!(result["output"]["agent"]["id"], "ai_chat");
    assert_eq!(result["output"]["input"]["protocol_version"], "agent.v1");
    let run_id = result["run_id"].as_str().expect("run id is string");
    let inspected = agent_cmd()
        .args([
            "inspect",
            run_id,
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let inspected: Value = serde_json::from_slice(&inspected).expect("inspect result is JSON");
    assert_eq!(inspected["scope"]["type"], "tenant");
    assert_eq!(inspected["scope"]["id"], "tenant_cli");

    let trace: Value =
        serde_json::from_slice(&std::fs::read(trace).expect("trace file was written"))
            .expect("trace is JSON");
    assert_eq!(trace["agent_id"], "ai_chat");
    assert_eq!(trace["events"][1]["kind"], "catalog_dry_run.agent_selected");
}

#[test]
fn catalog_dry_run_can_call_process_tool_host() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let input_path = dir.path().join("input.json");
    let trace_path = dir.path().join("trace.json");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&serde_json::json!({
            "tool_call": {
                "name": "echo_external",
                "input": {"value": 7}
            }
        }))
        .expect("input encodes"),
    )
    .expect("input writes");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");

    let output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_path.to_str().expect("utf8 input path"),
            "--trace-out",
            trace_path.to_str().expect("utf8 trace path"),
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result: Value = serde_json::from_slice(&output).expect("result is JSON");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["output"]["tool_result"]["host"], "dev-tool-host");
    assert_eq!(result["output"]["tool_result"]["tool"], "echo_external");
    assert_eq!(result["output"]["tool_result"]["input"]["value"], 7);

    let trace = read_json(trace_path);
    let events = trace["events"].as_array().expect("trace events is array");
    let started = events
        .iter()
        .find(|event| event["kind"] == "tool_call_started")
        .expect("tool start event exists");
    let finished = events
        .iter()
        .find(|event| event["kind"] == "tool_call_finished")
        .expect("tool finish event exists");
    let tool_call_id = started["payload"]["tool_call_id"]
        .as_str()
        .expect("tool call id is string");
    assert!(tool_call_id.starts_with("tool_"));
    assert_eq!(finished["payload"]["tool_call_id"], tool_call_id);
    assert_eq!(started["payload"]["tool_name"], "echo_external");
    assert_eq!(finished["payload"]["tool_name"], "echo_external");
    assert!(
        started["payload"]["input_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
    assert_eq!(
        finished["payload"]["input_hash"],
        started["payload"]["input_hash"]
    );
    assert_eq!(finished["payload"]["status"], "completed");
    assert!(finished["payload"]["duration_ms"].is_number());
    assert_eq!(finished["payload"]["output"]["input"]["value"], 7);

    let spans = trace["spans"].as_array().expect("trace spans is array");
    let tool_spans = spans
        .iter()
        .filter(|span| span["name"] == "tool.echo_external")
        .collect::<Vec<_>>();
    assert_eq!(tool_spans.len(), 1);
    assert_eq!(
        tool_spans[0]["parent_span_id"], spans[0]["span_id"],
        "tool span is nested under the run span",
    );
    assert_eq!(tool_spans[0]["status"], "completed");
    assert_eq!(
        tool_spans[0]["attributes"]["tool_call_id"],
        started["payload"]["tool_call_id"]
    );
    assert_eq!(
        tool_spans[0]["attributes"]["input_hash"],
        started["payload"]["input_hash"]
    );
}

#[test]
fn catalog_dry_run_retries_retryable_process_tool_errors() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let input_path = dir.path().join("input.json");
    let trace_path = dir.path().join("trace.json");
    let fail_once_path = dir.path().join("fail-once.marker");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&serde_json::json!({
            "tool_call": {
                "name": "echo_external",
                "input": {
                    "value": 7,
                    "fail_once_path": fail_once_path,
                }
            }
        }))
        .expect("input encodes"),
    )
    .expect("input writes");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");

    let output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_path.to_str().expect("utf8 input path"),
            "--trace-out",
            trace_path.to_str().expect("utf8 trace path"),
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--max-retries",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result: Value = serde_json::from_slice(&output).expect("result is JSON");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["output"]["tool_result"]["host"], "dev-tool-host");
    assert!(fail_once_path.exists());

    let trace = read_json(trace_path);
    let events = trace["events"].as_array().expect("trace events is array");
    assert_eq!(
        events
            .iter()
            .filter(|event| event["kind"] == "run_attempt_started")
            .count(),
        2
    );
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "tool_call_failed")
    );
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "run_retry_scheduled")
    );
    let retry = events
        .iter()
        .find(|event| event["kind"] == "run_retry_scheduled")
        .expect("retry event exists");
    assert_eq!(retry["payload"]["next_attempt"], 2);
}

#[test]
fn catalog_dry_run_can_call_mock_tool_from_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let input_path = dir.path().join("input.json");
    let mock_path = dir.path().join("mock-output.json");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&serde_json::json!({
            "tool_call": {
                "name": "propose_fake",
                "input": {"value": 9}
            }
        }))
        .expect("input encodes"),
    )
    .expect("input writes");
    std::fs::write(
        &mock_path,
        serde_json::to_vec(&serde_json::json!({
            "mocked": true,
            "decision": "accept"
        }))
        .expect("mock encodes"),
    )
    .expect("mock writes");

    let output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_path.to_str().expect("utf8 input path"),
            "--mock-tool",
            &format!(
                "propose_fake=@{}",
                mock_path.to_str().expect("utf8 mock path")
            ),
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result: Value = serde_json::from_slice(&output).expect("result is JSON");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["output"]["tool_result"]["mocked"], true);
    assert_eq!(result["output"]["tool_result"]["decision"], "accept");
}
