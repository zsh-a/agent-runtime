use super::*;

#[test]
fn http_server_persists_sessions_threads_and_steps() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-session-store");
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
        Some(r#"{"title":"HTTP debug","metadata":{"source":"test"}}"#),
    );
    let session_id = created["session"]["session_id"]
        .as_str()
        .expect("session id is string");
    let thread_id = created["thread"]["thread_id"]
        .as_str()
        .expect("thread id is string");
    assert_eq!(created["session"]["title"], "HTTP debug");
    assert_eq!(created["thread"]["session_id"], session_id);

    let listed = http_json_request(port, "GET", "/sessions", None);
    assert_eq!(listed[0]["session_id"], session_id);

    let run_body = format!(
        r#"{{"session_id":"{session_id}","thread_id":"{thread_id}","input":{{"message":"http session"}}}}"#
    );
    let run = http_json_request(port, "POST", "/agents/ai_chat/run", Some(&run_body));
    let run_id = run["result"]["run_id"].as_str().expect("run id is string");
    assert_eq!(run["result"]["status"], "completed");

    let shown = http_json_request(port, "GET", &format!("/sessions/{session_id}"), None);
    assert_eq!(shown["session"]["session_id"], session_id);
    assert_eq!(shown["threads"][0]["thread"]["thread_id"], thread_id);
    assert_eq!(shown["threads"][0]["steps"][0]["kind"], "agent_run");
    assert_eq!(shown["threads"][0]["steps"][0]["run_id"], run_id);

    let forked = http_json_request(
        port,
        "POST",
        &format!("/sessions/{session_id}/fork"),
        Some(&format!(
            r#"{{"parent_thread_id":"{thread_id}","title":"Alternative"}}"#
        )),
    );
    assert_eq!(forked["session_id"], session_id);
    assert_eq!(forked["parent_thread_id"], thread_id);
    assert_eq!(forked["thread"]["title"], "Alternative");

    let stored_run = read_json(store.join("runs").join(format!("{run_id}.json")));
    assert!(
        stored_run["idempotency_key"]
            .as_str()
            .is_some_and(|key| key.starts_with("idem_"))
    );
    assert_eq!(stored_run["metadata"]["session_id"], session_id);
    assert_eq!(stored_run["metadata"]["thread_id"], thread_id);
}
