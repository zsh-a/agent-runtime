use super::*;

#[test]
fn proposal_cli_persists_and_decides_proposals() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("proposal-store");

    let created = agent_cmd()
        .args([
            "proposal",
            "create",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--run-id",
            "run_test",
            "--agent-id",
            "ai_chat",
            "--kind",
            "fake",
            "--summary",
            "Review fake proposal",
            "--payload-json",
            r#"{"value":7}"#,
            "--diffs-json",
            r#"[{"path":"/value","operation":"replace","before":6,"after":7,"metadata":{"field":"value"}}]"#,
            "--warnings-json",
            r#"[{"severity":"warning","code":"review_required","message":"Review value change","metadata":{"field":"value"}}]"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let created: Value = serde_json::from_slice(&created).expect("proposal is JSON");
    let proposal_id = created["proposal_id"]
        .as_str()
        .expect("proposal_id is string");
    assert_eq!(created["protocol_version"], "agent.v1");
    assert_eq!(created["run_id"], "run_test");
    assert_eq!(created["status"], "pending_approval");
    assert_eq!(created["payload"]["value"], 7);
    assert_eq!(created["diffs"][0]["path"], "/value");
    assert_eq!(created["diffs"][0]["before"], 6);
    assert_eq!(created["diffs"][0]["after"], 7);
    assert_eq!(created["warnings"][0]["severity"], "warning");
    assert_eq!(created["warnings"][0]["code"], "review_required");

    let listed = agent_cmd()
        .args([
            "proposal",
            "list",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--run-id",
            "run_test",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listed: Value = serde_json::from_slice(&listed).expect("proposal list is JSON");
    assert_eq!(listed[0]["proposal_id"], proposal_id);

    let inspected = agent_cmd()
        .args([
            "proposal",
            "inspect",
            proposal_id,
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let inspected: Value = serde_json::from_slice(&inspected).expect("proposal is JSON");
    assert_eq!(inspected["proposal_id"], proposal_id);
    assert_eq!(inspected["diffs"][0]["metadata"]["field"], "value");
    assert_eq!(inspected["warnings"][0]["message"], "Review value change");

    let decided = agent_cmd()
        .args([
            "proposal",
            "decide",
            proposal_id,
            "--store",
            store.to_str().expect("utf8 store path"),
            "--decision",
            "approve",
            "--approval-level",
            "single_user",
            "--decided-by",
            "user_cli_reviewer",
            "--comment",
            "approved in test",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let decided: Value = serde_json::from_slice(&decided).expect("decision is JSON");
    assert_eq!(decided["decision"]["decision"], "approve");
    assert_eq!(decided["decision"]["approval_level"], "single_user");
    assert_eq!(decided["decision"]["decided_by"], "user_cli_reviewer");
    assert_eq!(decided["decision"]["comment"], "approved in test");
    assert_eq!(decided["proposal"]["status"], "approved");

    let applied = agent_cmd()
        .args([
            "proposal",
            "apply",
            proposal_id,
            "--store",
            store.to_str().expect("utf8 store path"),
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--mock-tool",
            r#"propose_fake={"applied":true}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let applied: Value = serde_json::from_slice(&applied).expect("apply response is JSON");
    assert_eq!(applied["action"], "apply");
    assert_eq!(applied["tool"], "propose_fake");
    assert_eq!(applied["tool_output"]["applied"], true);
    assert_eq!(applied["proposal"]["status"], "applied");

    let undone = agent_cmd()
        .args([
            "proposal",
            "undo",
            proposal_id,
            "--store",
            store.to_str().expect("utf8 store path"),
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--mock-tool",
            r#"propose_fake={"undone":true}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let undone: Value = serde_json::from_slice(&undone).expect("undo response is JSON");
    assert_eq!(undone["action"], "undo");
    assert_eq!(undone["tool"], "propose_fake");
    assert_eq!(undone["tool_output"]["undone"], true);
    assert_eq!(undone["proposal"]["status"], "undone");

    let stored = read_json(store.join("proposals").join(format!("{proposal_id}.json")));
    assert_eq!(stored["status"], "undone");
}

#[test]
fn proposal_cli_rejects_expired_apply() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("expired-proposal-store");

    let created = agent_cmd()
        .args([
            "proposal",
            "create",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--run-id",
            "run_test",
            "--agent-id",
            "ai_chat",
            "--kind",
            "fake",
            "--summary",
            "Expired fake proposal",
            "--payload-json",
            r#"{"value":7}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let created: Value = serde_json::from_slice(&created).expect("proposal is JSON");
    let proposal_id = created["proposal_id"]
        .as_str()
        .expect("proposal_id is string");
    let proposal_path = store.join("proposals").join(format!("{proposal_id}.json"));
    let mut stored = read_json(&proposal_path);
    stored["status"] = serde_json::json!("approved");
    stored["expires_at"] = serde_json::json!("2000-01-01T00:00:00Z");
    std::fs::write(
        &proposal_path,
        serde_json::to_vec_pretty(&stored).expect("proposal encodes"),
    )
    .expect("proposal writes");

    let stderr = agent_cmd()
        .args([
            "proposal",
            "apply",
            proposal_id,
            "--store",
            store.to_str().expect("utf8 store path"),
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--mock-tool",
            r#"propose_fake={"applied":true}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        stderr.contains("expired before") && stderr.contains("marked expired"),
        "stderr should explain expired apply rejection"
    );

    let stored = read_json(proposal_path);
    assert_eq!(stored["status"], "expired");
}
