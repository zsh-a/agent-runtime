use super::*;

#[tokio::test]
async fn mock_chat_turn_streams_text_and_done() {
    let runner = ChatTurnRunner::new(
        Arc::new(MockLlmProvider::new("mock", "mock-model", "hello")),
        Arc::new(TestServices),
    );
    let events = runner
        .stream(ChatTurnRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            turn_id: Some("turn_1".to_owned()),
            surface: None,
            mode: None,
            session_id: None,
            thread_id: None,
            agent_id: Some("chat".to_owned()),
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            messages: vec![user_message("ping")],
            temperature: None,
            max_output_tokens: None,
            tools: vec![],
            metadata: json!({}),
            context_policy: Default::default(),
            max_tool_rounds: 4,
            tool_execution: ChatToolExecution::Runtime,
        })
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("events ok");

    assert!(
        events
            .iter()
            .any(|event| event.kind == ChatTurnEventKind::Delta)
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == ChatTurnEventKind::Done)
    );
    let context_index = events
        .iter()
        .position(|event| event.kind == ChatTurnEventKind::ContextSnapshot)
        .expect("context snapshot event");
    let llm_index = events
        .iter()
        .position(|event| event.kind == ChatTurnEventKind::LlmStarted)
        .expect("llm started event");
    assert!(context_index < llm_index);
    assert!(
        events[context_index]
            .metadata
            .get("context_snapshot")
            .is_some_and(Value::is_object)
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == ChatTurnEventKind::RoundFinished)
            .count(),
        1
    );
}

#[tokio::test]
async fn chat_turn_executes_tools_and_continues() {
    let provider = Arc::new(ScriptedToolProvider {
        calls: AtomicUsize::new(0),
    });
    let runner = ChatTurnRunner::new(provider, Arc::new(TestServices));
    let events = runner
        .stream(ChatTurnRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            turn_id: None,
            surface: None,
            mode: None,
            session_id: None,
            thread_id: None,
            agent_id: Some("chat".to_owned()),
            provider: "scripted".to_owned(),
            model: "scripted-model".to_owned(),
            messages: vec![user_message("use a tool")],
            temperature: None,
            max_output_tokens: None,
            tools: vec![],
            metadata: json!({}),
            context_policy: Default::default(),
            max_tool_rounds: 4,
            tool_execution: ChatToolExecution::Runtime,
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
            .any(|event| event.content.as_deref() == Some("done"))
    );
}

#[tokio::test]
async fn chat_turn_forwards_turn_metadata_to_llm_request() {
    let provider = Arc::new(MetadataProvider {
        metadata: Mutex::new(None),
    });
    let runner = ChatTurnRunner::new(provider.clone(), Arc::new(TestServices));
    runner
        .stream(ChatTurnRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            turn_id: Some("turn_1".to_owned()),
            surface: Some("agent_tui".to_owned()),
            mode: Some("natural_language".to_owned()),
            session_id: Some("session_1".to_owned()),
            thread_id: Some("thread_1".to_owned()),
            agent_id: Some("chat".to_owned()),
            provider: "metadata".to_owned(),
            model: "metadata-model".to_owned(),
            messages: vec![user_message("ping")],
            temperature: None,
            max_output_tokens: None,
            tools: vec![],
            metadata: json!({"source": "test"}),
            context_policy: Default::default(),
            max_tool_rounds: 4,
            tool_execution: ChatToolExecution::Runtime,
        })
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("events ok");

    let metadata = provider
        .metadata
        .lock()
        .expect("metadata lock")
        .clone()
        .expect("metadata captured");
    assert_eq!(metadata["source"], "test");
    assert_eq!(metadata["chat_turn"], true);
    assert_eq!(metadata["turn_id"], "turn_1");
    assert_eq!(metadata["surface"], "agent_tui");
    assert_eq!(metadata["mode"], "natural_language");
    assert_eq!(metadata["session_id"], "session_1");
    assert_eq!(metadata["thread_id"], "thread_1");
    assert_eq!(metadata["agent_id"], "chat");
}

#[tokio::test]
async fn chat_turn_cancellation_stops_pending_stream_start() {
    let runner = ChatTurnRunner::new(Arc::new(PendingStreamProvider), Arc::new(TestServices));
    let cancellation = CancellationToken::new();
    let mut stream = runner.stream_with_cancellation(
        ChatTurnRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            turn_id: Some("turn_cancel".to_owned()),
            surface: Some("agent_tui".to_owned()),
            mode: Some("natural_language".to_owned()),
            session_id: None,
            thread_id: None,
            agent_id: Some("chat".to_owned()),
            provider: "pending".to_owned(),
            model: "pending-model".to_owned(),
            messages: vec![user_message("wait")],
            temperature: None,
            max_output_tokens: None,
            tools: vec![],
            metadata: json!({}),
            context_policy: Default::default(),
            max_tool_rounds: 4,
            tool_execution: ChatToolExecution::Runtime,
        },
        cancellation.clone(),
    );

    let started = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("started event arrives")
        .expect("stream still open")
        .expect("started event ok");
    assert_eq!(started.kind, ChatTurnEventKind::Started);

    cancellation.cancel();
    let mut next = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("post-cancel event arrives")
        .expect("stream still open")
        .expect("post-cancel event ok");
    if next.kind == ChatTurnEventKind::ContextSnapshot {
        next = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("cancelled event arrives")
            .expect("stream still open")
            .expect("cancelled event ok");
    }
    let error = next;
    assert_eq!(error.kind, ChatTurnEventKind::Error);
    assert_eq!(error.metadata["code"], "cancelled");
    let done = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("done event arrives")
        .expect("stream still open")
        .expect("done event ok");
    assert_eq!(done.kind, ChatTurnEventKind::Done);
    assert_eq!(done.metadata["stop_reason"], "cancelled");
}
