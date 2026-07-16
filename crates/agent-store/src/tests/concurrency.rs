use super::*;

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_run_event_append_allocates_unique_cursors_concurrently() {
    use std::sync::Arc;

    use serde_json::json;

    let path = temp_root().join("concurrent-events.sqlite");
    let store = Arc::new(SqliteStore::open(&path).await.expect("sqlite opens"));
    let run_id = RunId("run_concurrent_events".to_owned());
    let mut tasks = Vec::new();

    for index in 0..32 {
        let store = Arc::clone(&store);
        let run_id = run_id.clone();
        tasks.push(tokio::spawn(async move {
            store
                .append_run_event(
                    &run_id,
                    TraceEvent::new("concurrent_event", json!({ "idx": index })),
                )
                .await
                .expect("event appends");
        }));
    }

    for task in tasks {
        task.await.expect("append task joins");
    }

    let events = store
        .list_run_events_after(&run_id, 0)
        .await
        .expect("events read")
        .expect("event log exists");
    assert_eq!(events.len(), 32);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.cursor, (index + 1) as u64);
    }

    let mut payload_indices = events
        .into_iter()
        .map(|event| event.event.payload["idx"].as_i64().expect("idx is numeric"))
        .collect::<Vec<_>>();
    payload_indices.sort_unstable();
    assert_eq!(payload_indices, (0..32).collect::<Vec<_>>());
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_serializes_concurrent_schema_migrations() {
    let path = temp_root().join("concurrent-migrations.sqlite");
    let first_path = path.clone();
    let second_path = path.clone();
    let (first, second) = tokio::join!(
        async move { SqliteStore::open(first_path).await },
        async move { SqliteStore::open(second_path).await },
    );
    let first = first.expect("first concurrent store opens");
    let second = second.expect("second concurrent store opens");
    assert_eq!(
        first.schema_version().await.expect("first version reads"),
        SqliteStore::supported_schema_version()
    );
    assert_eq!(
        second.schema_version().await.expect("second version reads"),
        SqliteStore::supported_schema_version()
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_run_update_allows_only_one_concurrent_version_writer() {
    let path = temp_root().join("concurrent-run-update.sqlite");
    let first = SqliteStore::open(&path).await.expect("first store opens");
    let second = SqliteStore::open(&path).await.expect("second store opens");
    let run_id = RunId("run_concurrent_update".to_owned());
    let run = AgentRunRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        version: 1,
        run_id: run_id.clone(),
        idempotency_key: Some("idem_concurrent_update".to_owned()),
        agent_id: "sqlite_agent".to_owned(),
        status: AgentRunStatus::Running,
        scope: RunScope::Global,
        started_at: OffsetDateTime::now_utc(),
        finished_at: None,
        input: json!({}),
        output: json!({}),
        error: None,
        workflow: None,
        metadata: json!({}),
    };
    first.create_run(run.clone()).await.expect("run saved");
    let mut first_update = run.clone();
    first_update.version = 2;
    first_update.metadata = json!({"writer": "first"});
    let mut second_update = run;
    second_update.version = 2;
    second_update.metadata = json!({"writer": "second"});

    let (first_won, second_won) = tokio::join!(
        first.update_run(first_update, 1),
        second.update_run(second_update, 1),
    );
    assert_ne!(
        first_won.expect("first update completes"),
        second_won.expect("second update completes"),
        "exactly one writer wins the expected version"
    );
    let stored = first
        .get_run(&run_id)
        .await
        .expect("run reads")
        .expect("run exists");
    assert_eq!(stored.version, 2);
}
