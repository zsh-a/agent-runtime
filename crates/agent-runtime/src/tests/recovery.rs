use super::*;

#[tokio::test]
async fn recovery_abandons_only_stale_running_runs() {
    let store = agent_store::InMemoryRunStore::shared();
    let now = OffsetDateTime::now_utc();
    store
        .create_run(AgentRunRecord {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            version: 1,
            run_id: RunId("run_stale".to_owned()),
            idempotency_key: Some("idem_stale".to_owned()),
            agent_id: "echo".to_owned(),
            status: AgentRunStatus::Running,
            scope: RunScope::Global,
            started_at: now - time::Duration::seconds(120),
            finished_at: None,
            input: json!({"message": "old"}),
            output: json!({}),
            error: None,
            workflow: None,
            metadata: json!({}),
        })
        .await
        .expect("stale run saved");
    store
        .create_run(AgentRunRecord {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            version: 1,
            run_id: RunId("run_fresh".to_owned()),
            idempotency_key: Some("idem_fresh".to_owned()),
            agent_id: "echo".to_owned(),
            status: AgentRunStatus::Running,
            scope: RunScope::Global,
            started_at: now,
            finished_at: None,
            input: json!({"message": "fresh"}),
            output: json!({}),
            error: None,
            workflow: None,
            metadata: json!({}),
        })
        .await
        .expect("fresh run saved");
    store
        .create_run(AgentRunRecord {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            version: 1,
            run_id: RunId("run_completed_old".to_owned()),
            idempotency_key: Some("idem_completed_old".to_owned()),
            agent_id: "echo".to_owned(),
            status: AgentRunStatus::Completed,
            scope: RunScope::Global,
            started_at: now - time::Duration::seconds(120),
            finished_at: Some(now - time::Duration::seconds(119)),
            input: json!({"message": "already done"}),
            output: json!({}),
            error: None,
            workflow: None,
            metadata: json!({}),
        })
        .await
        .expect("completed run saved");

    let report = recover_stale_runs(
        store.as_ref(),
        &ExecutionPolicy {
            timeout: Duration::from_secs(60),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        },
    )
    .await
    .expect("recovery succeeds");

    // Recovery asks the store for running candidates instead of scanning every
    // historical run record.
    assert_eq!(report.scanned_runs, 2);
    assert_eq!(report.abandoned_count, 1);
    assert_eq!(report.recovered_runs[0].run_id.0, "run_stale");
    let stale = store
        .get_run(&RunId("run_stale".to_owned()))
        .await
        .expect("stale run reads")
        .expect("stale run exists");
    assert_eq!(stale.status, AgentRunStatus::Abandoned);
    assert_eq!(
        stale.error.expect("stale run has error").code,
        "stale_running_run_abandoned"
    );
    let fresh = store
        .get_run(&RunId("run_fresh".to_owned()))
        .await
        .expect("fresh run reads")
        .expect("fresh run exists");
    assert_eq!(fresh.status, AgentRunStatus::Running);
    assert!(fresh.finished_at.is_none());
}
