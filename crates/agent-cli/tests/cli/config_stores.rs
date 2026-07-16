use super::*;

#[test]
fn config_store_backend_sqlite_drives_runtime_commands() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("sqlite-runtime-store");
    let config_path = dir.path().join("agent-runtime.toml");
    let registry = std::path::Path::new("../../examples/agents.yaml")
        .canonicalize()
        .expect("registry path");
    std::fs::write(
        &config_path,
        format!(
            r#"[runtime]
store = "{}"
store_backend = "sqlite"

[runtime.sources]
registry = "{}"
"#,
            store.display(),
            registry.display()
        ),
    )
    .expect("config written");

    let shown = agent_cmd()
        .args(["--config", config_path.to_str().unwrap(), "config", "show"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown: Value = serde_json::from_slice(&shown).expect("config is JSON");
    assert_eq!(shown["runtime"]["store_backend"], "sqlite");

    let output = agent_cmd()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "run",
            "echo_agent",
            "--input",
            "../../examples/fixtures/echo-input.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output: Value = serde_json::from_slice(&output).expect("run is JSON");
    let run_id = output["run_id"].as_str().expect("run id");
    assert_eq!(output["status"], "completed");
    assert!(store.join("runtime.sqlite").exists());
    let stored_trace = read_sqlite_trace(&store, run_id);
    assert_eq!(stored_trace.run_id.0, run_id);
    assert_eq!(stored_trace.agent_id, "echo_agent");
    assert!(
        !store
            .join("traces")
            .join(format!("{run_id}.trace.json"))
            .exists(),
        "sqlite runtime trace should not be written through the file trace store"
    );
    assert!(!store.join("runs").join(format!("{run_id}.json")).exists());

    let inspected = agent_cmd()
        .args(["--config", config_path.to_str().unwrap(), "inspect", run_id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let inspected: Value = serde_json::from_slice(&inspected).expect("run record is JSON");
    assert_eq!(inspected["run_id"], run_id);
    assert_eq!(inspected["agent_id"], "echo_agent");

    let session = agent_cmd()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "session",
            "create",
            "--title",
            "SQLite session",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let session: Value = serde_json::from_slice(&session).expect("session is JSON");
    let session_id = session["session"]["session_id"]
        .as_str()
        .expect("session id");

    let sessions = agent_cmd()
        .args(["--config", config_path.to_str().unwrap(), "session", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let sessions: Value = serde_json::from_slice(&sessions).expect("sessions are JSON");
    assert!(
        sessions
            .as_array()
            .expect("sessions array")
            .iter()
            .any(|session| session["session_id"] == session_id
                && session["title"] == "SQLite session")
    );

    let proposal = agent_cmd()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "proposal",
            "create",
            "--run-id",
            run_id,
            "--agent-id",
            "echo_agent",
            "--kind",
            "change",
            "--summary",
            "SQLite proposal",
            "--payload-json",
            "{}",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let proposal: Value = serde_json::from_slice(&proposal).expect("proposal is JSON");
    let proposal_id = proposal["proposal_id"].as_str().expect("proposal id");

    let proposals = agent_cmd()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "proposal",
            "list",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let proposals: Value = serde_json::from_slice(&proposals).expect("proposals are JSON");
    assert!(
        proposals
            .as_array()
            .expect("proposals array")
            .iter()
            .any(|proposal| proposal["proposal_id"] == proposal_id
                && proposal["summary"] == "SQLite proposal")
    );

    let summary = agent_cmd()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "metrics",
            "summary",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let summary: Value = serde_json::from_slice(&summary).expect("metrics summary is JSON");
    assert_eq!(summary["store_root"], store.to_string_lossy().as_ref());
    assert_eq!(summary["run_count"], 1);
    assert_eq!(summary["proposal_count"], 1);
}

#[test]
fn sqlite_store_backend_drives_cli_workflows() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("sqlite-guard-store");
    let config_path = dir.path().join("agent-runtime.toml");
    let trace_path = std::path::Path::new("../../fixtures/contracts/trace.valid.json")
        .canonicalize()
        .expect("trace fixture exists");
    let registry_path = std::path::Path::new("../../examples/agents.yaml")
        .canonicalize()
        .expect("registry fixture exists");
    std::fs::write(
        &config_path,
        format!(
            r#"[runtime]
store = "{}"
store_backend = "sqlite"
"#,
            store.display()
        ),
    )
    .expect("config written");

    let config = config_path.to_str().expect("utf8 config path").to_owned();
    let trace = trace_path.to_str().expect("utf8 trace path").to_owned();
    let registry = registry_path
        .to_str()
        .expect("utf8 registry path")
        .to_owned();
    let bundle_path = dir.path().join("bundle");
    let bundle = bundle_path.to_str().expect("utf8 bundle path").to_owned();
    let compat_store_path = dir.path().join("compat-store");
    let compat_store = compat_store_path
        .to_str()
        .expect("utf8 compat store path")
        .to_owned();
    let compat_bundle_path = dir.path().join("compat-bundle");
    let compat_bundle = compat_bundle_path
        .to_str()
        .expect("utf8 compat bundle path")
        .to_owned();

    assert!(
        !store.join("tui.log").exists(),
        "sqlite backend should not create a file-store TUI log before TUI starts"
    );
    assert!(
        !store.join("runtime.sqlite").exists(),
        "setup should not open the SQLite runtime store"
    );

    let view_output = agent_cmd()
        .args(["--config", &config, "replay", &trace, "--mode", "view"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let viewed: Value = serde_json::from_slice(&view_output).expect("view replay prints JSON");
    assert_eq!(viewed["run_id"], "run_018f0000-0000-7000-8000-000000000000");

    let deterministic_trace = dir.path().join("deterministic-trace.json");
    let deterministic_output = agent_cmd()
        .args([
            "--config",
            &config,
            "replay",
            &trace,
            "--mode",
            "deterministic",
            "--trace-out",
            deterministic_trace
                .to_str()
                .expect("utf8 deterministic trace"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let deterministic: Value =
        serde_json::from_slice(&deterministic_output).expect("deterministic replay is JSON");
    assert_eq!(deterministic["mode"], "deterministic");
    assert_eq!(
        deterministic["source_run_id"],
        "run_018f0000-0000-7000-8000-000000000000"
    );
    assert_eq!(
        read_json(deterministic_trace)["run_id"],
        deterministic["source_run_id"]
    );
    assert!(
        !store.join("runtime.sqlite").exists(),
        "view and deterministic replay should not open the SQLite runtime store"
    );

    let tui_output = agent_cmd()
        .args([
            "--config",
            &config,
            "tui",
            "--registry",
            &registry,
            "--once",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let tui_output = String::from_utf8(tui_output).expect("tui output is utf8");
    assert!(tui_output.contains("Agent Runtime"));
    assert!(tui_output.contains("Chat"));
    assert!(store.join("runtime.sqlite").exists());
    assert!(
        !store.join("tui.log").exists(),
        "sqlite TUI should not create a file-store TUI log"
    );

    let live_output = agent_cmd()
        .args([
            "--config",
            &config,
            "replay",
            &trace,
            "--mode",
            "live",
            "--registry",
            &registry,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let live: Value = serde_json::from_slice(&live_output).expect("live replay is JSON");
    let replay_run_id = live["replay_run_id"]
        .as_str()
        .expect("replay run id is string");
    assert_eq!(live["mode"], "live");
    assert_eq!(live["agent_id"], "echo_agent");
    assert!(store.join("runtime.sqlite").exists());
    let stored_trace = read_sqlite_trace(&store, replay_run_id);
    assert_eq!(stored_trace.run_id.0, replay_run_id);
    assert_eq!(stored_trace.agent_id, "echo_agent");
    assert!(
        !store
            .join("traces")
            .join(format!("{replay_run_id}.trace.json"))
            .exists(),
        "sqlite replay trace should not be written through the file trace store"
    );

    let bundle_output = agent_cmd()
        .args([
            "--config",
            &config,
            "debug-bundle",
            "export",
            replay_run_id,
            "--out",
            &bundle,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let manifest: Value =
        serde_json::from_slice(&bundle_output).expect("sqlite debug bundle manifest is JSON");
    assert_eq!(manifest["bundle_version"], "debug_bundle.v1");
    assert_eq!(manifest["run_id"], replay_run_id);
    assert_eq!(manifest["agent_id"], "echo_agent");
    assert_eq!(manifest["files"]["trace"], "trace.json");
    assert_eq!(manifest["files"]["state_snapshot"], "state_snapshot.json");
    let bundled_trace = read_json(bundle_path.join("trace.json"));
    assert_eq!(bundled_trace["run_id"], replay_run_id);
    assert_eq!(bundled_trace["agent_id"], "echo_agent");
    let state_snapshot = read_json(bundle_path.join("state_snapshot.json"));
    assert_eq!(state_snapshot["run_id"], replay_run_id);
    assert_eq!(state_snapshot["agent_id"], "echo_agent");

    let compat_output = agent_cmd()
        .args([
            "--config",
            &config,
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
            &compat_store,
            "--debug-bundle-out",
            &compat_bundle,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let compat: Value = serde_json::from_slice(&compat_output).expect("sqlite compat is JSON");
    assert_eq!(compat["status"], "passed");
    let compat_run_id = compat["proposal_run"]["run_id"]
        .as_str()
        .expect("compat proposal run id is string");
    let compat_trace = read_sqlite_trace(&compat_store_path, compat_run_id);
    assert_eq!(compat_trace.run_id.0, compat_run_id);
    assert_eq!(compat_trace.agent_id, "customer_summary_agent");
    assert_eq!(compat["debug_bundle"]["run_id"], compat_run_id);
    assert!(compat_store_path.join("runtime.sqlite").exists());
    assert!(
        !compat_store_path
            .join("traces")
            .join(format!("{compat_run_id}.trace.json"))
            .exists(),
        "sqlite compat trace should not be written through the file trace store"
    );
    assert!(compat_bundle_path.join("manifest.json").exists());
    assert!(compat_bundle_path.join("trace.json").exists());

    let eval_output = agent_cmd()
        .args([
            "--config",
            &config,
            "eval",
            "../../evals/catalog_dry_run.yaml",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let eval: Value = serde_json::from_slice(&eval_output).expect("sqlite eval is JSON");
    let eval_run_id = eval["run_id"].as_str().expect("eval run id is string");
    assert_eq!(eval["passed"], true);
    assert_eq!(eval["agent_id"], "ai_chat");
    let eval_trace = read_sqlite_trace(&store, eval_run_id);
    assert_eq!(eval_trace.run_id.0, eval_run_id);
    assert_eq!(eval_trace.agent_id, "ai_chat");
    assert!(
        !store
            .join("traces")
            .join(format!("{eval_run_id}.trace.json"))
            .exists(),
        "sqlite eval trace should not be written through the file trace store"
    );
}

#[test]
fn config_store_backend_sqlite_drives_http_serve() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("configured-http-sqlite-store");
    let config_path = dir.path().join("agent-runtime.toml");
    let catalog = std::path::Path::new("../../fixtures/contracts/catalog.valid.json")
        .canonicalize()
        .expect("catalog path");
    let port = reserve_local_port();
    std::fs::write(
        &config_path,
        format!(
            r#"[runtime]
store = "{}"
store_backend = "sqlite"
host = "127.0.0.1"
port = {}

[runtime.sources]
catalog = "{}"

[runtime.tools]
mocks = ['propose_fake={{"http_sqlite":true}}']
"#,
            store.display(),
            port,
            catalog.display()
        ),
    )
    .expect("config written");

    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
        .args(["--config", config_path.to_str().unwrap(), "serve"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts from sqlite config");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let run = http_json_request(
        port,
        "POST",
        "/agents/ai_chat/run",
        Some(r#"{"input":{"message":"via sqlite http"}}"#),
    );
    let run_id = run["result"]["run_id"].as_str().expect("run id");
    assert_eq!(run["result"]["status"], "completed");
    assert!(store.join("runtime.sqlite").exists());
    let stored_trace = read_sqlite_trace(&store, run_id);
    assert_eq!(stored_trace.run_id.0, run_id);
    assert_eq!(stored_trace.agent_id, "ai_chat");
    let trace = http_json_request(port, "GET", &format!("/runs/{run_id}/trace"), None);
    assert_eq!(trace["run_id"], run_id);
    assert!(
        !store
            .join("traces")
            .join(format!("{run_id}.trace.json"))
            .exists(),
        "sqlite HTTP trace should not be written through the file trace store"
    );
    let trace_dir = store.join("traces");
    assert!(
        !trace_dir.exists()
            || !std::fs::read_dir(&trace_dir)
                .expect("trace dir reads")
                .filter_map(|entry| entry.ok())
                .any(|entry| entry
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".events.jsonl"))),
        "sqlite event store should not write file-backed event logs"
    );
    assert!(!store.join("runs").join(format!("{run_id}.json")).exists());
    let events = http_text_request(port, "GET", &format!("/runs/{run_id}/events?after=1"), None);
    assert!(events.contains("id: 2"));
    assert!(events.contains("event: catalog_dry_run.agent_selected"));

    let inspected = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["run_id"], run_id);
    assert_eq!(inspected["agent_id"], "ai_chat");

    let metrics = http_json_request(port, "GET", "/metrics/summary", None);
    assert_eq!(metrics["store_root"], store.to_string_lossy().as_ref());
    assert_eq!(metrics["run_count"], 1);
}
