use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use agent_core::{
    AgentRunRecord, AgentRunStatus, AgentRunStore, AgentTrace, AgentTraceStore, PROTOCOL_VERSION,
    RunId, RunScope,
};
use agent_store::SqliteStore;
use assert_cmd::Command;
use serde_json::Value;
use time::OffsetDateTime;

#[path = "llm.rs"]
mod llm;
#[path = "support.rs"]
mod support;

use support::*;

#[test]
fn catalog_summary_reads_flutter_export_shape() {
    let output = agent_cmd()
        .args([
            "catalog",
            "summary",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("summary is JSON");
    assert_eq!(json["protocol_version"], "agent.v1");
    assert_eq!(json["catalog_version"], "agent_catalog.v1");
    assert_eq!(json["active_domains"], serde_json::json!(["chat"]));
    assert_eq!(json["agent_count"], 1);
    assert_eq!(json["tool_count"], 1);
    assert_eq!(json["proposal_kind_count"], 1);
    assert_eq!(json["prompt_block_count"], 1);
}

#[test]
fn catalog_agents_and_tools_are_printable() {
    let agents = agent_cmd()
        .args([
            "catalog",
            "agents",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let agents: Value = serde_json::from_slice(&agents).expect("agents are JSON");
    assert_eq!(agents[0]["id"], "ai_chat");

    let tools = agent_cmd()
        .args([
            "catalog",
            "tools",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let tools: Value = serde_json::from_slice(&tools).expect("tools are JSON");
    assert_eq!(tools[0]["name"], "propose_fake");
    assert_eq!(tools[0]["risk"], "medium");
    assert_eq!(tools[0]["metadata"]["requires_confirmation"], "one_tap");
}

#[test]
fn catalog_prompt_manifest_records_prompt_model_and_block_hashes() {
    let output = agent_cmd()
        .args([
            "catalog",
            "prompt-manifest",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let manifest: Value = serde_json::from_slice(&output).expect("prompt manifest is JSON");
    assert_eq!(manifest["protocol_version"], "agent.v1");
    assert_eq!(manifest["id"], "ai_chat_prompt");
    assert_eq!(manifest["version"], "ai_chat.prompt.v1");
    assert_eq!(manifest["agent_id"], "ai_chat");
    assert_eq!(manifest["model_family"], "anthropic");
    assert_eq!(manifest["provider"], "anthropic");
    assert_eq!(manifest["model"], "stepfun-ai/Step-3.7-Flash");
    assert_eq!(manifest["tool_schema_version"], "chat.tools.v1");
    assert_eq!(
        manifest["blocks"][0]["content_hash"],
        "blake3:f4d4a59a0aed2318f1a9443b2a51a518cc8296305e2f8db1e1192aac1cc7cd02"
    );
}

#[test]
fn validate_accepts_valid_schema_fixture() {
    let output = agent_cmd()
        .args([
            "validate",
            "../../schemas/run-request.schema.json",
            "../../fixtures/contracts/run-request.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("validation report is JSON");
    assert_eq!(report["valid"], true);
    assert_eq!(report["errors"], serde_json::json!([]));
}

#[test]
fn validate_rejects_invalid_schema_fixture() {
    let output = agent_cmd()
        .args([
            "validate",
            "../../schemas/run-request.schema.json",
            "../../fixtures/contracts/run-request.invalid.missing-protocol-version.json",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("validation report is JSON");
    assert_eq!(report["valid"], false);
    assert!(
        report["errors"]
            .as_array()
            .expect("errors is an array")
            .iter()
            .any(|error| error
                .as_str()
                .unwrap_or_default()
                .contains("protocol_version"))
    );
}

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
fn run_can_use_catalog_backed_dry_run_registry() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let trace = dir.path().join("trace.json");

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
            trace.to_str().expect("utf8 trace path"),
            "--scope",
            "tenant:tenant_cli",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result: Value = serde_json::from_slice(&output).expect("result is JSON");
    assert_eq!(result["agent_id"], "ai_chat");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["summary"], "catalog dry-run completed");
    assert_eq!(result["output"]["mode"], "catalog_dry_run");
    assert_eq!(result["output"]["agent"]["id"], "ai_chat");
    assert_eq!(result["output"]["input"]["protocol_version"], "agent.v1");
    let run_id = result["run_id"].as_str().expect("run id is string");
    let inspected = agent_cmd()
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
    let inspected: Value = serde_json::from_slice(&inspected).expect("inspect result is JSON");
    assert_eq!(inspected["scope"]["type"], "tenant");
    assert_eq!(inspected["scope"]["id"], "tenant_cli");

    let trace: Value =
        serde_json::from_slice(&std::fs::read(trace).expect("trace file was written"))
            .expect("trace is JSON");
    assert_eq!(trace["agent_id"], "ai_chat");
    assert_eq!(trace["events"][1]["kind"], "catalog_dry_run.agent_selected");
}

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
fn trace_export_otel_converts_trace_spans() {
    let output = agent_cmd()
        .args([
            "trace",
            "export-otel",
            "../../fixtures/contracts/trace.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let export: Value = serde_json::from_slice(&output).expect("otel export is JSON");
    assert_eq!(export["protocol_version"], "agent.v1");
    assert_eq!(export["export_format"], "otlp_trace_json.v1");
    let resource = &export["resourceSpans"][0]["resource"]["attributes"];
    assert!(
        resource
            .as_array()
            .expect("resource attrs")
            .iter()
            .any(|attr| {
                attr["key"] == "service.name" && attr["value"]["stringValue"] == "agent-runtime"
            })
    );
    assert!(
        resource
            .as_array()
            .expect("resource attrs")
            .iter()
            .any(|attr| {
                attr["key"] == "run.scope.type" && attr["value"]["stringValue"] == "tenant"
            })
    );
    let spans = export["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .expect("spans array");
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0]["name"], "agent.run");
    assert_eq!(spans[0]["kind"], "SPAN_KIND_INTERNAL");
    assert_eq!(spans[0]["status"]["code"], "STATUS_CODE_OK");
    assert!(
        spans[0]["traceId"]
            .as_str()
            .is_some_and(|value| value.len() == 32)
    );
    assert!(
        spans[0]["spanId"]
            .as_str()
            .is_some_and(|value| value.len() == 16)
    );
    assert_eq!(spans[1]["name"], "llm.openai");
    assert_eq!(spans[1]["parentSpanId"], spans[0]["spanId"]);
    assert!(
        spans[1]["attributes"]
            .as_array()
            .expect("attrs")
            .iter()
            .any(|attr| { attr["key"] == "total_tokens" && attr["value"]["intValue"] == "18" })
    );
}

#[test]
fn trace_export_otel_pushes_otlp_http_json() {
    let (port, request_handle) = spawn_otlp_trace_collector();
    let endpoint = format!("http://127.0.0.1:{port}/v1/traces");
    let output = agent_cmd()
        .args([
            "trace",
            "export-otel",
            "../../fixtures/contracts/trace.valid.json",
            "--endpoint",
            &endpoint,
            "--header",
            "x-otlp-test=true",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("push report is JSON");
    assert_eq!(report["protocol_version"], "agent.v1");
    assert_eq!(report["export_format"], "otlp_trace_json.v1");
    assert_eq!(report["endpoint"], endpoint);
    assert_eq!(report["status_code"], 200);
    assert_eq!(report["span_count"], 2);

    let request = request_handle.join().expect("request captured");
    assert!(request.starts_with("POST /v1/traces HTTP/1.1"));
    assert!(request.contains("content-type: application/json"));
    assert!(request.contains("x-otlp-test: true"));
    assert!(request.contains(r#""resourceSpans""#));
    assert!(request.contains(r#""name":"agent.run""#));
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

#[test]
fn inspect_and_debug_bundle_export_use_file_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let bundle = dir.path().join("bundle");
    let input_path = dir.path().join("sensitive-input.json");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&serde_json::json!({
            "message": "secret run",
            "api_key": "sk-live",
            "nested": {
                "access_token": "access-123",
                "safe": "visible"
            },
            "tool_call": {
                "name": "echo",
                "input": {
                    "api_key": "tool-secret",
                    "value": 7
                }
            }
        }))
        .expect("input encodes"),
    )
    .expect("input writes");

    let session = agent_cmd()
        .args([
            "session",
            "create",
            "--title",
            "Debug bundle session",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let session: Value = serde_json::from_slice(&session).expect("session is JSON");
    let session_id = session["session"]["session_id"]
        .as_str()
        .expect("session id is string");
    let thread_id = session["thread"]["thread_id"]
        .as_str()
        .expect("thread id is string");

    let output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_path.to_str().expect("utf8 input path"),
            "--session",
            session_id,
            "--thread",
            thread_id,
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let result: Value = serde_json::from_slice(&output).expect("result is JSON");
    let run_id = result["run_id"].as_str().expect("run_id is string");

    let inspect = agent_cmd()
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
    let record: Value = serde_json::from_slice(&inspect).expect("record is JSON");
    assert_eq!(record["run_id"], run_id);
    assert!(
        record["idempotency_key"]
            .as_str()
            .is_some_and(|key| key.starts_with("idem_"))
    );
    assert_eq!(record["agent_id"], "ai_chat");
    assert_eq!(record["status"], "completed");

    let created_proposal = agent_cmd()
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
            "Debug bundle proposal",
            "--payload-json",
            r#"{"api_key":"proposal-secret","safe":"visible"}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let created_proposal: Value =
        serde_json::from_slice(&created_proposal).expect("proposal is JSON");
    let proposal_id = created_proposal["proposal_id"]
        .as_str()
        .expect("proposal id is string");

    let trace_path = store.join("traces").join(format!("{run_id}.trace.json"));
    let local_artifact_source = store.join("debug-artifact.txt");
    std::fs::write(&local_artifact_source, "artifact bytes\n").expect("local artifact writes");
    let artifact_resolver_root = dir.path().join("artifact-store");
    let remote_artifact_source = artifact_resolver_root
        .join("debug-bucket")
        .join("reports")
        .join("report.pdf");
    std::fs::create_dir_all(
        remote_artifact_source
            .parent()
            .expect("remote artifact has parent"),
    )
    .expect("remote artifact parent writes");
    std::fs::write(&remote_artifact_source, "remote artifact bytes\n")
        .expect("remote artifact writes");
    let artifact_resolver_path = dir.path().join("debug-artifact-resolvers.json");
    std::fs::write(
        &artifact_resolver_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "protocol_version": "agent.v1",
            "resolvers": [
                {
                    "provider": "host_blob_store",
                    "root": artifact_resolver_root.to_str().expect("utf8 resolver root")
                }
            ]
        }))
        .expect("artifact resolver encodes"),
    )
    .expect("artifact resolver writes");
    let mut stored_trace = read_json(&trace_path);
    stored_trace["artifact_refs"] = serde_json::json!([
        {
            "artifact_id": "artifact_debug_report",
            "kind": "document",
            "uri": "artifact://debug/report.pdf",
            "media_type": "application/pdf",
            "size_bytes": 2048,
            "sha256": "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            "redaction_classification": "confidential",
            "store": {
                "provider": "host_blob_store",
                "bucket": "debug-bucket",
                "key": "reports/report.pdf",
                "version": "v1",
                "metadata": {
                    "api_key": "artifact-store-secret"
                }
            },
            "metadata": {
                "safe": "visible",
                "api_key": "artifact-secret"
            }
        },
        {
            "artifact_id": "artifact_local_report",
            "kind": "log",
            "uri": "artifact://debug/local-report.txt",
            "media_type": "text/plain",
            "size_bytes": 15,
            "redaction_classification": "internal",
            "metadata": {
                "local_path": local_artifact_source.to_str().expect("utf8 artifact path"),
                "safe": "visible"
            }
        }
    ]);
    std::fs::write(
        &trace_path,
        serde_json::to_vec_pretty(&stored_trace).expect("trace encodes"),
    )
    .expect("trace writes");

    let manifest = agent_cmd()
        .args([
            "debug-bundle",
            "export",
            run_id,
            "--store",
            store.to_str().expect("utf8 store path"),
            "--out",
            bundle.to_str().expect("utf8 bundle path"),
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--materialize-artifacts",
            "--artifact-resolver",
            artifact_resolver_path
                .to_str()
                .expect("utf8 artifact resolver path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let manifest: Value = serde_json::from_slice(&manifest).expect("manifest is JSON");
    assert_eq!(manifest["bundle_version"], "debug_bundle.v1");
    assert_eq!(manifest["run_id"], run_id);
    assert_eq!(manifest["agent_id"], "ai_chat");
    assert_eq!(manifest["agent_version"], "0.1.0");
    assert_eq!(manifest["files"]["manifest"], "manifest.json");
    assert_eq!(manifest["files"]["trace"], "trace.json");
    assert_eq!(manifest["files"]["events"], "events.jsonl");
    assert_eq!(manifest["files"]["replay_config"], "replay_config.json");
    assert_eq!(manifest["files"]["agent_spec"], "agent_spec.json");
    assert_eq!(manifest["files"]["prompt_manifest"], "prompt_manifest.json");
    assert_eq!(manifest["files"]["tool_calls"], "tool_calls.jsonl");
    assert_eq!(manifest["files"]["artifacts"], "artifacts.json");
    assert_eq!(
        manifest["files"]["artifact_materializations"],
        "artifact_materializations.json"
    );
    assert_eq!(manifest["files"]["state_snapshot"], "state_snapshot.json");
    assert_eq!(manifest["files"]["redactions"], "redactions.json");

    let bundled_trace = read_json(bundle.join("trace.json"));
    assert_eq!(bundled_trace["run_id"], run_id);
    assert_eq!(
        bundled_trace["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );
    assert_eq!(bundled_trace["input"]["api_key"], "[REDACTED]");
    assert_eq!(
        bundled_trace["input"]["nested"]["access_token"],
        "[REDACTED]"
    );
    assert_eq!(bundled_trace["input"]["nested"]["safe"], "visible");
    assert_eq!(bundled_trace["output"]["input"]["api_key"], "[REDACTED]");

    let bundled_events = std::fs::read_to_string(bundle.join("events.jsonl"))
        .expect("events jsonl exists")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event line is JSON"))
        .collect::<Vec<_>>();
    assert_eq!(
        bundled_events.len(),
        bundled_trace["events"]
            .as_array()
            .expect("trace events is array")
            .len()
    );
    assert!(
        bundled_events
            .iter()
            .any(|event| event["kind"] == "tool_call_finished")
    );
    assert!(bundled_events.iter().any(|event| {
        event["payload"]["input"]["api_key"]
            .as_str()
            .is_some_and(|value| value == "[REDACTED]")
            || event["payload"]["output"]["echo"]["api_key"]
                .as_str()
                .is_some_and(|value| value == "[REDACTED]")
    }));

    let bundled_request = read_json(bundle.join("run_request.json"));
    assert_eq!(bundled_request["run_id"], run_id);
    assert_eq!(bundled_request["trigger"], "replay");
    assert_eq!(bundled_request["input"]["api_key"], "[REDACTED]");
    assert_eq!(
        bundled_request["input"]["nested"]["access_token"],
        "[REDACTED]"
    );
    assert_eq!(bundled_request["input"]["nested"]["safe"], "visible");
    assert_eq!(
        bundled_request["metadata"]["reconstructed_from"],
        "run_record"
    );

    let replay_config = read_json(bundle.join("replay_config.json"));
    assert_eq!(replay_config["run_id"], run_id);
    assert_eq!(replay_config["agent_id"], "ai_chat");
    assert_eq!(replay_config["replay_mode"], "live");
    assert_eq!(replay_config["assets"]["trace"], "trace.json");
    assert_eq!(replay_config["assets"]["events"], "events.jsonl");
    assert_eq!(replay_config["assets"]["tool_calls"], "tool_calls.jsonl");
    assert_eq!(replay_config["assets"]["artifacts"], "artifacts.json");
    assert_eq!(
        replay_config["assets"]["artifact_materializations"],
        "artifact_materializations.json"
    );
    assert_eq!(
        replay_config["assets"]["prompt_manifest"],
        "prompt_manifest.json"
    );
    assert_eq!(replay_config["timeout_seconds"], 60);
    assert_eq!(
        replay_config["run_request"]["input"]["api_key"],
        "[REDACTED]"
    );
    assert!(
        replay_config["replay_command"]
            .as_array()
            .expect("replay command is array")
            .iter()
            .any(|part| part == "--catalog")
    );

    let bundled_result = read_json(bundle.join("run_result.json"));
    assert_eq!(bundled_result["run_id"], run_id);
    assert_eq!(bundled_result["status"], "completed");
    assert_eq!(bundled_result["output"]["input"]["api_key"], "[REDACTED]");
    assert_eq!(
        bundled_result["output"]["tool_result"]["echo"]["api_key"],
        "[REDACTED]"
    );

    let bundled_tool_calls = std::fs::read_to_string(bundle.join("tool_calls.jsonl"))
        .expect("tool calls jsonl exists")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("tool call line is JSON"))
        .collect::<Vec<_>>();
    assert_eq!(bundled_tool_calls.len(), 1);
    assert!(
        bundled_tool_calls[0]["tool_call_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("tool_"))
    );
    assert_eq!(bundled_tool_calls[0]["tool_name"], "echo");
    assert!(
        bundled_tool_calls[0]["input_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
    assert_eq!(bundled_tool_calls[0]["status"], "completed");
    assert_eq!(
        bundled_tool_calls[0]["output"]["echo"]["api_key"],
        "[REDACTED]"
    );
    assert_eq!(bundled_tool_calls[0]["output"]["echo"]["value"], 7);

    let bundled_artifacts = read_json(bundle.join("artifacts.json"));
    assert_eq!(bundled_artifacts[0]["artifact_id"], "artifact_debug_report");
    assert_eq!(bundled_artifacts[0]["kind"], "document");
    assert_eq!(
        bundled_artifacts[0]["redaction_classification"],
        "confidential"
    );
    assert_eq!(bundled_artifacts[0]["metadata"]["safe"], "visible");
    assert_eq!(bundled_artifacts[0]["metadata"]["api_key"], "[REDACTED]");
    assert_eq!(
        bundled_artifacts[0]["store"]["metadata"]["api_key"],
        "[REDACTED]"
    );
    assert_eq!(bundled_artifacts[1]["artifact_id"], "artifact_local_report");
    assert_eq!(bundled_artifacts[1]["kind"], "log");
    assert_eq!(bundled_artifacts[1]["metadata"]["local_path"], "[REDACTED]");

    let materializations = read_json(bundle.join("artifact_materializations.json"));
    assert_eq!(materializations["protocol_version"], "agent.v1");
    assert_eq!(
        materializations["mode"],
        "local_files_and_artifact_store_resolvers"
    );
    assert_eq!(
        materializations["records"][0]["artifact_id"],
        "artifact_debug_report"
    );
    assert_eq!(materializations["records"][0]["status"], "materialized");
    assert_eq!(
        materializations["records"][0]["source"],
        "artifact_store:host_blob_store"
    );
    assert_eq!(materializations["records"][0]["size_bytes"], 22);
    assert!(
        materializations["records"][0]["blake3"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
    let remote_materialized_path = materializations["records"][0]["bundled_path"]
        .as_str()
        .expect("remote materialized path is string");
    assert_eq!(
        std::fs::read_to_string(bundle.join(remote_materialized_path))
            .expect("remote materialized artifact reads"),
        "remote artifact bytes\n"
    );
    assert_eq!(
        materializations["records"][1]["artifact_id"],
        "artifact_local_report"
    );
    assert_eq!(materializations["records"][1]["status"], "materialized");
    assert_eq!(
        materializations["records"][1]["source"],
        "metadata.local_path"
    );
    assert_eq!(materializations["records"][1]["size_bytes"], 15);
    assert!(
        materializations["records"][1]["blake3"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
    let materialized_path = materializations["records"][1]["bundled_path"]
        .as_str()
        .expect("materialized path is string");
    assert_eq!(
        std::fs::read_to_string(bundle.join(materialized_path))
            .expect("materialized artifact reads"),
        "artifact bytes\n"
    );

    let state_snapshot = read_json(bundle.join("state_snapshot.json"));
    assert_eq!(state_snapshot["run_id"], run_id);
    assert_eq!(state_snapshot["agent_id"], "ai_chat");
    assert_eq!(state_snapshot["run_status"], "completed");
    assert_eq!(state_snapshot["session_id"], session_id);
    assert_eq!(state_snapshot["thread_id"], thread_id);
    assert_eq!(state_snapshot["session"]["session_id"], session_id);
    assert_eq!(state_snapshot["thread"]["thread_id"], thread_id);
    assert_eq!(state_snapshot["steps"][0]["run_id"], run_id);
    assert_eq!(state_snapshot["proposals"][0]["proposal_id"], proposal_id);
    assert_eq!(
        state_snapshot["proposals"][0]["payload"]["api_key"],
        "[REDACTED]"
    );
    assert_eq!(state_snapshot["proposals"][0]["payload"]["safe"], "visible");

    let redactions = read_json(bundle.join("redactions.json"));
    assert_eq!(redactions["policy"], "builtin_sensitive_field_names.v1");
    let redacted_paths = redactions["redacted_paths"]
        .as_array()
        .expect("redacted paths is an array");
    assert!(
        redacted_paths
            .iter()
            .any(|path| path.as_str().is_some_and(|path| path.contains("api_key")))
    );
    assert!(redacted_paths.iter().any(|path| {
        path.as_str()
            .is_some_and(|path| path.contains("access_token"))
    }));
    assert!(redacted_paths.iter().any(|path| {
        path.as_str()
            .is_some_and(|path| path.contains("local_path"))
    }));

    let agent_spec = read_json(bundle.join("agent_spec.json"));
    assert_eq!(agent_spec["id"], "ai_chat");

    let prompt_manifest = read_json(bundle.join("prompt_manifest.json"));
    assert_eq!(prompt_manifest["id"], "ai_chat_prompt");
    assert_eq!(prompt_manifest["version"], "ai_chat.prompt.v1");
    assert_eq!(prompt_manifest["agent_id"], "ai_chat");
    assert_eq!(prompt_manifest["model"], "stepfun-ai/Step-3.7-Flash");
    assert_eq!(
        prompt_manifest["blocks"][0]["content_hash"],
        "blake3:f4d4a59a0aed2318f1a9443b2a51a518cc8296305e2f8db1e1192aac1cc7cd02"
    );
}

#[test]
fn recover_abandons_stale_running_runs_in_file_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let run_dir = store.join("runs");
    std::fs::create_dir_all(&run_dir).expect("run dir created");
    let stale_run = serde_json::json!({
        "protocol_version": "agent.v1",
        "run_id": "run_stale_cli",
        "agent_id": "ai_chat",
        "status": "running",
        "scope": {"type": "global"},
        "started_at": "2020-01-01T00:00:00Z",
        "finished_at": null,
        "input": {"message": "stale"},
        "output": {},
        "metadata": {}
    });
    std::fs::write(
        run_dir.join("run_stale_cli.json"),
        serde_json::to_vec_pretty(&stale_run).expect("stale run encodes"),
    )
    .expect("stale run writes");

    let output = agent_cmd()
        .args([
            "recover",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--timeout-seconds",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).expect("recovery report is JSON");
    assert_eq!(report["scanned_runs"], 1);
    assert_eq!(report["abandoned_count"], 1);
    assert_eq!(report["recovered_runs"][0]["run_id"], "run_stale_cli");
    assert_eq!(report["recovered_runs"][0]["new_status"], "abandoned");

    let updated = read_json(run_dir.join("run_stale_cli.json"));
    assert_eq!(updated["status"], "abandoned");
    assert_eq!(updated["error"]["code"], "stale_running_run_abandoned");
    assert_eq!(updated["error"]["retryable"], true);
    assert!(updated["finished_at"].is_string());
}

#[tokio::test]
async fn recover_abandons_stale_running_runs_in_sqlite_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("sqlite-recover-store");
    let config_path = dir.path().join("agent-runtime.toml");
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

    let run_id = RunId("run_stale_sqlite_cli".to_owned());
    let sqlite = SqliteStore::open(
        camino::Utf8PathBuf::from_path_buf(store.join("runtime.sqlite"))
            .expect("sqlite path is utf8"),
    )
    .await
    .expect("sqlite store opens");
    sqlite
        .create_run(AgentRunRecord {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            version: 1,
            run_id: run_id.clone(),
            idempotency_key: None,
            agent_id: "ai_chat".to_owned(),
            status: AgentRunStatus::Running,
            scope: RunScope::Global,
            started_at: OffsetDateTime::parse(
                "2020-01-01T00:00:00Z",
                &time::format_description::well_known::Rfc3339,
            )
            .expect("fixture time parses"),
            finished_at: None,
            input: serde_json::json!({"message": "stale sqlite"}),
            output: serde_json::json!({}),
            error: None,
            workflow: None,
            metadata: serde_json::json!({}),
        })
        .await
        .expect("stale run is seeded");
    drop(sqlite);

    let output = agent_cmd()
        .args([
            "--config",
            config_path.to_str().expect("utf8 config path"),
            "recover",
            "--timeout-seconds",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).expect("recovery report is JSON");
    assert_eq!(report["scanned_runs"], 1);
    assert_eq!(report["abandoned_count"], 1);
    assert_eq!(report["recovered_runs"][0]["run_id"], run_id.0);
    assert_eq!(report["recovered_runs"][0]["new_status"], "abandoned");
    assert!(store.join("runtime.sqlite").exists());
    assert!(
        !store
            .join("runs")
            .join(format!("{}.json", run_id.0))
            .exists()
    );

    let inspected = agent_cmd()
        .args([
            "--config",
            config_path.to_str().expect("utf8 config path"),
            "inspect",
            &run_id.0,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let inspected: Value = serde_json::from_slice(&inspected).expect("run record is JSON");
    assert_eq!(inspected["run_id"], run_id.0);
    assert_eq!(inspected["status"], "abandoned");
    assert_eq!(inspected["error"]["code"], "stale_running_run_abandoned");
    assert_eq!(inspected["error"]["retryable"], true);
    assert!(inspected["finished_at"].is_string());
}

#[test]
fn catalog_dry_run_can_call_process_tool_host() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let input_path = dir.path().join("input.json");
    let trace_path = dir.path().join("trace.json");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&serde_json::json!({
            "tool_call": {
                "name": "echo_external",
                "input": {"value": 7}
            }
        }))
        .expect("input encodes"),
    )
    .expect("input writes");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");

    let output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_path.to_str().expect("utf8 input path"),
            "--trace-out",
            trace_path.to_str().expect("utf8 trace path"),
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result: Value = serde_json::from_slice(&output).expect("result is JSON");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["output"]["tool_result"]["host"], "dev-tool-host");
    assert_eq!(result["output"]["tool_result"]["tool"], "echo_external");
    assert_eq!(result["output"]["tool_result"]["input"]["value"], 7);

    let trace = read_json(trace_path);
    let events = trace["events"].as_array().expect("trace events is array");
    let started = events
        .iter()
        .find(|event| event["kind"] == "tool_call_started")
        .expect("tool start event exists");
    let finished = events
        .iter()
        .find(|event| event["kind"] == "tool_call_finished")
        .expect("tool finish event exists");
    let tool_call_id = started["payload"]["tool_call_id"]
        .as_str()
        .expect("tool call id is string");
    assert!(tool_call_id.starts_with("tool_"));
    assert_eq!(finished["payload"]["tool_call_id"], tool_call_id);
    assert_eq!(started["payload"]["tool_name"], "echo_external");
    assert_eq!(finished["payload"]["tool_name"], "echo_external");
    assert!(
        started["payload"]["input_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
    assert_eq!(
        finished["payload"]["input_hash"],
        started["payload"]["input_hash"]
    );
    assert_eq!(finished["payload"]["status"], "completed");
    assert!(finished["payload"]["duration_ms"].is_number());
    assert_eq!(finished["payload"]["output"]["input"]["value"], 7);

    let spans = trace["spans"].as_array().expect("trace spans is array");
    let tool_spans = spans
        .iter()
        .filter(|span| span["name"] == "tool.echo_external")
        .collect::<Vec<_>>();
    assert_eq!(tool_spans.len(), 1);
    assert_eq!(
        tool_spans[0]["parent_span_id"], spans[0]["span_id"],
        "tool span is nested under the run span",
    );
    assert_eq!(tool_spans[0]["status"], "completed");
    assert_eq!(
        tool_spans[0]["attributes"]["tool_call_id"],
        started["payload"]["tool_call_id"]
    );
    assert_eq!(
        tool_spans[0]["attributes"]["input_hash"],
        started["payload"]["input_hash"]
    );
}

#[test]
fn catalog_dry_run_retries_retryable_process_tool_errors() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let input_path = dir.path().join("input.json");
    let trace_path = dir.path().join("trace.json");
    let fail_once_path = dir.path().join("fail-once.marker");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&serde_json::json!({
            "tool_call": {
                "name": "echo_external",
                "input": {
                    "value": 7,
                    "fail_once_path": fail_once_path,
                }
            }
        }))
        .expect("input encodes"),
    )
    .expect("input writes");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");

    let output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_path.to_str().expect("utf8 input path"),
            "--trace-out",
            trace_path.to_str().expect("utf8 trace path"),
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--max-retries",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result: Value = serde_json::from_slice(&output).expect("result is JSON");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["output"]["tool_result"]["host"], "dev-tool-host");
    assert!(fail_once_path.exists());

    let trace = read_json(trace_path);
    let events = trace["events"].as_array().expect("trace events is array");
    assert_eq!(
        events
            .iter()
            .filter(|event| event["kind"] == "run_attempt_started")
            .count(),
        2
    );
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "tool_call_failed")
    );
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "run_retry_scheduled")
    );
    let retry = events
        .iter()
        .find(|event| event["kind"] == "run_retry_scheduled")
        .expect("retry event exists");
    assert_eq!(retry["payload"]["next_attempt"], 2);
}

#[test]
fn metrics_summary_counts_runs_tools_and_proposals() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("metrics-store");
    let input_path = dir.path().join("input.json");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&serde_json::json!({
            "tool_call": {
                "name": "echo",
                "input": {"value": 23}
            },
            "proposal": {
                "kind": "fake",
                "summary": "Metrics fake proposal",
                "payload": {"value": 24}
            }
        }))
        .expect("input encodes"),
    )
    .expect("input writes");

    let run_output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_path.to_str().expect("utf8 input path"),
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run_result: Value = serde_json::from_slice(&run_output).expect("run result is JSON");
    let run_id = run_result["run_id"].as_str().expect("run id is string");
    let trace_path = store.join("traces").join(format!("{run_id}.trace.json"));
    let mut trace = read_json(&trace_path);
    let events = trace["events"]
        .as_array_mut()
        .expect("trace events are array");
    events.push(serde_json::json!({
        "kind": "llm_response",
        "occurred_at": "2026-06-28T09:12:31Z",
        "payload": {
            "provider": "openai",
            "model": "gpt-test",
            "duration_ms": 25,
            "usage": {
                "input_tokens": 5,
                "output_tokens": 3,
                "total_tokens": 8,
                "cost_micros": 42,
                "cost_currency": "USD"
            }
        }
    }));
    events.push(serde_json::json!({
        "kind": "proposal_decided",
        "occurred_at": "2026-06-28T09:12:32Z",
        "payload": {
            "proposal_id": "proposal_metrics",
            "decision": "approve",
            "status": "pending_approval"
        }
    }));
    events.push(serde_json::json!({
        "kind": "proposal_decided",
        "occurred_at": "2026-06-28T09:12:33Z",
        "payload": {
            "proposal_id": "proposal_metrics",
            "decision": "approve",
            "status": "approved"
        }
    }));
    std::fs::write(
        &trace_path,
        serde_json::to_vec_pretty(&trace).expect("trace encodes"),
    )
    .expect("trace writes");

    let output = agent_cmd()
        .args([
            "metrics",
            "summary",
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let metrics: Value = serde_json::from_slice(&output).expect("metrics are JSON");
    assert_eq!(metrics["protocol_version"], "agent.v1");
    assert_eq!(metrics["run_count"], 1);
    assert_eq!(metrics["successful_run_count"], 1);
    assert_eq!(metrics["runs_by_status"]["completed"], 1);
    assert_eq!(metrics["tool_call_count"], 1);
    assert_eq!(metrics["failed_tool_call_count"], 0);
    assert_eq!(metrics["runs_by_agent"]["ai_chat"]["run_count"], 1);
    assert_eq!(
        metrics["runs_by_agent"]["ai_chat"]["successful_run_count"],
        1
    );
    assert_eq!(
        metrics["runs_by_agent"]["ai_chat"]["runs_by_status"]["completed"],
        1
    );
    assert_eq!(metrics["tool_calls_by_tool"]["echo"]["tool_call_count"], 1);
    assert_eq!(
        metrics["tool_calls_by_tool"]["echo"]["failed_tool_call_count"],
        0
    );
    assert!(metrics["average_run_latency_ms"].is_number());
    assert!(metrics["average_tool_call_latency_ms"].is_number());
    assert_eq!(metrics["proposal_count"], 1);
    assert_eq!(metrics["proposal_created_count"], 1);
    assert_eq!(metrics["proposal_approved_count"], 1);
    assert_eq!(metrics["proposals_by_status"]["pending_approval"], 1);
    assert_eq!(metrics["artifact_ref_count"], 0);
    assert_eq!(metrics["replay_count"], 0);
    assert_eq!(metrics["llm_total_tokens"], 8);
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["request_count"],
        1
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["input_tokens"],
        5
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["output_tokens"],
        3
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["total_tokens"],
        8
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["cost_micros_by_currency"]["USD"],
        42
    );
    assert_eq!(
        metrics["llm_usage_by_provider"]["openai"]["total_latency_ms"],
        25
    );
    assert!(metrics["llm_usage_by_provider"]["openai"]["average_latency_ms"].is_number());
}

#[test]
fn catalog_dry_run_can_call_mock_tool_from_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("store");
    let input_path = dir.path().join("input.json");
    let mock_path = dir.path().join("mock-output.json");
    std::fs::write(
        &input_path,
        serde_json::to_vec(&serde_json::json!({
            "tool_call": {
                "name": "propose_fake",
                "input": {"value": 9}
            }
        }))
        .expect("input encodes"),
    )
    .expect("input writes");
    std::fs::write(
        &mock_path,
        serde_json::to_vec(&serde_json::json!({
            "mocked": true,
            "decision": "accept"
        }))
        .expect("mock encodes"),
    )
    .expect("mock writes");

    let output = agent_cmd()
        .args([
            "run",
            "ai_chat",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input",
            input_path.to_str().expect("utf8 input path"),
            "--mock-tool",
            &format!(
                "propose_fake=@{}",
                mock_path.to_str().expect("utf8 mock path")
            ),
            "--store",
            store.to_str().expect("utf8 store path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result: Value = serde_json::from_slice(&output).expect("result is JSON");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["output"]["tool_result"]["mocked"], true);
    assert_eq!(result["output"]["tool_result"]["decision"], "accept");
}

#[test]
fn tool_cli_lists_catalog_tools_and_calls_process_tool_host() {
    let tools = agent_cmd()
        .args([
            "tool",
            "list",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let tools: Value = serde_json::from_slice(&tools).expect("tools are JSON");
    assert_eq!(tools[0]["name"], "propose_fake");

    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "propose_fake",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input-json",
            r#"{"value":42}"#,
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["host"], "dev-tool-host");
    assert_eq!(output["tool"], "propose_fake");
    assert_eq!(output["input"]["value"], 42);
}

#[test]
fn tool_cli_can_call_inline_mock_tool() {
    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "propose_fake",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input-json",
            r#"{"value":42}"#,
            "--mock-tool",
            r#"propose_fake={"mocked":true,"value":123}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["mocked"], true);
    assert_eq!(output["value"], 123);
}

#[test]
fn tool_cli_validates_catalog_tool_input_schema_for_inline_mock() {
    let stderr = agent_cmd()
        .args([
            "tool",
            "call",
            "propose_fake",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input-json",
            r#""not an object""#,
            "--mock-tool",
            r#"propose_fake={"mocked":true}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(stderr).expect("stderr is utf8");

    assert!(stderr.contains("tool 'propose_fake' input failed schema validation"));
}

#[test]
fn tool_cli_lists_and_calls_tool_source_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("tool-source.json");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "dev",
                "command": agent_bin.to_str().expect("utf8 agent bin"),
                "args": ["dev-tool-host"],
                "tools": [{
                    "name": "sourced_echo",
                    "description": "Echo through a configured tool source.",
                    "input_schema": {"type": "object"},
                    "output_schema": {"type": "object"},
                    "risk": "read_only",
                    "metadata": {"source": "test"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let tools = agent_cmd()
        .args([
            "tool",
            "list",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let tools: Value = serde_json::from_slice(&tools).expect("tools are JSON");
    assert!(
        tools
            .as_array()
            .expect("tools array")
            .iter()
            .any(|tool| tool["name"] == "sourced_echo")
    );

    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "sourced_echo",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"value":77}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["host"], "dev-tool-host");
    assert_eq!(output["tool"], "sourced_echo");
    assert_eq!(output["input"]["value"], 77);
}

#[test]
fn tool_cli_validates_tool_source_input_schema() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("tool-source.json");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "dev",
                "command": agent_bin.to_str().expect("utf8 agent bin"),
                "args": ["dev-tool-host"],
                "tools": [{
                    "name": "strict_echo",
                    "description": "Echo through a configured tool source with strict input.",
                    "input_schema": {
                        "type": "object",
                        "required": ["value"],
                        "properties": {
                            "value": {"type": "string"}
                        },
                        "additionalProperties": false
                    },
                    "output_schema": {"type": "object"},
                    "risk": "read_only",
                    "metadata": {"source": "test"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let stderr = agent_cmd()
        .args([
            "tool",
            "call",
            "strict_echo",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"value":77}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(stderr).expect("stderr is utf8");

    assert!(stderr.contains("tool 'strict_echo' input failed schema validation"));
}

#[test]
fn tool_cli_calls_mcp_stdio_tool_source_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("mcp-tool-source.json");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "mcp-dev",
                "protocol": "mcp_stdio",
                "command": agent_bin.to_str().expect("utf8 agent bin"),
                "args": ["dev-mcp-server"],
                "tools": [{
                    "name": "mcp_echo",
                    "description": "Echo through a dev MCP server.",
                    "input_schema": {"type": "object"},
                    "output_schema": {"type": "object"},
                    "risk": "read_only",
                    "metadata": {"source": "mcp-dev", "protocol": "mcp_stdio"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "mcp_echo",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"value":91}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["structuredContent"]["host"], "dev-mcp-server");
    assert_eq!(output["structuredContent"]["tool"], "mcp_echo");
    assert_eq!(output["structuredContent"]["input"]["value"], 91);
    assert_eq!(output["isError"], false);
}

#[test]
fn tool_cli_calls_http_json_tool_source_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("http-tool-source.json");
    let (port, request_handle) = spawn_http_tool_source_server();
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "http-dev",
                "protocol": "http_json",
                "endpoint": format!("http://127.0.0.1:{port}/tools/call"),
                "headers": {"x-agent-runtime-source": "http-dev"},
                "tools": [{
                    "name": "http_echo",
                    "description": "Echo through a dev HTTP endpoint.",
                    "input_schema": {"type": "object"},
                    "output_schema": {"type": "object"},
                    "risk": "read_only",
                    "metadata": {"source": "http-dev", "protocol": "http_json"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "http_echo",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"value":64}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["host"], "http-tool-source");
    assert_eq!(output["tool"], "http_echo");
    assert_eq!(output["input"]["value"], 64);

    let request = request_handle.join().expect("request captured");
    assert!(request.starts_with("POST /tools/call HTTP/1.1"));
    assert!(request.contains("x-agent-runtime-source: http-dev"));
    assert!(request.contains(r#""protocol_version":"agent.v1""#));
    assert!(request.contains(r#""method":"tool.call""#));
    assert!(request.contains(r#""tool":"http_echo""#));
    assert!(request.contains(r#""value":64"#));
}

#[test]
fn tool_cli_calls_shell_tool_source_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("shell-tool-source.json");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "local-shell",
                "protocol": "jsonl_tool_call",
                "command": agent_bin.to_str().expect("utf8 agent bin"),
                "args": ["shell-tool-host"],
                "tools": [{
                    "name": "shell.exec",
                    "description": "Execute a shell command in a bounded local workspace directory.",
                    "input_schema": {
                        "type": "object",
                        "required": ["command"],
                        "properties": {
                            "command": {"type": "string", "minLength": 1},
                            "cwd": {"type": "string"},
                            "timeout_ms": {"type": "integer", "minimum": 1},
                            "max_output_bytes": {"type": "integer", "minimum": 1},
                            "env": {
                                "type": "object",
                                "additionalProperties": {"type": "string"}
                            }
                        },
                        "additionalProperties": false
                    },
                    "output_schema": {"type": "object"},
                    "risk": "high",
                    "metadata": {"source": "local-shell", "protocol": "jsonl_tool_call"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "shell.exec",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"command":"printf shell-ok","timeout_ms":5000}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["exit_code"], 0);
    assert_eq!(output["timed_out"], false);
    assert_eq!(output["stdout"], "shell-ok");
    assert_eq!(output["stderr"], "");
}

#[test]
fn tool_cli_rejects_tools_missing_from_catalog() {
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    agent_cmd()
        .args([
            "tool",
            "call",
            "missing_tool",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
        ])
        .assert()
        .failure();
}

#[test]
fn tui_once_renders_catalog_and_trace_snapshot() {
    let output = agent_cmd()
        .args([
            "tui",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--trace",
            "../../fixtures/contracts/trace.valid.json",
            "--tool-source",
            "../../fixtures/contracts/tool-source.example.json",
            "--once",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).expect("stdout is utf8");
    assert!(output.contains("Agent Runtime"));
    assert!(output.contains("Chat"));
    assert!(output.contains("[Details]  Timeline"));
    assert!(output.contains("Message"));
    assert!(output.contains("agent  echo_agent@0.1.0"));
    assert!(output.contains("ai_chat"));
}

#[test]
fn tui_once_reads_unified_runtime_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("configured-tui-store");
    let config_path = dir.path().join("agent-runtime.toml");
    let registry = std::path::Path::new("../../examples/agents.yaml")
        .canonicalize()
        .expect("registry path");
    let catalog = std::path::Path::new("../../fixtures/contracts/catalog.valid.json")
        .canonicalize()
        .expect("catalog path");
    let tool_source = std::path::Path::new("../../fixtures/contracts/tool-source.example.json")
        .canonicalize()
        .expect("tool source path");
    let trace = std::path::Path::new("../../fixtures/contracts/trace.valid.json")
        .canonicalize()
        .expect("trace path");
    std::fs::write(
        &config_path,
        format!(
            r#"[runtime]
store = "{}"
default_agent = "ai_chat"
timeout_seconds = 5

[runtime.sources]
registry = "{}"
catalog = "{}"

[runtime.tools]
sources = ["{}"]

[runtime.llm]
provider = "mock"
model = "configured-model"
max_tool_rounds = 2
"#,
            store.display(),
            registry.display(),
            catalog.display(),
            tool_source.display()
        ),
    )
    .expect("config written");

    let output = agent_cmd()
        .args([
            "--config",
            config_path.to_str().expect("utf8 config path"),
            "tui",
            "--trace",
            trace.to_str().expect("utf8 trace path"),
            "--once",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).expect("stdout is utf8");
    assert!(output.contains("mock / configured-model"));
    assert!(output.contains("ai_chat"));
    assert!(output.contains("catalog  1 agents / 1 tools"));
    assert!(output.contains("tools  3 | high 0 | blocked 0"));
}

#[test]
fn stdio_server_handles_catalog_summary_and_agent_run() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("stdio-store");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":"summary","method":"catalog.summary","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":"run","method":"agent.run","params":{"agent_id":"ai_chat","input":{"message":"via stdio","tool_call":{"name":"stdio_external","input":{"ok":true}}}}}"#,
        "\n"
    );

    let output = agent_cmd()
        .args([
            "serve",
            "--stdio",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
        ])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let lines = String::from_utf8(output).expect("stdout is utf8");
    let responses = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("response JSON"))
        .collect::<Vec<_>>();

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["jsonrpc"], "2.0");
    assert_eq!(responses[0]["id"], "summary");
    assert_eq!(responses[0]["result"]["agent_count"], 1);

    assert_eq!(responses[1]["jsonrpc"], "2.0");
    assert_eq!(responses[1]["id"], "run");
    assert_eq!(responses[1]["result"]["result"]["status"], "completed");
    assert_eq!(
        responses[1]["result"]["result"]["output"]["tool_result"]["tool"],
        "stdio_external"
    );
    assert_eq!(
        responses[1]["result"]["trace"]["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );
}

#[test]
fn http_server_handles_catalog_summary_and_agent_run() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
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

    let summary = http_json_request(port, "GET", "/catalog/summary", None);
    assert_eq!(summary["protocol_version"], "agent.v1");
    assert_eq!(summary["catalog_version"], "agent_catalog.v1");
    assert_eq!(summary["agent_count"], 1);

    let run = http_json_request(
        port,
        "POST",
        "/agents/ai_chat/run",
        Some(
            r#"{"input":{"message":"via http"},"trigger":"webhook","trigger_envelope":{"source":"github.webhook","id":"evt_http_1","payload":{"action":"opened"}},"user":{"user_id":"user_http_1"},"scope":{"type":"tenant","id":"tenant_http_1"},"workflow":{"workflow_id":"workflow_http_test","root_run_id":"run_http_root","dependencies":[{"run_id":"run_http_dependency","edge":"after","metadata":{"fixture":"catalog_cli"}}],"metadata":{"case":"catalog_cli"}},"metadata":{"delivery_attempt":1}}"#,
        ),
    );
    assert_eq!(run["result"]["agent_id"], "ai_chat");
    assert_eq!(run["result"]["status"], "completed");
    assert_eq!(run["result"]["output"]["mode"], "catalog_dry_run");
    assert_eq!(
        run["result"]["workflow"]["workflow_id"],
        "workflow_http_test"
    );
    assert_eq!(
        run["trace"]["workflow"]["workflow_id"],
        "workflow_http_test"
    );
    assert_eq!(run["trace"]["events"][0]["payload"]["trigger"], "webhook");
    assert_eq!(
        run["trace"]["events"][0]["payload"]["trigger_envelope"]["source"],
        "github.webhook"
    );
    assert_eq!(
        run["trace"]["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );

    let run_id = run["result"]["run_id"].as_str().expect("run_id is string");
    let runs = http_json_request(port, "GET", "/runs?agent_id=ai_chat&limit=1", None);
    assert_eq!(runs[0]["run_id"], run_id);
    assert_eq!(runs[0]["agent_id"], "ai_chat");

    let inspected_run = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected_run["run_id"], run_id);
    assert_eq!(inspected_run["agent_id"], "ai_chat");
    assert_eq!(inspected_run["status"], "completed");
    assert_eq!(inspected_run["scope"]["type"], "tenant");
    assert_eq!(inspected_run["scope"]["id"], "tenant_http_1");
    assert_eq!(inspected_run["metadata"]["delivery_attempt"], 1);
    assert_eq!(
        inspected_run["workflow"]["dependencies"][0]["run_id"],
        "run_http_dependency"
    );
    assert_eq!(
        inspected_run["metadata"]["session_id"],
        serde_json::Value::Null
    );
    assert_eq!(
        inspected_run["metadata"]["thread_id"],
        serde_json::Value::Null
    );

    let inspected_trace = http_json_request(port, "GET", &format!("/runs/{run_id}/trace"), None);
    assert_eq!(inspected_trace["run_id"], run_id);
    assert_eq!(inspected_trace["agent_id"], "ai_chat");
    assert_eq!(
        inspected_trace["workflow"]["metadata"]["case"],
        "catalog_cli"
    );
    assert_eq!(
        inspected_trace["events"][0]["payload"]["trigger"],
        "webhook"
    );
    assert_eq!(
        inspected_trace["events"][0]["payload"]["trigger_envelope"]["id"],
        "evt_http_1"
    );
    assert_eq!(
        inspected_trace["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );

    let events = http_text_request(port, "GET", &format!("/runs/{run_id}/events"), None);
    assert!(events.starts_with("HTTP/1.1 200"));
    assert!(events.contains("content-type: text/event-stream"));
    assert!(events.contains("id: 1"));
    assert!(events.contains("id: 2"));
    assert!(events.contains("event: run_started"));
    assert!(events.contains("event: catalog_dry_run.agent_selected"));
    assert!(events.contains(r#""kind":"catalog_dry_run.agent_selected""#));
    let resumed_events =
        http_text_request(port, "GET", &format!("/runs/{run_id}/events?after=1"), None);
    assert!(!resumed_events.contains("event: run_started"));
    assert!(resumed_events.contains("id: 2"));
    assert!(resumed_events.contains("event: catalog_dry_run.agent_selected"));
    let header_resumed_events = http_text_request_with_headers(
        port,
        "GET",
        &format!("/runs/{run_id}/events"),
        None,
        &[("Last-Event-ID", "1")],
    );
    assert!(!header_resumed_events.contains("event: run_started"));
    assert!(header_resumed_events.contains("id: 2"));
    let query_cursor_wins = http_text_request_with_headers(
        port,
        "GET",
        &format!("/runs/{run_id}/events?after=1"),
        None,
        &[("Last-Event-ID", "2")],
    );
    assert!(query_cursor_wins.contains("id: 2"));
    assert!(query_cursor_wins.contains("event: catalog_dry_run.agent_selected"));
    let invalid_cursor = try_http_text_request(
        port,
        "GET",
        &format!("/runs/{run_id}/events?after=not-a-cursor"),
        None,
    )
    .expect_err("invalid run event cursor should return an HTTP error");
    assert!(invalid_cursor.contains("HTTP/1.1 400"));
    assert!(invalid_cursor.contains("invalid_event_cursor"));

    let event_log = find_single_run_event_log(&store);
    std::fs::remove_file(event_log).expect("event log removed for trace fallback");
    let fallback_events =
        http_text_request(port, "GET", &format!("/runs/{run_id}/events?after=1"), None);
    assert!(!fallback_events.contains("event: run_started"));
    assert!(fallback_events.contains("id: 2"));
    assert!(fallback_events.contains("event: catalog_dry_run.agent_selected"));

    let replay = http_json_request(port, "POST", &format!("/runs/{run_id}/replay"), Some("{}"));
    let replay_run_id = replay["replay_run_id"]
        .as_str()
        .expect("replay run id is string");
    assert_eq!(replay["source_run_id"], run_id);
    assert_eq!(replay["agent_id"], "ai_chat");
    assert_eq!(replay["result"]["status"], "completed");
    assert_eq!(replay["output_matches"], true);
    assert_ne!(replay_run_id, run_id);
    assert_eq!(replay["trace"]["run_id"], replay_run_id);

    let metrics = http_json_request(port, "GET", "/metrics/summary", None);
    assert_eq!(metrics["run_count"], 2);
    assert_eq!(metrics["successful_run_count"], 2);
    assert_eq!(metrics["replay_count"], 1);
    assert_eq!(metrics["runs_by_status"]["completed"], 2);

    let persisted_trace = store.join("traces").join(format!("{run_id}.trace.json"));
    assert!(
        persisted_trace.exists(),
        "HTTP agent.run persists trace for debug bundle export"
    );
}

#[test]
fn http_server_runs_workflow_dag_and_persists_node_traces() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-workflow-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
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
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let workflow = http_json_request(
        port,
        "POST",
        "/workflows/run",
        Some(
            r#"{"protocol_version":"agent.v1","workflow_id":"workflow_http_dag","nodes":[{"node_id":"collect","agent_id":"ai_chat","input":{"message":"collect"}},{"node_id":"summarize","agent_id":"ai_chat","depends_on":["collect"],"input":{"message":"summarize"},"input_mappings":[{"from_node":"collect","from_path":"/input/message","to_path":"/from_collect"}]}],"metadata":{"case":"http_workflow"}}"#,
        ),
    );

    assert_eq!(workflow["protocol_version"], "agent.v1");
    assert_eq!(workflow["workflow_id"], "workflow_http_dag");
    assert_eq!(workflow["status"], "completed");
    let nodes = workflow["nodes"].as_array().expect("workflow nodes");
    assert_eq!(nodes.len(), 2);
    assert_eq!(
        nodes[0]["trace"]["workflow"]["workflow_id"],
        "workflow_http_dag"
    );
    assert_eq!(
        nodes[1]["trace"]["workflow"]["dependencies"][0]["run_id"],
        nodes[0]["run_id"]
    );
    assert_eq!(nodes[1]["output"]["input"]["from_collect"], "collect");
    let first_run_id = nodes[0]["run_id"].as_str().expect("first run id");
    let inspected_trace =
        http_json_request(port, "GET", &format!("/runs/{first_run_id}/trace"), None);
    assert_eq!(inspected_trace["run_id"], first_run_id);
    assert_eq!(
        inspected_trace["workflow"]["metadata"]["workflow_node_id"],
        "collect"
    );
    assert_eq!(
        inspected_trace["events"][1]["kind"],
        "catalog_dry_run.agent_selected"
    );
}

#[test]
fn http_server_can_cancel_active_run_and_stream_live_events() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-cancel-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
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
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let run_id = "run_http_cancel";
    let run_body = format!(r#"{{"run_id":"{run_id}","input":{{"sleep_ms":3000}}}}"#);
    let run_path = "/agents/ai_chat/run".to_owned();
    let run_handle = std::thread::spawn({
        let run_body = run_body.clone();
        move || http_json_request(port, "POST", &run_path, Some(&run_body))
    });

    let inspected = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["run_id"], run_id);
    assert_eq!(inspected["status"], "running");
    wait_for_event_log_contains(&store, r#""kind":"run_started""#);
    let active_before_events = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(active_before_events["status"], "running");
    let active_snapshot = http_text_request(
        port,
        "GET",
        &format!("/runs/{run_id}/events?follow=false"),
        None,
    );
    assert!(active_snapshot.contains("content-type: text/event-stream"));
    assert!(active_snapshot.contains("id: 1"));
    assert!(active_snapshot.contains("event: run_started"));
    assert!(!active_snapshot.contains("event: run_finished"));

    let events_handle = std::thread::spawn({
        let path = format!("/runs/{run_id}/events");
        move || http_text_request(port, "GET", &path, None)
    });
    std::thread::sleep(Duration::from_millis(100));

    let cancelled = http_json_request(port, "POST", &format!("/runs/{run_id}/cancel"), Some("{}"));
    assert_eq!(cancelled["run_id"], run_id);
    assert_eq!(cancelled["cancellation_requested"], true);

    let run = run_handle.join().expect("run request joins");
    assert_eq!(run["result"]["run_id"], run_id);
    assert_eq!(run["result"]["status"], "cancelled");
    assert_eq!(run["result"]["error"]["code"], "cancelled");

    let events = events_handle.join().expect("events request joins");
    assert!(events.contains("content-type: text/event-stream"));
    assert!(events.contains("id: 1"));
    assert!(events.contains("event: run_started"));
    assert_eq!(events.matches("event: run_started").count(), 1);
    assert!(events.contains("event: run_cancel_requested"));
    assert!(events.contains("event: run_cancelled"));
    assert!(events.contains("event: run_finished"));
    assert!(events.contains(r#""status":"cancelled""#));

    let inspected = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["status"], "cancelled");
    assert_eq!(inspected["metadata"]["control"]["cancel_requested"], true);
    assert_eq!(
        inspected["metadata"]["control"]["cancel_requested_by"],
        "http"
    );
    let trace = http_json_request(port, "GET", &format!("/runs/{run_id}/trace"), None);
    let event_kinds = trace["events"]
        .as_array()
        .expect("trace events")
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"run_cancel_requested"));
    assert!(event_kinds.contains(&"run_cancelled"));
    assert!(event_kinds.contains(&"run_finished"));

    let event_log = find_single_run_event_log(&store);
    let event_log_text = std::fs::read_to_string(&event_log).expect("event log reads");
    assert!(event_log_text.contains(r#""kind":"run_finished""#));

    std::fs::remove_file(store.join("traces").join(format!("{run_id}.trace.json")))
        .expect("trace fallback removed");
    let persisted_events = http_text_request(port, "GET", &format!("/runs/{run_id}/events"), None);
    assert!(persisted_events.contains("content-type: text/event-stream"));
    assert!(persisted_events.contains("event: run_finished"));
    assert!(persisted_events.contains(r#""status":"cancelled""#));
}

#[test]
fn http_server_persists_cancel_intent_for_running_run_record() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-remote-cancel-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
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
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let run_id = "run_remote_cancel";
    std::fs::write(
        store.join("runs").join(format!("{run_id}.json")),
        serde_json::to_vec_pretty(&serde_json::json!({
            "protocol_version": "agent.v1",
            "run_id": run_id,
            "agent_id": "ai_chat",
            "status": "running",
            "scope": {"type": "global"},
            "started_at": "2026-07-03T00:00:00Z",
            "finished_at": null,
            "input": {},
            "output": {},
            "metadata": {}
        }))
        .expect("run record encodes"),
    )
    .expect("run record writes");

    let cancelled = http_json_request(port, "POST", &format!("/runs/{run_id}/cancel"), Some("{}"));
    assert_eq!(cancelled["run_id"], run_id);
    assert_eq!(cancelled["cancellation_requested"], true);
    assert_eq!(cancelled["status"], "running");

    let inspected = http_json_request(port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["status"], "running");
    assert_eq!(inspected["metadata"]["control"]["cancel_requested"], true);
    assert_eq!(
        inspected["metadata"]["control"]["cancel_requested_by"],
        "http"
    );
    assert!(
        inspected["metadata"]["control"]["cancel_requested_at"]
            .as_str()
            .unwrap_or_default()
            .contains('T')
    );
}

#[test]
fn http_server_validates_json_request_schemas() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-schema-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
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
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let run_error = try_http_text_request(
        port,
        "POST",
        "/agents/ai_chat/run",
        Some(r#"{"input":{},"unexpected":true}"#),
    )
    .expect_err("extra run field is rejected");
    assert!(run_error.contains("HTTP/1.1 400"));
    assert!(run_error.contains("schema_validation_failed"));

    let chat_error = try_http_text_request(
        port,
        "POST",
        "/chat/turn",
        Some(r#"{"protocol_version":"agent.v1","provider":"mock","model":"mock-model"}"#),
    )
    .expect_err("chat without messages is rejected");
    assert!(chat_error.contains("HTTP/1.1 400"));
    assert!(chat_error.contains("schema_validation_failed"));

    let resume_error = try_http_text_request(
        port,
        "POST",
        "/chat/resume",
        Some(r#"{"protocol_version":"agent.v1","tool_results":[]}"#),
    )
    .expect_err("chat resume without state is rejected");
    assert!(resume_error.contains("HTTP/1.1 400"));
    assert!(resume_error.contains("schema_validation_failed"));

    let workflow_error = try_http_text_request(
        port,
        "POST",
        "/workflows/run",
        Some(r#"{"protocol_version":"agent.v1","workflow_id":"workflow_invalid"}"#),
    )
    .expect_err("workflow without nodes is rejected");
    assert!(workflow_error.contains("HTTP/1.1 400"));
    assert!(workflow_error.contains("schema_validation_failed"));
}

#[test]
fn http_server_streams_chat_turn_events() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-chat-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
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
            "--mock-response",
            "hello over sse",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let response = http_text_request(
        port,
        "POST",
        "/chat/turn",
        Some(
            r#"{"protocol_version":"agent.v1","turn_id":"turn_http_1","agent_id":"ai_chat","provider":"mock","model":"mock-model","messages":[{"role":"user","content":"ping"}],"metadata":{"source":"http_test"}}"#,
        ),
    );

    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("content-type: text/event-stream"));
    assert!(response.contains("event: chat_turn_event"));
    assert!(response.contains(r#""kind":"started""#));
    assert!(response.contains(r#""kind":"delta""#));
    assert!(response.contains(r#""content":"hello over sse""#));
    assert!(response.contains(r#""kind":"round_finished""#));
    assert!(response.contains(r#""kind":"done""#));
}

#[test]
fn http_server_resumes_chat_turn_and_records_session_steps() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-chat-resume-store");
    let port = reserve_local_port();
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let child = std::process::Command::new(agent_bin)
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
            "--mock-response",
            "resumed over sse",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let created = http_json_request(
        port,
        "POST",
        "/sessions",
        Some(r#"{"title":"Chat resume","metadata":{"source":"test"}}"#),
    );
    let session_id = created["session"]["session_id"]
        .as_str()
        .expect("session id is string");
    let thread_id = created["thread"]["thread_id"]
        .as_str()
        .expect("thread id is string");
    let resume_body = serde_json::json!({
        "protocol_version": "agent.v1",
        "state": {
            "protocol_version": "agent.v1",
            "turn_id": "turn_resume_http",
            "surface": "ai_chat",
            "mode": "chat",
            "session_id": session_id,
            "thread_id": thread_id,
            "agent_id": "ai_chat",
            "provider": "mock",
            "model": "mock-model",
            "messages": [
                {"role": "user", "content": "read task", "metadata": {}},
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "read_task",
                            "input": {"id": "task_1"}
                        }
                    ],
                    "metadata": {}
                }
            ],
            "tools": [],
            "metadata": {"source": "http_test"},
            "max_tool_rounds": 4,
            "round": 1,
            "pending_tool_calls": [
                {"id": "call_1", "name": "read_task", "input": {"id": "task_1"}}
            ],
            "tool_execution": "client"
        },
        "tool_results": [
            {
                "tool_call_id": "call_1",
                "tool_name": "read_task",
                "output": {"title": "Task title"},
                "is_error": false
            }
        ]
    })
    .to_string();

    let response = http_text_request(port, "POST", "/chat/resume", Some(&resume_body));
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("content-type: text/event-stream"));
    assert!(response.contains(r#""kind":"tool_result""#));
    assert!(response.contains(r#""content":"resumed over sse""#));
    assert!(response.contains(r#""status":"completed""#));
    assert!(response.contains(r#""kind":"done""#));

    let shown = http_json_request(port, "GET", &format!("/sessions/{session_id}"), None);
    let steps = shown["threads"][0]["steps"]
        .as_array()
        .expect("steps are array");
    let kinds = steps
        .iter()
        .filter_map(|step| step["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"tool_call"));
    assert!(kinds.contains(&"llm_round"));
    assert!(kinds.contains(&"state_update"));
    assert!(steps.iter().any(|step| {
        step["kind"] == "llm_round" && step["payload"]["event"]["metadata"]["status"] == "completed"
    }));
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

#[test]
fn http_server_lists_and_calls_tools() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-tool-store");
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
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let tools = http_json_request(port, "GET", "/tools", None);
    assert_eq!(tools[0]["name"], "propose_fake");

    let call = http_json_request(
        port,
        "POST",
        "/tools/propose_fake/call",
        Some(r#"{"input":{"value":9}}"#),
    );
    assert_eq!(call["tool"], "propose_fake");
    assert_eq!(call["output"]["host"], "dev-tool-host");
    assert_eq!(call["output"]["input"]["value"], 9);
}

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

#[test]
fn http_server_persists_sessions_threads_and_steps() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("http-session-store");
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
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("HTTP server starts");
    let _server = ChildGuard(child);
    wait_for_http_server(port);

    let created = http_json_request(
        port,
        "POST",
        "/sessions",
        Some(r#"{"title":"HTTP debug","metadata":{"source":"test"}}"#),
    );
    let session_id = created["session"]["session_id"]
        .as_str()
        .expect("session id is string");
    let thread_id = created["thread"]["thread_id"]
        .as_str()
        .expect("thread id is string");
    assert_eq!(created["session"]["title"], "HTTP debug");
    assert_eq!(created["thread"]["session_id"], session_id);

    let listed = http_json_request(port, "GET", "/sessions", None);
    assert_eq!(listed[0]["session_id"], session_id);

    let run_body = format!(
        r#"{{"session_id":"{session_id}","thread_id":"{thread_id}","input":{{"message":"http session"}}}}"#
    );
    let run = http_json_request(port, "POST", "/agents/ai_chat/run", Some(&run_body));
    let run_id = run["result"]["run_id"].as_str().expect("run id is string");
    assert_eq!(run["result"]["status"], "completed");

    let shown = http_json_request(port, "GET", &format!("/sessions/{session_id}"), None);
    assert_eq!(shown["session"]["session_id"], session_id);
    assert_eq!(shown["threads"][0]["thread"]["thread_id"], thread_id);
    assert_eq!(shown["threads"][0]["steps"][0]["kind"], "agent_run");
    assert_eq!(shown["threads"][0]["steps"][0]["run_id"], run_id);

    let forked = http_json_request(
        port,
        "POST",
        &format!("/sessions/{session_id}/fork"),
        Some(&format!(
            r#"{{"parent_thread_id":"{thread_id}","title":"Alternative"}}"#
        )),
    );
    assert_eq!(forked["session_id"], session_id);
    assert_eq!(forked["parent_thread_id"], thread_id);
    assert_eq!(forked["thread"]["title"], "Alternative");

    let stored_run = read_json(store.join("runs").join(format!("{run_id}.json")));
    assert!(
        stored_run["idempotency_key"]
            .as_str()
            .is_some_and(|key| key.starts_with("idem_"))
    );
    assert_eq!(stored_run["metadata"]["session_id"], session_id);
    assert_eq!(stored_run["metadata"]["thread_id"], thread_id);
}

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

#[test]
fn http_server_follows_and_cancels_run_from_another_instance() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("multi-instance-store");
    let config_path = dir.path().join("agent-runtime.toml");
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
    .expect("config writes");
    let first_port = reserve_local_port();
    let mut second_port = reserve_local_port();
    while second_port == first_port {
        second_port = reserve_local_port();
    }
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let spawn_server = |port: u16| {
        std::process::Command::new(&agent_bin)
            .args([
                "--config",
                config_path.to_str().expect("utf8 config path"),
                "serve",
                "--catalog",
                "../../fixtures/contracts/catalog.valid.json",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("HTTP server starts")
    };
    let _first_server = ChildGuard(spawn_server(first_port));
    let _second_server = ChildGuard(spawn_server(second_port));
    wait_for_http_server(first_port);
    wait_for_http_server(second_port);

    let run_id = "run_cross_instance_follow";
    let run_body = format!(r#"{{"run_id":"{run_id}","input":{{"sleep_ms":3000}}}}"#);
    let run_handle = std::thread::spawn(move || {
        http_json_request(first_port, "POST", "/agents/ai_chat/run", Some(&run_body))
    });

    let inspected = http_json_request(second_port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["status"], "running");
    let events_handle = std::thread::spawn({
        let path = format!("/runs/{run_id}/events");
        move || http_text_request(second_port, "GET", &path, None)
    });
    std::thread::sleep(Duration::from_millis(100));

    let cancelled = http_json_request(
        second_port,
        "POST",
        &format!("/runs/{run_id}/cancel"),
        Some("{}"),
    );
    assert_eq!(cancelled["cancellation_requested"], true);

    let run = run_handle.join().expect("run request joins");
    assert_eq!(run["result"]["status"], "cancelled");
    let events = events_handle.join().expect("events request joins");
    assert!(events.contains("event: run_started"));
    assert!(events.contains("event: run_cancel_requested"));
    assert!(events.contains("event: run_cancelled"));
    assert!(events.contains("event: run_finished"));
    assert_eq!(events.matches("event: run_started").count(), 1);

    let inspected = http_json_request(second_port, "GET", &format!("/runs/{run_id}"), None);
    assert_eq!(inspected["status"], "cancelled");
    assert_eq!(inspected["version"], 3);
    assert_eq!(inspected["metadata"]["control"]["cancel_requested"], true);
}
