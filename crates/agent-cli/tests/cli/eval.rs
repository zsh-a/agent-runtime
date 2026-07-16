use super::*;

#[test]
fn eval_runs_catalog_dry_run_and_checks_expectations() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("eval-store");
    let output = agent_cmd()
        .args([
            "eval",
            "../../evals/catalog_dry_run.yaml",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("eval report is JSON");
    assert_eq!(report["id"], "catalog_dry_run_basic");
    assert_eq!(report["passed"], true);
    assert_eq!(report["agent_id"], "ai_chat");
    assert_eq!(report["status"], "completed");
    assert!(
        report["checked"]
            .as_array()
            .expect("checked is array")
            .iter()
            .any(|value| value == "trace_event:catalog_dry_run.agent_selected")
    );
    assert!(
        report["checked"]
            .as_array()
            .expect("checked is array")
            .iter()
            .any(|value| value == "golden_trace")
    );
    assert!(
        report["checked"]
            .as_array()
            .expect("checked is array")
            .iter()
            .any(|value| value == "prompt_manifest")
    );
}

#[test]
fn eval_checks_expected_tool_call_sequence() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("eval-tool-store");
    let output = agent_cmd()
        .args([
            "eval",
            "../../evals/tool_call_sequence.yaml",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("eval report is JSON");
    assert_eq!(report["id"], "catalog_dry_run_tool_call_sequence");
    assert_eq!(report["passed"], true);
    assert!(
        report["checked"]
            .as_array()
            .expect("checked is array")
            .iter()
            .any(|value| value == "tool_calls")
    );
}

#[test]
fn eval_checks_expected_proposals() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("eval-proposal-store");
    let output = agent_cmd()
        .args([
            "eval",
            "../../evals/proposal_expectation.yaml",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("eval report is JSON");
    assert_eq!(report["id"], "catalog_dry_run_proposal_expectation");
    assert_eq!(report["passed"], true);
    assert!(
        report["checked"]
            .as_array()
            .expect("checked is array")
            .iter()
            .any(|value| value == "proposals")
    );

    let proposals_dir = store.join("proposals");
    let proposal_files = std::fs::read_dir(&proposals_dir)
        .expect("proposal dir exists")
        .collect::<Result<Vec<_>, _>>()
        .expect("proposal files read");
    assert_eq!(proposal_files.len(), 1);
    let proposal = read_json(proposal_files[0].path());
    assert_eq!(proposal["kind"], "fake");
    assert_eq!(proposal["status"], "pending_approval");
    assert_eq!(proposal["summary"], "Eval fake proposal");
    assert_eq!(proposal["payload"]["value"], 19);
}

#[test]
fn eval_runs_scoring_hook_and_reports_score() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("eval-score-store");
    let eval_file = dir.path().join("scored.yaml");
    let catalog = std::path::Path::new("../../fixtures/contracts/catalog.valid.json")
        .canonicalize()
        .expect("catalog path");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    std::fs::write(
        &eval_file,
        format!(
            r#"id: scored_eval
agent_id: ai_chat
catalog: "{}"
input:
  message: scored fixture
expect:
  status: completed
  agent_id: ai_chat
  output_mode: catalog_dry_run
scoring_hook:
  command:
    - "{}"
    - dev-score-hook
  min_score: 0.5
"#,
            catalog.display(),
            agent_bin.display()
        ),
    )
    .expect("scored eval writes");

    let output = agent_cmd()
        .args([
            "eval",
            eval_file.to_str().expect("utf8 eval path"),
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("eval report is JSON");
    assert_eq!(report["id"], "scored_eval");
    assert_eq!(report["passed"], true);
    assert_eq!(report["score"], 1.0);
    assert_eq!(
        report["hooks"][0]["protocol_version"],
        serde_json::json!("agent.v1")
    );
    assert_eq!(
        report["hooks"][0]["hook_event"],
        serde_json::json!("AfterAgentStep")
    );
    assert_eq!(
        report["hooks"][0]["hook_kind"],
        serde_json::json!("process")
    );
    assert_eq!(
        report["hooks"][0]["hook_name"],
        serde_json::json!("eval.scoring_hook")
    );
    assert_eq!(report["hooks"][0]["status"], serde_json::json!("completed"));
    assert_eq!(report["hooks"][0]["agent_id"], serde_json::json!("ai_chat"));
    assert_eq!(report["hooks"][0]["output"]["score"], 1.0);
    assert!(
        report["hooks"][0]["duration_ms"]
            .as_u64()
            .is_some_and(|value| value < u64::MAX)
    );
    assert!(
        report["scoring_comment"]
            .as_str()
            .is_some_and(|comment| comment.contains("completed"))
    );
    assert!(
        report["checked"]
            .as_array()
            .expect("checked is array")
            .iter()
            .any(|value| value == "scoring_hook")
    );
}

#[test]
fn eval_can_run_directory_suites() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("eval-suite-store");
    let output = agent_cmd()
        .args([
            "eval",
            "../../evals",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("suite report is JSON");
    assert_eq!(report["passed"], true);
    assert_eq!(report["total"], 3);
    assert_eq!(report["passed_count"], 3);
    assert_eq!(report["failed_count"], 0);
    let ids = report["reports"]
        .as_array()
        .expect("reports is array")
        .iter()
        .map(|report| report["id"].as_str().expect("report id is string"))
        .collect::<Vec<_>>();
    assert!(ids.contains(&"catalog_dry_run_basic"));
    assert!(ids.contains(&"catalog_dry_run_tool_call_sequence"));
    assert!(ids.contains(&"catalog_dry_run_proposal_expectation"));
}

#[test]
fn eval_create_generates_case_and_golden_trace_from_run_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let eval_file = dir.path().join("generated_eval.yaml");

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
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let result: Value = serde_json::from_slice(&output).expect("run result is JSON");
    let run_id = result["run_id"].as_str().expect("run_id is string");

    let create_report = agent_cmd()
        .args([
            "eval",
            "create",
            "--from-run",
            run_id,
            "--store",
            store.to_str().expect("utf8 store path"),
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--out",
            eval_file.to_str().expect("utf8 eval path"),
            "--id",
            "generated_from_run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let create_report: Value =
        serde_json::from_slice(&create_report).expect("create report is JSON");
    assert_eq!(create_report["id"], "generated_from_run");
    assert_eq!(create_report["run_id"], run_id);
    assert_eq!(create_report["agent_id"], "ai_chat");

    let generated = std::fs::read_to_string(&eval_file).expect("eval file exists");
    assert!(generated.contains("id: generated_from_run"));
    assert!(generated.contains("golden_trace: golden/generated_from_run.trace.json"));
    assert!(generated.contains("prompt_manifest:"));
    assert!(generated.contains("id: ai_chat_prompt"));
    assert!(generated.contains("version: ai_chat.prompt.v1"));
    assert!(generated.contains("tool_schema_version: chat.tools.v1"));
    assert!(
        generated
            .contains("blake3:f4d4a59a0aed2318f1a9443b2a51a518cc8296305e2f8db1e1192aac1cc7cd02")
    );
    assert!(
        dir.path()
            .join("golden/generated_from_run.trace.json")
            .exists()
    );

    let eval_report = agent_cmd()
        .args([
            "eval",
            eval_file.to_str().expect("utf8 eval path"),
            "--store",
            dir.path()
                .join("generated-eval-store")
                .to_str()
                .expect("utf8 generated store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let eval_report: Value = serde_json::from_slice(&eval_report).expect("eval report is JSON");
    assert_eq!(eval_report["id"], "generated_from_run");
    assert_eq!(eval_report["passed"], true);
    assert!(
        eval_report["checked"]
            .as_array()
            .expect("checked is array")
            .iter()
            .any(|value| value == "golden_trace")
    );
}

#[test]
fn eval_create_generates_proposal_expectations_from_run_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let input_file = dir.path().join("proposal-run-request.json");
    let eval_file = dir.path().join("generated_proposal_eval.yaml");
    std::fs::write(
        &input_file,
        r#"{
  "message": "generate proposal expectation",
  "proposal": {
    "kind": "fake",
    "summary": "Generated eval fake proposal",
    "payload": {
      "value": 23
    }
  }
}"#,
    )
    .expect("input file written");

    let output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_file.to_str().expect("utf8 input path"),
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let result: Value = serde_json::from_slice(&output).expect("run result is JSON");
    let run_id = result["run_id"].as_str().expect("run_id is string");

    agent_cmd()
        .args([
            "eval",
            "create",
            "--from-run",
            run_id,
            "--store",
            store.to_str().expect("utf8 store path"),
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--out",
            eval_file.to_str().expect("utf8 eval path"),
            "--id",
            "generated_proposal_from_run",
        ])
        .assert()
        .success();

    let generated = std::fs::read_to_string(&eval_file).expect("eval file exists");
    assert!(generated.contains("id: generated_proposal_from_run"));
    assert!(generated.contains("proposal:"));
    assert!(generated.contains("summary: Generated eval fake proposal"));
    assert!(generated.contains("proposals:"));
    assert!(generated.contains("min_count: 1"));
    assert!(generated.contains("kinds:"));
    assert!(generated.contains("- fake"));
    assert!(generated.contains("statuses:"));
    assert!(generated.contains("- pending_approval"));
    assert!(generated.contains("prompt_manifest:"));

    let eval_report = agent_cmd()
        .args([
            "eval",
            eval_file.to_str().expect("utf8 eval path"),
            "--store",
            dir.path()
                .join("generated-proposal-eval-store")
                .to_str()
                .expect("utf8 generated store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let eval_report: Value = serde_json::from_slice(&eval_report).expect("eval report is JSON");
    assert_eq!(eval_report["id"], "generated_proposal_from_run");
    assert_eq!(eval_report["passed"], true);
    let checked = eval_report["checked"].as_array().expect("checked is array");
    assert!(checked.iter().any(|value| value == "proposals"));
    assert!(checked.iter().any(|value| value == "prompt_manifest"));
}
