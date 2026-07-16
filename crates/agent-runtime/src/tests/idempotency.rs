use super::*;

#[test]
fn run_idempotency_key_is_stable_for_retry_material() {
    let scope = RunScope::Global;
    let request = RunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: None,
        input: json!({"message": "ignored"}),
        user: None,
        scope: None,
        trigger: agent_core::TriggerKind::Scheduled,
        trigger_envelope: None,
        workflow: None,
        metadata: json!({"scheduled_for": "2026-06-28T09:00:00Z"}),
    };
    let same_retry = RunRequest {
        input: json!({"message": "different input does not affect retry identity"}),
        ..request.clone()
    };
    let different_schedule = RunRequest {
        metadata: json!({"scheduled_for": "2026-06-28T10:00:00Z"}),
        ..request.clone()
    };

    let run_id = RunId("run_test".to_owned());
    let first = run_idempotency_key("echo", &scope, &request, &run_id);
    let second = run_idempotency_key("echo", &scope, &same_retry, &run_id);
    let third = run_idempotency_key("echo", &scope, &different_schedule, &run_id);

    assert_eq!(first, second);
    assert_ne!(first, third);
    assert_eq!(first.len(), "idem_".len() + 64);
}

#[test]
fn run_idempotency_key_uses_external_trigger_identity() {
    let scope = RunScope::Global;
    let request = RunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: None,
        input: json!({"message": "ignored"}),
        user: None,
        scope: None,
        trigger: agent_core::TriggerKind::Webhook,
        trigger_envelope: Some(agent_core::TriggerEnvelope {
            source: "github.webhook".to_owned(),
            id: Some("evt_1".to_owned()),
            received_at: None,
            payload: json!({"action": "opened"}),
            metadata: json!({}),
        }),
        workflow: None,
        metadata: json!({}),
    };
    let same_retry = RunRequest {
        input: json!({"message": "different input"}),
        ..request.clone()
    };
    let different_event = RunRequest {
        trigger_envelope: Some(agent_core::TriggerEnvelope {
            id: Some("evt_2".to_owned()),
            ..request
                .trigger_envelope
                .clone()
                .expect("request has trigger envelope")
        }),
        ..request.clone()
    };
    let payload_without_id = RunRequest {
        trigger_envelope: Some(agent_core::TriggerEnvelope {
            id: None,
            payload: json!({"action": "closed"}),
            ..request
                .trigger_envelope
                .clone()
                .expect("request has trigger envelope")
        }),
        ..request.clone()
    };

    let run_id = RunId("run_test".to_owned());
    let first = run_idempotency_key("echo", &scope, &request, &run_id);
    let second = run_idempotency_key("echo", &scope, &same_retry, &run_id);
    let third = run_idempotency_key("echo", &scope, &different_event, &run_id);
    let fourth = run_idempotency_key("echo", &scope, &payload_without_id, &run_id);

    assert_eq!(first, second);
    assert_ne!(first, third);
    assert_ne!(first, fourth);
}

#[tokio::test]
async fn runner_deduplicates_external_trigger_identity() {
    let executions = Arc::new(AtomicUsize::new(0));
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(CountingAgent {
        executions: executions.clone(),
    })]);
    let runner = AgentRunner::new(
        registry,
        agent_store::InMemoryRunStore::shared(),
        Arc::new(NoopServices {
            state_store: agent_store::InMemoryStateStore::shared(),
        }),
    );
    let request = RunRequest {
        trigger: agent_core::TriggerKind::Queue,
        trigger_envelope: Some(agent_core::TriggerEnvelope {
            source: "orders".to_owned(),
            id: Some("message-1".to_owned()),
            received_at: None,
            payload: json!({}),
            metadata: json!({}),
        }),
        ..run_request()
    };

    let first = runner
        .run_once("counting", request.clone())
        .await
        .expect("first delivery runs");
    let second = runner
        .run_once("counting", request)
        .await
        .expect("duplicate delivery resolves");

    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(first.result.run_id, second.result.run_id);
    assert!(first.should_persist_trace());
    assert!(!second.should_persist_trace());
    assert_eq!(second.trace.events[0].kind, "run_deduplicated");
}
