use super::*;

#[test]
fn http_server_streams_chat_turn_events() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-chat-store");
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
            "--mock-response",
            "hello over sse",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let response = http_text_request(
        port,
        "POST",
        "/chat/turn",
        Some(
            r#"{"protocol_version":"agent.v1","turn_id":"turn_http_1","agent_id":"ai_chat","provider":"mock","model":"mock-model","messages":[{"role":"user","content":"ping"}],"metadata":{"source":"http_test"}}"#,
        ),
    );

    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("content-type: text/event-stream"));
    assert!(response.contains("event: chat_turn_event"));
    assert!(response.contains(r#""kind":"started""#));
    assert!(response.contains(r#""kind":"delta""#));
    assert!(response.contains(r#""content":"hello over sse""#));
    assert!(response.contains(r#""kind":"round_finished""#));
    assert!(response.contains(r#""kind":"done""#));
}

#[test]
fn http_server_resumes_chat_turn_and_records_session_steps() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-chat-resume-store");
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
            "--mock-response",
            "resumed over sse",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let created = http_json_request(
        port,
        "POST",
        "/sessions",
        Some(r#"{"title":"Chat resume","metadata":{"source":"test"}}"#),
    );
    let session_id = created["session"]["session_id"]
        .as_str()
        .expect("session id is string");
    let thread_id = created["thread"]["thread_id"]
        .as_str()
        .expect("thread id is string");
    let resume_body = serde_json::json!({
        "protocol_version": "agent.v1",
        "state": {
            "protocol_version": "agent.v1",
            "turn_id": "turn_resume_http",
            "surface": "ai_chat",
            "mode": "chat",
            "session_id": session_id,
            "thread_id": thread_id,
            "agent_id": "ai_chat",
            "provider": "mock",
            "model": "mock-model",
            "messages": [
                {"role": "user", "content": "read task", "metadata": {}},
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "read_task",
                            "input": {"id": "task_1"}
                        }
                    ],
                    "metadata": {}
                }
            ],
            "tools": [],
            "metadata": {"source": "http_test"},
            "max_tool_rounds": 4,
            "round": 1,
            "pending_tool_calls": [
                {"id": "call_1", "name": "read_task", "input": {"id": "task_1"}}
            ],
            "tool_execution": "client"
        },
        "tool_results": [
            {
                "tool_call_id": "call_1",
                "tool_name": "read_task",
                "output": {"title": "Task title"},
                "is_error": false
            }
        ]
    })
    .to_string();

    let response = http_text_request(port, "POST", "/chat/resume", Some(&resume_body));
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("content-type: text/event-stream"));
    assert!(response.contains(r#""kind":"tool_result""#));
    assert!(response.contains(r#""content":"resumed over sse""#));
    assert!(response.contains(r#""status":"completed""#));
    assert!(response.contains(r#""kind":"done""#));

    let shown = http_json_request(port, "GET", &format!("/sessions/{session_id}"), None);
    let steps = shown["threads"][0]["steps"]
        .as_array()
        .expect("steps are array");
    let kinds = steps
        .iter()
        .filter_map(|step| step["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"tool_call"));
    assert!(kinds.contains(&"llm_round"));
    assert!(kinds.contains(&"state_update"));
    assert!(steps.iter().any(|step| {
        step["kind"] == "llm_round" && step["payload"]["event"]["metadata"]["status"] == "completed"
    }));
}
