use agent_core::{AgentLockStore, AgentRunEventStore, RunId, TraceEvent};
use camino::Utf8PathBuf;
use std::time::Duration;

use super::*;
use crate::testkit::{
    assert_lock_store_conformance, assert_proposal_store_conformance,
    assert_run_event_store_conformance, assert_run_store_conformance,
    assert_session_store_conformance, assert_state_store_conformance,
};

#[tokio::test]
async fn file_run_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileRunStore::new(root).await.expect("store opens");
    assert_run_store_conformance(&store).await;
}

#[tokio::test]
async fn file_run_event_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileRunEventStore::new(root).await.expect("store opens");
    assert_run_event_store_conformance(&store).await;
}

#[tokio::test]
async fn in_memory_run_store_satisfies_conformance() {
    let store = InMemoryRunStore::default();
    assert_run_store_conformance(&store).await;
}

#[tokio::test]
async fn file_proposal_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileProposalStore::new(root).await.expect("store opens");
    assert_proposal_store_conformance(&store).await;
}

#[tokio::test]
async fn in_memory_proposal_store_satisfies_conformance() {
    let store = InMemoryProposalStore::default();
    assert_proposal_store_conformance(&store).await;
}

#[tokio::test]
async fn file_session_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileSessionStore::new(root).await.expect("store opens");
    assert_session_store_conformance(&store).await;
}

#[tokio::test]
async fn in_memory_session_store_satisfies_conformance() {
    let store = InMemorySessionStore::default();
    assert_session_store_conformance(&store).await;
}

#[tokio::test]
async fn in_memory_state_store_satisfies_conformance() {
    let store = InMemoryStateStore::default();
    assert_state_store_conformance(&store).await;
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_satisfies_conformance() {
    let store = SqliteStore::in_memory().await.expect("sqlite opens");
    assert_run_store_conformance(&store).await;
    assert_run_event_store_conformance(&store).await;
    assert_proposal_store_conformance(&store).await;
    assert_session_store_conformance(&store).await;
    assert_state_store_conformance(&store).await;
    assert_lock_store_conformance(&store).await;
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_reopens_file_backed_records() {
    use agent_core::{
        AgentLockStore, AgentProposalStore, AgentRunRecord, AgentRunStatus, AgentRunStore,
        AgentSessionStore, AgentStateStore, PROTOCOL_VERSION, ProposalEnvelope, RunId, RunScope,
        SessionRecord, StepRecord, ThreadRecord,
    };
    use serde_json::json;
    use time::OffsetDateTime;

    let path = temp_root().join("store.sqlite");
    let run_id = RunId("run_sqlite_reopen".to_owned());
    let lock_key = "sqlite_reopen_lock";
    let session = SessionRecord::new("SQLite session", json!({"source": "reopen"}));
    let thread = ThreadRecord::root(
        session.session_id.clone(),
        Some("SQLite thread".to_owned()),
        json!({}),
    );
    let step = StepRecord::agent_run(
        thread.thread_id.clone(),
        run_id.clone(),
        Some("completed".to_owned()),
        json!({"status": "completed"}),
    );
    let proposal = ProposalEnvelope::new(
        run_id.clone(),
        "sqlite_agent",
        "fake",
        "SQLite proposal",
        json!({"idx": 1}),
    );
    let run = AgentRunRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: run_id.clone(),
        idempotency_key: Some("idem_sqlite_reopen".to_owned()),
        agent_id: "sqlite_agent".to_owned(),
        status: AgentRunStatus::Completed,
        scope: RunScope::Global,
        started_at: OffsetDateTime::now_utc(),
        finished_at: None,
        input: json!({"input": true}),
        output: json!({"output": true}),
        error: None,
        workflow: None,
        metadata: json!({}),
    };

    {
        let store = SqliteStore::open(&path).await.expect("sqlite opens");
        store.create_run(run.clone()).await.expect("run saved");
        store
            .create_proposal(proposal.clone())
            .await
            .expect("proposal saved");
        store
            .create_session(session.clone())
            .await
            .expect("session saved");
        store
            .create_thread(thread.clone())
            .await
            .expect("thread saved");
        store.create_step(step.clone()).await.expect("step saved");
        store
            .save("sqlite_agent", "state_key", json!({"state": true}))
            .await
            .expect("state saved");
        store
            .acquire(lock_key, "owner_1", Duration::from_secs(60))
            .await
            .expect("lock acquire checks")
            .expect("lock acquired");
    }

    let reopened = SqliteStore::open(&path).await.expect("sqlite reopens");
    assert_eq!(
        reopened
            .schema_version()
            .await
            .expect("schema version reads"),
        3
    );
    assert_eq!(
        reopened
            .get_run(&run_id)
            .await
            .expect("run reads")
            .expect("run exists")
            .output,
        json!({"output": true})
    );
    assert_eq!(
        reopened
            .get_proposal(&proposal.proposal_id)
            .await
            .expect("proposal reads")
            .expect("proposal exists")
            .summary,
        "SQLite proposal"
    );
    assert_eq!(
        reopened
            .get_session(&session.session_id)
            .await
            .expect("session reads")
            .expect("session exists")
            .title,
        "SQLite session"
    );
    assert_eq!(
        reopened
            .get_thread(&thread.thread_id)
            .await
            .expect("thread reads")
            .expect("thread exists")
            .title,
        Some("SQLite thread".to_owned())
    );
    assert_eq!(
        reopened
            .list_steps(&thread.thread_id)
            .await
            .expect("steps read")
            .first()
            .expect("step exists")
            .step_id,
        step.step_id
    );
    assert_eq!(
        reopened
            .load("sqlite_agent", "state_key")
            .await
            .expect("state reads")
            .expect("state exists"),
        json!({"state": true})
    );
    assert!(
        reopened
            .acquire(lock_key, "owner_2", Duration::from_secs(60))
            .await
            .expect("lock contention reads")
            .is_none()
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_upgrades_old_schema_version() {
    let path = temp_root().join("old-version.sqlite");
    {
        let store = SqliteStore::open(&path).await.expect("sqlite opens");
        sqlx::query("PRAGMA user_version = 1")
            .execute(store.pool())
            .await
            .expect("schema version rewinds for fixture");
        assert_eq!(
            store.schema_version().await.expect("schema version reads"),
            1
        );
    }

    let reopened = SqliteStore::open(&path)
        .await
        .expect("old schema version upgrades");
    assert_eq!(
        reopened
            .schema_version()
            .await
            .expect("schema version reads"),
        3
    );
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
        3
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
async fn sqlite_store_reports_supported_schema_version_from_migrations() {
    let store = SqliteStore::in_memory().await.expect("sqlite opens");
    assert_eq!(SqliteStore::supported_schema_version(), 3);
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
            .contains("schema version 999 is newer than supported version 3"),
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

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_lock_replaces_expired_lease() {
    let store = SqliteStore::in_memory().await.expect("sqlite opens");
    let key = "sqlite_expired_lock";
    store
        .acquire(key, "owner_1", Duration::from_secs(1))
        .await
        .expect("first acquire checks")
        .expect("first owner acquires");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let replacement = store
        .acquire(key, "owner_2", Duration::from_secs(60))
        .await
        .expect("expired acquire checks")
        .expect("expired lease is replaced");
    assert_eq!(replacement.owner, "owner_2");
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_file_lock_allows_one_concurrent_owner() {
    use std::sync::Arc;

    let path = temp_root().join("locks.sqlite");
    let store = Arc::new(SqliteStore::open(&path).await.expect("sqlite opens"));
    let first = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .acquire("sqlite_concurrent_lock", "owner_1", Duration::from_secs(60))
                .await
                .expect("first acquire checks")
        })
    };
    let second = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .acquire("sqlite_concurrent_lock", "owner_2", Duration::from_secs(60))
                .await
                .expect("second acquire checks")
        })
    };
    let first = first.await.expect("first task joins");
    let second = second.await.expect("second task joins");
    assert_eq!(
        usize::from(first.is_some()) + usize::from(second.is_some()),
        1
    );
}

#[tokio::test]
async fn file_lock_store_coordinates_lease_owners() {
    let root = temp_root();
    let store = FileLockStore::new(root).await.expect("store opens");

    let first = store
        .acquire("agent:echo:scope:global", "run_1", Duration::from_secs(60))
        .await
        .expect("lock acquired")
        .expect("first owner gets lease");
    assert_eq!(first.owner, "run_1");

    let contended = store
        .acquire("agent:echo:scope:global", "run_2", Duration::from_secs(60))
        .await
        .expect("contended lock checks");
    assert!(contended.is_none());

    store.release(first).await.expect("lock released");
    let second = store
        .acquire("agent:echo:scope:global", "run_2", Duration::from_secs(60))
        .await
        .expect("second lock acquired")
        .expect("second owner gets released lease");
    assert_eq!(second.owner, "run_2");
}

#[tokio::test]
async fn file_lock_store_satisfies_conformance() {
    let root = temp_root();
    let store = FileLockStore::new(root).await.expect("store opens");
    assert_lock_store_conformance(&store).await;
}

fn temp_root() -> Utf8PathBuf {
    let temp = tempfile::tempdir().expect("tempdir");
    Utf8PathBuf::from_path_buf(temp.keep()).expect("utf8 temp path")
}
