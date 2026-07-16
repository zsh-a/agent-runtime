use super::*;

#[test]
fn stdio_server_handles_catalog_summary_and_agent_run() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("stdio-store");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":"summary","method":"catalog.summary","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":"run","method":"agent.run","params":{"agent_id":"ai_chat","input":{"message":"via stdio","tool_call":{"name":"stdio_external","input":{"ok":true}}}}}"#,
        "\n"
    );

    let output = agent_cmd()
        .args([
            "serve",
            "--stdio",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
        ])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let lines = String::from_utf8(output).expect("stdout is utf8");
    let responses = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("response JSON"))
        .collect::<Vec<_>>();

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["jsonrpc"], "2.0");
    assert_eq!(responses[0]["id"], "summary");
    assert_eq!(responses[0]["result"]["agent_count"], 1);

    assert_eq!(responses[1]["jsonrpc"], "2.0");
    assert_eq!(responses[1]["id"], "run");
    assert_eq!(responses[1]["result"]["result"]["status"], "completed");
    assert_eq!(
        responses[1]["result"]["result"]["output"]["tool_result"]["tool"],
        "stdio_external"
    );
    assert_eq!(
        responses[1]["result"]["trace"]["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );
}

#[test]
fn http_server_handles_catalog_summary_and_agent_run() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
        .args([
            "serve",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--mock-tool",
            r#"propose_fake={"http_action":true}"#,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let summary = http_json_request(port, "GET", "/catalog/summary", None);
    assert_eq!(summary["protocol_version"], "agent.v1");
    assert_eq!(summary["catalog_version"], "agent_catalog.v1");
    assert_eq!(summary["agent_count"], 1);

    let run = http_json_request(
        port,
        "POST",
        "/agents/ai_chat/run",
        Some(
            r#"{"input":{"message":"via http"},"trigger":"webhook","trigger_envelope":{"source":"github.webhook","id":"evt_http_1","payload":{"action":"opened"}},"user":{"user_id":"user_http_1"},"scope":{"type":"tenant","id":"tenant_http_1"},"workflow":{"workflow_id":"workflow_http_test","root_run_id":"run_http_root","dependencies":[{"run_id":"run_http_dependency","edge":"after","metadata":{"fixture":"catalog_cli"}}],"metadata":{"case":"catalog_cli"}},"metadata":{"delivery_attempt":1}}"#,
        ),
    );
    assert_eq!(run["result"]["agent_id"], "ai_chat");
    assert_eq!(run["result"]["status"], "completed");
    assert_eq!(run["result"]["output"]["mode"], "catalog_dry_run");
    assert_eq!(
        run["result"]["workflow"]["workflow_id"],
        "workflow_http_test"
    );
    assert_eq!(
        run["trace"]["workflow"]["workflow_id"],
        "workflow_http_test"
    );
    assert_eq!(run["trace"]["events"][0]["payload"]["trigger"], "webhook");
    assert_eq!(
        run["trace"]["events"][0]["payload"]["trigger_envelope"]["source"],
        "github.webhook"
    );
    assert_eq!(
        run["trace"]["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );

    let run_id = run["result"]["run_id"].as_str().expect("run_id is string");
    let runs = http_json_request(port, "GET", "/runs?agent_id=ai_chat&limit=1", None);
    assert_eq!(runs[0]["run_id"], run_id);
    assert_eq!(runs[0]["agent_id"], "ai_chat");

    let inspected_run = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected_run["run_id"], run_id);
    assert_eq!(inspected_run["agent_id"], "ai_chat");
    assert_eq!(inspected_run["status"], "completed");
    assert_eq!(inspected_run["scope"]["type"], "tenant");
    assert_eq!(inspected_run["scope"]["id"], "tenant_http_1");
    assert_eq!(inspected_run["metadata"]["delivery_attempt"], 1);
    assert_eq!(
        inspected_run["workflow"]["dependencies"][0]["run_id"],
        "run_http_dependency"
    );
    assert_eq!(
        inspected_run["metadata"]["session_id"],
        serde_json::Value::Null
    );
    assert_eq!(
        inspected_run["metadata"]["thread_id"],
        serde_json::Value::Null
    );

    let inspected_trace = http_json_request(port, "GET", &format!("/runs/{run_id}/trace"), None);
    assert_eq!(inspected_trace["run_id"], run_id);
    assert_eq!(inspected_trace["agent_id"], "ai_chat");
    assert_eq!(
        inspected_trace["workflow"]["metadata"]["case"],
        "catalog_cli"
    );
    assert_eq!(
        inspected_trace["events"][0]["payload"]["trigger"],
        "webhook"
    );
    assert_eq!(
        inspected_trace["events"][0]["payload"]["trigger_envelope"]["id"],
        "evt_http_1"
    );
    assert_eq!(
        inspected_trace["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );

    let events = http_text_request(port, "GET", &format!("/runs/{run_id}/events"), None);
    assert!(events.starts_with("HTTP/1.1 200"));
    assert!(events.contains("content-type: text/event-stream"));
    assert!(events.contains("id: 1"));
    assert!(events.contains("id: 2"));
    assert!(events.contains("event: run_started"));
    assert!(events.contains("event: catalog_dry_run.agent_selected"));
    assert!(events.contains(r#""kind":"catalog_dry_run.agent_selected""#));
    let resumed_events =
        http_text_request(port, "GET", &format!("/runs/{run_id}/events?after=1"), None);
    assert!(!resumed_events.contains("event: run_started"));
    assert!(resumed_events.contains("id: 2"));
    assert!(resumed_events.contains("event: catalog_dry_run.agent_selected"));
    let header_resumed_events = http_text_request_with_headers(
        port,
        "GET",
        &format!("/runs/{run_id}/events"),
        None,
        &[("Last-Event-ID", "1")],
    );
    assert!(!header_resumed_events.contains("event: run_started"));
    assert!(header_resumed_events.contains("id: 2"));
    let query_cursor_wins = http_text_request_with_headers(
        port,
        "GET",
        &format!("/runs/{run_id}/events?after=1"),
        None,
        &[("Last-Event-ID", "2")],
    );
    assert!(query_cursor_wins.contains("id: 2"));
    assert!(query_cursor_wins.contains("event: catalog_dry_run.agent_selected"));
    let invalid_cursor = try_http_text_request(
        port,
        "GET",
        &format!("/runs/{run_id}/events?after=not-a-cursor"),
        None,
    )
    .expect_err("invalid run event cursor should return an HTTP error");
    assert!(invalid_cursor.contains("HTTP/1.1 400"));
    assert!(invalid_cursor.contains("invalid_event_cursor"));

    let event_log = find_single_run_event_log(&store);
    std::fs::remove_file(event_log).expect("event log removed for trace fallback");
    let fallback_events =
        http_text_request(port, "GET", &format!("/runs/{run_id}/events?after=1"), None);
    assert!(!fallback_events.contains("event: run_started"));
    assert!(fallback_events.contains("id: 2"));
    assert!(fallback_events.contains("event: catalog_dry_run.agent_selected"));

    let replay = http_json_request(port, "POST", &format!("/runs/{run_id}/replay"), Some("{}"));
    let replay_run_id = replay["replay_run_id"]
        .as_str()
        .expect("replay run id is string");
    assert_eq!(replay["source_run_id"], run_id);
    assert_eq!(replay["agent_id"], "ai_chat");
    assert_eq!(replay["result"]["status"], "completed");
    assert_eq!(replay["output_matches"], true);
    assert_ne!(replay_run_id, run_id);
    assert_eq!(replay["trace"]["run_id"], replay_run_id);

    let metrics = http_json_request(port, "GET", "/metrics/summary", None);
    assert_eq!(metrics["run_count"], 2);
    assert_eq!(metrics["successful_run_count"], 2);
    assert_eq!(metrics["replay_count"], 1);
    assert_eq!(metrics["runs_by_status"]["completed"], 2);

    let persisted_trace = store.join("traces").join(format!("{run_id}.trace.json"));
    assert!(
        persisted_trace.exists(),
        "HTTP agent.run persists trace for debug bundle export"
    );
}

#[test]
fn http_server_validates_json_request_schemas() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-schema-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
        .args([
            "serve",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let run_error = try_http_text_request(
        port,
        "POST",
        "/agents/ai_chat/run",
        Some(r#"{"input":{},"unexpected":true}"#),
    )
    .expect_err("extra run field is rejected");
    assert!(run_error.contains("HTTP/1.1 400"));
    assert!(run_error.contains("schema_validation_failed"));

    let chat_error = try_http_text_request(
        port,
        "POST",
        "/chat/turn",
        Some(r#"{"protocol_version":"agent.v1","provider":"mock","model":"mock-model"}"#),
    )
    .expect_err("chat without messages is rejected");
    assert!(chat_error.contains("HTTP/1.1 400"));
    assert!(chat_error.contains("schema_validation_failed"));

    let resume_error = try_http_text_request(
        port,
        "POST",
        "/chat/resume",
        Some(r#"{"protocol_version":"agent.v1","tool_results":[]}"#),
    )
    .expect_err("chat resume without state is rejected");
    assert!(resume_error.contains("HTTP/1.1 400"));
    assert!(resume_error.contains("schema_validation_failed"));

    let workflow_error = try_http_text_request(
        port,
        "POST",
        "/workflows/run",
        Some(r#"{"protocol_version":"agent.v1","workflow_id":"workflow_invalid"}"#),
    )
    .expect_err("workflow without nodes is rejected");
    assert!(workflow_error.contains("HTTP/1.1 400"));
    assert!(workflow_error.contains("schema_validation_failed"));
}

#[test]
fn http_server_lists_and_calls_tools() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-tool-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(&agent_bin)
        .args([
            "serve",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let tools = http_json_request(port, "GET", "/tools", None);
    assert_eq!(tools[0]["name"], "propose_fake");

    let call = http_json_request(
        port,
        "POST",
        "/tools/propose_fake/call",
        Some(r#"{"input":{"value":9}}"#),
    );
    assert_eq!(call["tool"], "propose_fake");
    assert_eq!(call["output"]["host"], "dev-tool-host");
    assert_eq!(call["output"]["input"]["value"], 9);
}
