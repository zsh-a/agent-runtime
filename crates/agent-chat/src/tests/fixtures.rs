use super::*;

#[test]
fn committed_chat_turn_fixtures_match_runtime_types() {
    let request: ChatTurnRequest = serde_json::from_str(include_str!(
        "../../../../fixtures/contracts/chat-turn-request.valid.json"
    ))
    .expect("request fixture");
    let state = chat_turn_initial_state(&request).expect("request creates state");
    assert_eq!(state.provider, "mock");
    assert_eq!(state.max_tool_rounds, 4);

    let pending_state: ChatTurnState = serde_json::from_str(include_str!(
        "../../../../fixtures/contracts/chat-turn-state.requires-tool-results.valid.json"
    ))
    .expect("state fixture");
    let result: ChatToolResult = serde_json::from_str(include_str!(
        "../../../../fixtures/contracts/chat-tool-result.valid.json"
    ))
    .expect("tool result fixture");
    let resumed =
        chat_turn_apply_tool_results(pending_state, vec![result]).expect("resume fixture");
    assert_eq!(resumed.messages.len(), 3);
    assert!(resumed.pending_tool_calls.is_empty());

    let event: ChatTurnEvent = serde_json::from_str(include_str!(
            "../../../../fixtures/contracts/chat-turn-event.round-finished.requires-tool-results.valid.json"
        ))
        .expect("event fixture");
    assert_eq!(event.kind, ChatTurnEventKind::RoundFinished);
    assert_eq!(event.metadata["status"], "requires_tool_results");

    let event: ChatTurnEvent = serde_json::from_str(include_str!(
        "../../../../fixtures/contracts/chat-turn-event.context-snapshot.valid.json"
    ))
    .expect("context snapshot event fixture");
    assert_eq!(event.kind, ChatTurnEventKind::ContextSnapshot);
    assert!(event.metadata["context_snapshot"].is_object());
}

#[test]
fn shared_agent_chat_turn_event_fixture_matches_runtime_types() {
    let events: Vec<ChatTurnEvent> =
        serde_json::from_str(include_str!("../../../../fixtures/chat/turn_events.json"))
            .expect("shared chat turn events fixture");

    assert_eq!(
        events
            .iter()
            .map(|event| event.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            ChatTurnEventKind::Started,
            ChatTurnEventKind::Delta,
            ChatTurnEventKind::ToolCallStart,
            ChatTurnEventKind::ToolCallDelta,
            ChatTurnEventKind::ToolCallEnd,
            ChatTurnEventKind::Usage,
            ChatTurnEventKind::RoundFinished,
        ]
    );
    assert_eq!(events[2].tool_name.as_deref(), Some("get_holdings"));
    assert_eq!(
        events[4].tool_input.as_ref(),
        Some(&json!({"as_of": "today"}))
    );
    assert_eq!(events[5].usage.as_ref().expect("usage").total_tokens, 18);
    assert_eq!(events[6].metadata["status"], "requires_tool_results");
}
