use super::*;

#[test]
fn http_server_persists_and_decides_proposals() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-proposal-store");
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
            "--mock-tool",
            r#"propose_fake={"http_action":true}"#,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let run = http_json_request(
        port,
        "POST",
        "/agents/ai_chat/run",
        Some(r#"{"input":{"message":"proposal trace seed"}}"#),
    );
    let run_id = run["result"]["run_id"].as_str().expect("run_id is string");

    let created = http_json_request(
        port,
        "POST",
        "/proposals",
        Some(&format!(
            r#"{{"run_id":"{run_id}","agent_id":"ai_chat","kind":"fake","summary":"HTTP proposal","payload":{{"value":11}},"diffs":[{{"path":"/value","operation":"replace","before":10,"after":11}}],"warnings":[{{"severity":"danger","code":"http_review","message":"HTTP proposal needs review"}}]}}"#
        )),
    );
    let proposal_id = created["proposal_id"]
        .as_str()
        .expect("proposal id is string");
    assert_eq!(created["status"], "pending_approval");
    assert_eq!(created["payload"]["value"], 11);
    assert_eq!(created["diffs"][0]["path"], "/value");
    assert_eq!(created["diffs"][0]["before"], 10);
    assert_eq!(created["diffs"][0]["after"], 11);
    assert_eq!(created["warnings"][0]["severity"], "danger");
    assert_eq!(created["warnings"][0]["code"], "http_review");
    assert_eq!(created["risk"], "medium");
    assert_eq!(created["approval_policy"], "manual");
    assert_eq!(created["approval_required"], true);
    assert_eq!(created["required_approval_level"], "single_user");
    assert_eq!(created["policy_id"], "finance.proposal.default");
    assert_eq!(created["policy_version"], "2026-06-28");
    assert!(created["expires_at"].as_str().is_some());

    let listed = http_json_request(port, "GET", &format!("/proposals?run_id={run_id}"), None);
    assert_eq!(listed[0]["proposal_id"], proposal_id);

    let inspected = http_json_request(port, "GET", &format!("/proposals/{proposal_id}"), None);
    assert_eq!(inspected["proposal_id"], proposal_id);
    assert_eq!(inspected["diffs"][0]["operation"], "replace");
    assert_eq!(
        inspected["warnings"][0]["message"],
        "HTTP proposal needs review"
    );

    let decided = http_json_request(
        port,
        "POST",
        &format!("/proposals/{proposal_id}/decision"),
        Some(
            r#"{"decision":"approve","approval_level":"single_user","decided_by":"user_http_reviewer","comment":"approved over HTTP"}"#,
        ),
    );
    assert_eq!(decided["decision"]["decision"], "approve");
    assert_eq!(decided["decision"]["approval_level"], "single_user");
    assert_eq!(decided["decision"]["decided_by"], "user_http_reviewer");
    assert_eq!(decided["decision"]["comment"], "approved over HTTP");
    assert_eq!(decided["proposal"]["status"], "approved");

    let stored = read_json(store.join("proposals").join(format!("{proposal_id}.json")));
    assert_eq!(stored["status"], "approved");

    let applied = http_json_request(
        port,
        "POST",
        &format!("/proposals/{proposal_id}/apply"),
        Some("{}"),
    );
    assert_eq!(applied["action"], "apply");
    assert_eq!(applied["tool"], "propose_fake");
    assert_eq!(applied["tool_output"]["http_action"], true);
    assert_eq!(applied["proposal"]["status"], "applied");

    let undone = http_json_request(
        port,
        "POST",
        &format!("/proposals/{proposal_id}/undo"),
        Some("{}"),
    );
    assert_eq!(undone["action"], "undo");
    assert_eq!(undone["tool"], "propose_fake");
    assert_eq!(undone["tool_output"]["http_action"], true);
    assert_eq!(undone["proposal"]["status"], "undone");

    let trace = http_json_request(port, "GET", &format!("/runs/{run_id}/trace"), None);
    let event_kinds = trace["events"]
        .as_array()
        .expect("trace events is array")
        .iter()
        .map(|event| event["kind"].as_str().expect("event kind is string"))
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"proposal_created"));
    assert!(event_kinds.contains(&"proposal_decided"));
    assert!(event_kinds.contains(&"proposal_applied"));
    assert!(event_kinds.contains(&"proposal_undone"));

    let metrics = http_json_request(port, "GET", "/metrics/summary", None);
    assert_eq!(metrics["proposal_count"], 1);
    assert_eq!(metrics["proposal_created_count"], 1);
    assert_eq!(metrics["proposal_approved_count"], 1);
    assert_eq!(metrics["proposal_applied_count"], 1);
    assert_eq!(metrics["proposals_by_status"]["undone"], 1);
}

#[test]
fn http_server_applies_proposal_kind_policy() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-proposal-policy-store");
    let catalog_path = dir.path().join("catalog.auto-approve.json");
    let mut catalog = read_json("../../fixtures/contracts/catalog.valid.json");
    catalog["proposal_kinds"][0]["risk"] = serde_json::json!("low");
    catalog["proposal_kinds"][0]["approval_policy"] = serde_json::json!("auto_approve");
    std::fs::write(
        &catalog_path,
        serde_json::to_vec_pretty(&catalog).expect("catalog encodes"),
    )
    .expect("catalog written");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
        .args([
            "serve",
            "--catalog",
            catalog_path.to_str().expect("utf8 catalog path"),
            "--store",
            store.to_str().expect("utf8 store path"),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--mock-tool",
            r#"propose_fake={"policy_action":true}"#,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let run = http_json_request(
        port,
        "POST",
        "/agents/ai_chat/run",
        Some(r#"{"input":{"message":"proposal policy trace seed"}}"#),
    );
    let run_id = run["result"]["run_id"].as_str().expect("run_id is string");
    let created = http_json_request(
        port,
        "POST",
        "/proposals",
        Some(&format!(
            r#"{{"run_id":"{run_id}","agent_id":"ai_chat","kind":"fake","summary":"Auto proposal","payload":{{"value":17}}}}"#
        )),
    );
    let proposal_id = created["proposal_id"]
        .as_str()
        .expect("proposal id is string");
    assert_eq!(created["risk"], "low");
    assert_eq!(created["approval_policy"], "auto_approve");
    assert_eq!(created["approval_required"], false);
    assert_eq!(created["required_approval_level"], "none");
    assert_eq!(created["policy_id"], "finance.proposal.default");
    assert_eq!(created["policy_version"], "2026-06-28");
    assert_eq!(created["status"], "approved");

    let applied = http_json_request(
        port,
        "POST",
        &format!("/proposals/{proposal_id}/apply"),
        Some("{}"),
    );
    assert_eq!(applied["proposal"]["status"], "applied");
    assert_eq!(applied["tool_output"]["policy_action"], true);
}

#[test]
fn http_server_create_policy_hook_can_deny_proposal() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-proposal-create-policy-store");
    let config_path = dir.path().join("agent-runtime.toml");
    std::fs::write(
        &config_path,
        r#"[runtime]
hooks = [
  { name = "deny_http_create", event = "BeforeProposalCreate", kind = "process", effect = "policy", command = ["sh", "-c", "cat >/dev/null; printf '{\"decision\":\"deny\",\"reason\":\"http create blocked by policy\"}'"], timeout_ms = 1000 },
]
"#,
    )
    .expect("config written");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
        .args([
            "--config",
            config_path.to_str().expect("utf8 config path"),
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
            r#"propose_fake={"policy_action":true}"#,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let run = http_json_request(
        port,
        "POST",
        "/agents/ai_chat/run",
        Some(r#"{"input":{"message":"proposal create policy trace seed"}}"#),
    );
    let run_id = run["result"]["run_id"].as_str().expect("run_id is string");

    let (status, error) = http_json_status_request(
        port,
        "POST",
        "/proposals",
        Some(&format!(
            r#"{{"run_id":"{run_id}","agent_id":"ai_chat","kind":"fake","summary":"Denied HTTP proposal","payload":{{"value":17}}}}"#
        )),
    );
    assert_eq!(status, 403);
    assert_eq!(error["code"], "policy_denied");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("http create blocked by policy")
    );
    assert_eq!(error["details"]["event"], "BeforeProposalCreate");
    assert_eq!(error["details"]["run_id"], run_id);

    let proposal_count = std::fs::read_dir(store.join("proposals"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(proposal_count, 0);
    let trace = http_json_request(port, "GET", &format!("/runs/{run_id}/trace"), None);
    let hook = trace["events"]
        .as_array()
        .expect("trace events")
        .iter()
        .find(|event| {
            event["kind"] == "hook_invocation"
                && event["payload"]["hook_event"] == "BeforeProposalCreate"
        })
        .expect("proposal create policy hook invocation is traced");
    assert_eq!(hook["payload"]["hook_name"], "deny_http_create");
    assert_eq!(hook["payload"]["output"]["decision"], "deny");
}

#[test]
fn http_server_apply_policy_hook_can_deny_proposal() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-proposal-apply-policy-store");
    let config_path = dir.path().join("agent-runtime.toml");
    std::fs::write(
        &config_path,
        r#"[runtime]
hooks = [
  { name = "deny_http_apply", event = "BeforeProposalApply", kind = "process", effect = "policy", command = ["sh", "-c", "cat >/dev/null; printf '{\"decision\":\"deny\",\"reason\":\"http apply blocked by policy\"}'"], timeout_ms = 1000 },
]
"#,
    )
    .expect("config written");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
        .args([
            "--config",
            config_path.to_str().expect("utf8 config path"),
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
            r#"propose_fake={"policy_action":true}"#,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let run = http_json_request(
        port,
        "POST",
        "/agents/ai_chat/run",
        Some(r#"{"input":{"message":"proposal apply policy trace seed"}}"#),
    );
    let run_id = run["result"]["run_id"].as_str().expect("run_id is string");

    let created = http_json_request(
        port,
        "POST",
        "/proposals",
        Some(&format!(
            r#"{{"run_id":"{run_id}","agent_id":"ai_chat","kind":"fake","summary":"HTTP apply denied proposal","payload":{{"value":19}}}}"#
        )),
    );
    let proposal_id = created["proposal_id"]
        .as_str()
        .expect("proposal id is string");
    http_json_request(
        port,
        "POST",
        &format!("/proposals/{proposal_id}/decision"),
        Some(r#"{"decision":"approve","approval_level":"single_user"}"#),
    );

    let (status, error) = http_json_status_request(
        port,
        "POST",
        &format!("/proposals/{proposal_id}/apply"),
        Some("{}"),
    );
    assert_eq!(status, 403);
    assert_eq!(error["code"], "policy_denied");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("http apply blocked by policy")
    );
    assert_eq!(error["details"]["event"], "BeforeProposalApply");
    assert_eq!(error["details"]["proposal_id"], proposal_id);
    assert_eq!(error["details"]["tool"], "propose_fake");

    let stored = read_json(store.join("proposals").join(format!("{proposal_id}.json")));
    assert_eq!(stored["status"], "approved");
    let trace = http_json_request(port, "GET", &format!("/runs/{run_id}/trace"), None);
    let hook = trace["events"]
        .as_array()
        .expect("trace events")
        .iter()
        .find(|event| {
            event["kind"] == "hook_invocation"
                && event["payload"]["hook_event"] == "BeforeProposalApply"
        })
        .expect("proposal apply policy hook invocation is traced");
    assert_eq!(hook["payload"]["hook_name"], "deny_http_apply");
    assert_eq!(hook["payload"]["output"]["decision"], "deny");
}
