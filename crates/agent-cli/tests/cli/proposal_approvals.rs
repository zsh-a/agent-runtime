use super::*;

#[test]
fn proposal_cli_requires_sufficient_approval_level() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("proposal-level-store");

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
            "Admin fake proposal",
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
    stored["required_approval_level"] = serde_json::json!("admin");
    std::fs::write(
        &proposal_path,
        serde_json::to_vec_pretty(&stored).expect("proposal encodes"),
    )
    .expect("proposal writes");

    let stderr = agent_cmd()
        .args([
            "proposal",
            "decide",
            proposal_id,
            "--store",
            store.to_str().expect("utf8 store path"),
            "--decision",
            "approve",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        stderr.contains("does not satisfy required level"),
        "stderr should explain insufficient approval level"
    );
    assert_eq!(read_json(&proposal_path)["status"], "pending_approval");

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
            "admin",
            "--decided-by",
            "user_admin_reviewer",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let decided: Value = serde_json::from_slice(&decided).expect("decision is JSON");
    assert_eq!(decided["decision"]["approval_level"], "admin");
    assert_eq!(decided["decision"]["decided_by"], "user_admin_reviewer");
    assert_eq!(decided["proposal"]["status"], "approved");
}

#[test]
fn proposal_cli_accumulates_multi_approver_decisions() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("proposal-chain-store");

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
            "Multi approver fake proposal",
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
    stored["required_approval_level"] = serde_json::json!("multi_approver");
    stored["required_approver_count"] = serde_json::json!(2);
    std::fs::write(
        &proposal_path,
        serde_json::to_vec_pretty(&stored).expect("proposal encodes"),
    )
    .expect("proposal writes");

    let first = agent_cmd()
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
            "user_reviewer_one",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first: Value = serde_json::from_slice(&first).expect("decision is JSON");
    assert_eq!(first["proposal"]["status"], "pending_approval");
    assert_eq!(
        first["proposal"]["approval_decisions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let duplicate = agent_cmd()
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
            "user_reviewer_one",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let duplicate = String::from_utf8_lossy(&duplicate);
    assert!(duplicate.contains("has already approved"));
    assert_eq!(read_json(&proposal_path)["status"], "pending_approval");

    let second = agent_cmd()
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
            "user_reviewer_two",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second: Value = serde_json::from_slice(&second).expect("decision is JSON");
    assert_eq!(second["proposal"]["status"], "approved");
    assert_eq!(
        second["proposal"]["approval_decisions"][0]["decided_by"],
        "user_reviewer_one"
    );
    assert_eq!(
        second["proposal"]["approval_decisions"][1]["decided_by"],
        "user_reviewer_two"
    );
    assert_eq!(second["proposal"]["required_approver_count"], 2);
}
