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

pub(super) fn anthropic_messages_from_llm(
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
                content: message.content.clone(),
            }),
            LlmRole::Assistant => mapped.push(AnthropicMessage {
                role: "assistant".to_owned(),
                content: message.content.clone(),
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

pub(super) fn anthropic_tool_from_spec(tool: &ToolSpec) -> AnthropicTool {
    AnthropicTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: tool.input_schema.clone(),
    }
}

pub(super) fn append_structured_instruction(
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

pub(super) fn anthropic_text_from_blocks(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

pub(super) fn anthropic_finish_reason(value: Option<&str>) -> LlmFinishReason {
    match value {
        Some("end_turn") | Some("stop_sequence") | None => LlmFinishReason::Stop,
        Some("max_tokens") => LlmFinishReason::Length,
        Some("tool_use") => LlmFinishReason::ToolCall,
        _ => LlmFinishReason::Error,
    }
}
