use super::*;

#[tokio::test]
async fn runner_captures_agent_events_and_usage_summary() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(UsageAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let outcome = runner
        .run_once("usage_agent", run_request())
        .await
        .expect("usage run succeeds");

    let llm_event = outcome
        .trace
        .events
        .iter()
        .find(|event| event.kind == "llm_response")
        .expect("llm event is traced");
    assert_eq!(llm_event.payload["agent_id"], json!("usage_agent"));
    assert_eq!(
        llm_event.payload["run_id"],
        json!(outcome.result.run_id.0.clone())
    );

    let usage = outcome.trace.usage_summary.as_ref().expect("usage summary");
    assert_eq!(usage.llm_request_count, 1);
    assert_eq!(usage.input_tokens, 11);
    assert_eq!(usage.output_tokens, 7);
    assert_eq!(usage.total_tokens, 18);
    assert_eq!(usage.cost_micros_by_currency["USD"], 123);
    assert_eq!(usage.by_provider[0].provider, "openai");
    assert_eq!(usage.by_provider[0].model.as_deref(), Some("gpt-test"));

    let llm_span = outcome
        .trace
        .spans
        .iter()
        .find(|span| span.name == "llm.openai")
        .expect("llm span");
    assert_eq!(llm_span.status, "completed");
    assert_eq!(llm_span.duration_ms, 42);
    assert_eq!(llm_span.attributes["provider"], json!("openai"));
    assert_eq!(llm_span.attributes["model"], json!("gpt-test"));
    assert_eq!(llm_span.attributes["total_tokens"], json!(18));
}

#[tokio::test]
async fn runner_collects_published_artifact_refs() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(ArtifactAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let outcome = runner
        .run_once("artifact_agent", run_request())
        .await
        .expect("artifact run succeeds");

    assert_eq!(outcome.trace.artifact_refs.len(), 1);
    let artifact = &outcome.trace.artifact_refs[0];
    assert_eq!(artifact.artifact_id, "artifact_test_pdf");
    assert_eq!(artifact.kind, ArtifactKind::Document);
    assert_eq!(
        artifact.redaction_classification,
        RedactionClassification::Confidential
    );
    assert_eq!(
        artifact.store.as_ref().expect("store ref").provider,
        "test_artifact_store"
    );
    assert!(
        outcome
            .trace
            .events
            .iter()
            .any(|event| event.kind == "artifact_published")
    );
}

#[tokio::test]
async fn runner_derives_tool_spans_from_tool_events() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(ToolAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let outcome = runner
        .run_once(
            "tool_user",
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input: json!({"query": "hello"}),
                user: None,
                scope: None,
                trigger: agent_core::TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata: json!({}),
            },
        )
        .await
        .expect("tool run succeeds");

    assert!(
        outcome
            .trace
            .events
            .iter()
            .any(|event| event.kind == "tool_call")
    );
    let run_span_id = outcome.trace.spans[0].span_id.clone();
    let tool_span = outcome
        .trace
        .spans
        .iter()
        .find(|span| span.name == "tool.lookup")
        .expect("tool span exists");
    assert_eq!(
        tool_span.parent_span_id.as_deref(),
        Some(run_span_id.as_str())
    );
    assert_eq!(tool_span.status, "completed");
    assert_eq!(tool_span.attributes["tool_name"], json!("lookup"));
    assert!(
        tool_span.attributes["input_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
}

#[tokio::test]
async fn runner_traces_state_reads_and_writes() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(StateAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let outcome = runner
        .run_once(
            "stateful",
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input: json!({"counter": 7}),
                user: None,
                scope: None,
                trigger: agent_core::TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata: json!({}),
            },
        )
        .await
        .expect("stateful run succeeds");

    let write = outcome
        .trace
        .events
        .iter()
        .find(|event| event.kind == "state_write")
        .expect("state write event exists");
    assert_eq!(write.payload["agent_id"], "stateful");
    assert_eq!(write.payload["key"], "last_input");
    assert_eq!(write.payload["status"], "completed");
    assert!(write.payload.get("value").is_none());
    assert!(
        write.payload["value_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );

    let read = outcome
        .trace
        .events
        .iter()
        .find(|event| event.kind == "state_read")
        .expect("state read event exists");
    assert_eq!(read.payload["agent_id"], "stateful");
    assert_eq!(read.payload["key"], "last_input");
    assert_eq!(read.payload["found"], true);
    assert!(read.payload.get("value").is_none());
    assert_eq!(outcome.result.output["loaded"]["counter"], 7);

    let run_span_id = outcome.trace.spans[0].span_id.clone();
    let state_span_names = outcome
        .trace
        .spans
        .iter()
        .filter(|span| span.parent_span_id.as_deref() == Some(run_span_id.as_str()))
        .map(|span| span.name.as_str())
        .collect::<Vec<_>>();
    assert!(state_span_names.contains(&"state.write"));
    assert!(state_span_names.contains(&"state.read"));
}
