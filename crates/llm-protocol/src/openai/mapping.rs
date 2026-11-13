use super::*;
use crate::{ContentBlock, ImageSource, MessageContent};
use base64::{Engine, engine::general_purpose::STANDARD};

pub(crate) fn openai_messages_from_llm(
    message: &LlmMessage,
) -> Result<Vec<OpenAiMessage>, LlmError> {
    match message.role {
        LlmRole::System => Ok(vec![openai_plain_message(
            "system",
            message.content.to_value(),
            message.name.clone(),
        )]),
        LlmRole::Tool => Ok(vec![OpenAiMessage {
            role: "tool".into(),
            content: openai_content_as_text_value(&message.content.to_value()),
            name: message.name.clone(),
            tool_call_id: openai_tool_call_id_from_message(message),
            tool_calls: Vec::new(),
        }]),
        LlmRole::User => {
            let MessageContent::Blocks(blocks) = &message.content else {
                return Ok(vec![openai_plain_message(
                    "user",
                    message.content.to_value(),
                    message.name.clone(),
                )]);
            };
            let mut messages = Vec::new();
            let mut parts = Vec::new();
            for block in blocks {
                match block {
                ContentBlock::Text{text}=>parts.push(json!({"type":"text","text":text})),
                ContentBlock::Image{source}=>{let url=match source {ImageSource::Base64{media_type,data}=>format!("data:{media_type};base64,{}",STANDARD.encode(data)),ImageSource::Url{url}=>url.clone()};parts.push(json!({"type":"image_url","image_url":{"url":url}}));},
                ContentBlock::ImageUrl{image_url}=>parts.push(json!({"type":"image_url","image_url":image_url})),
                ContentBlock::ToolResult{tool_use_id,content,..}=>messages.push(OpenAiMessage{role:"tool".into(),content:openai_content_as_text_value(content),name:message.name.clone(),tool_call_id:Some(tool_use_id.clone()),tool_calls:Vec::new()}),
                ContentBlock::InteractionResult{..}=>parts.push(json!({"type":"text","text":serde_json::to_string(block).expect("typed interaction")})),
                ContentBlock::Document{..}=>return Err(LlmError::validation("OpenAI-compatible document blocks are not supported by this adapter")),
                ContentBlock::ToolUse{..}|ContentBlock::Thinking{..}|ContentBlock::RedactedThinking{..}=>return Err(LlmError::validation("Assistant content block cannot be a user message")),
            }
            }
            if !parts.is_empty() {
                messages.insert(
                    0,
                    openai_plain_message("user", Value::Array(parts), message.name.clone()),
                );
            }
            Ok(messages)
        }
        LlmRole::Assistant => {
            let MessageContent::Blocks(blocks) = &message.content else {
                return Ok(vec![openai_plain_message(
                    "assistant",
                    message.content.to_value(),
                    message.name.clone(),
                )]);
            };
            let mut text = String::new();
            let mut calls = Vec::new();
            for block in blocks {
                match block {
                    ContentBlock::Text { text: part } => text.push_str(part),
                    ContentBlock::ToolUse { id, name, input } => calls.push(OpenAiToolCall {
                        id: id.clone(),
                        r#type: "function".into(),
                        function: OpenAiToolCallFunction {
                            name: name.clone(),
                            arguments: input.to_string(),
                        },
                    }),
                    ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
                    ContentBlock::Image { .. }
                    | ContentBlock::ImageUrl { .. }
                    | ContentBlock::Document { .. }
                    | ContentBlock::ToolResult { .. }
                    | ContentBlock::InteractionResult { .. } => {
                        return Err(LlmError::validation(
                            "Unsupported assistant content block for OpenAI",
                        ));
                    }
                }
            }
            Ok(vec![OpenAiMessage {
                role: "assistant".into(),
                content: if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                },
                name: message.name.clone(),
                tool_call_id: None,
                tool_calls: calls,
            }])
        }
    }
}
pub(crate) fn openai_plain_message(
    role: &str,
    content: Value,
    name: Option<String>,
) -> OpenAiMessage {
    OpenAiMessage {
        role: role.into(),
        content,
        name,
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}
fn openai_tool_call_id_from_message(message: &LlmMessage) -> Option<String> {
    message
        .metadata
        .get("tool_call_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}
fn openai_content_as_text_value(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        Value::Null => json!(""),
        _ => json!(value.to_string()),
    }
}
pub(crate) fn openai_tool_from_spec(tool: &ToolSpec) -> OpenAiTool {
    OpenAiTool {
        r#type: "function".into(),
        function: OpenAiToolFunction {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        },
    }
}
pub(crate) fn openai_response_format(
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
pub(crate) fn openai_finish_reason(value: Option<&str>) -> LlmFinishReason {
    match value {
        Some("stop") | None => LlmFinishReason::Stop,
        Some("length") => LlmFinishReason::Length,
        Some("tool_calls") | Some("function_call") => LlmFinishReason::ToolCall,
        Some("content_filter") => LlmFinishReason::ContentFilter,
        _ => LlmFinishReason::Error,
    }
}
pub(crate) fn openai_tool_id(index: i64, state: &OpenAiToolCallState) -> String {
    if state.id.is_empty() {
        format!("call_{index}")
    } else {
        state.id.clone()
    }
}
