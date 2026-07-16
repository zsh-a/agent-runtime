use super::*;

pub(super) fn truncate_for_log(value: &str) -> String {
    const MAX_CHARS: usize = 500;
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

pub(super) fn openai_messages_from_llm(
    message: &LlmMessage,
) -> Result<Vec<OpenAiMessage>, LlmError> {
    match message.role {
        LlmRole::System => Ok(vec![openai_plain_message(
            "system",
            message.content.clone(),
            message.name.clone(),
        )]),
        LlmRole::User => openai_user_messages_from_llm(message),
        LlmRole::Assistant => Ok(vec![openai_assistant_message_from_llm(message)?]),
        LlmRole::Tool => Ok(vec![OpenAiMessage {
            role: "tool".to_owned(),
            content: openai_content_as_text_value(&message.content),
            name: message.name.clone(),
            tool_call_id: openai_tool_call_id_from_message(message),
            tool_calls: Vec::new(),
        }]),
    }
}

pub(super) fn openai_plain_message(
    role: &str,
    content: Value,
    name: Option<String>,
) -> OpenAiMessage {
    OpenAiMessage {
        role: role.to_owned(),
        content,
        name,
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}

pub(super) fn openai_user_messages_from_llm(
    message: &LlmMessage,
) -> Result<Vec<OpenAiMessage>, LlmError> {
    let Some(blocks) = message.content.as_array() else {
        return Ok(vec![openai_plain_message(
            "user",
            message.content.clone(),
            message.name.clone(),
        )]);
    };

    let mut messages = Vec::new();
    let mut text_parts = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    text_parts.push(json!({"type": "text", "text": text}));
                }
            }
            Some("tool_result") => messages.push(OpenAiMessage {
                role: "tool".to_owned(),
                tool_call_id: block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| openai_tool_call_id_from_message(message)),
                content: openai_content_as_text_value(block.get("content").unwrap_or(&Value::Null)),
                name: message.name.clone(),
                tool_calls: Vec::new(),
            }),
            _ => {}
        }
    }
    if !text_parts.is_empty() {
        messages.insert(
            0,
            openai_plain_message("user", Value::Array(text_parts), message.name.clone()),
        );
    }
    if messages.is_empty() {
        messages.push(openai_plain_message(
            "user",
            message.content.clone(),
            message.name.clone(),
        ));
    }
    Ok(messages)
}

pub(super) fn openai_assistant_message_from_llm(
    message: &LlmMessage,
) -> Result<OpenAiMessage, LlmError> {
    let Some(blocks) = message.content.as_array() else {
        return Ok(openai_plain_message(
            "assistant",
            message.content.clone(),
            message.name.clone(),
        ));
    };

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(value) = block.get("text").and_then(Value::as_str) {
                    text.push_str(value);
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if name.is_empty() {
                    return Err(LlmError::validation(
                        "OpenAI assistant tool_use block requires a name",
                    ));
                }
                tool_calls.push(OpenAiToolCall {
                    id,
                    r#type: "function".to_owned(),
                    function: OpenAiToolCallFunction {
                        name,
                        arguments: serde_json::to_string(block.get("input").unwrap_or(&json!({})))
                            .map_err(|err| {
                                LlmError::validation(format!(
                                    "OpenAI assistant tool input is not serializable: {err}"
                                ))
                            })?,
                    },
                });
            }
            _ => {}
        }
    }

    Ok(OpenAiMessage {
        role: "assistant".to_owned(),
        content: if text.is_empty() {
            Value::Null
        } else {
            Value::String(text)
        },
        name: message.name.clone(),
        tool_call_id: None,
        tool_calls,
    })
}

pub(super) fn openai_tool_call_id_from_message(message: &LlmMessage) -> Option<String> {
    message
        .metadata
        .get("tool_call_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub(super) fn openai_content_as_text_value(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        Value::Null => Value::String(String::new()),
        _ => Value::String(value.to_string()),
    }
}

pub(super) fn openai_tool_from_spec(tool: &ToolSpec) -> OpenAiTool {
    OpenAiTool {
        r#type: "function".to_owned(),
        function: OpenAiToolFunction {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        },
    }
}

pub(super) fn openai_response_format(
    format: Option<&LlmResponseFormat>,
) -> Option<OpenAiResponseFormat> {
    match format {
        None => None,
        Some(LlmResponseFormat::JsonObject) => Some(OpenAiResponseFormat::JsonObject),
        Some(LlmResponseFormat::JsonSchema {
            name,
            schema,
            strict,
        }) => Some(OpenAiResponseFormat::JsonSchema {
            json_schema: OpenAiJsonSchema {
                name: name.clone(),
                schema: schema.clone(),
                strict: Some(strict.unwrap_or(true)),
            },
        }),
    }
}

pub(super) fn openai_finish_reason(value: Option<&str>) -> LlmFinishReason {
    match value {
        Some("stop") | None => LlmFinishReason::Stop,
        Some("length") => LlmFinishReason::Length,
        Some("tool_calls") | Some("function_call") => LlmFinishReason::ToolCall,
        Some("content_filter") => LlmFinishReason::ContentFilter,
        _ => LlmFinishReason::Error,
    }
}

pub(super) fn openai_tool_id(index: i64, state: &OpenAiToolCallState) -> String {
    if state.id.is_empty() {
        format!("call_{index}")
    } else {
        state.id.clone()
    }
}
