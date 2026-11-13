use super::*;
use serde_json::{Value, json};
fn endpoint(protocol: Protocol) -> Endpoint {
    Endpoint::new(protocol, "test", "https://example.test/v1", "secret-key").unwrap()
}
fn request(model: &str) -> LlmRequest {
    serde_json::from_value(json!({"protocol_version":PROTOCOL_VERSION,"provider":"test","model":model,"messages":[{"role":"user","content":"hello"}],"temperature":0.2,"effort":"high"})).unwrap()
}
fn payload(protocol: Protocol, request: &LlmRequest) -> Value {
    serde_json::from_slice(
        &encode(&endpoint(protocol), request, false, StructuredMode::Native)
            .unwrap()
            .body,
    )
    .unwrap()
}
#[test]
fn modern_claude_sampling_and_effort_are_capability_aware() {
    let latest = payload(Protocol::Anthropic, &request("claude-opus-5-5"));
    assert!(latest.get("temperature").is_none());
    assert_eq!(latest["output_config"]["effort"], "high");
    let previous = payload(Protocol::Anthropic, &request("claude-sonnet-4-5-20250929"));
    assert!(previous["temperature"].as_f64().unwrap() > 0.19);
    assert!(previous.get("output_config").is_none());
}
#[test]
fn openai_reasoning_uses_completion_limit_and_uniform_effort() {
    let mut request = request("gpt-6");
    request.max_output_tokens = Some(4000);
    request.effort = Some(ReasoningEffort::Max);
    let value = payload(Protocol::OpenAiCompatible, &request);
    assert_eq!(value["max_completion_tokens"], 4000);
    assert!(value.get("temperature").is_none());
    assert_eq!(value["reasoning_effort"], "high");
}
#[test]
fn anthropic_schema_is_lowered_but_original_bounds_are_enforced() {
    let mut request = request("claude-opus-5-5");
    let schema = json!({"type":"object","required":["weight"],"additionalProperties":false,"properties":{"weight":{"type":"number","minimum":1,"maximum":5000}}});
    request.response_format = Some(LlmResponseFormat::JsonSchema {
        name: "meal".into(),
        schema: schema.clone(),
        strict: Some(true),
    });
    let value = payload(Protocol::Anthropic, &request);
    let weight = &value["output_config"]["format"]["schema"]["properties"]["weight"];
    assert!(weight.get("minimum").is_none());
    assert!(
        weight["description"]
            .as_str()
            .unwrap()
            .contains("minimum: 1")
    );
    let response =
        json!({"content":[{"type":"text","text":"{\"weight\":0}"}],"stop_reason":"end_turn"});
    assert_eq!(
        decode(
            &endpoint(Protocol::Anthropic),
            &request,
            200,
            &[],
            &serde_json::to_vec(&response).unwrap()
        )
        .unwrap_err()
        .record
        .code,
        "structured_output_schema_validation_failed"
    );
    assert_eq!(
        request.response_format,
        Some(LlmResponseFormat::JsonSchema {
            name: "meal".into(),
            schema,
            strict: Some(true)
        })
    );
}
#[test]
fn typed_image_bytes_encode_once_for_each_provider() {
    let mut request = request("vision");
    request.messages[0].content = vec![ContentBlock::image("image/jpeg", vec![1, 2, 3])].into();
    assert_eq!(
        payload(Protocol::Anthropic, &request)["messages"][0]["content"][0]["source"]["data"],
        "AQID"
    );
    assert_eq!(
        payload(Protocol::OpenAiCompatible, &request)["messages"][0]["content"][0]["image_url"]["url"],
        "data:image/jpeg;base64,AQID"
    );
}
#[test]
fn document_blocks_remain_supported_for_anthropic_finance_caller() {
    let mut request = request("vision");
    request.messages[0].content=serde_json::from_value(json!([{"type":"document","source":{"type":"base64","media_type":"application/pdf","data":"AQID"}}])).unwrap();
    assert_eq!(
        payload(Protocol::Anthropic, &request)["messages"][0]["content"][0]["type"],
        "document"
    );
    assert!(
        encode(
            &endpoint(Protocol::OpenAiCompatible),
            &request,
            false,
            StructuredMode::Native
        )
        .is_err()
    );
}
#[test]
fn unknown_or_malformed_content_never_silently_disappears() {
    assert!(
        serde_json::from_value::<MessageContent>(json!([{"type":"unknown","data":"x"}])).is_err()
    );
    assert!(serde_json::from_value::<MessageContent>(json!([{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"invalid@"}}])).is_err());
}
#[test]
fn credentials_and_requests_redact_debug_and_errors() {
    let ep = endpoint(Protocol::Anthropic);
    let req = encode(&ep, &request("model"), false, StructuredMode::Native).unwrap();
    for debug in [
        format!("{ep:?}"),
        format!("{req:?}"),
        format!("{:?}", ep.api_key()),
    ] {
        assert!(!debug.contains("secret-key"));
        assert!(!debug.contains("hello"));
    }
    let err = http_error(
        400,
        &[],
        br#"{"error":{"message":"secret-key prompt and photo"}}"#,
    );
    assert!(!format!("{err:?}").contains("secret-key"));
}
#[test]
fn endpoint_validation_rejects_missing_hosts_and_embedded_secrets() {
    for url in [
        "https://",
        "file:///tmp/model",
        "https://u:p@example.test/v1",
        "https://example.test/v1?key=secret",
        "https://example.test/#token",
    ] {
        assert!(
            Endpoint::new(Protocol::OpenAiCompatible, "x", url, "").is_err(),
            "{url}"
        );
    }
}
#[test]
fn explicit_unsupported_options_are_distinguished_from_other_errors() {
    assert!(rejects_structured_output(
        400,
        br#"{"error":{"message":"unknown field output_config"}}"#
    ));
    for (status, msg) in [
        (400, "invalid json_schema: minimum is not allowed"),
        (400, "image too large"),
        (401, "response_format not supported"),
        (500, "response_format not supported"),
    ] {
        assert!(!rejects_structured_output(
            status,
            &serde_json::to_vec(&json!({"error":{"message":msg}})).unwrap()
        ));
    }
}
#[test]
fn refusals_and_truncated_json_preserve_finish_reason() {
    let mut req = request("m");
    req.response_format = Some(LlmResponseFormat::JsonObject);
    for reason in ["refusal", "max_tokens"] {
        let res = json!({"content":[{"type":"text","text":"{"}],"stop_reason":reason});
        let response = decode(
            &endpoint(Protocol::Anthropic),
            &req,
            200,
            &[],
            &serde_json::to_vec(&res).unwrap(),
        )
        .unwrap();
        assert_ne!(response.finish_reason, LlmFinishReason::Stop);
        assert!(response.object.is_none());
    }
}
#[test]
fn streamed_utf8_can_split_at_every_byte() {
    let ep = endpoint(Protocol::OpenAiCompatible);
    let req = request("m");
    let data = "data: {\"choices\":[{\"delta\":{\"content\":\"你好🥗\"},\"finish_reason\":\"stop\"}]}\r\n\r\ndata: [DONE]\r\n\r\n";
    let mut decoder = StreamDecoder::new(&ep, &req).unwrap();
    let mut events = Vec::new();
    for byte in data.as_bytes() {
        events.extend(decoder.push(&[*byte]));
    }
    events.extend(decoder.finish());
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(
        events.last().unwrap().response.as_ref().unwrap().content,
        "你好🥗"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == LlmEventKind::Finished)
            .count(),
        1
    );
}
#[test]
fn complete_tool_calls_are_normalized_for_shared_host_adapter() {
    let req = request("m");
    let body = json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"record","arguments":"{\"value\":2}"}}]},"finish_reason":"tool_calls"}]});
    let response = decode(
        &endpoint(Protocol::OpenAiCompatible),
        &req,
        200,
        &[],
        &serde_json::to_vec(&body).unwrap(),
    )
    .unwrap();
    assert_eq!(
        response.metadata["anthropic_content"][0]["type"],
        "tool_use"
    );
    assert_eq!(
        response.metadata["anthropic_content"][0]["input"]["value"],
        2
    );
}

#[test]
fn request_debug_hides_host_credentials_prompts_and_image_bytes() {
    let mut request = request("vision");
    request.metadata = json!({"api_key":"profile-secret-key"});
    request.messages[0].content = vec![
        ContentBlock::Text {
            text: "secret-photo-caption".into(),
        },
        ContentBlock::image("image/jpeg", vec![1, 2, 3]),
    ]
    .into();
    let debug = format!("{request:?}");
    for secret in [
        "profile-secret-key",
        "secret-photo-caption",
        "AQID",
        "[1, 2, 3]",
    ] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn effort_is_sent_only_for_models_with_documented_support() {
    let mut req = request("claude-sonnet-4-6");
    req.effort = Some(ReasoningEffort::Max);
    assert_eq!(
        payload(Protocol::Anthropic, &req)["output_config"]["effort"],
        "max"
    );
    req.model = "claude-sonnet-4-5".into();
    assert!(
        payload(Protocol::Anthropic, &req)
            .get("output_config")
            .is_none()
    );
}
