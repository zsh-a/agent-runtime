use crate::anthropic::*;
use crate::openai::*;
use crate::*;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredMode {
    Native,
    JsonObject,
    Prompt,
}
impl StructuredMode {
    pub fn downgrade(self, protocol: Protocol) -> Option<Self> {
        match (self, protocol) {
            (Self::Native, Protocol::OpenAiCompatible | Protocol::OpenAiResponses) => {
                Some(Self::JsonObject)
            }
            (Self::Native, _) | (Self::JsonObject, _) => Some(Self::Prompt),
            (Self::Prompt, _) => None,
        }
    }
}

/// Encode only: provider I/O and retry policy are supplied by the host.
pub fn encode(
    ep: &Endpoint,
    request: &LlmRequest,
    stream: bool,
    mode: StructuredMode,
) -> Result<HttpRequest, LlmError> {
    request.validate_protocol()?;
    if request.messages.is_empty() || request.model.trim().is_empty() {
        return Err(LlmError::validation("LLM model and messages are required"));
    }
    for message in &request.messages {
        if let Some(blocks) = message.content.blocks() {
            for block in blocks {
                block.validate()?;
            }
        }
    }
    if let Some(LlmResponseFormat::JsonSchema { schema, .. }) = &request.response_format {
        jsonschema::validator_for(schema)
            .map_err(|_| LlmError::validation("Invalid response JSON schema"))?;
    }
    let structured = request.response_format.is_some();
    let mut headers = vec![("content-type".into(), "application/json".into())];
    let (path, mut payload) = match ep.protocol {
        Protocol::Anthropic => {
            headers.push(("x-api-key".into(), ep.api_key().expose().into()));
            headers.push(("anthropic-version".into(), ep.anthropic_version.clone()));
            let (mut system, messages) = anthropic_messages_from_llm(&request.messages)?;
            if messages.is_empty() {
                return Err(LlmError::validation(
                    "Anthropic requires a user or assistant message",
                ));
            }
            if structured
                && (mode != StructuredMode::Native
                    || matches!(request.response_format, Some(LlmResponseFormat::JsonObject)))
            {
                system = append_structured_instruction(system, &request.response_format);
            }
            let mut body = json!({"model":request.model,"max_tokens":request.max_output_tokens.unwrap_or(1024),"messages":messages,"stream":stream});
            if let Some(system) = system {
                body["system"] = json!(system);
            }
            if !request.tools.is_empty() {
                body["tools"] = serde_json::to_value(
                    request
                        .tools
                        .iter()
                        .map(anthropic_tool_from_spec)
                        .collect::<Vec<_>>(),
                )
                .expect("tool serialization");
            }
            if let Some(temperature) = request.temperature
                && !modern_claude(&request.model)
            {
                body["temperature"] = json!(temperature);
            }
            if structured
                && mode == StructuredMode::Native
                && matches!(
                    request.response_format,
                    Some(LlmResponseFormat::JsonSchema { .. })
                )
            {
                let schema = match request.response_format.as_ref().expect("structured format") {
                    LlmResponseFormat::JsonSchema { schema, .. } => schema.clone(),
                    LlmResponseFormat::JsonObject => {
                        json!({"type":"object","additionalProperties":true})
                    }
                };
                body["output_config"]["format"] =
                    json!({"type":"json_schema","schema":anthropic_schema(&schema)});
            }
            if let Some(effort) = request.effort
                && claude_effort(&request.model)
            {
                body["output_config"]["effort"] = json!(effort);
            }
            ("/messages", body)
        }
        Protocol::OpenAiCompatible => {
            if !ep.api_key().expose().is_empty() {
                headers.push((
                    "authorization".into(),
                    format!("Bearer {}", ep.api_key().expose()),
                ));
            }
            let mut messages = request
                .messages
                .iter()
                .map(openai_messages_from_llm)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            if structured
                && mode != StructuredMode::Native
                && let Some(instruction) = structured_output_instruction(&request.response_format)
            {
                messages.insert(0, openai_plain_message("system", json!(instruction), None));
            }
            let mut body = json!({"model":request.model,"messages":messages,"stream":stream});
            if let Some(temperature) = request.temperature
                && !openai_reasoning(&request.model)
                && !modern_claude(&request.model)
            {
                body["temperature"] = json!(temperature);
            }
            if let Some(tokens) = request.max_output_tokens {
                let field = if openai_reasoning(&request.model) {
                    "max_completion_tokens"
                } else {
                    "max_tokens"
                };
                body[field] = json!(tokens);
            }
            if !request.tools.is_empty() {
                body["tools"] = serde_json::to_value(
                    request
                        .tools
                        .iter()
                        .map(openai_tool_from_spec)
                        .collect::<Vec<_>>(),
                )
                .expect("tool serialization");
            }
            if stream {
                body["stream_options"] = json!({"include_usage":true});
            }
            if structured && mode != StructuredMode::Prompt {
                body["response_format"] = if mode == StructuredMode::JsonObject {
                    json!({"type":"json_object"})
                } else {
                    serde_json::to_value(openai_response_format(request.response_format.as_ref()))
                        .expect("format serialization")
                };
            }
            if let Some(effort) = request.effort
                && openai_reasoning(&request.model)
            {
                body["reasoning_effort"] = json!(match effort {
                    ReasoningEffort::Max => "high",
                    ReasoningEffort::High => "high",
                    ReasoningEffort::Medium => "medium",
                    ReasoningEffort::Low => "low",
                });
            }
            ("/chat/completions", body)
        }
        Protocol::OpenAiResponses => {
            if !ep.api_key().expose().is_empty() {
                headers.push((
                    "authorization".into(),
                    format!("Bearer {}", ep.api_key().expose()),
                ));
            }
            (
                "/responses",
                crate::responses::encode(request, stream, mode)?,
            )
        }
        Protocol::Ollama => {
            if !request.tools.is_empty() {
                return Err(LlmError::validation(
                    "Ollama tool calls are not implemented",
                ));
            }
            let messages=request.messages.iter().map(|m|Ok(json!({"role":m.role,"content":crate::llm_content_as_text(&m.content,"Ollama")?}))).collect::<Result<Vec<_>,LlmError>>()?;
            let mut body = json!({"model":request.model,"messages":messages,"stream":false});
            if let Some(t) = request.temperature {
                body["options"]["temperature"] = json!(t);
            }
            if let Some(t) = request.max_output_tokens {
                body["options"]["num_predict"] = json!(t);
            }
            if let Some(format) = &request.response_format {
                body["format"] = match format {
                    LlmResponseFormat::JsonObject => json!("json"),
                    LlmResponseFormat::JsonSchema { schema, .. } => schema.clone(),
                };
            }
            ("/api/chat", body)
        }
    };
    // No request bodies, photos, credentials, or provider output are logged.
    let body = serde_json::to_vec(&payload)
        .map_err(|_| LlmError::validation("Cannot serialize LLM request"))?;
    payload.take();
    Ok(HttpRequest {
        url: format!("{}{path}", ep.base_url),
        headers,
        body,
    })
}

// Source: https://platform.claude.com/docs/en/build-with-claude/thinking
// Sampling remains available for older Claude and unknown compatible models.
fn modern_claude(model: &str) -> bool {
    [
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-haiku-5-5",
        "claude-fable-5",
        "claude-mythos-5",
        "claude-mythos-preview",
    ]
    .iter()
    .any(|prefix| model.starts_with(prefix))
}
fn claude_effort(model: &str) -> bool {
    modern_claude(model)
        || model.starts_with("claude-opus-4-6")
        || model.starts_with("claude-sonnet-4-6")
}
fn openai_reasoning(model: &str) -> bool {
    ["o1", "o3", "o4", "gpt-5", "gpt-6"]
        .iter()
        .any(|prefix| model.starts_with(prefix))
}

pub fn decode(
    ep: &Endpoint,
    request: &LlmRequest,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<LlmResponse, LlmError> {
    if !(200..300).contains(&status) {
        return Err(http_error(status, headers, body));
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| {
        LlmError::provider(
            "provider_decode_failed",
            "Provider response was not valid JSON",
            false,
            json!({}),
        )
    })?;
    if ep.protocol == Protocol::OpenAiResponses {
        return crate::responses::decode(ep, request, value);
    }
    let (content, finish_reason, usage, metadata, model) = match ep.protocol {
        Protocol::Anthropic => {
            let blocks = value
                .get("content")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    LlmError::provider(
                        "provider_decode_failed",
                        "Provider response has no content",
                        false,
                        json!({}),
                    )
                })?;
            let content = anthropic_text_from_blocks(blocks);
            let reason = anthropic_finish_reason(value.get("stop_reason").and_then(Value::as_str));
            let input = value
                .pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let output = value
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            (
                content,
                reason,
                Some(LlmUsage {
                    input_tokens: input,
                    output_tokens: output,
                    total_tokens: input.saturating_add(output),
                }),
                json!({"api":"anthropic_messages","anthropic_version":ep.anthropic_version,"anthropic_content":blocks,"refusal":value["stop_reason"]=="refusal"}),
                value["model"].as_str().unwrap_or(&request.model).to_owned(),
            )
        }
        Protocol::OpenAiCompatible => {
            let choice = value.pointer("/choices/0").ok_or_else(|| {
                LlmError::provider(
                    "provider_missing_choice",
                    "Provider response has no choice",
                    false,
                    json!({}),
                )
            })?;
            let message = &choice["message"];
            let content = match &message["content"] {
                Value::String(s) => s.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join(""),
                _ => String::new(),
            };
            // Reasoning is never substituted for the answer or JSON payload.
            let refusal = message["refusal"].as_str().is_some_and(|s| !s.is_empty());
            let reason = if refusal {
                LlmFinishReason::ContentFilter
            } else {
                openai_finish_reason(choice["finish_reason"].as_str())
            };
            let usage = value.get("usage").filter(|u| u.is_object()).map(|u| {
                let i = u["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                let o = u["completion_tokens"].as_u64().unwrap_or(0) as u32;
                LlmUsage {
                    input_tokens: i,
                    output_tokens: o,
                    total_tokens: u["total_tokens"]
                        .as_u64()
                        .map(|x| x as u32)
                        .unwrap_or(i.saturating_add(o)),
                }
            });
            (
                content,
                reason,
                usage,
                json!({"api":"openai_chat_completions","anthropic_content":openai_content_blocks(message)?,"refusal":refusal}),
                value["model"].as_str().unwrap_or(&request.model).to_owned(),
            )
        }
        Protocol::OpenAiResponses => unreachable!("Responses decoded above"),
        Protocol::Ollama => {
            let content = value
                .pointer("/message/content")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    LlmError::provider(
                        "provider_decode_failed",
                        "Ollama response has no content",
                        false,
                        json!({}),
                    )
                })?
                .to_owned();
            let reason = if value["done_reason"] == "length" {
                LlmFinishReason::Length
            } else {
                LlmFinishReason::Stop
            };
            let i = value["prompt_eval_count"].as_u64().unwrap_or(0) as u32;
            let o = value["eval_count"].as_u64().unwrap_or(0) as u32;
            (
                content,
                reason,
                Some(LlmUsage {
                    input_tokens: i,
                    output_tokens: o,
                    total_tokens: i.saturating_add(o),
                }),
                json!({"api":"ollama_chat"}),
                request.model.clone(),
            )
        }
    };
    let object = if matches!(finish_reason, LlmFinishReason::Stop) {
        structured_output_from_content(&request.response_format, &content)?
    } else {
        None
    };
    Ok(LlmResponse {
        protocol_version: agent_core::PROTOCOL_VERSION.into(),
        provider: ep.provider.clone(),
        model,
        content,
        finish_reason,
        object,
        usage,
        metadata,
    })
}

pub fn http_error(status: u16, headers: &[(String, String)], body: &[u8]) -> LlmError {
    // Inspect upstream message only to classify it. Never expose untrusted body:
    // compatible services may echo API keys, prompts, or image data in errors.
    let value = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let text = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let (code, message, retryable) = match status {
        401 | 403 => ("unauthorized", "Provider rejected the credential", false),
        404 => (
            "not_found",
            "Provider endpoint or model was not found",
            false,
        ),
        429 => ("rate_limited", "Provider rate limit reached", true),
        529 => ("overloaded", "Provider is overloaded", true),
        400 | 413 | 422 if text.contains("context") || text.contains("token limit") => {
            ("context_overflow", "Provider context limit exceeded", false)
        }
        408 => ("provider_timeout", "Provider request timed out", true),
        500..=599 => ("provider_server_error", "Provider request failed", true),
        _ => ("provider_rejected", "Provider rejected the request", false),
    };
    let mut details = json!({"status":status});
    if let Some((_, value)) = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
    {
        details["retry_after"] = json!(value);
    }
    let mut error = LlmError::provider(code, message, retryable, details);
    if status == 429 {
        error.record.kind = LlmErrorKind::RateLimited;
    }
    if status == 408 {
        error.record.kind = LlmErrorKind::Timeout;
    }
    error
}

/// Downgrade only explicit protocol-option rejections, never malformed schema,
/// image errors, authentication failures, context overflows, or server errors.
pub fn rejects_structured_output(status: u16, body: &[u8]) -> bool {
    if !matches!(status, 400 | 404 | 422) {
        return false;
    }
    let value = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let text = value
        .pointer("/error/message")
        .or(value.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    [
        "response_format",
        "text.format",
        "json_schema",
        "json_object",
        "output_config",
        "output_format",
    ]
    .iter()
    .any(|key| text.contains(key))
        && [
            "not supported",
            "unsupported",
            "unknown field",
            "unknown parameter",
            "unrecognized",
            "not available",
        ]
        .iter()
        .any(|word| text.contains(word))
}

// Match the official SDK transformation, retaining the original for local
// validation: https://platform.claude.com/docs/en/build-with-claude/structured-outputs
fn anthropic_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(original) => {
            let mut object = original.clone();
            let mut hints = Vec::new();
            for key in [
                "minimum",
                "maximum",
                "exclusiveMinimum",
                "exclusiveMaximum",
                "multipleOf",
                "minLength",
                "maxLength",
                "maxItems",
                "uniqueItems",
                "minContains",
                "maxContains",
            ] {
                if let Some(value) = object.remove(key) {
                    hints.push(format!("{key}: {value}"));
                }
            }
            if object
                .get("minItems")
                .and_then(Value::as_u64)
                .is_some_and(|v| v > 1)
                && let Some(value) = object.remove("minItems")
            {
                hints.push(format!("minItems: {value}"));
            }
            if !hints.is_empty() {
                let description = object
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                object.insert(
                    "description".into(),
                    json!(format!("{description} Constraints: {}", hints.join(", "))),
                );
            }
            if object.get("type").and_then(Value::as_str) == Some("object") {
                object.insert("additionalProperties".into(), json!(false));
            }
            for (key, value) in &mut object {
                if key == "properties" || key == "$defs" || key == "definitions" {
                    if let Value::Object(entries) = value {
                        for entry in entries.values_mut() {
                            *entry = anthropic_schema(entry);
                        }
                    }
                } else if ["items", "anyOf", "oneOf", "allOf"].contains(&key.as_str()) {
                    *value = anthropic_schema(value);
                }
            }
            Value::Object(object)
        }
        Value::Array(items) => Value::Array(items.iter().map(anthropic_schema).collect()),
        other => other.clone(),
    }
}

fn openai_content_blocks(message: &Value) -> Result<Vec<Value>, LlmError> {
    let mut blocks = Vec::new();
    if let Some(text) = message["content"].as_str().filter(|s| !s.is_empty()) {
        blocks.push(json!({"type":"text","text":text}));
    }
    if let Some(calls) = message["tool_calls"].as_array() {
        for call in calls {
            let id = call["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    LlmError::provider(
                        "provider_decode_failed",
                        "Provider tool call omitted its id",
                        false,
                        json!({}),
                    )
                })?;
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    LlmError::provider(
                        "provider_decode_failed",
                        "Provider tool call omitted its name",
                        false,
                        json!({}),
                    )
                })?;
            let arguments = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let input = serde_json::from_str::<Value>(arguments).map_err(|_| {
                LlmError::provider(
                    "provider_decode_failed",
                    "Provider tool call arguments were invalid JSON",
                    false,
                    json!({}),
                )
            })?;
            blocks.push(json!({"type":"tool_use","id":id,"name":name,"input":input}));
        }
    }
    Ok(blocks)
}
