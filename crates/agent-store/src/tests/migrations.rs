use super::*;

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_upgrades_old_schema_version() {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let path = temp_root().join("old-version.sqlite");
    {
        let options = SqliteConnectOptions::new()
            .filename(path.as_std_path())
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("raw sqlite opens");
        sqlx::query("PRAGMA user_version = 1")
            .execute(&pool)
            .await
            .expect("old schema version fixture is set");
    }

    let reopened = SqliteStore::open(&path)
        .await
        .expect("old schema version upgrades");
    assert_eq!(
        reopened
            .schema_version()
            .await
            .expect("schema version reads"),
        8
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_migrates_unscoped_state_and_removes_legacy_table() {
    use serde_json::json;
    use sqlx::Row;

    let path = temp_root().join("v7-unscoped-state.sqlite");
    {
        let store = SqliteStore::open(&path).await.expect("sqlite opens");
        sqlx::query(
            r#"
            CREATE TABLE agent_state (
                agent_id TEXT NOT NULL,
                state_key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                PRIMARY KEY(agent_id, state_key)
            )
            "#,
        )
        .execute(store.pool())
        .await
        .expect("legacy state table is recreated");
        sqlx::query("INSERT INTO agent_state(agent_id, state_key, value_json) VALUES (?, ?, ?)")
            .bind("legacy_agent")
            .bind("settings")
            .bind(json!({"enabled": true}).to_string())
            .execute(store.pool())
            .await
            .expect("legacy state is inserted");
        sqlx::query("PRAGMA user_version = 7")
            .execute(store.pool())
            .await
            .expect("schema version rewinds to v7");
    }

    let reopened = SqliteStore::open(&path).await.expect("v7 schema upgrades");
    assert_eq!(
        reopened
            .load("legacy_agent", &RunScope::Global, "settings")
            .await
            .expect("migrated state reads"),
        Some(json!({"enabled": true}))
    );
    let legacy_table_count = sqlx::query(
        "SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'agent_state'",
    )
    .fetch_one(reopened.pool())
    .await
    .expect("sqlite schema reads")
    .get::<i64, _>("count");
    assert_eq!(legacy_table_count, 0);
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_upgrades_v2_schema_with_run_event_tables() {
    use serde_json::json;

    let path = temp_root().join("v2-events.sqlite");
    let run_id = RunId("run_v2_event_upgrade".to_owned());
    {
        let store = SqliteStore::open(&path).await.expect("sqlite opens");
        sqlx::query("DROP TABLE agent_run_events")
            .execute(store.pool())
            .await
            .expect("event table dropped");
        sqlx::query("DROP TABLE agent_run_event_logs")
            .execute(store.pool())
            .await
            .expect("event log table dropped");
        sqlx::query("DROP INDEX idx_agent_runs_status_started")
            .execute(store.pool())
            .await
            .expect("status index dropped");
        sqlx::query("ALTER TABLE agent_runs DROP COLUMN status")
            .execute(store.pool())
            .await
            .expect("status column dropped");
        sqlx::query("PRAGMA user_version = 2")
            .execute(store.pool())
            .await
            .expect("schema version rewinds to v2");
    }

    let reopened = SqliteStore::open(&path).await.expect("v2 schema upgrades");
    assert_eq!(
        reopened
            .schema_version()
            .await
            .expect("schema version reads"),
        8
    );
    reopened
        .append_run_event(&run_id, TraceEvent::new("run_started", json!({"v": 3})))
        .await
        .expect("event append works after migration");
    let events = reopened
        .list_run_events_after(&run_id, 0)
        .await
        .expect("events read")
        .expect("event log exists");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].cursor, 1);
    assert_eq!(events[0].event.kind, "run_started");
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_upgrades_v3_schema_with_run_status_index() {
    use agent_core::{AgentRunRecord, AgentRunStatus, PROTOCOL_VERSION, RunScope};
    use serde_json::json;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use time::OffsetDateTime;

    let path = temp_root().join("v3-status.sqlite");
    let now = OffsetDateTime::now_utc();
    let running = AgentRunRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        version: 1,
        run_id: RunId("run_v3_running".to_owned()),
        idempotency_key: Some("idem_v3_running".to_owned()),
        agent_id: "sqlite_agent".to_owned(),
        status: AgentRunStatus::Running,
        scope: RunScope::Global,
        started_at: now,
        finished_at: None,
        input: json!({}),
        output: json!({}),
        error: None,
        workflow: None,
        metadata: json!({}),
    };
    let completed = AgentRunRecord {
        status: AgentRunStatus::Completed,
        run_id: RunId("run_v3_completed".to_owned()),
        idempotency_key: Some("idem_v3_completed".to_owned()),
        finished_at: Some(now),
        ..running.clone()
    };
    let running_json = serde_json::to_string(&running).expect("running serializes");
    let completed_json = serde_json::to_string(&completed).expect("completed serializes");

    {
        let options = SqliteConnectOptions::new()
            .filename(path.as_std_path())
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("raw sqlite opens");
        sqlx::query(
            r#"
            CREATE TABLE agent_runs (
                run_id TEXT PRIMARY KEY NOT NULL,
                agent_id TEXT NOT NULL,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                started_at_sort INTEGER NOT NULL,
                record_json TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("v3 run table created");
        sqlx::query(
            r#"
            CREATE TABLE agent_run_event_logs (
                run_id TEXT PRIMARY KEY NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("v3 event log table created");
        sqlx::query(
            r#"
            CREATE TABLE agent_run_events (
                run_id TEXT NOT NULL,
                cursor INTEGER NOT NULL,
                event_json TEXT NOT NULL,
                PRIMARY KEY(run_id, cursor)
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("v3 event table created");
        sqlx::query(
            r#"
            INSERT INTO agent_runs(
                run_id,
                agent_id,
                scope_type,
                scope_id,
                started_at_sort,
                record_json
            )
            VALUES (?, ?, ?, ?, ?, ?), (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&running.run_id.0)
        .bind(&running.agent_id)
        .bind("global")
        .bind("")
        .bind(running.started_at.unix_timestamp_nanos() as i64)
        .bind(running_json)
        .bind(&completed.run_id.0)
        .bind(&completed.agent_id)
        .bind("global")
        .bind("")
        .bind(completed.started_at.unix_timestamp_nanos() as i64)
        .bind(completed_json)
        .execute(&pool)
        .await
        .expect("v3 run records inserted");
        sqlx::query("PRAGMA user_version = 3")
            .execute(&pool)
            .await
            .expect("schema version set to v3");
    }

    let reopened = SqliteStore::open(&path).await.expect("v3 schema upgrades");
    assert_eq!(
        reopened
            .schema_version()
            .await
            .expect("schema version reads"),
        8
    );
    let running_runs = reopened
        .list_runs_by_status(AgentRunStatus::Running, None)
        .await
        .expect("running runs list");
    assert_eq!(running_runs.len(), 1);
    assert_eq!(running_runs[0].run_id, running.run_id);
    let completed_runs = reopened
        .list_runs_by_status(AgentRunStatus::Completed, None)
        .await
        .expect("completed runs list");
    assert_eq!(completed_runs.len(), 1);
    assert_eq!(completed_runs[0].run_id, completed.run_id);
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_upgrades_v4_schema_with_trace_table() {
    let path = temp_root().join("v4-traces.sqlite");
    let run_id = RunId("run_v4_trace_upgrade".to_owned());
    {
        let store = SqliteStore::open(&path).await.expect("sqlite opens");
        sqlx::query("DROP TABLE agent_traces")
            .execute(store.pool())
            .await
            .expect("trace table dropped");
        sqlx::query("PRAGMA user_version = 4")
            .execute(store.pool())
            .await
            .expect("schema version rewinds to v4");
    }

    let reopened = SqliteStore::open(&path).await.expect("v4 schema upgrades");
    assert_eq!(
        reopened
            .schema_version()
            .await
            .expect("schema version reads"),
        8
    );
    let trace = sqlite_trace_record(run_id.clone(), "sqlite_agent");
    reopened
        .write_trace(trace)
        .await
        .expect("trace writes after migration");
    let stored = reopened
        .read_trace(&run_id)
        .await
        .expect("trace reads after migration")
        .expect("trace exists");
    assert_eq!(stored.run_id, run_id);
    assert_eq!(stored.events[0].kind, "run_started");
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_reports_supported_schema_version_from_migrations() {
    let store = SqliteStore::in_memory().await.expect("sqlite opens");
    assert_eq!(SqliteStore::supported_schema_version(), 8);
    assert_eq!(
        store.schema_version().await.expect("schema version reads"),
        SqliteStore::supported_schema_version()
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_rejects_future_schema_version_without_downgrade() {
    use sqlx::{
        Row,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };

    let path = temp_root().join("future-version.sqlite");
    {
        let store = SqliteStore::open(&path).await.expect("sqlite opens");
        sqlx::query("PRAGMA user_version = 999")
            .execute(store.pool())
            .await
            .expect("future schema version is set");
    }

    let err = match SqliteStore::open(&path).await {
        Ok(_) => panic!("future schema version is rejected"),
        Err(err) => err,
    };
    assert!(
        err.message
            .contains("schema version 999 is newer than supported version 8"),
        "unexpected error: {}",
        err.message
    );

    let options = SqliteConnectOptions::new().filename(path.as_std_path());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("raw sqlite pool opens after rejected migration");
    let user_version = sqlx::query("PRAGMA user_version")
        .fetch_one(&pool)
        .await
        .expect("schema version reads")
        .get::<i64, _>(0);
    assert_eq!(user_version, 999);
}
