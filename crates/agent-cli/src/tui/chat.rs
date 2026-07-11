use agent_chat::{
    ChatEventStream, ChatResumeRequest, ChatToolCall, ChatToolExecution, ChatToolResult,
    ChatTurnEvent, ChatTurnEventKind, ChatTurnRequest, ChatTurnRunner, ChatTurnState,
};
use agent_core::PROTOCOL_VERSION;
use agent_llm::{LlmMessage, LlmRole, user_message};
use futures::StreamExt;
use miette::{Result, miette};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{cancellation::agent_cancellation, chat::provider_from_options};

use super::{
    chat_events::{is_cancelled_chat_event, updates_from_chat_event},
    data::{TuiActivityItem, TuiActivityKind, TuiOptions, TuiPendingApproval, TuiState, TuiUpdate},
    policy::TuiToolRisk,
    runtime::TuiRuntime,
};

pub(super) fn start_natural_language_task(
    state: &mut TuiState,
    text: String,
    sender: UnboundedSender<TuiUpdate>,
) -> TuiTaskHandle {
    let mut messages = state.chat_messages.clone();
    messages.push(user_message(text.clone()));
    let options = state.options.clone();
    let selected_agent_id = state.selected_agent_id.clone();
    let cancellation = CancellationToken::new();
    state.push_user_message(text);
    state.start_assistant_stream();
    state.set_busy(true);
    let join = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            if let Err(error) = run_natural_language_stream(
                options,
                selected_agent_id,
                messages,
                sender.clone(),
                cancellation,
            )
            .await
            {
                let _ = sender.send(TuiUpdate::Error(error.to_string()));
            }
            let _ = sender.send(TuiUpdate::Busy(false));
        }
    });
    TuiTaskHandle { join, cancellation }
}

pub(super) struct TuiTaskHandle {
    pub(super) join: JoinHandle<()>,
    pub(super) cancellation: CancellationToken,
}

pub(super) async fn run_natural_language_command(state: &mut TuiState, text: &str) -> Result<()> {
    let runtime = TuiRuntime::load(&state.options).await?;
    let agent_id = runtime.resolve_agent_id(state.selected_agent_id.as_deref())?;
    state.push_user_message(text.to_owned());
    run_chat_turn(state, &runtime, &agent_id, text).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChatApprovalDecision {
    Approve,
    Deny,
}

pub(super) async fn resume_chat_approval(
    state: &mut TuiState,
    decision: ChatApprovalDecision,
    agent_id: String,
    chat_state: ChatTurnState,
    tool_calls: Vec<ChatToolCall>,
    surface_messages: Vec<LlmMessage>,
) -> Result<()> {
    let cancellation = CancellationToken::new();
    let options = state.options.clone();
    let mut emit = |update| state.apply_update(update);
    resume_chat_approval_with_emit(
        options,
        decision,
        agent_id,
        chat_state,
        tool_calls,
        surface_messages,
        cancellation,
        &mut emit,
    )
    .await
}

pub(super) async fn resume_chat_approval_with_emit<Emit>(
    options: TuiOptions,
    decision: ChatApprovalDecision,
    agent_id: String,
    chat_state: ChatTurnState,
    tool_calls: Vec<ChatToolCall>,
    surface_messages: Vec<LlmMessage>,
    cancellation: CancellationToken,
    emit: &mut Emit,
) -> Result<()>
where
    Emit: FnMut(TuiUpdate),
{
    let runtime = TuiRuntime::load_with_cancellation(&options, cancellation.clone()).await?;
    let provider = provider_from_options(&options.chat)?;
    let services = runtime.tool_services(Some(agent_id.clone()));
    let runner = ChatTurnRunner::new(provider, services);
    let tool_results = match decision {
        ChatApprovalDecision::Approve => {
            execute_chat_tool_calls(&runtime, &agent_id, &tool_calls, cancellation.clone(), emit)
                .await
        }
        ChatApprovalDecision::Deny => denied_chat_tool_results(&tool_calls),
    };
    let stream = runner.resume_with_cancellation(
        ChatResumeRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            state: chat_state,
            tool_results,
        },
        cancellation.clone(),
    );
    run_chat_client_loop(
        &runtime,
        &runner,
        &agent_id,
        surface_messages,
        stream,
        cancellation,
        emit,
    )
    .await
}

async fn run_chat_turn(
    state: &mut TuiState,
    runtime: &TuiRuntime,
    agent_id: &str,
    text: &str,
) -> Result<()> {
    let provider = provider_from_options(&state.options.chat)?;
    let services = runtime.tool_services(Some(agent_id.to_owned()));
    let runner = ChatTurnRunner::new(provider, services);
    let user = user_message(text);
    state.chat_messages.push(user.clone());
    let surface_messages = state.chat_messages.clone();
    let request = chat_turn_request(&state.options, runtime, agent_id, surface_messages.clone());
    let cancellation = CancellationToken::new();
    let stream = runner.stream_with_cancellation(request, cancellation.clone());
    state.start_assistant_stream();
    run_chat_client_loop(
        runtime,
        &runner,
        agent_id,
        surface_messages,
        stream,
        cancellation,
        |update| state.apply_update(update),
    )
    .await
}

async fn run_natural_language_stream(
    options: TuiOptions,
    selected_agent_id: Option<String>,
    messages: Vec<LlmMessage>,
    sender: UnboundedSender<TuiUpdate>,
    cancellation: CancellationToken,
) -> Result<()> {
    let runtime = TuiRuntime::load_with_cancellation(&options, cancellation.clone()).await?;
    let agent_id = runtime.resolve_agent_id(selected_agent_id.as_deref())?;
    let provider = provider_from_options(&options.chat)?;
    let services = runtime.tool_services(Some(agent_id.clone()));
    let runner = ChatTurnRunner::new(provider, services);
    let request = chat_turn_request(&options, &runtime, &agent_id, messages.clone());
    let stream = runner.stream_with_cancellation(request, cancellation.clone());
    run_chat_client_loop(
        &runtime,
        &runner,
        &agent_id,
        messages,
        stream,
        cancellation,
        |update| {
            let _ = sender.send(update);
        },
    )
    .await
}

fn chat_turn_request(
    options: &TuiOptions,
    runtime: &TuiRuntime,
    agent_id: &str,
    messages: Vec<LlmMessage>,
) -> ChatTurnRequest {
    ChatTurnRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        turn_id: None,
        surface: Some("agent_tui".to_owned()),
        mode: Some("natural_language".to_owned()),
        session_id: None,
        thread_id: None,
        agent_id: Some(agent_id.to_owned()),
        provider: options.chat.provider.clone(),
        model: options.chat.model.clone(),
        messages: runtime.chat_request_messages(agent_id, messages),
        temperature: options.chat.temperature,
        max_output_tokens: options.chat.max_output_tokens,
        tools: runtime.chat_tools(),
        metadata: json!({
            "source": "agent_tui",
            "surface": "agent_tui",
            "mode": "natural_language",
        }),
        context_policy: options.context_policy.clone(),
        max_tool_rounds: options.chat.max_tool_rounds,
        tool_execution: ChatToolExecution::Client,
    }
}

#[derive(Debug, Clone)]
pub(super) struct PendingChatResume {
    pub(super) state: ChatTurnState,
    pub(super) tool_calls: Vec<ChatToolCall>,
}

async fn run_chat_client_loop<Emit>(
    runtime: &TuiRuntime,
    runner: &ChatTurnRunner,
    agent_id: &str,
    surface_messages: Vec<LlmMessage>,
    mut stream: ChatEventStream,
    cancellation: CancellationToken,
    mut emit: Emit,
) -> Result<()>
where
    Emit: FnMut(TuiUpdate),
{
    let mut assistant_text = String::new();
    let mut final_response = None;

    loop {
        let mut cancelled = false;
        let mut pending_resume = None;
        while let Some(event) = stream.next().await {
            let event = event.map_err(|err| miette!(err.record.message))?;
            if is_cancelled_chat_event(&event) {
                cancelled = true;
            }
            for update in updates_from_chat_event(&event, &mut assistant_text, &mut final_response)
            {
                emit(update);
            }
            if let Some(resume) = pending_chat_resume_from_round_event(&event)? {
                pending_resume = Some(resume);
            }
        }

        if cancelled {
            emit(TuiUpdate::AssistantFinish);
            return Ok(());
        }

        let Some(pending_resume) = pending_resume else {
            finish_completed_chat(surface_messages, assistant_text, final_response, emit);
            return Ok(());
        };

        emit(TuiUpdate::ChatMessages(surface_messages.clone()));
        if let Some(approval) = pending_chat_approval(
            runtime,
            agent_id,
            pending_resume.state.clone(),
            pending_resume.tool_calls.clone(),
            surface_messages.clone(),
        ) {
            emit_chat_tool_policy(runtime, &pending_resume.tool_calls, &mut emit);
            emit(TuiUpdate::AssistantFinish);
            emit(TuiUpdate::PendingApproval(Some(approval.clone())));
            emit(TuiUpdate::Activity(TuiActivityItem::with_detail(
                TuiActivityKind::Approval,
                "approval required",
                approval.summary(),
            )));
            emit(TuiUpdate::SystemMessage(format!(
                "Approval required for high-risk chat tool '{}'.",
                approval.subject()
            )));
            return Ok(());
        }

        let tool_results = execute_chat_tool_calls(
            runtime,
            agent_id,
            &pending_resume.tool_calls,
            cancellation.clone(),
            &mut emit,
        )
        .await;
        stream = runner.resume_with_cancellation(
            ChatResumeRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                state: pending_resume.state,
                tool_results,
            },
            cancellation.clone(),
        );
    }
}

fn finish_completed_chat<Emit>(
    mut surface_messages: Vec<LlmMessage>,
    assistant_text: String,
    final_response: Option<agent_llm::LlmResponse>,
    mut emit: Emit,
) where
    Emit: FnMut(TuiUpdate),
{
    let assistant_content = final_response
        .as_ref()
        .map(|response| response.content.clone())
        .filter(|content| !content.is_empty())
        .unwrap_or(assistant_text);
    if !assistant_content.is_empty() {
        emit(TuiUpdate::AssistantReplace(assistant_content.clone()));
        surface_messages.push(LlmMessage {
            role: LlmRole::Assistant,
            content: Value::String(assistant_content),
            name: None,
            metadata: json!({}),
        });
    } else {
        emit(TuiUpdate::AssistantFinish);
    }
    emit(TuiUpdate::ChatMessages(surface_messages));
}

pub(super) fn pending_chat_resume_from_round_event(
    event: &ChatTurnEvent,
) -> Result<Option<PendingChatResume>> {
    if event.kind != ChatTurnEventKind::RoundFinished {
        return Ok(None);
    }
    if event.metadata.get("status").and_then(Value::as_str) != Some("requires_tool_results") {
        return Ok(None);
    }
    let state = event
        .metadata
        .get("chat_state")
        .cloned()
        .ok_or_else(|| miette!("chat round is missing resume state"))?;
    let tool_calls = event
        .metadata
        .get("tool_calls")
        .cloned()
        .ok_or_else(|| miette!("chat round is missing pending tool calls"))?;
    let state = serde_json::from_value::<ChatTurnState>(state)
        .map_err(|error| miette!("failed to decode chat resume state: {error}"))?;
    let tool_calls = serde_json::from_value::<Vec<ChatToolCall>>(tool_calls)
        .map_err(|error| miette!("failed to decode chat tool calls: {error}"))?;
    if tool_calls.is_empty() {
        return Ok(None);
    }
    Ok(Some(PendingChatResume { state, tool_calls }))
}

fn pending_chat_approval(
    runtime: &TuiRuntime,
    agent_id: &str,
    state: ChatTurnState,
    tool_calls: Vec<ChatToolCall>,
    surface_messages: Vec<LlmMessage>,
) -> Option<TuiPendingApproval> {
    let requires_approval = tool_calls.iter().any(|call| {
        let decision = runtime.tool_policy_decision(&call.name);
        decision.risk == TuiToolRisk::High && decision.allowed
    });
    requires_approval.then(|| {
        TuiPendingApproval::chat_tools(
            agent_id.to_owned(),
            TuiToolRisk::High,
            state,
            tool_calls,
            surface_messages,
        )
    })
}

async fn execute_chat_tool_calls<Emit>(
    runtime: &TuiRuntime,
    agent_id: &str,
    tool_calls: &[ChatToolCall],
    cancellation: CancellationToken,
    emit: &mut Emit,
) -> Vec<ChatToolResult>
where
    Emit: FnMut(TuiUpdate),
{
    let services = runtime.tool_services(Some(agent_id.to_owned()));
    let mut results = Vec::with_capacity(tool_calls.len());
    for call in tool_calls {
        let decision = runtime.tool_policy_decision(&call.name);
        emit(TuiUpdate::Activity(TuiActivityItem::with_detail(
            TuiActivityKind::Policy,
            "tool policy",
            format!(
                "{} risk={} allowed={}",
                call.name,
                decision.risk.label(),
                decision.allowed
            ),
        )));
        if !decision.allowed {
            results.push(ChatToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                output: json!({
                    "code": "policy_denied",
                    "message": format!("tool '{}' is blocked by the current TUI tool policy", call.name),
                    "retryable": false,
                    "details": {
                        "tool_name": call.name,
                        "risk": decision.risk.label(),
                        "surface": "agent_tui",
                    },
                }),
                is_error: true,
            });
            continue;
        }
        emit(TuiUpdate::Activity(TuiActivityItem::with_detail(
            TuiActivityKind::Tool,
            "tool executing",
            call.name.clone(),
        )));
        match services
            .call_tool_with_cancellation(
                &call.name,
                call.input.clone(),
                agent_cancellation(cancellation.clone()),
            )
            .await
        {
            Ok(output) => results.push(ChatToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                output,
                is_error: false,
            }),
            Err(error) => results.push(ChatToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                output: json!({
                    "code": error.record.code,
                    "message": error.record.message,
                    "retryable": error.record.retryable,
                    "details": error.record.details,
                }),
                is_error: true,
            }),
        }
    }
    results
}

fn emit_chat_tool_policy<Emit>(runtime: &TuiRuntime, tool_calls: &[ChatToolCall], emit: &mut Emit)
where
    Emit: FnMut(TuiUpdate),
{
    for call in tool_calls {
        let decision = runtime.tool_policy_decision(&call.name);
        emit(TuiUpdate::Activity(TuiActivityItem::with_detail(
            TuiActivityKind::Policy,
            "tool policy",
            format!(
                "{} risk={} allowed={}",
                call.name,
                decision.risk.label(),
                decision.allowed
            ),
        )));
    }
}

fn denied_chat_tool_results(tool_calls: &[ChatToolCall]) -> Vec<ChatToolResult> {
    tool_calls
        .iter()
        .map(|call| ChatToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            output: json!({
                "code": "approval_denied",
                "message": format!("user denied approval for tool '{}'", call.name),
                "retryable": false,
                "details": {
                    "tool_name": call.name,
                    "surface": "agent_tui",
                },
            }),
            is_error: true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{
        data::TranscriptRole,
        test_support::{test_options, test_state},
    };
    use agent_chat::{ChatTurnEvent, ChatTurnEventKind, chat_turn_initial_state};
    use agent_core::ContextPolicy;
    use agent_llm::{LlmFinishReason, user_message};
    use serde_json::json;

    #[tokio::test]
    async fn natural_language_task_streams_updates_to_tui_state() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = test_state(&dir, "background answer").await;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();

        let task = start_natural_language_task(&mut state, "Hello async".to_owned(), sender);
        assert!(state.busy);
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("Hello async"))
        );

        loop {
            let update = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
                .await
                .expect("update arrives")
                .expect("update exists");
            state.apply_update(update);
            if !state.busy {
                break;
            }
        }
        task.join.await.expect("task joins");

        assert_eq!(state.chat_messages.len(), 2);
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("background answer"))
        );
        let assistant_items = state
            .transcript
            .iter()
            .filter(|item| item.role == TranscriptRole::Assistant)
            .collect::<Vec<_>>();
        assert_eq!(assistant_items.len(), 1);
        assert_eq!(assistant_items[0].content, "background answer");
        assert!(
            crate::tui::render::render_tui_once(&state)
                .expect("tui renders")
                .contains("background answer")
        );
    }

    #[tokio::test]
    async fn chat_turn_request_uses_configured_context_policy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut options = test_options(&dir, "background answer", true);
        options.context_policy = ContextPolicy {
            max_input_tokens: 512,
            reserve_output_tokens: 64,
            preserve_recent_messages: 3,
            compact_when_over_budget: false,
        };
        let runtime = TuiRuntime::load(&options).await.expect("runtime loads");

        let request = chat_turn_request(
            &options,
            &runtime,
            "echo_agent",
            vec![user_message("hello")],
        );

        assert_eq!(request.context_policy, options.context_policy);
    }

    #[test]
    fn requires_tool_results_round_event_decodes_pending_chat_resume() {
        let tool_call = ChatToolCall {
            id: "call_1".to_owned(),
            name: "echo".to_owned(),
            input: json!({"message": "from chat approval"}),
        };
        let request = ChatTurnRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            turn_id: None,
            surface: Some("agent_tui".to_owned()),
            mode: Some("natural_language".to_owned()),
            session_id: None,
            thread_id: None,
            agent_id: Some("echo_agent".to_owned()),
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            messages: vec![user_message("run a tool")],
            temperature: None,
            max_output_tokens: None,
            tools: Vec::new(),
            metadata: json!({}),
            context_policy: Default::default(),
            max_tool_rounds: 4,
            tool_execution: ChatToolExecution::Client,
        };
        let mut chat_state = chat_turn_initial_state(&request).expect("initial chat state");
        chat_state.round = 1;
        chat_state.pending_tool_calls = vec![tool_call.clone()];
        let event = ChatTurnEvent {
            kind: ChatTurnEventKind::RoundFinished,
            content: None,
            response: Some(agent_llm::LlmResponse {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                provider: "mock".to_owned(),
                model: "mock-model".to_owned(),
                content: String::new(),
                finish_reason: LlmFinishReason::ToolCall,
                object: None,
                usage: None,
                metadata: json!({}),
            }),
            tool_call_id: None,
            tool_name: None,
            partial_input_json: None,
            tool_input: None,
            tool_output: None,
            usage: None,
            round: 1,
            metadata: json!({
                "status": "requires_tool_results",
                "chat_state": chat_state,
                "tool_calls": [tool_call],
            }),
        };

        let pending = pending_chat_resume_from_round_event(&event)
            .expect("resume metadata decodes")
            .expect("pending resume exists");

        assert_eq!(pending.tool_calls.len(), 1);
        assert_eq!(pending.tool_calls[0].id, "call_1");
        assert_eq!(pending.tool_calls[0].name, "echo");
        assert_eq!(pending.state.pending_tool_calls[0].id, "call_1");
        assert_eq!(pending.state.tool_execution, ChatToolExecution::Client);
    }
}
