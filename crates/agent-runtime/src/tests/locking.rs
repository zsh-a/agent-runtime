use super::*;

#[tokio::test]
async fn runner_respects_max_concurrent_runs_policy() {
    let counters = Arc::new(ConcurrencyCounters::default());
    let registry =
        InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::new("slow", counters.clone()))]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = Arc::new(AgentRunner::new(registry, run_store, services).with_policy(
        ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        },
    ));

    let first = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run_once("slow", run_request()).await })
    };
    let second = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run_once("slow", run_request()).await })
    };

    first
        .await
        .expect("first task joins")
        .expect("first run succeeds");
    second
        .await
        .expect("second task joins")
        .expect("second run succeeds");

    assert_eq!(counters.max_seen.load(Ordering::SeqCst), 1);
    assert_eq!(counters.completed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn runner_skips_duplicate_agent_scope_when_lease_is_active() {
    let counters = Arc::new(ConcurrencyCounters::default());
    let registry =
        InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::new("slow", counters.clone()))]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = Arc::new(AgentRunner::new(registry, run_store, services).with_policy(
        ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 2,
        },
    ));

    let first = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run_once("slow", run_request()).await })
    };
    sleep(Duration::from_millis(10)).await;
    let second = runner
        .run_once("slow", run_request())
        .await
        .expect("second run returns skipped outcome");
    let first = first
        .await
        .expect("first task joins")
        .expect("first run succeeds");

    let statuses = [first.result.status, second.result.status];
    assert!(statuses.contains(&AgentRunStatus::Completed));
    assert!(statuses.contains(&AgentRunStatus::Skipped));
    assert_eq!(counters.max_seen.load(Ordering::SeqCst), 1);
    assert_eq!(counters.completed.load(Ordering::SeqCst), 1);
    assert_eq!(second.trace.events[0].kind, "run_skipped");
}

#[tokio::test]
async fn sqlite_shared_store_coordinates_duplicate_agent_scope_across_runner_handles() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp
        .path()
        .join("runtime.sqlite")
        .to_str()
        .expect("utf8 temp path")
        .to_owned();
    let counters = Arc::new(ConcurrencyCounters::default());
    let first_started = Arc::new(Notify::new());

    let first_store = Arc::new(
        agent_store::SqliteStore::open(db_path.as_str())
            .await
            .expect("first sqlite handle opens"),
    );
    let second_store = Arc::new(
        agent_store::SqliteStore::open(db_path.as_str())
            .await
            .expect("second sqlite handle opens"),
    );

    let first_runner = Arc::new(
        AgentRunner::new(
            InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::with_started_notify(
                "slow",
                counters.clone(),
                first_started.clone(),
            ))]),
            first_store.clone(),
            Arc::new(NoopServices {
                state_store: agent_store::InMemoryStateStore::shared(),
            }),
        )
        .with_lock_store(first_store.clone())
        .with_policy(ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 2,
        }),
    );
    let second_runner = AgentRunner::new(
        InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::new("slow", counters.clone()))]),
        second_store.clone(),
        Arc::new(NoopServices {
            state_store: agent_store::InMemoryStateStore::shared(),
        }),
    )
    .with_lock_store(second_store.clone())
    .with_policy(ExecutionPolicy {
        timeout: Duration::from_secs(5),
        max_retries: 0,
        retry_backoff: Duration::ZERO,
        max_concurrent_runs: 2,
    });

    let first = {
        let first_runner = first_runner.clone();
        tokio::spawn(async move { first_runner.run_once("slow", run_request()).await })
    };
    timeout(Duration::from_secs(1), first_started.notified())
        .await
        .expect("first sqlite-backed run enters the slow agent before duplicate run starts");
    let second = second_runner
        .run_once("slow", run_request())
        .await
        .expect("second runner returns outcome");
    let first = first
        .await
        .expect("first task joins")
        .expect("first runner returns outcome");

    let statuses = [first.result.status, second.result.status];
    assert!(statuses.contains(&AgentRunStatus::Completed));
    assert!(statuses.contains(&AgentRunStatus::Skipped));
    assert_eq!(counters.max_seen.load(Ordering::SeqCst), 1);
    assert_eq!(counters.completed.load(Ordering::SeqCst), 1);

    let persisted = agent_store::SqliteStore::open(db_path.as_str())
        .await
        .expect("verification sqlite handle opens")
        .list_runs(Some("slow"), None)
        .await
        .expect("runs list from sqlite");
    assert_eq!(persisted.len(), 2);
    assert_eq!(
        persisted
            .iter()
            .filter(|run| run.status == AgentRunStatus::Completed)
            .count(),
        1
    );
    assert_eq!(
        persisted
            .iter()
            .filter(|run| run.status == AgentRunStatus::Skipped)
            .count(),
        1
    );
}

#[tokio::test]
async fn runner_skips_duplicate_workflow_scope_when_lease_is_active() {
    let counters = Arc::new(ConcurrencyCounters::default());
    let registry =
        InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::new("slow", counters.clone()))]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = Arc::new(AgentRunner::new(registry, run_store, services).with_policy(
        ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 2,
        },
    ));

    let first = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run_workflow(slow_workflow_request()).await })
    };
    sleep(Duration::from_millis(10)).await;
    let second = runner
        .run_workflow(slow_workflow_request())
        .await
        .expect("second workflow returns skipped result");
    let first = first
        .await
        .expect("first workflow task joins")
        .expect("first workflow succeeds");
    let third = runner
        .run_workflow(slow_workflow_request())
        .await
        .expect("workflow lease is released after first run");

    assert_eq!(first.status, AgentRunStatus::Completed);
    assert_eq!(second.status, AgentRunStatus::Skipped);
    assert_eq!(third.status, AgentRunStatus::Completed);
    assert_eq!(second.nodes.len(), 1);
    assert_eq!(second.nodes[0].status, AgentRunStatus::Skipped);
    assert_eq!(second.nodes[0].metadata["reason"], "workflow_lease_active");
    assert_eq!(
        second.nodes[0].metadata["workflow"]["scope"]["type"],
        "tenant"
    );
    assert_eq!(
        second.nodes[0].metadata["workflow"]["scope"]["id"],
        "tenant_slow"
    );
    assert_eq!(counters.completed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn runner_renews_run_lease_while_active() {
    let lock_store = Arc::new(CountingLockStore::default());
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(LeaseProbeAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services)
        .with_lock_store(lock_store.clone())
        .with_policy(ExecutionPolicy {
            timeout: Duration::from_millis(500),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });

    let outcome = runner
        .run_once("lease_probe", run_request())
        .await
        .expect("lease probe run succeeds");

    assert_eq!(outcome.result.status, AgentRunStatus::Completed);
    assert_eq!(lock_store.release_count.load(Ordering::SeqCst), 1);
    assert!(
        lock_store
            .renewed_keys()
            .iter()
            .any(|key| key == "agent:lease_probe:scope:global")
    );
}

#[tokio::test]
async fn runner_renews_workflow_lease_while_active() {
    let lock_store = Arc::new(CountingLockStore::default());
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(LeaseProbeAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services)
        .with_lock_store(lock_store.clone())
        .with_policy(ExecutionPolicy {
            timeout: Duration::from_millis(500),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });

    let result = runner
        .run_workflow(lease_probe_workflow_request())
        .await
        .expect("lease probe workflow succeeds");

    assert_eq!(result.status, AgentRunStatus::Completed);
    let renewed_keys = lock_store.renewed_keys();
    assert!(
        renewed_keys
            .iter()
            .any(|key| { key == "workflow:workflow_lease_probe:scope:tenant:tenant_lease" })
    );
    assert!(
        renewed_keys
            .iter()
            .any(|key| key == "agent:lease_probe:scope:tenant:tenant_lease")
    );
}

#[tokio::test]
async fn runner_releases_lease_when_final_run_update_fails() {
    let lock_store = Arc::new(CountingLockStore::default());
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = Arc::new(FailingUpdateRunStore);
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner =
        AgentRunner::new(registry, run_store, services).with_lock_store(lock_store.clone());

    let error = match runner.run_once("echo", run_request()).await {
        Ok(_) => panic!("final run update should fail"),
        Err(error) => error,
    };

    assert_eq!(error.record.code, "internal_error");
    assert_eq!(lock_store.release_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runner_cancels_run_when_lease_ownership_is_lost() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(BlockingAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let services = Arc::new(NoopServices {
        state_store: agent_store::InMemoryStateStore::shared(),
    });
    let runner = AgentRunner::new(registry, run_store.clone(), services)
        .with_lock_store(Arc::new(LosingLockStore))
        .with_policy(ExecutionPolicy {
            timeout: Duration::from_millis(300),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });

    let outcome = timeout(
        Duration::from_secs(2),
        runner.run_once("blocking", run_request()),
    )
    .await
    .expect("lease loss cancels promptly")
    .expect("cancelled run returns an outcome");

    assert_eq!(outcome.result.status, AgentRunStatus::Cancelled);
    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run reads")
        .expect("run exists");
    assert_eq!(stored.status, AgentRunStatus::Cancelled);
}
