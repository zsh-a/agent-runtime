use super::*;

#[test]
fn workflow_cli_runs_dag_and_persists_node_traces() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("workflow-store");
    let input = dir.path().join("workflow.json");
    std::fs::write(
        &input,
        r#"{
  "protocol_version": "agent.v1",
  "workflow_id": "workflow_cli_test",
  "scope": {"type": "tenant", "id": "tenant_workflow_cli"},
  "nodes": [
    {
      "node_id": "collect",
      "agent_id": "ai_chat",
      "input": {"message": "collect"}
    },
    {
      "node_id": "summarize",
      "agent_id": "ai_chat",
      "depends_on": ["collect"],
      "input": {"message": "summarize"},
      "input_mappings": [
        {
          "from_node": "collect",
          "from_path": "/input/message",
          "to_path": "/from_collect"
        }
      ]
    }
  ],
  "metadata": {"case": "catalog_cli"}
}"#,
    )
    .expect("workflow writes");

    let output = agent_cmd()
        .args([
            "workflow",
            "run",
            input.to_str().expect("utf8 workflow path"),
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result: Value = serde_json::from_slice(&output).expect("workflow result is JSON");
    assert_eq!(result["protocol_version"], "agent.v1");
    assert_eq!(result["workflow_id"], "workflow_cli_test");
    assert_eq!(result["status"], "completed");
    let nodes = result["nodes"].as_array().expect("workflow nodes");
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["node_id"], "collect");
    assert_eq!(
        nodes[0]["trace"]["workflow"]["workflow_id"],
        "workflow_cli_test"
    );
    assert_eq!(nodes[1]["depends_on"], serde_json::json!(["collect"]));
    assert_eq!(nodes[1]["output"]["input"]["from_collect"], "collect");
    assert_eq!(
        nodes[1]["trace"]["workflow"]["dependencies"][0]["run_id"],
        nodes[0]["run_id"]
    );

    for node in nodes {
        let run_id = node["run_id"].as_str().expect("node run id");
        let stored_run = agent_cmd()
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
        let stored_run: Value = serde_json::from_slice(&stored_run).expect("run record is JSON");
        assert_eq!(stored_run["scope"]["type"], "tenant");
        assert_eq!(stored_run["scope"]["id"], "tenant_workflow_cli");
        let stored_trace = read_json(store.join("traces").join(format!("{run_id}.trace.json")));
        assert_eq!(stored_trace["run_id"], run_id);
        assert_eq!(stored_trace["workflow"]["workflow_id"], "workflow_cli_test");
        assert_eq!(
            stored_trace["events"][1]["kind"],
            "catalog_dry_run.agent_selected"
        );
    }
}

#[test]
fn workflow_cli_validates_request_schema() {
    let dir = tempfile::tempdir().expect("temp dir");
    let input = dir.path().join("invalid-workflow.json");
    std::fs::write(
        &input,
        r#"{"protocol_version":"agent.v1","workflow_id":"workflow_invalid"}"#,
    )
    .expect("workflow writes");

    let stderr = agent_cmd()
        .args([
            "workflow",
            "run",
            input.to_str().expect("utf8 workflow path"),
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--store",
            dir.path()
                .join("workflow-store")
                .to_str()
                .expect("utf8 store path"),
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(stderr).expect("stderr is utf8");
    assert!(stderr.contains("workflow request failed schema validation"));
}
