use super::*;

#[test]
fn llm_request_requires_explicit_protocol_version() {
    let error = serde_json::from_value::<LlmRequest>(json!({
        "provider": "mock",
        "model": "mock-model",
        "messages": []
    }))
    .expect_err("missing protocol version is rejected");
    assert!(error.to_string().contains("protocol_version"));
}

#[tokio::test]
async fn mock_provider_completes_and_streams() {
    let provider = MockLlmProvider::new("mock", "mock-fast", "hello");
    let request = LlmRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        provider: "mock".to_owned(),
        model: "mock-fast".to_owned(),
        messages: vec![user_message("ping")],
        temperature: None,
        max_output_tokens: Some(16),
        tools: vec![],
        response_format: None,
        metadata: json!({}),
    };

    let response = provider
        .complete(request.clone())
        .await
        .expect("mock completes");
    assert_eq!(response.content, "hello");
    assert_eq!(response.finish_reason, LlmFinishReason::Stop);

    let events = provider
        .stream(request)
        .await
        .expect("mock streams")
        .collect::<Vec<_>>()
        .await;
    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[2].as_ref().expect("event ok").kind,
        LlmEventKind::Finished
    ));
}

#[tokio::test]
async fn mock_provider_validates_structured_json_schema_output() {
    let provider = MockLlmProvider::new("mock", "mock-fast", "{}");
    let response = provider
        .complete(LlmRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "mock".to_owned(),
            model: "mock-fast".to_owned(),
            messages: vec![user_message("summary")],
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
            metadata: json!({"mock_response": "{\"title\":\"Runtime\"}"}),
        })
        .await
        .expect("mock validates structured output");

    assert_eq!(response.object, Some(json!({"title": "Runtime"})));

    let error = provider
        .complete(LlmRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "mock".to_owned(),
            model: "mock-fast".to_owned(),
            messages: vec![user_message("summary")],
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
            metadata: json!({"mock_response": "{\"status\":\"missing title\"}"}),
        })
        .await
        .expect_err("schema mismatch fails");

    assert_eq!(
        error.record.code,
        "structured_output_schema_validation_failed"
    );
}
