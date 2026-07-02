use agent_llm::{LlmFinishReason, LlmMessage, LlmRequest, LlmResponse, LlmRole};
use serde_json::{Value, json};

use crate::{
    ChatError, ChatToolCall, ChatToolResult, ChatTurnAdvance, ChatTurnRequest, ChatTurnState,
};

pub fn chat_turn_initial_state(request: &ChatTurnRequest) -> Result<ChatTurnState, ChatError> {
    if request.messages.is_empty() {
        return Err(ChatError::validation(
            "chat turn requires at least one message",
        ));
    }
    if !request.metadata.is_null() && !request.metadata.is_object() {
        return Err(ChatError::validation(
            "chat turn metadata must be a JSON object",
        ));
    }
    Ok(ChatTurnState {
        protocol_version: request.protocol_version.clone(),
        turn_id: request.turn_id.clone(),
        surface: request.surface.clone(),
        mode: request.mode.clone(),
        session_id: request.session_id.clone(),
        thread_id: request.thread_id.clone(),
        agent_id: request.agent_id.clone(),
        provider: request.provider.clone(),
        model: request.model.clone(),
        messages: request.messages.clone(),
        temperature: request.temperature,
        max_output_tokens: request.max_output_tokens,
        tools: request.tools.clone(),
        metadata: if request.metadata.is_null() {
            json!({})
        } else {
            request.metadata.clone()
        },
        max_tool_rounds: request.max_tool_rounds.max(1),
        round: 0,
        pending_tool_calls: Vec::new(),
    })
}

pub fn chat_turn_llm_request(state: &ChatTurnState) -> LlmRequest {
    LlmRequest {
        protocol_version: state.protocol_version.clone(),
        provider: state.provider.clone(),
        model: state.model.clone(),
        messages: state.messages.clone(),
        temperature: state.temperature,
        max_output_tokens: state.max_output_tokens,
        tools: state.tools.clone(),
        response_format: None,
        metadata: llm_metadata(state),
    }
}

pub fn chat_turn_next_round(state: &ChatTurnState) -> u32 {
    state.round.saturating_add(1)
}

pub fn chat_turn_apply_response(
    mut state: ChatTurnState,
    assistant_text: &str,
    tool_calls: Vec<ChatToolCall>,
    response: &LlmResponse,
) -> Result<ChatTurnAdvance, ChatError> {
    let round = chat_turn_next_round(&state);
    state.round = round;
    if !matches!(response.finish_reason, LlmFinishReason::ToolCall) || tool_calls.is_empty() {
        state.pending_tool_calls.clear();
        return Ok(ChatTurnAdvance::Completed {
            state,
            stop_reason: finish_reason(&response.finish_reason).to_owned(),
        });
    }
    if round >= state.max_tool_rounds {
        return Err(ChatError::validation(
            "chat turn exceeded the tool round budget",
        ));
    }

    state
        .messages
        .push(assistant_message(assistant_text, &tool_calls));
    state.pending_tool_calls = tool_calls.clone();
    Ok(ChatTurnAdvance::RequiresToolResults { state, tool_calls })
}

pub fn chat_turn_apply_tool_results(
    mut state: ChatTurnState,
    results: Vec<ChatToolResult>,
) -> Result<ChatTurnState, ChatError> {
    if state.pending_tool_calls.is_empty() {
        return Err(ChatError::validation(
            "chat turn has no pending tool calls to resume",
        ));
    }
    let mut result_blocks = Vec::new();
    for call in state.pending_tool_calls.clone() {
        let result = results
            .iter()
            .find(|result| result.tool_call_id == call.id)
            .ok_or_else(|| {
                ChatError::validation(format!("missing tool result for tool call '{}'", call.id))
            })?;
        result_blocks.push(tool_result_block(
            &call.id,
            ToolOutput {
                value: result.output.clone(),
                is_error: result.is_error,
            },
        ));
    }
    state.pending_tool_calls.clear();
    state.messages.push(LlmMessage {
        role: LlmRole::User,
        content: Value::Array(result_blocks),
        name: None,
        metadata: json!({}),
    });
    Ok(state)
}

#[derive(Debug, Clone)]
pub(crate) struct ToolOutput {
    pub(crate) value: Value,
    pub(crate) is_error: bool,
}

fn assistant_message(text: &str, tool_calls: &[ChatToolCall]) -> LlmMessage {
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(json!({"type": "text", "text": text}));
    }
    for call in tool_calls {
        blocks.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": call.input,
        }));
    }
    LlmMessage {
        role: LlmRole::Assistant,
        content: Value::Array(blocks),
        name: None,
        metadata: json!({}),
    }
}

fn tool_result_block(tool_call_id: &str, output: ToolOutput) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": tool_call_id,
        "content": match output.value {
            Value::String(value) => Value::String(value),
            value => Value::String(value.to_string()),
        },
        "is_error": output.is_error,
    })
}

pub(crate) fn llm_metadata(state: &ChatTurnState) -> Value {
    let mut metadata = if state.metadata.is_null() {
        json!({})
    } else {
        state.metadata.clone()
    };
    if let Some(object) = metadata.as_object_mut() {
        object.insert("chat_turn".to_owned(), Value::Bool(true));
        if let Some(value) = &state.turn_id {
            object.insert("turn_id".to_owned(), Value::String(value.clone()));
        }
        if let Some(value) = &state.session_id {
            object.insert("session_id".to_owned(), Value::String(value.clone()));
        }
        if let Some(value) = &state.thread_id {
            object.insert("thread_id".to_owned(), Value::String(value.clone()));
        }
        if let Some(value) = &state.agent_id {
            object.insert("agent_id".to_owned(), Value::String(value.clone()));
        }
        if let Some(value) = &state.surface {
            object.insert("surface".to_owned(), Value::String(value.clone()));
        }
        if let Some(value) = &state.mode {
            object.insert("mode".to_owned(), Value::String(value.clone()));
        }
    }
    metadata
}

fn finish_reason(reason: &LlmFinishReason) -> &'static str {
    match reason {
        LlmFinishReason::Stop => "end_turn",
        LlmFinishReason::Length => "max_tokens",
        LlmFinishReason::ToolCall => "tool_use",
        LlmFinishReason::ContentFilter => "content_filter",
        LlmFinishReason::Error => "error",
    }
}
