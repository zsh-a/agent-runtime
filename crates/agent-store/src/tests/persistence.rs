use super::*;

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_store_reopens_file_backed_records() {
    use agent_core::{
        AgentLockStore, AgentProposalStore, AgentRunRecord, AgentRunStatus, AgentRunStore,
        AgentSessionStore, AgentStateStore, ProposalEnvelope, RunId, SessionRecord, StepRecord,
        ThreadRecord,
    };
    use serde_json::json;
    use time::OffsetDateTime;

    let path = temp_root().join("store.sqlite");
    let run_id = RunId("run_sqlite_reopen".to_owned());
    let trace = sqlite_trace_record(run_id.clone(), "sqlite_agent");
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
        version: 1,
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
        store.write_trace(trace.clone()).await.expect("trace saved");
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
            .save(
                "sqlite_agent",
                &RunScope::Global,
                "state_key",
                json!({"state": true}),
            )
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
        8
    );
    assert_eq!(
        reopened
            .read_trace(&run_id)
            .await
            .expect("trace reads")
            .expect("trace exists")
            .events[0]
            .kind,
        "run_started"
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
            .load("sqlite_agent", &RunScope::Global, "state_key")
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
