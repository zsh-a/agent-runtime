use super::*;

#[tokio::test]
async fn runner_executes_agent_and_records_trace() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store.clone(), services);

    let outcome = runner
        .run_once(
            "echo",
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input: json!({"hello": "world"}),
                user: None,
                scope: None,
                trigger: agent_core::TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata: json!({}),
            },
        )
        .await
        .expect("run succeeds");

    assert!(matches!(outcome.result.status, AgentRunStatus::Completed));
    assert_eq!(outcome.result.output, json!({"hello": "world"}));
    assert_eq!(outcome.trace.events.len(), 2);
    assert_eq!(outcome.trace.spans.len(), 1);
    let span = &outcome.trace.spans[0];
    assert_eq!(span.name, "agent.run");
    assert_eq!(span.status, "completed");
    assert_eq!(
        span.attributes["run_id"],
        json!(outcome.result.run_id.0.clone())
    );
    assert_eq!(span.attributes["agent_id"], json!("echo"));
    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert!(
        stored
            .idempotency_key
            .as_deref()
            .is_some_and(|key| key.starts_with("idem_"))
    );
}

#[tokio::test]
async fn runner_uses_explicit_tenant_scope_for_runs() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store.clone(), services);

    let mut request = run_request();
    request.scope = Some(RunScope::Tenant("tenant_acme".to_owned()));
    request.user = Some(agent_core::UserContext {
        user_id: "user_123".to_owned(),
        metadata: json!({}),
    });

    let outcome = runner
        .run_once("echo", request)
        .await
        .expect("tenant scoped run succeeds");

    let record = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert_eq!(record.scope, RunScope::Tenant("tenant_acme".to_owned()));
}

#[tokio::test]
async fn runner_retries_retryable_agent_errors() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(FlakyAgent {
        attempts: attempts.clone(),
    })]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner =
        AgentRunner::new(registry, run_store.clone(), services).with_policy(ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 1,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });

    let outcome = runner
        .run_once("flaky", run_request())
        .await
        .expect("retryable run eventually succeeds");

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(outcome.result.status, AgentRunStatus::Completed);
    assert_eq!(outcome.result.output["attempt"], 2);
    let event_kinds = outcome
        .trace
        .events
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        event_kinds
            .iter()
            .filter(|kind| **kind == "run_attempt_started")
            .count(),
        2
    );
    assert!(event_kinds.contains(&"run_retry_scheduled"));

    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert_eq!(stored.status, AgentRunStatus::Completed);
    assert_eq!(stored.output["attempt"], 2);
}

#[tokio::test]
async fn runner_can_cancel_active_run_and_broadcast_events() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(BlockingAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner =
        AgentRunner::new(registry, run_store.clone(), services).with_policy(ExecutionPolicy {
            timeout: Duration::from_secs(30),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });
    let cancellation = CancellationToken::new();
    let (events, mut receiver) = broadcast::channel(16);
    let event_buffer = Arc::new(TraceEventBuffer::default());
    let request = RunRequest {
        run_id: Some(RunId("run_cancel_test".to_owned())),
        ..run_request()
    };
    let run = tokio::spawn({
        let cancellation = cancellation.clone();
        let event_buffer = event_buffer.clone();
        async move {
            let control = RunControl {
                cancellation,
                trace_events: Some(events),
                trace_event_buffer: Some(event_buffer.clone()),
            };
            runner
                .run_once_with_control("blocking", request, control)
                .await
        }
    });

    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("run_started event arrives")
            .expect("event channel stays open");
        if event.kind == "run_started" {
            break;
        }
    }
    assert!(
        event_buffer
            .events()
            .await
            .iter()
            .any(|event| event.kind == "run_started"),
        "trace event buffer should observe events before they are broadcast"
    );
    cancellation.cancel();

    let outcome = run
        .await
        .expect("run task joins")
        .expect("cancelled run returns outcome");

    assert_eq!(outcome.result.status, AgentRunStatus::Cancelled);
    assert_eq!(
        outcome.result.error.as_ref().expect("cancel error").code,
        "cancelled"
    );
    let event_kinds = outcome
        .trace
        .events
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"run_started"));
    assert!(event_kinds.contains(&"run_cancel_requested"));
    assert!(event_kinds.contains(&"run_cancelled"));
    assert!(event_kinds.contains(&"run_finished"));
    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert_eq!(stored.status, AgentRunStatus::Cancelled);
}

#[tokio::test]
async fn runner_observes_persisted_cancellation_request() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(BlockingAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = Arc::new(
        AgentRunner::new(registry, run_store.clone(), services).with_policy(ExecutionPolicy {
            timeout: Duration::from_secs(30),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        }),
    );
    let (events, mut receiver) = broadcast::channel(16);
    let run_id = RunId("run_store_cancel_test".to_owned());
    let request = RunRequest {
        run_id: Some(run_id.clone()),
        ..run_request()
    };
    let run = tokio::spawn({
        let runner = runner.clone();
        async move {
            let control = RunControl {
                trace_events: Some(events),
                ..RunControl::default()
            };
            runner
                .run_once_with_control("blocking", request, control)
                .await
        }
    });

    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("run_started event arrives")
            .expect("event channel stays open");
        if event.kind == "run_started" {
            break;
        }
    }
    let mut stored = run_store
        .get_run(&run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    stored.request_cancellation(OffsetDateTime::now_utc(), Some("test".to_owned()));
    let expected_version = stored.version;
    stored.version += 1;
    run_store
        .update_run(stored, expected_version)
        .await
        .expect("run cancellation intent persists")
        .then_some(())
        .expect("run cancellation update wins");

    let outcome = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("persisted cancellation is observed")
        .expect("run task joins")
        .expect("cancelled run returns outcome");

    assert_eq!(outcome.result.status, AgentRunStatus::Cancelled);
    assert_eq!(
        outcome.result.error.as_ref().expect("cancel error").code,
        "cancelled"
    );
    let event_kinds = outcome
        .trace
        .events
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"run_cancel_requested"));
    assert!(event_kinds.contains(&"run_cancelled"));
    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert_eq!(stored.status, AgentRunStatus::Cancelled);
    assert_eq!(stored.metadata["control"]["cancel_requested"], true);
    assert_eq!(stored.metadata["control"]["cancel_requested_by"], "test");
}
