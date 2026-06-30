use agent_llm::{LlmEvent, LlmEventKind};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::{ChatError, ChatTurnEvent, ChatTurnEventKind, ChatTurnState};

pub(crate) fn chat_event_from_llm_event(event: LlmEvent, round: u32) -> ChatTurnEvent {
    ChatTurnEvent {
        kind: match event.kind {
            LlmEventKind::Started => ChatTurnEventKind::LlmStarted,
            LlmEventKind::Delta => ChatTurnEventKind::Delta,
            LlmEventKind::ThinkingDelta => ChatTurnEventKind::ThinkingDelta,
            LlmEventKind::ThinkingSignatureDelta => ChatTurnEventKind::ThinkingSignatureDelta,
            LlmEventKind::ToolCallStart => ChatTurnEventKind::ToolCallStart,
            LlmEventKind::ToolCallDelta => ChatTurnEventKind::ToolCallDelta,
            LlmEventKind::ToolCallEnd => ChatTurnEventKind::ToolCallEnd,
            LlmEventKind::Finished => ChatTurnEventKind::RoundFinished,
        },
        content: event.content,
        response: event.response,
        tool_call_id: event.tool_call_id,
        tool_name: event.tool_name,
        partial_input_json: event.partial_input_json,
        tool_input: event.tool_input,
        tool_output: None,
        usage: None,
        round,
        metadata: event.metadata,
    }
}

pub(crate) async fn send_done(
    sender: &mpsc::Sender<Result<ChatTurnEvent, ChatError>>,
    round: u32,
    reason: &str,
) {
    send_event(
        sender,
        ChatTurnEvent {
            kind: ChatTurnEventKind::Done,
            content: None,
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            tool_output: None,
            usage: None,
            round,
            metadata: json!({"stop_reason": reason}),
        },
    )
    .await;
}

pub(crate) async fn send_error(
    sender: &mpsc::Sender<Result<ChatTurnEvent, ChatError>>,
    round: u32,
    error: ChatError,
) {
    let _ = sender
        .send(Ok(ChatTurnEvent {
            kind: ChatTurnEventKind::Error,
            content: Some(error.record.message.clone()),
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            tool_output: None,
            usage: None,
            round,
            metadata: json!({
                "code": error.record.code,
                "retryable": error.record.retryable,
                "details": error.record.details,
            }),
        }))
        .await;
    let _ = sender.send(Err(error)).await;
}

pub(crate) async fn send_event(
    sender: &mpsc::Sender<Result<ChatTurnEvent, ChatError>>,
    event: ChatTurnEvent,
) {
    let _ = sender.send(Ok(event)).await;
}

pub(crate) fn turn_metadata(state: &ChatTurnState) -> Value {
    json!({
        "turn_id": state.turn_id,
        "session_id": state.session_id,
        "thread_id": state.thread_id,
        "agent_id": state.agent_id,
        "surface": state.surface,
        "mode": state.mode,
        "provider": state.provider,
        "model": state.model,
    })
}
