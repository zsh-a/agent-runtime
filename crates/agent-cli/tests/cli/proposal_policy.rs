use super::*;

#[test]
fn proposal_cli_apply_policy_hook_can_deny() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("proposal-policy-store");
    let config_path = dir.path().join("agent-runtime.toml");
    std::fs::write(
        &config_path,
        r#"[runtime]
hooks = [
  { name = "deny_apply", event = "BeforeProposalApply", kind = "process", effect = "policy", command = ["sh", "-c", "cat >/dev/null; printf '{\"decision\":\"deny\",\"reason\":\"blocked by apply policy\"}'"], timeout_ms = 1000 },
]
"#,
    )
    .expect("config written");

    let run = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            "../../fixtures/contracts/run-request.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run: Value = serde_json::from_slice(&run).expect("run is JSON");
    let run_id = run["run_id"].as_str().expect("run_id is string");

    let created = agent_cmd()
        .args([
            "proposal",
            "create",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--run-id",
            run_id,
            "--agent-id",
            "ai_chat",
            "--kind",
            "fake",
            "--summary",
            "Policy guarded proposal",
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

    agent_cmd()
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
        .success();

    let stderr = agent_cmd()
        .args([
            "--config",
            config_path.to_str().expect("utf8 config path"),
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
    assert!(
        String::from_utf8_lossy(&stderr).contains("blocked by apply policy"),
        "stderr should explain policy denial"
    );

    let stored = read_json(store.join("proposals").join(format!("{proposal_id}.json")));
    assert_eq!(stored["status"], "approved");
    let trace = read_json(store.join("traces").join(format!("{run_id}.trace.json")));
    let hook = trace["events"]
        .as_array()
        .expect("trace events")
        .iter()
        .find(|event| {
            event["kind"] == "hook_invocation"
                && event["payload"]["hook_event"] == "BeforeProposalApply"
        })
        .expect("proposal apply policy hook invocation is traced");
    assert_eq!(hook["payload"]["hook_name"], "deny_apply");
    assert_eq!(hook["payload"]["output"]["decision"], "deny");
}

#[test]
fn proposal_cli_create_policy_hook_can_deny() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("proposal-create-policy-store");
    let config_path = dir.path().join("agent-runtime.toml");
    std::fs::write(
        &config_path,
        r#"[runtime]
hooks = [
  { name = "deny_create", event = "BeforeProposalCreate", kind = "process", effect = "policy", command = ["sh", "-c", "cat >/dev/null; printf '{\"decision\":\"deny\",\"reason\":\"blocked by create policy\"}'"], timeout_ms = 1000 },
]
"#,
    )
    .expect("config written");

    let run = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            "../../fixtures/contracts/run-request.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run: Value = serde_json::from_slice(&run).expect("run is JSON");
    let run_id = run["run_id"].as_str().expect("run_id is string");

    let stderr = agent_cmd()
        .args([
            "--config",
            config_path.to_str().expect("utf8 config path"),
            "proposal",
            "create",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--run-id",
            run_id,
            "--agent-id",
            "ai_chat",
            "--kind",
            "fake",
            "--summary",
            "Policy guarded create",
            "--payload-json",
            r#"{"value":7}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(
        String::from_utf8_lossy(&stderr).contains("blocked by create policy"),
        "stderr should explain policy denial"
    );

    let proposal_count = std::fs::read_dir(store.join("proposals"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(proposal_count, 0);
    let trace = read_json(store.join("traces").join(format!("{run_id}.trace.json")));
    let hook = trace["events"]
        .as_array()
        .expect("trace events")
        .iter()
        .find(|event| {
            event["kind"] == "hook_invocation"
                && event["payload"]["hook_event"] == "BeforeProposalCreate"
        })
        .expect("proposal create policy hook invocation is traced");
    assert_eq!(hook["payload"]["hook_name"], "deny_create");
    assert_eq!(hook["payload"]["output"]["decision"], "deny");
}
