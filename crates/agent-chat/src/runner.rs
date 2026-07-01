use std::sync::Arc;

use agent_core::AgentServices;
use agent_llm::{LlmEventKind, LlmProvider};
use futures::stream;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{
    ChatError, ChatEventStream, ChatToolCall, ChatToolResult, ChatTurnAdvance, ChatTurnEvent,
    ChatTurnEventKind, ChatTurnRequest, ToolOutput, chat_event_from_llm_event,
    chat_turn_apply_response, chat_turn_apply_tool_results, chat_turn_initial_state,
    chat_turn_llm_request, chat_turn_next_round, send_done, send_error, send_event, turn_metadata,
};

#[derive(Clone)]
pub struct ChatTurnRunner {
    provider: Arc<dyn LlmProvider>,
    services: Arc<dyn AgentServices>,
}

impl ChatTurnRunner {
    pub fn new(provider: Arc<dyn LlmProvider>, services: Arc<dyn AgentServices>) -> Self {
        Self { provider, services }
    }

    pub fn stream(&self, request: ChatTurnRequest) -> ChatEventStream {
        let (sender, receiver) = mpsc::channel(64);
        let provider = self.provider.clone();
        let services = self.services.clone();
        tokio::spawn(async move {
            run_chat_turn(provider, services, request, sender).await;
        });
        Box::pin(stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|event| (event, receiver))
        }))
    }
}

async fn run_chat_turn(
    provider: Arc<dyn LlmProvider>,
    services: Arc<dyn AgentServices>,
    request: ChatTurnRequest,
    sender: mpsc::Sender<Result<ChatTurnEvent, ChatError>>,
) {
    let turn_timer = std::time::Instant::now();
    info!(
        turn_id = request.turn_id.as_deref().unwrap_or("none"),
        session_id = request.session_id.as_deref().unwrap_or("none"),
        thread_id = request.thread_id.as_deref().unwrap_or("none"),
        agent_id = request.agent_id.as_deref().unwrap_or("none"),
        provider = %request.provider,
        model = %request.model,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        max_tool_rounds = request.max_tool_rounds,
        "starting chat turn",
    );
    let mut state = match chat_turn_initial_state(&request) {
        Ok(state) => state,
        Err(error) => {
            warn!(
                error_code = %error.record.code,
                retryable = error.record.retryable,
                "chat turn initial state failed",
            );
            send_error(&sender, 0, error).await;
            return;
        }
    };
    send_event(
        &sender,
        ChatTurnEvent {
            kind: ChatTurnEventKind::Started,
            content: None,
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            tool_output: None,
            usage: None,
            round: 0,
            metadata: turn_metadata(&state),
        },
    )
    .await;

    loop {
        let round = chat_turn_next_round(&state);
        let llm_request = chat_turn_llm_request(&state);
        info!(
            turn_id = state.turn_id.as_deref().unwrap_or("none"),
            round,
            provider = %state.provider,
            model = %state.model,
            message_count = llm_request.messages.len(),
            tool_count = llm_request.tools.len(),
            "starting chat round",
        );
        let mut stream = match provider.stream(llm_request).await {
            Ok(stream) => stream,
            Err(error) => {
                let error = ChatError::llm(error);
                warn!(
                    turn_id = state.turn_id.as_deref().unwrap_or("none"),
                    round,
                    error_code = %error.record.code,
                    retryable = error.record.retryable,
                    "chat LLM stream failed to start",
                );
                send_error(&sender, round, error).await;
                return;
            }
        };

        let mut assistant_text = String::new();
        let mut tool_calls = Vec::new();
        let mut response = None;
        while let Some(event) = futures::StreamExt::next(&mut stream).await {
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    let error = ChatError::llm(error);
                    warn!(
                        turn_id = state.turn_id.as_deref().unwrap_or("none"),
                        round,
                        error_code = %error.record.code,
                        retryable = error.record.retryable,
                        "chat LLM stream returned an error",
                    );
                    send_error(&sender, round, error).await;
                    return;
                }
            };
            if let Some(content) = event.content.as_ref()
                && matches!(event.kind, LlmEventKind::Delta)
            {
                assistant_text.push_str(content);
            }
            if matches!(event.kind, LlmEventKind::ToolCallEnd) {
                let Some(id) = non_empty(event.tool_call_id.clone()) else {
                    warn!(
                        turn_id = state.turn_id.as_deref().unwrap_or("none"),
                        round, "LLM tool_call_end missing tool_call_id",
                    );
                    send_error(
                        &sender,
                        round,
                        ChatError::validation("tool_call_end requires tool_call_id"),
                    )
                    .await;
                    return;
                };
                let Some(name) = non_empty(event.tool_name.clone()) else {
                    warn!(
                        turn_id = state.turn_id.as_deref().unwrap_or("none"),
                        round,
                        tool_call_id = %id,
                        "LLM tool_call_end missing tool_name",
                    );
                    send_error(
                        &sender,
                        round,
                        ChatError::validation("tool_call_end requires tool_name"),
                    )
                    .await;
                    return;
                };
                tool_calls.push(ChatToolCall {
                    id,
                    name,
                    input: event.tool_input.clone().unwrap_or_else(|| json!({})),
                });
            }
            if matches!(event.kind, LlmEventKind::Finished) {
                response = event.response.clone();
                continue;
            }
            send_event(&sender, chat_event_from_llm_event(event, round)).await;
        }

        let Some(response) = response else {
            warn!(
                turn_id = state.turn_id.as_deref().unwrap_or("none"),
                round, "LLM stream ended without a finished event",
            );
            send_error(
                &sender,
                round,
                ChatError::validation("LLM stream ended without a finished event"),
            )
            .await;
            return;
        };
        if let Some(usage) = response.usage.clone() {
            send_event(
                &sender,
                ChatTurnEvent {
                    kind: ChatTurnEventKind::Usage,
                    content: None,
                    response: None,
                    tool_call_id: None,
                    tool_name: None,
                    partial_input_json: None,
                    tool_input: None,
                    tool_output: None,
                    usage: Some(usage),
                    round,
                    metadata: json!({}),
                },
            )
            .await;
        }
        send_event(
            &sender,
            ChatTurnEvent {
                kind: ChatTurnEventKind::RoundFinished,
                content: None,
                response: Some(response.clone()),
                tool_call_id: None,
                tool_name: None,
                partial_input_json: None,
                tool_input: None,
                tool_output: None,
                usage: response.usage.clone(),
                round,
                metadata: json!({"finish_reason": response.finish_reason}),
            },
        )
        .await;
        info!(
            turn_id = state.turn_id.as_deref().unwrap_or("none"),
            round,
            finish_reason = ?response.finish_reason,
            tool_call_count = tool_calls.len(),
            assistant_chars = assistant_text.chars().count(),
            input_tokens = response.usage.as_ref().map(|usage| usage.input_tokens).unwrap_or(0),
            output_tokens = response.usage.as_ref().map(|usage| usage.output_tokens).unwrap_or(0),
            "chat round finished",
        );

        let advance = match chat_turn_apply_response(state, &assistant_text, tool_calls, &response)
        {
            Ok(advance) => advance,
            Err(error) => {
                warn!(
                    round,
                    error_code = %error.record.code,
                    retryable = error.record.retryable,
                    "failed to apply chat response",
                );
                send_error(&sender, round, error).await;
                return;
            }
        };
        let (pending_state, tool_calls) = match advance {
            ChatTurnAdvance::Completed {
                state: _,
                stop_reason,
            } => {
                info!(
                    round,
                    stop_reason = %stop_reason,
                    duration_ms = turn_timer.elapsed().as_millis(),
                    "chat turn completed",
                );
                send_done(&sender, round, &stop_reason).await;
                return;
            }
            ChatTurnAdvance::RequiresToolResults { state, tool_calls } => {
                info!(
                    turn_id = state.turn_id.as_deref().unwrap_or("none"),
                    round,
                    tool_call_count = tool_calls.len(),
                    "chat turn requires tool results",
                );
                (state, tool_calls)
            }
        };

        let mut results = Vec::new();
        for tool_call in tool_calls {
            let tool_timer = std::time::Instant::now();
            debug!(
                turn_id = pending_state.turn_id.as_deref().unwrap_or("none"),
                round,
                tool_call_id = %tool_call.id,
                tool_name = %tool_call.name,
                input_bytes = serialized_value_len(&tool_call.input),
                "calling chat tool",
            );
            let output = match services
                .call_tool(&tool_call.name, tool_call.input.clone())
                .await
            {
                Ok(output) => {
                    info!(
                        turn_id = pending_state.turn_id.as_deref().unwrap_or("none"),
                        round,
                        tool_call_id = %tool_call.id,
                        tool_name = %tool_call.name,
                        output_bytes = serialized_value_len(&output),
                        duration_ms = tool_timer.elapsed().as_millis(),
                        "chat tool completed",
                    );
                    ToolOutput {
                        value: output,
                        is_error: false,
                    }
                }
                Err(error) => {
                    warn!(
                        turn_id = pending_state.turn_id.as_deref().unwrap_or("none"),
                        round,
                        tool_call_id = %tool_call.id,
                        tool_name = %tool_call.name,
                        error_code = %error.record.code,
                        retryable = error.record.retryable,
                        duration_ms = tool_timer.elapsed().as_millis(),
                        "chat tool failed",
                    );
                    ToolOutput {
                        value: json!({
                        "code": error.record.code,
                        "message": error.record.message,
                        "retryable": error.record.retryable,
                        "details": error.record.details,
                        }),
                        is_error: true,
                    }
                }
            };
            send_event(
                &sender,
                ChatTurnEvent {
                    kind: ChatTurnEventKind::ToolResult,
                    content: None,
                    response: None,
                    tool_call_id: Some(tool_call.id.clone()),
                    tool_name: Some(tool_call.name.clone()),
                    partial_input_json: None,
                    tool_input: Some(tool_call.input.clone()),
                    tool_output: Some(output.value.clone()),
                    usage: None,
                    round,
                    metadata: json!({"is_error": output.is_error}),
                },
            )
            .await;
            results.push(ChatToolResult {
                tool_call_id: tool_call.id,
                tool_name: tool_call.name,
                output: output.value,
                is_error: output.is_error,
            });
        }
        state = match chat_turn_apply_tool_results(pending_state, results) {
            Ok(state) => state,
            Err(error) => {
                warn!(
                    round,
                    error_code = %error.record.code,
                    retryable = error.record.retryable,
                    "failed to apply chat tool results",
                );
                send_error(&sender, round, error).await;
                return;
            }
        };
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

fn serialized_value_len(value: &serde_json::Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}
