use std::sync::Arc;

use agent_core::{
    AgentCancellation, AgentServices, CancellationFuture, CancellationSignal, PROTOCOL_VERSION,
    infer_tool_outcome,
};
use agent_llm::{LlmEventKind, LlmProvider, LlmResponse};
use futures::stream;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{
    ChatError, ChatEventStream, ChatResumeRequest, ChatToolCall, ChatToolExecution, ChatToolResult,
    ChatTurnAdvance, ChatTurnEvent, ChatTurnEventKind, ChatTurnRequest, ChatTurnSnapshot,
    ChatTurnState, ToolOutput, chat_event_from_llm_event, chat_turn_apply_response,
    chat_turn_apply_tool_results, chat_turn_initial_state, chat_turn_next_round,
    chat_turn_prepare_llm_request, send_done, send_error, send_event, turn_metadata,
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
        self.stream_with_cancellation(request, CancellationToken::new())
    }

    pub fn stream_with_cancellation(
        &self,
        request: ChatTurnRequest,
        cancellation: CancellationToken,
    ) -> ChatEventStream {
        let (sender, receiver) = mpsc::channel(64);
        let provider = self.provider.clone();
        let services = self.services.clone();
        tokio::spawn(async move {
            run_chat_turn(provider, services, request, sender, cancellation).await;
        });
        Box::pin(stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|event| (event, receiver))
        }))
    }

    pub fn resume(&self, request: ChatResumeRequest) -> ChatEventStream {
        self.resume_with_cancellation(request, CancellationToken::new())
    }

    pub fn resume_with_cancellation(
        &self,
        request: ChatResumeRequest,
        cancellation: CancellationToken,
    ) -> ChatEventStream {
        let (sender, receiver) = mpsc::channel(64);
        let provider = self.provider.clone();
        let services = self.services.clone();
        tokio::spawn(async move {
            run_chat_resume(provider, services, request, sender, cancellation).await;
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
    cancellation: CancellationToken,
) {
    if request.protocol_version != PROTOCOL_VERSION {
        send_error(
            &sender,
            0,
            ChatError::validation(format!(
                "protocol_version '{}' is not supported; expected '{PROTOCOL_VERSION}'",
                request.protocol_version
            )),
        )
        .await;
        return;
    }
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
    let state = match chat_turn_initial_state(&request) {
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

    run_chat_state(provider, services, state, sender, turn_timer, cancellation).await;
}

async fn run_chat_resume(
    provider: Arc<dyn LlmProvider>,
    services: Arc<dyn AgentServices>,
    request: ChatResumeRequest,
    sender: mpsc::Sender<Result<ChatTurnEvent, ChatError>>,
    cancellation: CancellationToken,
) {
    if request.protocol_version != PROTOCOL_VERSION
        || request.state.protocol_version != PROTOCOL_VERSION
    {
        send_error(
            &sender,
            request.state.round,
            ChatError::validation(format!(
                "chat resume protocol versions must be '{PROTOCOL_VERSION}'"
            )),
        )
        .await;
        return;
    }
    let turn_timer = std::time::Instant::now();
    let pending_calls = request.state.pending_tool_calls.clone();
    let previous_round = request.state.round;
    info!(
        turn_id = request.state.turn_id.as_deref().unwrap_or("none"),
        session_id = request.state.session_id.as_deref().unwrap_or("none"),
        thread_id = request.state.thread_id.as_deref().unwrap_or("none"),
        agent_id = request.state.agent_id.as_deref().unwrap_or("none"),
        provider = %request.state.provider,
        model = %request.state.model,
        pending_tool_call_count = pending_calls.len(),
        tool_result_count = request.tool_results.len(),
        "resuming chat turn",
    );
    let state = match chat_turn_apply_tool_results(request.state, request.tool_results.clone()) {
        Ok(state) => state,
        Err(error) => {
            warn!(
                round = previous_round,
                error_code = %error.record.code,
                retryable = error.record.retryable,
                "chat turn resume failed",
            );
            send_error(&sender, previous_round, error).await;
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
            round: state.round,
            metadata: turn_metadata(&state),
        },
    )
    .await;
    for call in pending_calls {
        if let Some(result) = request
            .tool_results
            .iter()
            .find(|result| result.tool_call_id == call.id)
        {
            send_event(
                &sender,
                ChatTurnEvent {
                    kind: ChatTurnEventKind::ToolResult,
                    content: None,
                    response: None,
                    tool_call_id: Some(call.id.clone()),
                    tool_name: Some(call.name.clone()),
                    partial_input_json: None,
                    tool_input: Some(call.input.clone()),
                    tool_output: Some(result.output.clone()),
                    usage: None,
                    round: state.round,
                    metadata: json!({"is_error": result.is_error, "resumed": true}),
                },
            )
            .await;
        }
    }

    run_chat_state(provider, services, state, sender, turn_timer, cancellation).await;
}

async fn run_chat_state(
    provider: Arc<dyn LlmProvider>,
    services: Arc<dyn AgentServices>,
    mut state: ChatTurnState,
    sender: mpsc::Sender<Result<ChatTurnEvent, ChatError>>,
    turn_timer: std::time::Instant,
    cancellation: CancellationToken,
) {
    loop {
        let round = chat_turn_next_round(&state);
        if cancellation.is_cancelled() {
            send_cancelled(&sender, round, "before_round").await;
            return;
        }
        let llm_request = match chat_turn_prepare_llm_request(&mut state) {
            Ok(request) => request,
            Err(error) => {
                warn!(
                    turn_id = state.turn_id.as_deref().unwrap_or("none"),
                    round,
                    error_code = %error.record.code,
                    retryable = error.record.retryable,
                    "chat context preparation failed",
                );
                send_error(&sender, round, error).await;
                return;
            }
        };
        send_context_snapshot(&sender, round, &state).await;
        info!(
            turn_id = state.turn_id.as_deref().unwrap_or("none"),
            round,
            provider = %state.provider,
            model = %state.model,
            message_count = llm_request.messages.len(),
            tool_count = llm_request.tools.len(),
            "starting chat round",
        );
        let mut stream = match tokio::select! {
            _ = cancellation.cancelled() => {
                send_cancelled(&sender, round, "before_llm_stream").await;
                return;
            }
            stream = provider.stream(llm_request) => stream,
        } {
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
        loop {
            let event = tokio::select! {
                _ = cancellation.cancelled() => {
                    send_cancelled(&sender, round, "during_llm_stream").await;
                    return;
                }
                event = futures::StreamExt::next(&mut stream) => event,
            };
            let Some(event) = event else {
                break;
            };
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

        let advance =
            match chat_turn_apply_response(state, &assistant_text, tool_calls.clone(), &response) {
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
                state: completed_state,
                stop_reason,
            } => {
                send_round_finished(
                    &sender,
                    round,
                    &response,
                    "completed",
                    &completed_state,
                    &[],
                )
                .await;
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
                send_round_finished(
                    &sender,
                    round,
                    &response,
                    "requires_tool_results",
                    &state,
                    &tool_calls,
                )
                .await;
                info!(
                    turn_id = state.turn_id.as_deref().unwrap_or("none"),
                    round,
                    tool_call_count = tool_calls.len(),
                    "chat turn requires tool results",
                );
                if state.tool_execution == ChatToolExecution::Client {
                    send_done(&sender, round, "requires_tool_results").await;
                    return;
                }
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
            if cancellation.is_cancelled() {
                send_cancelled(&sender, round, "before_tool_call").await;
                return;
            }
            let output = match services
                .call_tool_with_cancellation(
                    &tool_call.name,
                    tool_call.input.clone(),
                    agent_cancellation(cancellation.clone()),
                )
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
            let outcome = infer_tool_outcome(&output.value, output.is_error);
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
                    metadata: json!({
                        "is_error": outcome.is_error(),
                        "outcome": &outcome,
                    }),
                },
            )
            .await;
            results.push(ChatToolResult {
                tool_call_id: tool_call.id,
                tool_name: tool_call.name,
                output: output.value,
                is_error: outcome.is_error(),
                outcome: Some(outcome),
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

async fn send_context_snapshot(
    sender: &mpsc::Sender<Result<ChatTurnEvent, ChatError>>,
    round: u32,
    state: &ChatTurnState,
) {
    send_event(
        sender,
        ChatTurnEvent {
            kind: ChatTurnEventKind::ContextSnapshot,
            content: None,
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            tool_output: None,
            usage: None,
            round,
            metadata: json!({
                "context_snapshot": state.context_snapshot.clone(),
                "compaction": state.compaction.clone(),
            }),
        },
    )
    .await;
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

fn agent_cancellation(token: CancellationToken) -> AgentCancellation {
    AgentCancellation::new(Arc::new(TokioCancellation { token }))
}

struct TokioCancellation {
    token: CancellationToken,
}

impl CancellationSignal for TokioCancellation {
    fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    fn cancelled(&self) -> CancellationFuture<'_> {
        Box::pin(self.token.cancelled())
    }
}

async fn send_round_finished(
    sender: &mpsc::Sender<Result<ChatTurnEvent, ChatError>>,
    round: u32,
    response: &LlmResponse,
    status: &str,
    state: &ChatTurnState,
    tool_calls: &[ChatToolCall],
) {
    let chat_snapshot = if status == "requires_tool_results" {
        ChatTurnSnapshot::requires_tool_results(state.clone())
    } else {
        ChatTurnSnapshot::completed(state.clone(), finish_reason_label(&response.finish_reason))
    };
    send_event(
        sender,
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
            metadata: json!({
                "status": status,
                "chat_state": state,
                "chat_snapshot": chat_snapshot,
                "tool_calls": tool_calls,
                "finish_reason": response.finish_reason,
                "context_snapshot": state.context_snapshot.clone(),
                "compaction": state.compaction.clone(),
            }),
        },
    )
    .await;
}

fn finish_reason_label(reason: &agent_llm::LlmFinishReason) -> &'static str {
    match reason {
        agent_llm::LlmFinishReason::Stop => "end_turn",
        agent_llm::LlmFinishReason::Length => "max_tokens",
        agent_llm::LlmFinishReason::ToolCall => "tool_use",
        agent_llm::LlmFinishReason::ContentFilter => "content_filter",
        agent_llm::LlmFinishReason::Error => "error",
    }
}

async fn send_cancelled(
    sender: &mpsc::Sender<Result<ChatTurnEvent, ChatError>>,
    round: u32,
    stage: &str,
) {
    send_event(
        sender,
        ChatTurnEvent {
            kind: ChatTurnEventKind::Error,
            content: Some("chat turn cancelled".to_owned()),
            response: None,
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            tool_output: None,
            usage: None,
            round,
            metadata: json!({
                "code": "cancelled",
                "retryable": false,
                "details": {"stage": stage},
            }),
        },
    )
    .await;
    send_done(sender, round, "cancelled").await;
}

fn serialized_value_len(value: &serde_json::Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}
