use super::*;

pub(crate) fn anthropic_messages_from_llm(
    messages: &[LlmMessage],
) -> Result<(Option<String>, Vec<AnthropicMessage>), LlmError> {
    let mut system = Vec::new();
    let mut mapped = Vec::new();
    for message in messages {
        match message.role {
            LlmRole::System => system.push(
                llm_content_as_text(&message.content, "Anthropic system message")?.to_owned(),
            ),
            LlmRole::User => mapped.push(AnthropicMessage {
                role: "user".to_owned(),
                content: anthropic_content(&message.content)?,
            }),
            LlmRole::Assistant => mapped.push(AnthropicMessage {
                role: "assistant".to_owned(),
                content: anthropic_content(&message.content)?,
            }),
            LlmRole::Tool => {
                return Err(LlmError::validation(
                    "Anthropic provider does not yet support tool role messages",
                ));
            }
        }
    }
    let system = if system.is_empty() {
        None
    } else {
        Some(system.join("\n\n"))
    };
    Ok((system, mapped))
}

pub(crate) fn anthropic_tool_from_spec(tool: &ToolSpec) -> AnthropicTool {
    AnthropicTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: tool.input_schema.clone(),
    }
}

pub(crate) fn append_structured_instruction(
    system: Option<String>,
    format: &Option<LlmResponseFormat>,
) -> Option<String> {
    let Some(instruction) = structured_output_instruction(format) else {
        return system;
    };
    match system {
        Some(system) if !system.is_empty() => Some(format!("{system}\n\n{instruction}")),
        _ => Some(instruction),
    }
}

pub(crate) fn anthropic_text_from_blocks(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

pub(crate) fn anthropic_finish_reason(value: Option<&str>) -> LlmFinishReason {
    match value {
        Some("end_turn") | Some("stop_sequence") | None => LlmFinishReason::Stop,
        Some("max_tokens") => LlmFinishReason::Length,
        Some("tool_use") => LlmFinishReason::ToolCall,
        Some("refusal") => LlmFinishReason::ContentFilter,
        _ => LlmFinishReason::Error,
    }
}

fn anthropic_content(content: &crate::MessageContent) -> Result<Value, LlmError> {
    use crate::{ContentBlock, MessageContent};
    match content {
        MessageContent::Text(text)=>Ok(json!(text)),
        MessageContent::Blocks(blocks)=>Ok(Value::Array(blocks.iter().map(|block|match block {
            ContentBlock::Text{..}|ContentBlock::Image{..}|ContentBlock::Document{..}|ContentBlock::ToolUse{..}|ContentBlock::ToolResult{..}|ContentBlock::Thinking{..}|ContentBlock::RedactedThinking{..}=>serde_json::to_value(block).expect("typed block serialization"),
            ContentBlock::ImageUrl{image_url}=>json!({"type":"image","source":{"type":"url","url":image_url.url}}),
            ContentBlock::InteractionResult{..}=>json!({"type":"text","text":serde_json::to_string(block).expect("typed interaction serialization")}),
        }).collect())),
    }
}
