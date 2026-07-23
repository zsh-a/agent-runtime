use agent_core::{
    InteractionEnvelope, InteractionResponse, InteractionResumeKind, InteractionStatus,
};
use agent_llm::{LlmFinishReason, LlmMessage, LlmRequest, LlmResponse, LlmRole};
use serde_json::{Value, json};

use crate::{
    ChatError, ChatToolCall, ChatToolResult, ChatTurnAdvance, ChatTurnRequest, ChatTurnState,
    context::{build_llm_request_without_state_update, prepare_llm_request},
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
        context_blocks: request.context_blocks.clone(),
        metadata: if request.metadata.is_null() {
            json!({})
        } else {
            request.metadata.clone()
        },
        context_policy: request.context_policy.clone(),
        context_snapshot: None,
        compaction: None,
        max_tool_rounds: request.max_tool_rounds.max(1),
        round: 0,
        pending_tool_calls: Vec::new(),
        pending_interaction: None,
        tool_execution: request.tool_execution,
    })
}

pub fn chat_turn_llm_request(state: &ChatTurnState) -> LlmRequest {
    build_llm_request_without_state_update(state)
}

pub fn chat_turn_prepare_llm_request(state: &mut ChatTurnState) -> Result<LlmRequest, ChatError> {
    prepare_llm_request(state).map(|prepared| prepared.request)
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
        let content = if assistant_text.is_empty() {
            response.content.as_str()
        } else {
            assistant_text
        };
        if !content.is_empty() {
            state.messages.push(assistant_text_message(content));
        }
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
    if state.pending_interaction.is_some() {
        return Err(ChatError::validation(
            "chat turn cannot apply tool results while an interaction is pending",
        ));
    }
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
        if result.tool_name != call.name {
            return Err(ChatError::validation(format!(
                "tool result '{}' uses tool '{}' but pending call requires '{}'",
                result.tool_call_id, result.tool_name, call.name
            )));
        }
        result_blocks.push(tool_result_block(
            &call.id,
            ToolOutput {
                value: result.output.clone(),
                is_error: result.effective_is_error(),
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

/// Pause a turn at a durable human-interaction boundary.
pub fn chat_turn_suspend_for_interaction(
    mut state: ChatTurnState,
    interaction: InteractionEnvelope,
) -> Result<ChatTurnState, ChatError> {
    if !state.pending_tool_calls.is_empty() {
        return Err(ChatError::validation(
            "chat turn cannot suspend for interaction with pending tool calls",
        ));
    }
    if state.pending_interaction.is_some() {
        return Err(ChatError::validation(
            "chat turn already has a pending interaction",
        ));
    }
    interaction.validate().map_err(ChatError::validation)?;
    if interaction.status != InteractionStatus::Pending {
        return Err(ChatError::validation(
            "chat interaction must be pending when execution suspends",
        ));
    }
    if interaction.resume.kind != InteractionResumeKind::ChatTurn {
        return Err(ChatError::validation(
            "chat interaction must resume through chat_turn",
        ));
    }
    state.pending_interaction = Some(interaction);
    Ok(state)
}

/// Resolve a pending interaction and append a provider-neutral result block
/// before the next model round.
pub fn chat_turn_apply_interaction_response(
    mut state: ChatTurnState,
    response: InteractionResponse,
) -> Result<ChatTurnState, ChatError> {
    if !state.pending_tool_calls.is_empty() {
        return Err(ChatError::validation(
            "chat turn cannot resolve interaction with pending tool calls",
        ));
    }
    let mut interaction = state
        .pending_interaction
        .take()
        .ok_or_else(|| ChatError::validation("chat turn has no pending interaction to resume"))?;
    interaction
        .resolve(response, time::OffsetDateTime::now_utc())
        .map_err(ChatError::validation)?;
    let response = interaction
        .response
        .as_ref()
        .expect("resolved interaction always carries a response");
    state.messages.push(LlmMessage {
        role: LlmRole::User,
        content: Value::Array(vec![json!({
            "type": "interaction_result",
            "interaction_id": interaction.interaction_id,
            "interaction_kind": interaction.kind,
            "action": response.action,
            "value": response.value,
            "responded_at": response.responded_at,
        })]),
        name: None,
        metadata: json!({
            "interaction_status": interaction.status,
            "interaction_subject": interaction.subject,
        }),
    });
    Ok(state)
}

/// Apply exactly one continuation input. Tool dispatch and human interaction
/// are intentionally mutually exclusive so a recovered host cannot guess
/// which side effect should be replayed.
pub fn chat_turn_resume_state(
    state: ChatTurnState,
    tool_results: Vec<ChatToolResult>,
    interaction_response: Option<InteractionResponse>,
) -> Result<ChatTurnState, ChatError> {
    if state.pending_interaction.is_some() {
        if !tool_results.is_empty() {
            return Err(ChatError::validation(
                "interaction resume cannot include tool results",
            ));
        }
        let response = interaction_response.ok_or_else(|| {
            ChatError::validation("interaction resume requires interaction_response")
        })?;
        return chat_turn_apply_interaction_response(state, response);
    }
    if interaction_response.is_some() {
        return Err(ChatError::validation(
            "tool resume cannot include interaction_response",
        ));
    }
    chat_turn_apply_tool_results(state, tool_results)
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

fn assistant_text_message(text: &str) -> LlmMessage {
    LlmMessage {
        role: LlmRole::Assistant,
        content: Value::String(text.to_owned()),
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
