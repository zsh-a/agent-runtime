//! Stateless Responses API: native output items are retained for exact replay.
use crate::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

pub(crate) fn encode(
    request: &LlmRequest,
    stream: bool,
    mode: StructuredMode,
) -> Result<Value, LlmError> {
    if stream {
        return Err(LlmError::validation(
            "OpenAI Responses streaming is not implemented",
        ));
    }
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for message in &request.messages {
        if message.role == LlmRole::System {
            instructions.push(
                crate::llm_content_as_text(&message.content, "Responses instructions")?.to_owned(),
            );
            continue;
        }
        if message.role == LlmRole::Assistant
            && let Some(items) = message.metadata.get("responses_output")
        {
            let items = items
                .as_array()
                .ok_or_else(|| LlmError::validation("Invalid Responses replay items"))?;
            validate_items(items)?;
            input.extend(items.iter().cloned());
            continue;
        }
        let assistant = message.role == LlmRole::Assistant;
        let mut parts = Vec::new();
        let blocks = match &message.content {
            MessageContent::Text(text) => vec![ContentBlock::Text { text: text.clone() }],
            MessageContent::Blocks(blocks) => blocks.clone(),
        };
        for block in blocks {
            match block {
                ContentBlock::Text { text } => parts.push(
                    json!({"type":if assistant {"output_text"} else {"input_text"},"text":text}),
                ),
                ContentBlock::Image { source } if !assistant => {
                    let url = match source {
                        ImageSource::Base64 { media_type, data } => {
                            format!("data:{media_type};base64,{}", STANDARD.encode(data))
                        }
                        ImageSource::Url { url } => url,
                    };
                    parts.push(json!({"type":"input_image","image_url":url}));
                }
                ContentBlock::ImageUrl { image_url } if !assistant => {
                    let mut part = json!({"type":"input_image","image_url":image_url.url});
                    if let Some(detail) = image_url.detail {
                        part["detail"] = json!(detail);
                    }
                    parts.push(part);
                }
                ContentBlock::ToolUse {
                    id,
                    name,
                    input: args,
                } if assistant => {
                    flush_message(&mut input, &mut parts, assistant);
                    input.push(json!({"type":"function_call","call_id":id,"name":name,"arguments":args.to_string()}));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } if !assistant => {
                    flush_message(&mut input, &mut parts, assistant);
                    let output = content
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| content.to_string());
                    input.push(json!({"type":"function_call_output","call_id":tool_use_id,"output":output}));
                }
                _ => {
                    return Err(LlmError::validation(
                        "Unsupported content block for OpenAI Responses",
                    ));
                }
            }
        }
        flush_message(&mut input, &mut parts, assistant);
    }
    if mode != StructuredMode::Native
        && let Some(instruction) = structured_output_instruction(&request.response_format)
    {
        instructions.push(instruction);
    }
    let mut body = json!({"model":request.model,"input":input,"store":false,"stream":false,"include":["reasoning.encrypted_content"]});
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions.join("\n\n"));
    }
    if let Some(tokens) = request.max_output_tokens {
        body["max_output_tokens"] = json!(tokens);
    }
    if let Some(effort) = request.effort {
        body["reasoning"]["effort"] = json!(match effort {
            ReasoningEffort::Max | ReasoningEffort::High => "high",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::Low => "low",
        });
    }
    if !request.tools.is_empty() {
        body["tools"] = json!(request.tools.iter().map(|t| json!({"type":"function","name":t.name,"description":t.description,"parameters":t.input_schema})).collect::<Vec<_>>());
    }
    if let Some(format) = &request.response_format
        && mode != StructuredMode::Prompt
    {
        body["text"]["format"] = match (mode, format) {
            (
                StructuredMode::Native,
                LlmResponseFormat::JsonSchema {
                    name,
                    schema,
                    strict,
                },
            ) => {
                let mut format = json!({"type":"json_schema","name":name,"schema":schema});
                if let Some(strict) = strict {
                    format["strict"] = json!(strict);
                }
                format
            }
            _ => json!({"type":"json_object"}),
        };
    }
    Ok(body)
}

fn flush_message(input: &mut Vec<Value>, parts: &mut Vec<Value>, assistant: bool) {
    if !parts.is_empty() {
        input.push(json!({"role":if assistant {"assistant"} else {"user"},"content":std::mem::take(parts)}));
    }
}

fn malformed() -> LlmError {
    LlmError::provider(
        "provider_decode_failed",
        "Invalid or unsupported OpenAI Responses output",
        false,
        json!({}),
    )
}

fn required<'a>(item: &'a Value, field: &str) -> Result<&'a str, LlmError> {
    item[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(malformed)
}

fn validate_items(items: &[Value]) -> Result<(), LlmError> {
    for item in items {
        match item["type"].as_str() {
            Some("reasoning") => {}
            Some("function_call") => {
                required(item, "call_id")?;
                required(item, "name")?;
                if !item["arguments"].is_string() {
                    return Err(malformed());
                }
            }
            Some("message") => {
                if item["role"] != "assistant" {
                    return Err(malformed());
                }
                for part in item["content"].as_array().ok_or_else(malformed)? {
                    match part["type"].as_str() {
                        Some("output_text") if part["text"].is_string() => {}
                        Some("refusal") if part["refusal"].is_string() => {}
                        _ => return Err(malformed()),
                    }
                }
            }
            _ => return Err(malformed()),
        }
    }
    Ok(())
}

pub(crate) fn decode(
    ep: &Endpoint,
    request: &LlmRequest,
    value: Value,
) -> Result<LlmResponse, LlmError> {
    let status = required(&value, "status")?;
    let mut reason = match status {
        "completed" => LlmFinishReason::Stop,
        "incomplete"
            if value
                .pointer("/incomplete_details/reason")
                .and_then(Value::as_str)
                == Some("max_output_tokens") =>
        {
            LlmFinishReason::Length
        }
        "incomplete"
            if value
                .pointer("/incomplete_details/reason")
                .and_then(Value::as_str)
                == Some("content_filter") =>
        {
            LlmFinishReason::ContentFilter
        }
        "failed" | "incomplete" | "cancelled" => LlmFinishReason::Error,
        _ => return Err(malformed()),
    };
    let empty = Vec::new();
    let items = match value.get("output") {
        Some(Value::Array(items)) => items,
        None if status != "completed" => &empty,
        _ => return Err(malformed()),
    };
    validate_items(items)?;
    let mut content = String::new();
    let mut blocks = Vec::new();
    let mut refusal = false;
    for item in items {
        match item["type"].as_str() {
            Some("message") => {
                for part in item["content"].as_array().ok_or_else(malformed)? {
                    if part["type"] == "refusal" {
                        refusal = true;
                    }
                    if let Some(text) = part["text"].as_str() {
                        content.push_str(text);
                        blocks.push(json!({"type":"text","text":text}));
                    }
                }
            }
            Some("function_call") if status == "completed" => {
                if item.get("status").is_some_and(|s| s != "completed") {
                    return Err(malformed());
                }
                let args = serde_json::from_str::<Value>(
                    item["arguments"].as_str().ok_or_else(malformed)?,
                )
                .map_err(|_| malformed())?;
                if !args.is_object() {
                    return Err(malformed());
                }
                blocks.push(json!({"type":"tool_use","id":required(item,"call_id")?,"name":required(item,"name")?,"input":args}));
                reason = LlmFinishReason::ToolCall;
            }
            _ => {}
        }
    }
    if refusal {
        reason = LlmFinishReason::ContentFilter;
    }
    // Never permit hosts to execute calls attached to refusal or incomplete output.
    if reason != LlmFinishReason::ToolCall {
        blocks.retain(|b| b["type"] != "tool_use");
    }
    let object = if reason == LlmFinishReason::Stop {
        structured_output_from_content(&request.response_format, &content)?
    } else {
        None
    };
    let usage = value.get("usage").filter(|u| u.is_object()).map(|u| {
        let count = |key: &str| u[key].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
        let input_tokens = count("input_tokens");
        let output_tokens = count("output_tokens");
        LlmUsage {
            input_tokens,
            output_tokens,
            total_tokens: u["total_tokens"]
                .as_u64()
                .map(|n| n.min(u32::MAX as u64) as u32)
                .unwrap_or_else(|| input_tokens.saturating_add(output_tokens)),
        }
    });
    Ok(LlmResponse {
        protocol_version: agent_core::PROTOCOL_VERSION.into(),
        provider: ep.provider.clone(),
        model: value["model"].as_str().unwrap_or(&request.model).into(),
        content,
        finish_reason: reason,
        object,
        usage,
        metadata: json!({"api":"openai_responses","responses_output":items,"anthropic_content":blocks,"refusal":refusal}),
    })
}

#[cfg(test)]
mod tests;
