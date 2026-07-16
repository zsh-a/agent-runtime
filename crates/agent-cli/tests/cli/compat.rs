use super::*;

#[test]
fn compat_check_runs_business_integration_smoke() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("compat-store");
    let bundle = dir.path().join("compat-bundle");
    let output = agent_cmd()
        .args([
            "compat",
            "check",
            "--catalog",
            "../../examples/business-integration/catalog.json",
            "--tool-source",
            "../../examples/business-integration/tool-source.json",
            "--agent-id",
            "customer_summary_agent",
            "--run-input",
            "../../examples/business-integration/run-customer-summary.json",
            "--proposal-input",
            "../../examples/business-integration/run-followup-proposal.json",
            "--schema-root",
            "../../schemas",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--debug-bundle-out",
            bundle.to_str().expect("utf8 bundle path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("compat report is JSON");
    assert_eq!(report["status"], "passed");
    let steps = report["steps"].as_array().expect("steps array");
    for expected in [
        "catalog_schema",
        "tool_source_schema",
        "catalog_agent",
        "run_fixture",
        "proposal_fixture",
        "trace_redaction",
    ] {
        assert!(
            steps
                .iter()
                .any(|step| step["name"] == expected && step["status"] == "passed"),
            "step {expected} passed"
        );
    }
    assert_eq!(report["run"]["agent_id"], "customer_summary_agent");
    assert_eq!(report["proposal_run"]["agent_id"], "customer_summary_agent");
    assert!(
        steps
            .iter()
            .find(|step| step["name"] == "proposal_fixture")
            .expect("proposal step exists")["details"]["proposal_count"]
            .as_u64()
            .expect("proposal count")
            > 0
    );
    assert!(bundle.join("redactions.json").exists());
    assert!(bundle.join("manifest.json").exists());
}
