use super::*;
fn endpoint() -> Endpoint {
    Endpoint::new(
        Protocol::OpenAiResponses,
        "test",
        "https://example.test/v1",
        "secret",
    )
    .unwrap()
}
fn request() -> LlmRequest {
    serde_json::from_value(json!({"protocol_version":agent_core::PROTOCOL_VERSION,"provider":"test","model":"model","messages":[{"role":"system","content":"recognize"},{"role":"user","content":[{"type":"text","text":"meal"},{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"AQID"}}]}],"max_output_tokens":2048,"effort":"high","tools":[{"name":"ask_user","description":"Clarify","input_schema":{"type":"object"},"risk":"read_only"}]})).unwrap()
}
fn native_output() -> Value {
    json!([{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque encrypted secret"},{"type":"function_call","id":"fc_item_1","call_id":"call_9","name":"ask_user","arguments":"{\"question\":\"More image?\"}","status":"completed"}])
}
fn response(req: &LlmRequest, value: Value) -> Result<LlmResponse, LlmError> {
    decode(&endpoint(), req, value)
}
#[test]
fn wire_round_trip_retains_native_reasoning_and_uses_call_id() {
    let mut req = request();
    let output = native_output();
    let decoded = response(&req,json!({"status":"completed","output":output,"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}})).unwrap();
    assert_eq!(decoded.finish_reason, LlmFinishReason::ToolCall);
    assert_eq!(decoded.metadata["anthropic_content"][0]["id"], "call_9");
    assert_eq!(decoded.metadata["responses_output"], output);
    assert_eq!(decoded.usage.unwrap().total_tokens, 15);
    req.messages.push(LlmMessage {
        role: LlmRole::Assistant,
        content: "".into(),
        name: None,
        metadata: json!({"responses_output":decoded.metadata["responses_output"]}),
    });
    req.messages.push(LlmMessage {
        role: LlmRole::User,
        content: vec![
            ContentBlock::ToolResult {
                tool_use_id: "call_9".into(),
                content: json!({"answer":"rice"}),
                is_error: None,
            },
            ContentBlock::image("image/png", vec![4, 5, 6]),
        ]
        .into(),
        name: None,
        metadata: json!({}),
    });
    let wire = crate::encode(&endpoint(), &req, false, StructuredMode::Native).unwrap();
    assert_eq!(wire.url, "https://example.test/v1/responses");
    assert!(
        wire.headers
            .contains(&("authorization".into(), "Bearer secret".into()))
    );
    let body: Value = serde_json::from_slice(&wire.body).unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert!(body.get("previous_response_id").is_none());
    assert_eq!(body["instructions"], "recognize");
    assert_eq!(body["max_output_tokens"], 2048);
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["tools"][0]["name"], "ask_user");
    assert!(body["tools"][0].get("function").is_none());
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(
        body["input"][0]["content"][1]["image_url"],
        "data:image/jpeg;base64,AQID"
    );
    assert_eq!(body["input"][1], output[0]);
    assert_eq!(body["input"][2], output[1]);
    assert_eq!(
        body["input"][3],
        json!({"type":"function_call_output","call_id":"call_9","output":"{\"answer\":\"rice\"}"})
    );
    assert_eq!(
        body["input"][4]["content"][0]["image_url"],
        "data:image/png;base64,BAUG"
    );
}
#[test]
fn schema_modes_and_streaming_contract() {
    let mut req = request();
    req.response_format = Some(LlmResponseFormat::JsonSchema {
        name: "meal".into(),
        schema: json!({"type":"object","required":["weight"],"properties":{"weight":{"type":"integer"}}}),
        strict: Some(true),
    });
    let native = encode(&req, false, StructuredMode::Native).unwrap();
    assert_eq!(native["text"]["format"]["type"], "json_schema");
    assert_eq!(native["text"]["format"]["name"], "meal");
    assert_eq!(native["text"]["format"]["strict"], true);
    assert!(native.get("response_format").is_none());
    let object = encode(&req, false, StructuredMode::JsonObject).unwrap();
    assert_eq!(object["text"]["format"], json!({"type":"json_object"}));
    let prompt = encode(&req, false, StructuredMode::Prompt).unwrap();
    assert!(prompt.get("text").is_none());
    assert!(prompt["instructions"].as_str().unwrap().contains("weight"));
    assert_eq!(
        StructuredMode::Native.downgrade(Protocol::OpenAiResponses),
        Some(StructuredMode::JsonObject)
    );
    assert_eq!(
        StructuredMode::JsonObject.downgrade(Protocol::OpenAiResponses),
        Some(StructuredMode::Prompt)
    );
    assert!(crate::rejects_structured_output(
        400,
        br#"{"error":{"message":"unknown parameter text.format"}}"#
    ));
    assert!(encode(&req, true, StructuredMode::Native).is_err());
    assert!(StreamDecoder::new(&endpoint(), &req).is_err());
    assert_eq!(
        serde_json::to_value(Protocol::OpenAiResponses).unwrap(),
        "open_ai_responses"
    );
}
#[test]
fn status_refusal_truncation_and_unknown_output_are_safe() {
    let mut req = request();
    req.response_format = Some(LlmResponseFormat::JsonObject);
    let completed = json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"{\"weight\":200}"}]}]});
    assert_eq!(
        response(&req, completed).unwrap().object,
        Some(json!({"weight":200}))
    );
    let refusal = json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"no"}]}]});
    assert_eq!(
        response(&req, refusal).unwrap().finish_reason,
        LlmFinishReason::ContentFilter
    );
    let truncated = json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"function_call","id":"fc","call_id":"call","name":"ask_user","arguments":"{"}]});
    let decoded = response(&req, truncated).unwrap();
    assert_eq!(decoded.finish_reason, LlmFinishReason::Length);
    assert!(
        decoded.metadata["anthropic_content"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let failed = response(
        &req,
        json!({"status":"failed","error":{"message":"secret prompt"},"output":[]}),
    )
    .unwrap();
    assert_eq!(failed.finish_reason, LlmFinishReason::Error);
    assert!(!format!("{:?}", failed).contains("secret prompt"));
    for output in [
        json!([{"type":"web_search_call","secret":"secret prompt"}]),
        json!([{"type":"function_call","call_id":"call","name":"ask_user","arguments":"secret prompt"}]),
        json!([{"type":"function_call","call_id":"call","name":"ask_user","arguments":"[]"}]),
    ] {
        let err = response(&req, json!({"status":"completed","output":output})).unwrap_err();
        assert!(!format!("{err:?}").contains("secret prompt"));
    }
}
