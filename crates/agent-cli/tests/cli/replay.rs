use super::*;

#[test]
fn replay_can_execute_from_trace() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let trace_path = dir.path().join("source-trace.json");
    let replay_trace_path = dir.path().join("replay-trace.json");

    agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            "../../fixtures/contracts/run-request.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--trace-out",
            trace_path.to_str().expect("utf8 trace path"),
        ])
        .assert()
        .success();

    let output = agent_cmd()
        .args([
            "replay",
            trace_path.to_str().expect("utf8 trace path"),
            "--mode",
            "live",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--trace-out",
            replay_trace_path.to_str().expect("utf8 replay trace path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("replay report is JSON");
    assert_eq!(report["mode"], "live");
    assert_eq!(report["agent_id"], "ai_chat");
    assert_eq!(report["result"]["status"], "completed");
    assert_eq!(report["output_matches"], true);
    assert_ne!(report["source_run_id"], report["replay_run_id"]);
    assert_eq!(report["trace"]["events"][0]["payload"]["trigger"], "replay");

    let replay_trace = read_json(replay_trace_path);
    assert_eq!(
        replay_trace["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );
}

#[test]
fn replay_can_run_deterministically_from_trace_without_writing_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let trace_path = dir.path().join("source-trace.json");
    let replay_trace_path = dir.path().join("deterministic-trace.json");

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
            "--trace-out",
            trace_path.to_str().expect("utf8 trace path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run: Value = serde_json::from_slice(&output).expect("run result is JSON");
    let source_run_id = run["run_id"].as_str().expect("source run id");

    let output = agent_cmd()
        .args([
            "replay",
            trace_path.to_str().expect("utf8 trace path"),
            "--mode",
            "deterministic",
            "--trace-out",
            replay_trace_path.to_str().expect("utf8 replay trace path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("replay report is JSON");
    assert_eq!(report["mode"], "deterministic");
    assert_eq!(report["source_run_id"], source_run_id);
    assert_eq!(report["replay_run_id"], source_run_id);
    assert_eq!(report["output_matches"], true);
    assert_eq!(report["result"]["output"], run["output"]);

    let deterministic_trace = read_json(replay_trace_path);
    assert_eq!(deterministic_trace["run_id"], source_run_id);
    let run_files = std::fs::read_dir(store.join("runs"))
        .expect("run dir exists")
        .count();
    assert_eq!(run_files, 1);
}
