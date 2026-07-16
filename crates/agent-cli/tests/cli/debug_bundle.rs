use super::*;

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
