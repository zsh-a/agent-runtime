use super::*;

#[tokio::test]
async fn events_command_loads_recent_trace_events_into_activity_and_context() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let run_id = RunId("run_events_test".to_owned());
    let now = OffsetDateTime::now_utc();
    let trace = AgentTrace {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: "test".to_owned(),
        run_id: run_id.clone(),
        agent_id: "echo_agent".to_owned(),
        agent_version: "0.1.0".to_owned(),
        scope: RunScope::Global,
        started_at: now,
        finished_at: now,
        input: json!({"message": "trace me"}),
        output: json!({"message": "done"}),
        workflow: None,
        usage_summary: None,
        spans: Vec::new(),
        events: vec![
            TraceEvent::new("run_started", json!({"agent_id": "echo_agent"})),
            TraceEvent::new(
                "tool_call_finished",
                json!({
                    "tool_name": "echo",
                    "status": "completed",
                    "duration_ms": 7
                }),
            ),
            TraceEvent::new(
                "proposal_decided",
                json!({
                    "proposal_id": "proposal_1",
                    "decision": "approve"
                }),
            ),
        ],
        artifact_refs: Vec::new(),
    };
    write_test_trace(&state.options.store_path, &trace).await;

    execute_command(&mut state, &format!("/events {} 2", run_id.0))
        .await
        .expect("events command succeeds");

    let events = state.latest_events.as_ref().expect("event summary");
    assert_eq!(events.run_id, "run_events_test");
    assert_eq!(events.agent_id, "echo_agent");
    assert_eq!(events.event_count, 3);
    assert_eq!(events.shown_count, 2);
    assert_eq!(events.events[0].kind, "tool_call_finished");
    assert_eq!(events.events[1].kind, "proposal_decided");
    assert_eq!(
        state.trace.as_ref().expect("trace loaded").run_id.0,
        "run_events_test"
    );
    assert!(
        state
            .trace_label
            .as_deref()
            .is_some_and(|label| label.contains("store trace run_events_test"))
    );
    assert!(state.activity.iter().any(|activity| {
        activity.kind == TuiActivityKind::Tool && activity.title == "tool_call_finished"
    }));
    assert!(state.activity.iter().any(|activity| {
        activity.kind == TuiActivityKind::Approval && activity.title == "proposal_decided"
    }));
    assert!(state.transcript.iter().any(|item| {
        item.content
            .contains("Events for run run_events_test: showing 2/3")
            && item.content.contains("tool_name=echo")
            && item.content.contains("proposal_id=proposal_1")
    }));
    let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
    assert!(rendered.contains("events"));
    assert!(rendered.contains("run_events_test 2/3"));
    assert!(rendered.contains("tool_call_finished"));
}

#[tokio::test]
async fn events_command_defaults_to_current_trace_run() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let now = OffsetDateTime::now_utc();
    let trace = AgentTrace {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: "test".to_owned(),
        run_id: RunId("run_events_default".to_owned()),
        agent_id: "echo_agent".to_owned(),
        agent_version: "0.1.0".to_owned(),
        scope: RunScope::Global,
        started_at: now,
        finished_at: now,
        input: json!({}),
        output: json!({}),
        workflow: None,
        usage_summary: None,
        spans: Vec::new(),
        events: vec![TraceEvent::new(
            "run_finished",
            json!({"status": "completed"}),
        )],
        artifact_refs: Vec::new(),
    };
    write_test_trace(&state.options.store_path, &trace).await;
    state.set_trace("current trace", trace);

    execute_command(&mut state, "/events")
        .await
        .expect("events command succeeds");

    let events = state.latest_events.as_ref().expect("event summary");
    assert_eq!(events.run_id, "run_events_default");
    assert_eq!(events.shown_count, 1);
    assert!(state.transcript.iter().any(|item| {
        item.role == TranscriptRole::User && item.content == "/events run_events_default 12"
    }));
    assert!(state.transcript.iter().any(|item| {
        item.content
            .contains("Events for run run_events_default: showing 1/1")
    }));
}

#[tokio::test]
async fn events_command_treats_single_numeric_argument_as_default_run_limit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let now = OffsetDateTime::now_utc();
    let trace = AgentTrace {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: "test".to_owned(),
        run_id: RunId("run_events_limit".to_owned()),
        agent_id: "echo_agent".to_owned(),
        agent_version: "0.1.0".to_owned(),
        scope: RunScope::Global,
        started_at: now,
        finished_at: now,
        input: json!({}),
        output: json!({}),
        workflow: None,
        usage_summary: None,
        spans: Vec::new(),
        events: vec![
            TraceEvent::new("run_started", json!({})),
            TraceEvent::new("tool_call_finished", json!({"tool_name": "echo"})),
            TraceEvent::new("run_finished", json!({"status": "completed"})),
        ],
        artifact_refs: Vec::new(),
    };
    write_test_trace(&state.options.store_path, &trace).await;
    state.set_trace("current trace", trace);

    execute_command(&mut state, "/events 2")
        .await
        .expect("events command succeeds");

    let events = state.latest_events.as_ref().expect("event summary");
    assert_eq!(events.run_id, "run_events_limit");
    assert_eq!(events.event_count, 3);
    assert_eq!(events.shown_count, 2);
    assert_eq!(events.events[0].kind, "tool_call_finished");
    assert_eq!(events.events[1].kind, "run_finished");
    assert!(state.transcript.iter().any(|item| {
        item.role == TranscriptRole::User && item.content == "/events run_events_limit 2"
    }));
}
