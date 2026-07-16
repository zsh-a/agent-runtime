use super::*;

#[tokio::test]
async fn ollama_provider_completes_against_chat_api() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener binds");
    let addr = listener.local_addr().expect("local addr");
    let app = Router::new().route(
        "/api/chat",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["model"], "llama-test");
            assert_eq!(body["stream"], false);
            assert_eq!(body["messages"][0]["role"], "user");
            assert_eq!(body["options"]["num_predict"], 32);
            Json(json!({
                "message": {"role": "assistant", "content": "local answer"},
                "done_reason": "stop",
                "prompt_eval_count": 6,
                "eval_count": 4
            }))
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("test server runs");
    });

    let provider =
        OllamaProvider::new("ollama", format!("http://{addr}")).expect("provider builds");
    let response = provider
        .complete(LlmRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "ollama".to_owned(),
            model: "llama-test".to_owned(),
            messages: vec![user_message("ping")],
            temperature: Some(0.3),
            max_output_tokens: Some(32),
            tools: vec![],
            response_format: None,
            metadata: json!({}),
        })
        .await
        .expect("provider completes");

    assert_eq!(response.provider, "ollama");
    assert_eq!(response.model, "llama-test");
    assert_eq!(response.content, "local answer");
    assert_eq!(response.finish_reason, LlmFinishReason::Stop);
    assert_eq!(response.usage.expect("usage").total_tokens, 10);
}

#[tokio::test]
async fn ollama_provider_sends_schema_format() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener binds");
    let addr = listener.local_addr().expect("local addr");
    let app = Router::new().route(
        "/api/chat",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["format"]["type"], "object");
            assert_eq!(body["format"]["required"][0], "title");
            Json(json!({
                "message": {"role": "assistant", "content": "{\"title\":\"Runtime\"}"},
                "done_reason": "stop"
            }))
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("test server runs");
    });

    let provider =
        OllamaProvider::new("ollama", format!("http://{addr}")).expect("provider builds");
    let response = provider
        .complete(LlmRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "ollama".to_owned(),
            model: "llama-test".to_owned(),
            messages: vec![user_message("summarize")],
            temperature: None,
            max_output_tokens: Some(64),
            tools: vec![],
            response_format: Some(LlmResponseFormat::JsonSchema {
                name: "summary".to_owned(),
                schema: json!({
                    "type": "object",
                    "required": ["title"],
                    "properties": {"title": {"type": "string"}},
                    "additionalProperties": false
                }),
                strict: Some(true),
            }),
            metadata: json!({}),
        })
        .await
        .expect("provider completes");

    assert_eq!(response.object, Some(json!({"title": "Runtime"})));
}
