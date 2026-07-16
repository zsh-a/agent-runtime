use super::*;

#[tokio::test]
async fn openai_compatible_provider_completes_against_chat_api() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener binds");
    let addr = listener.local_addr().expect("local addr");
    let app = Router::new().route(
        "/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["model"], "gpt-test");
            assert_eq!(body["messages"][0]["role"], "user");
            Json(json!({
                "choices": [{
                    "message": {"content": "provider answer"},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 3,
                    "completion_tokens": 2,
                    "total_tokens": 5
                }
            }))
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("test server runs");
    });

    let provider =
        OpenAiCompatibleProvider::new("openai-compatible", format!("http://{addr}"), "test-key")
            .expect("provider builds");
    let response = provider
        .complete(LlmRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "openai-compatible".to_owned(),
            model: "gpt-test".to_owned(),
            messages: vec![user_message("ping")],
            temperature: Some(0.2),
            max_output_tokens: Some(32),
            tools: vec![],
            response_format: None,
            metadata: json!({}),
        })
        .await
        .expect("provider completes");

    assert_eq!(response.provider, "openai-compatible");
    assert_eq!(response.model, "gpt-test");
    assert_eq!(response.content, "provider answer");
    assert_eq!(response.finish_reason, LlmFinishReason::Stop);
    assert_eq!(
        response.usage.expect("usage").total_tokens,
        5,
        "usage maps from provider response"
    );
}

#[tokio::test]
async fn openai_compatible_provider_sends_json_schema_response_format() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener binds");
    let addr = listener.local_addr().expect("local addr");
    let app = Router::new().route(
        "/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["response_format"]["type"], "json_schema");
            assert_eq!(
                body["response_format"]["json_schema"]["name"],
                "project_summary"
            );
            assert_eq!(
                body["response_format"]["json_schema"]["schema"]["required"][0],
                "title"
            );
            assert_eq!(body["response_format"]["json_schema"]["strict"], true);
            Json(json!({
                "choices": [{
                    "message": {"content": "{\"title\":\"Runtime\",\"status\":\"ok\"}"},
                    "finish_reason": "stop"
                }]
            }))
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("test server runs");
    });

    let provider =
        OpenAiCompatibleProvider::new("openai-compatible", format!("http://{addr}"), "test-key")
            .expect("provider builds");
    let response = provider
        .complete(LlmRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "openai-compatible".to_owned(),
            model: "gpt-test".to_owned(),
            messages: vec![user_message("summarize")],
            temperature: None,
            max_output_tokens: Some(64),
            tools: vec![],
            response_format: Some(LlmResponseFormat::JsonSchema {
                name: "project_summary".to_owned(),
                schema: json!({
                    "type": "object",
                    "required": ["title", "status"],
                    "properties": {
                        "title": {"type": "string"},
                        "status": {"type": "string"}
                    },
                    "additionalProperties": false
                }),
                strict: Some(true),
            }),
            metadata: json!({}),
        })
        .await
        .expect("provider completes");

    assert_eq!(
        response.object,
        Some(json!({"title": "Runtime", "status": "ok"}))
    );
}

#[tokio::test]
async fn openai_compatible_provider_streams_sse_text_and_usage() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener binds");
    let addr = listener.local_addr().expect("local addr");
    let app = Router::new().route(
        "/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["model"], "gpt-stream-test");
            assert_eq!(body["stream"], true);
            assert_eq!(body["stream_options"]["include_usage"], true);
            (
                [("content-type", "text/event-stream")],
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"total_tokens\":6}}\n\n",
                    "data: [DONE]\n\n"
                ),
            )
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("test server runs");
    });

    let provider =
        OpenAiCompatibleProvider::new("openai-compatible", format!("http://{addr}"), "test-key")
            .expect("provider builds");
    let events = provider
        .stream(LlmRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "openai-compatible".to_owned(),
            model: "gpt-stream-test".to_owned(),
            messages: vec![user_message("ping")],
            temperature: None,
            max_output_tokens: Some(32),
            tools: vec![],
            response_format: None,
            metadata: json!({}),
        })
        .await
        .expect("provider streams")
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("stream events ok");

    assert!(matches!(events[0].kind, LlmEventKind::Started));
    assert_eq!(events[1].content.as_deref(), Some("hel"));
    assert_eq!(events[2].content.as_deref(), Some("lo"));
    let finished = events.last().expect("finished event");
    assert!(matches!(finished.kind, LlmEventKind::Finished));
    let response = finished.response.as_ref().expect("response");
    assert_eq!(response.content, "hello");
    assert_eq!(response.finish_reason, LlmFinishReason::Stop);
    assert_eq!(response.usage.as_ref().expect("usage").total_tokens, 6);
    assert_eq!(response.metadata["stream"], true);
}

#[tokio::test]
async fn openai_compatible_provider_streams_reasoning_and_tool_calls() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener binds");
    let addr = listener.local_addr().expect("local addr");
    let app = Router::new().route(
        "/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["stream"], true);
            (
                [("content-type", "text/event-stream")],
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read_task\",\"arguments\":\"{\\\"\"}}]},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"id\\\":\\\"task_1\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                    "data: [DONE]\n\n"
                ),
            )
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("test server runs");
    });

    let provider =
        OpenAiCompatibleProvider::new("openai-compatible", format!("http://{addr}"), "test-key")
            .expect("provider builds");
    let events = provider
        .stream(LlmRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "openai-compatible".to_owned(),
            model: "gpt-stream-test".to_owned(),
            messages: vec![user_message("ping")],
            temperature: None,
            max_output_tokens: Some(32),
            tools: vec![],
            response_format: None,
            metadata: json!({}),
        })
        .await
        .expect("provider streams")
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("stream events ok");

    assert!(matches!(events[1].kind, LlmEventKind::ThinkingDelta));
    assert_eq!(events[1].content.as_deref(), Some("think"));
    assert!(matches!(events[2].kind, LlmEventKind::ToolCallStart));
    assert_eq!(events[2].tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(events[2].tool_name.as_deref(), Some("read_task"));
    assert_eq!(events[3].partial_input_json.as_deref(), Some("{\""));
    assert_eq!(
        events[4].partial_input_json.as_deref(),
        Some("id\":\"task_1\"}")
    );
    assert!(matches!(events[5].kind, LlmEventKind::ToolCallEnd));
    assert_eq!(events[5].tool_input, Some(json!({"id": "task_1"})));
    let response = events
        .last()
        .and_then(|event| event.response.as_ref())
        .unwrap();
    assert_eq!(response.finish_reason, LlmFinishReason::ToolCall);
}

#[tokio::test]
async fn openai_compatible_provider_sends_tools_and_tool_results() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener binds");
    let addr = listener.local_addr().expect("local addr");
    let app = Router::new().route(
        "/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["tools"][0]["type"], "function");
            assert_eq!(body["tools"][0]["function"]["name"], "read_task");
            assert_eq!(
                body["tools"][0]["function"]["parameters"]["required"][0],
                "id"
            );
            assert_eq!(body["messages"][1]["role"], "assistant");
            assert_eq!(body["messages"][1]["content"], Value::Null);
            assert_eq!(body["messages"][1]["tool_calls"][0]["id"], "call_1");
            assert_eq!(
                body["messages"][1]["tool_calls"][0]["function"]["arguments"],
                "{\"id\":\"task_1\"}"
            );
            assert_eq!(body["messages"][2]["role"], "tool");
            assert_eq!(body["messages"][2]["tool_call_id"], "call_1");
            assert_eq!(body["messages"][2]["content"], "{\"title\":\"Task\"}");
            Json(json!({
                "choices": [{
                    "message": {"content": "done"},
                    "finish_reason": "stop"
                }]
            }))
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("test server runs");
    });

    let provider =
        OpenAiCompatibleProvider::new("openai-compatible", format!("http://{addr}"), "test-key")
            .expect("provider builds");
    let response = provider
        .complete(LlmRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "openai-compatible".to_owned(),
            model: "gpt-tool-test".to_owned(),
            messages: vec![
                user_message("read task"),
                LlmMessage {
                    role: LlmRole::Assistant,
                    content: json!([{
                        "type": "tool_use",
                        "id": "call_1",
                        "name": "read_task",
                        "input": {"id": "task_1"}
                    }]),
                    name: None,
                    metadata: json!({}),
                },
                LlmMessage {
                    role: LlmRole::User,
                    content: json!([{
                        "type": "tool_result",
                        "tool_use_id": "call_1",
                        "content": {"title": "Task"}
                    }]),
                    name: None,
                    metadata: json!({}),
                },
            ],
            temperature: None,
            max_output_tokens: Some(32),
            tools: vec![ToolSpec {
                name: "read_task".to_owned(),
                description: "Read a task".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {"id": {"type": "string"}},
                    "required": ["id"]
                }),
                output_schema: None,
                risk: agent_core::ToolRisk::ReadOnly,
                metadata: json!({}),
            }],
            response_format: None,
            metadata: json!({}),
        })
        .await
        .expect("provider completes");

    assert_eq!(response.content, "done");
}
