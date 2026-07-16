use super::*;

#[test]
fn config_profile_drives_run_defaults() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("configured-run-store");
    let config_path = dir.path().join("agent-runtime.toml");
    let registry = std::path::Path::new("../../examples/agents.yaml")
        .canonicalize()
        .expect("registry path");
    std::fs::write(
        &config_path,
        format!(
            r#"[runtime]
profile = "ci"
store = "{}"

[runtime.sources]
registry = "{}"

[profiles.ci]
timeout_seconds = 5
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
    assert_eq!(shown["profile"], "ci");
    assert_eq!(shown["runtime"]["timeout_seconds"], 5);

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
    assert!(store.join("runs").join(format!("{run_id}.json")).exists());
}

#[test]
fn config_profile_installs_process_hooks_for_run() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("hooked-run-store");
    let trace = dir.path().join("trace.json");
    let config_path = dir.path().join("agent-runtime.toml");
    let registry = std::path::Path::new("../../examples/agents.yaml")
        .canonicalize()
        .expect("registry path");
    std::fs::write(
        &config_path,
        format!(
            r#"[runtime]
store = "{}"

[runtime.sources]
registry = "{}"

[[runtime.hooks]]
name = "audit_run"
event = "RunStart"
kind = "process"
effect = "observe"
command = ["sh", "-c", "cat >/dev/null; printf '{{\"observed\":true}}'"]
timeout_ms = 1000
"#,
            store.display(),
            registry.display()
        ),
    )
    .expect("config written");

    agent_cmd()
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "run",
            "echo_agent",
            "--input",
            "../../examples/fixtures/echo-input.json",
            "--trace-out",
            trace.to_str().expect("utf8 trace path"),
        ])
        .assert()
        .success();

    let trace = read_json(trace);
    let hook = trace["events"]
        .as_array()
        .expect("trace events")
        .iter()
        .find(|event| {
            event["kind"] == "hook_invocation"
                && event["payload"]["hook_name"] == "audit_run"
                && event["payload"]["hook_event"] == "RunStart"
        })
        .expect("configured hook invocation is traced");
    assert_eq!(hook["payload"]["output"]["observed"], true);
}

#[test]
fn config_profile_drives_http_serve_defaults() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("configured-http-store");
    let config_path = dir.path().join("agent-runtime.toml");
    let catalog = std::path::Path::new("../../fixtures/contracts/catalog.valid.json")
        .canonicalize()
        .expect("catalog path");
    let port = reserve_local_port();
    std::fs::write(
        &config_path,
        format!(
            r#"[runtime]
profile = "local"

[profiles.local]
store = "{}"
host = "127.0.0.1"
port = {}

[profiles.local.sources]
catalog = "{}"
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
        .expect("HTTP server starts from config");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let summary = http_json_request(port, "GET", "/catalog/summary", None);
    assert_eq!(summary["agent_count"], 1);
    assert!(store.exists());
}
