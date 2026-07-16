use super::*;

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
