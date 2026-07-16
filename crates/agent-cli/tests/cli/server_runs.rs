use super::*;

#[test]
fn http_server_runs_workflow_dag_and_persists_node_traces() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-workflow-store");
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

    let workflow = http_json_request(
        port,
        "POST",
        "/workflows/run",
        Some(
            r#"{"protocol_version":"agent.v1","workflow_id":"workflow_http_dag","nodes":[{"node_id":"collect","agent_id":"ai_chat","input":{"message":"collect"}},{"node_id":"summarize","agent_id":"ai_chat","depends_on":["collect"],"input":{"message":"summarize"},"input_mappings":[{"from_node":"collect","from_path":"/input/message","to_path":"/from_collect"}]}],"metadata":{"case":"http_workflow"}}"#,
        ),
    );

    assert_eq!(workflow["protocol_version"], "agent.v1");
    assert_eq!(workflow["workflow_id"], "workflow_http_dag");
    assert_eq!(workflow["status"], "completed");
    let nodes = workflow["nodes"].as_array().expect("workflow nodes");
    assert_eq!(nodes.len(), 2);
    assert_eq!(
        nodes[0]["trace"]["workflow"]["workflow_id"],
        "workflow_http_dag"
    );
    assert_eq!(
        nodes[1]["trace"]["workflow"]["dependencies"][0]["run_id"],
        nodes[0]["run_id"]
    );
    assert_eq!(nodes[1]["output"]["input"]["from_collect"], "collect");
    let first_run_id = nodes[0]["run_id"].as_str().expect("first run id");
    let inspected_trace =
        http_json_request(port, "GET", &format!("/runs/{first_run_id}/trace"), None);
    assert_eq!(inspected_trace["run_id"], first_run_id);
    assert_eq!(
        inspected_trace["workflow"]["metadata"]["workflow_node_id"],
        "collect"
    );
    assert_eq!(
        inspected_trace["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );
}

#[test]
fn http_server_can_cancel_active_run_and_stream_live_events() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-cancel-store");
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

    let run_id = "run_http_cancel";
    let run_body = format!(r#"{{"run_id":"{run_id}","input":{{"sleep_ms":3000}}}}"#);
    let run_path = "/agents/ai_chat/run".to_owned();
    let run_handle = std::thread::spawn({
        let run_body = run_body.clone();
        move || http_json_request(port, "POST", &run_path, Some(&run_body))
    });

    let inspected = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["run_id"], run_id);
    assert_eq!(inspected["status"], "running");
    wait_for_event_log_contains(&store, r#""kind":"run_started""#);
    let active_before_events = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(active_before_events["status"], "running");
    let active_snapshot = http_text_request(
        port,
        "GET",
        &format!("/runs/{run_id}/events?follow=false"),
        None,
    );
    assert!(active_snapshot.contains("content-type: text/event-stream"));
    assert!(active_snapshot.contains("id: 1"));
    assert!(active_snapshot.contains("event: run_started"));
    assert!(!active_snapshot.contains("event: run_finished"));

    let events_handle = std::thread::spawn({
        let path = format!("/runs/{run_id}/events");
        move || http_text_request(port, "GET", &path, None)
    });
    std::thread::sleep(Duration::from_millis(100));

    let cancelled = http_json_request(port, "POST", &format!("/runs/{run_id}/cancel"), Some("{}"));
    assert_eq!(cancelled["run_id"], run_id);
    assert_eq!(cancelled["cancellation_requested"], true);

    let run = run_handle.join().expect("run request joins");
    assert_eq!(run["result"]["run_id"], run_id);
    assert_eq!(run["result"]["status"], "cancelled");
    assert_eq!(run["result"]["error"]["code"], "cancelled");

    let events = events_handle.join().expect("events request joins");
    assert!(events.contains("content-type: text/event-stream"));
    assert!(events.contains("id: 1"));
    assert!(events.contains("event: run_started"));
    assert_eq!(events.matches("event: run_started").count(), 1);
    assert!(events.contains("event: run_cancel_requested"));
    assert!(events.contains("event: run_cancelled"));
    assert!(events.contains("event: run_finished"));
    assert!(events.contains(r#""status":"cancelled""#));

    let inspected = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["status"], "cancelled");
    assert_eq!(inspected["metadata"]["control"]["cancel_requested"], true);
    assert_eq!(
        inspected["metadata"]["control"]["cancel_requested_by"],
        "http"
    );
    let trace = http_json_request(port, "GET", &format!("/runs/{run_id}/trace"), None);
    let event_kinds = trace["events"]
        .as_array()
        .expect("trace events")
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"run_cancel_requested"));
    assert!(event_kinds.contains(&"run_cancelled"));
    assert!(event_kinds.contains(&"run_finished"));

    let event_log = find_single_run_event_log(&store);
    let event_log_text = std::fs::read_to_string(&event_log).expect("event log reads");
    assert!(event_log_text.contains(r#""kind":"run_finished""#));

    std::fs::remove_file(store.join("traces").join(format!("{run_id}.trace.json")))
        .expect("trace fallback removed");
    let persisted_events = http_text_request(port, "GET", &format!("/runs/{run_id}/events"), None);
    assert!(persisted_events.contains("content-type: text/event-stream"));
    assert!(persisted_events.contains("event: run_finished"));
    assert!(persisted_events.contains(r#""status":"cancelled""#));
}

#[test]
fn http_server_persists_cancel_intent_for_running_run_record() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-remote-cancel-store");
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

    let run_id = "run_remote_cancel";
    std::fs::write(
        store.join("runs").join(format!("{run_id}.json")),
        serde_json::to_vec_pretty(&serde_json::json!({
            "protocol_version": "agent.v1",
            "run_id": run_id,
            "agent_id": "ai_chat",
            "status": "running",
            "scope": {"type": "global"},
            "started_at": "2026-07-03T00:00:00Z",
            "finished_at": null,
            "input": {},
            "output": {},
            "metadata": {}
        }))
        .expect("run record encodes"),
    )
    .expect("run record writes");

    let cancelled = http_json_request(port, "POST", &format!("/runs/{run_id}/cancel"), Some("{}"));
    assert_eq!(cancelled["run_id"], run_id);
    assert_eq!(cancelled["cancellation_requested"], true);
    assert_eq!(cancelled["status"], "running");

    let inspected = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["status"], "running");
    assert_eq!(inspected["metadata"]["control"]["cancel_requested"], true);
    assert_eq!(
        inspected["metadata"]["control"]["cancel_requested_by"],
        "http"
    );
    assert!(
        inspected["metadata"]["control"]["cancel_requested_at"]
            .as_str()
            .unwrap_or_default()
            .contains('T')
    );
}

#[test]
fn http_server_follows_and_cancels_run_from_another_instance() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("multi-instance-store");
    let config_path = dir.path().join("agent-runtime.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"[runtime]
store = "{}"
store_backend = "sqlite"
"#,
            store.display()
        ),
    )
    .expect("config writes");
    let first_port = reserve_local_port();
    let mut second_port = reserve_local_port();
    while second_port == first_port {
        second_port = reserve_local_port();
    }
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let spawn_server = |port: u16| {
        std::process::Command::new(&agent_bin)
            .args([
                "--config",
                config_path.to_str().expect("utf8 config path"),
                "serve",
                "--catalog",
                "../../fixtures/contracts/catalog.valid.json",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("HTTP server starts")
    };
    let _first_server = ChildGuard(spawn_server(first_port));
    let _second_server = ChildGuard(spawn_server(second_port));
    wait_for_http_server(first_port);
    wait_for_http_server(second_port);

    let run_id = "run_cross_instance_follow";
    let run_body = format!(r#"{{"run_id":"{run_id}","input":{{"sleep_ms":3000}}}}"#);
    let run_handle = std::thread::spawn(move || {
        http_json_request(first_port, "POST", "/agents/ai_chat/run", Some(&run_body))
    });

    let inspected = http_json_request(second_port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["status"], "running");
    let events_handle = std::thread::spawn({
        let path = format!("/runs/{run_id}/events");
        move || http_text_request(second_port, "GET", &path, None)
    });
    std::thread::sleep(Duration::from_millis(100));

    let cancelled = http_json_request(
        second_port,
        "POST",
        &format!("/runs/{run_id}/cancel"),
        Some("{}"),
    );
    assert_eq!(cancelled["cancellation_requested"], true);

    let run = run_handle.join().expect("run request joins");
    assert_eq!(run["result"]["status"], "cancelled");
    let events = events_handle.join().expect("events request joins");
    assert!(events.contains("event: run_started"));
    assert!(events.contains("event: run_cancel_requested"));
    assert!(events.contains("event: run_cancelled"));
    assert!(events.contains("event: run_finished"));
    assert_eq!(events.matches("event: run_started").count(), 1);

    let inspected = http_json_request(second_port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["status"], "cancelled");
    assert_eq!(inspected["version"], 3);
    assert_eq!(inspected["metadata"]["control"]["cancel_requested"], true);
}
