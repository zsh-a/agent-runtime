use super::*;

#[test]
fn legacy_nested_policy_denial_is_an_effective_chat_error() {
    let result = ChatToolResult {
        tool_call_id: "call_policy".to_owned(),
        tool_name: "external_lookup".to_owned(),
        output: json!({
            "error": {"code": "policy_denied", "message": "blocked"},
            "policy_denied": true
        }),
        is_error: false,
        outcome: None,
    };

    assert!(result.effective_is_error());
    assert_eq!(
        result.effective_outcome().status,
        agent_core::ToolOutcomeStatus::PolicyDenied
    );
}

#[tokio::test]
async fn chat_turn_resumes_from_state_and_tool_results() {
    let initial = chat_turn_initial_state(&ChatTurnRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        turn_id: Some("turn_resume".to_owned()),
        surface: Some("agent_tui".to_owned()),
        mode: Some("natural_language".to_owned()),
        session_id: Some("session_1".to_owned()),
        thread_id: Some("thread_1".to_owned()),
        agent_id: Some("chat".to_owned()),
        provider: "mock".to_owned(),
        model: "mock-model".to_owned(),
        messages: vec![user_message("use a tool")],
        temperature: None,
        max_output_tokens: None,
        tools: vec![],
        metadata: json!({}),
        context_policy: Default::default(),
        max_tool_rounds: 4,
        tool_execution: ChatToolExecution::Runtime,
    })
    .expect("initial state");
    let response = LlmResponse {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        provider: "mock".to_owned(),
        model: "mock-model".to_owned(),
        content: String::new(),
        finish_reason: LlmFinishReason::ToolCall,
        object: None,
        usage: None,
        metadata: json!({}),
    };
    let pending = match chat_turn_apply_response(
        initial,
        "",
        vec![ChatToolCall {
            id: "call_1".to_owned(),
            name: "echo".to_owned(),
            input: json!({"value": "ok"}),
        }],
        &response,
    )
    .expect("requires tools")
    {
        ChatTurnAdvance::RequiresToolResults { state, .. } => state,
        ChatTurnAdvance::Completed { .. } => panic!("expected tool results"),
    };

    let runner = ChatTurnRunner::new(
        Arc::new(MockLlmProvider::new("mock", "mock-model", "resumed")),
        Arc::new(TestServices),
    );
    let events = runner
        .resume(ChatResumeRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            state: pending,
            tool_results: vec![ChatToolResult {
                tool_call_id: "call_1".to_owned(),
                tool_name: "echo".to_owned(),
                output: json!({"value": "ok"}),
                is_error: false,
                outcome: None,
            }],
        })
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("events ok");

    assert!(
        events
            .iter()
            .any(|event| event.kind == ChatTurnEventKind::ToolResult)
    );
    assert!(
        events
            .iter()
            .any(|event| event.content.as_deref() == Some("resumed"))
    );
    assert!(events.iter().any(|event| {
        event.kind == ChatTurnEventKind::RoundFinished
            && event.metadata["status"] == Value::String("completed".to_owned())
    }));
}

#[test]
fn chat_turn_state_applies_tool_results_for_resume() {
    let state = chat_turn_initial_state(&ChatTurnRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        turn_id: Some("turn_1".to_owned()),
        surface: Some("agent_tui".to_owned()),
        mode: Some("natural_language".to_owned()),
        session_id: None,
        thread_id: None,
        agent_id: Some("chat".to_owned()),
        provider: "mock".to_owned(),
        model: "mock-model".to_owned(),
        messages: vec![user_message("use a tool")],
        temperature: None,
        max_output_tokens: None,
        tools: vec![],
        metadata: json!({}),
        context_policy: Default::default(),
        max_tool_rounds: 4,
        tool_execution: ChatToolExecution::Runtime,
    })
    .expect("initial state");
    let response = LlmResponse {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        provider: "mock".to_owned(),
        model: "mock-model".to_owned(),
        content: String::new(),
        finish_reason: LlmFinishReason::ToolCall,
        object: None,
        usage: None,
        metadata: json!({}),
    };
    let advance = chat_turn_apply_response(
        state,
        "",
        vec![ChatToolCall {
            id: "call_1".to_owned(),
            name: "echo".to_owned(),
            input: json!({"value": "ok"}),
        }],
        &response,
    )
    .expect("requires tools");
    let pending = match advance {
        ChatTurnAdvance::RequiresToolResults { state, tool_calls } => {
            assert_eq!(tool_calls[0].id, "call_1");
            state
        }
        ChatTurnAdvance::Completed { .. } => panic!("expected tool results"),
    };

    let resumed = chat_turn_apply_tool_results(
        pending,
        vec![ChatToolResult {
            tool_call_id: "call_1".to_owned(),
            tool_name: "echo".to_owned(),
            output: json!({"value": "ok"}),
            is_error: false,
            outcome: None,
        }],
    )
    .expect("resume applies");

    assert_eq!(resumed.round, 1);
    assert!(resumed.pending_tool_calls.is_empty());
    assert_eq!(resumed.messages.len(), 3);
    assert_eq!(resumed.messages[1].role, LlmRole::Assistant);
    assert_eq!(resumed.messages[2].role, LlmRole::User);
    assert_eq!(
        resumed.messages[2].content[0]["tool_use_id"],
        Value::String("call_1".to_owned())
    );
}

#[test]
fn chat_turn_prepare_llm_request_compacts_over_budget_context() {
    let mut messages = vec![LlmMessage {
        role: LlmRole::System,
        content: Value::String("system instructions stay pinned".to_owned()),
        name: None,
        metadata: json!({}),
    }];
    for index in 0..8 {
        messages.push(user_message(format!(
            "older message {index} with enough text to exceed the tiny context budget"
        )));
    }
    let mut state = chat_turn_initial_state(&ChatTurnRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        turn_id: Some("turn_context".to_owned()),
        surface: None,
        mode: None,
        session_id: None,
        thread_id: None,
        agent_id: Some("chat".to_owned()),
        provider: "mock".to_owned(),
        model: "mock-model".to_owned(),
        messages,
        temperature: None,
        max_output_tokens: None,
        tools: vec![],
        metadata: json!({}),
        context_policy: ContextPolicy {
            max_input_tokens: 24,
            reserve_output_tokens: 0,
            preserve_recent_messages: 2,
            compact_when_over_budget: true,
        },
        max_tool_rounds: 4,
        tool_execution: ChatToolExecution::Runtime,
    })
    .expect("initial state");

    let request = chat_turn_prepare_llm_request(&mut state).expect("context prepares");

    assert!(
        state
            .context_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.compacted)
    );
    assert!(state.compaction.is_some());
    assert_eq!(request.messages[0].role, LlmRole::System);
    assert_eq!(
        request.messages[1].name.as_deref(),
        Some("context_compaction")
    );
    assert_eq!(request.messages.len(), 4);
    assert_eq!(
        request.metadata["context_snapshot"]["compacted"],
        Value::Bool(true)
    );
}
