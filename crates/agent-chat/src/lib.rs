mod error;
mod events;
mod runner;
mod state;
mod types;

pub use error::{ChatError, ChatErrorRecord};
pub use runner::ChatTurnRunner;
pub use state::{
    chat_turn_apply_response, chat_turn_apply_tool_results, chat_turn_initial_state,
    chat_turn_llm_request, chat_turn_next_round,
};
pub use types::{
    ChatEventStream, ChatToolCall, ChatToolResult, ChatTurnAdvance, ChatTurnEvent,
    ChatTurnEventKind, ChatTurnRequest, ChatTurnState,
};

pub(crate) use events::{
    chat_event_from_llm_event, send_done, send_error, send_event, turn_metadata,
};
pub(crate) use state::ToolOutput;

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use agent_core::{
        AgentError, AgentEvent, AgentServices, PROTOCOL_VERSION, ToolError, TraceEvent,
    };
    use agent_llm::{
        LlmError, LlmEvent, LlmEventKind, LlmEventStream, LlmFinishReason, LlmProvider, LlmRequest,
        LlmResponse, LlmRole, LlmUsage, MockLlmProvider, user_message,
    };
    use async_trait::async_trait;
    use futures::{StreamExt, stream};
    use serde_json::{Value, json};

    use super::*;

    #[tokio::test]
    async fn mock_chat_turn_streams_text_and_done() {
        let runner = ChatTurnRunner::new(
            Arc::new(MockLlmProvider::new("mock", "mock-model", "hello")),
            Arc::new(TestServices),
        );
        let events = runner
            .stream(ChatTurnRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                turn_id: Some("turn_1".to_owned()),
                surface: None,
                mode: None,
                session_id: None,
                thread_id: None,
                agent_id: Some("chat".to_owned()),
                provider: "mock".to_owned(),
                model: "mock-model".to_owned(),
                messages: vec![user_message("ping")],
                temperature: None,
                max_output_tokens: None,
                tools: vec![],
                metadata: json!({}),
                max_tool_rounds: 4,
            })
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("events ok");

        assert!(
            events
                .iter()
                .any(|event| event.kind == ChatTurnEventKind::Delta)
        );
        assert!(
            events
                .iter()
                .any(|event| event.kind == ChatTurnEventKind::Done)
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == ChatTurnEventKind::RoundFinished)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn chat_turn_executes_tools_and_continues() {
        let provider = Arc::new(ScriptedToolProvider {
            calls: AtomicUsize::new(0),
        });
        let runner = ChatTurnRunner::new(provider, Arc::new(TestServices));
        let events = runner
            .stream(ChatTurnRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                turn_id: None,
                surface: None,
                mode: None,
                session_id: None,
                thread_id: None,
                agent_id: Some("chat".to_owned()),
                provider: "scripted".to_owned(),
                model: "scripted-model".to_owned(),
                messages: vec![user_message("use a tool")],
                temperature: None,
                max_output_tokens: None,
                tools: vec![],
                metadata: json!({}),
                max_tool_rounds: 4,
            })
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("events ok");

        assert!(
            events
                .iter()
                .any(|event| event.kind == ChatTurnEventKind::ToolResult)
        );
        assert!(
            events
                .iter()
                .any(|event| event.content.as_deref() == Some("done"))
        );
    }

    #[tokio::test]
    async fn chat_turn_forwards_turn_metadata_to_llm_request() {
        let provider = Arc::new(MetadataProvider {
            metadata: Mutex::new(None),
        });
        let runner = ChatTurnRunner::new(provider.clone(), Arc::new(TestServices));
        runner
            .stream(ChatTurnRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                turn_id: Some("turn_1".to_owned()),
                surface: Some("agent_tui".to_owned()),
                mode: Some("natural_language".to_owned()),
                session_id: Some("session_1".to_owned()),
                thread_id: Some("thread_1".to_owned()),
                agent_id: Some("chat".to_owned()),
                provider: "metadata".to_owned(),
                model: "metadata-model".to_owned(),
                messages: vec![user_message("ping")],
                temperature: None,
                max_output_tokens: None,
                tools: vec![],
                metadata: json!({"source": "test"}),
                max_tool_rounds: 4,
            })
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("events ok");

        let metadata = provider
            .metadata
            .lock()
            .expect("metadata lock")
            .clone()
            .expect("metadata captured");
        assert_eq!(metadata["source"], "test");
        assert_eq!(metadata["chat_turn"], true);
        assert_eq!(metadata["turn_id"], "turn_1");
        assert_eq!(metadata["surface"], "agent_tui");
        assert_eq!(metadata["mode"], "natural_language");
        assert_eq!(metadata["session_id"], "session_1");
        assert_eq!(metadata["thread_id"], "thread_1");
        assert_eq!(metadata["agent_id"], "chat");
    }

    #[test]
    fn chat_turn_state_applies_tool_results_for_resume() {
        let state = chat_turn_initial_state(&ChatTurnRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            turn_id: Some("turn_1".to_owned()),
            surface: Some("agent_tui".to_owned()),
            mode: Some("natural_language".to_owned()),
            session_id: None,
            thread_id: None,
            agent_id: Some("chat".to_owned()),
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            messages: vec![user_message("use a tool")],
            temperature: None,
            max_output_tokens: None,
            tools: vec![],
            metadata: json!({}),
            max_tool_rounds: 4,
        })
        .expect("initial state");
        let response = LlmResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            content: String::new(),
            finish_reason: LlmFinishReason::ToolCall,
            object: None,
            usage: None,
            metadata: json!({}),
        };
        let advance = chat_turn_apply_response(
            state,
            "",
            vec![ChatToolCall {
                id: "call_1".to_owned(),
                name: "echo".to_owned(),
                input: json!({"value": "ok"}),
            }],
            &response,
        )
        .expect("requires tools");
        let pending = match advance {
            ChatTurnAdvance::RequiresToolResults { state, tool_calls } => {
                assert_eq!(tool_calls[0].id, "call_1");
                state
            }
            ChatTurnAdvance::Completed { .. } => panic!("expected tool results"),
        };

        let resumed = chat_turn_apply_tool_results(
            pending,
            vec![ChatToolResult {
                tool_call_id: "call_1".to_owned(),
                tool_name: "echo".to_owned(),
                output: json!({"value": "ok"}),
                is_error: false,
            }],
        )
        .expect("resume applies");

        assert_eq!(resumed.round, 1);
        assert!(resumed.pending_tool_calls.is_empty());
        assert_eq!(resumed.messages.len(), 3);
        assert_eq!(resumed.messages[1].role, LlmRole::Assistant);
        assert_eq!(resumed.messages[2].role, LlmRole::User);
        assert_eq!(
            resumed.messages[2].content[0]["tool_use_id"],
            Value::String("call_1".to_owned())
        );
    }

    #[test]
    fn committed_chat_turn_fixtures_match_runtime_types() {
        let request: ChatTurnRequest = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/chat-turn-request.valid.json"
        ))
        .expect("request fixture");
        let state = chat_turn_initial_state(&request).expect("request creates state");
        assert_eq!(state.provider, "mock");
        assert_eq!(state.max_tool_rounds, 4);

        let pending_state: ChatTurnState = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/chat-turn-state.requires-tool-results.valid.json"
        ))
        .expect("state fixture");
        let result: ChatToolResult = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/chat-tool-result.valid.json"
        ))
        .expect("tool result fixture");
        let resumed =
            chat_turn_apply_tool_results(pending_state, vec![result]).expect("resume fixture");
        assert_eq!(resumed.messages.len(), 3);
        assert!(resumed.pending_tool_calls.is_empty());

        let event: ChatTurnEvent = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/chat-turn-event.round-finished.requires-tool-results.valid.json"
        ))
        .expect("event fixture");
        assert_eq!(event.kind, ChatTurnEventKind::RoundFinished);
        assert_eq!(event.metadata["status"], "requires_tool_results");
    }

    #[test]
    fn shared_agent_chat_turn_event_fixture_matches_runtime_types() {
        let events: Vec<ChatTurnEvent> = serde_json::from_str(include_str!(
            "../../../fixtures/docs/agent_chat_turn_events.json"
        ))
        .expect("shared chat turn events fixture");

        assert_eq!(
            events
                .iter()
                .map(|event| event.kind.clone())
                .collect::<Vec<_>>(),
            vec![
                ChatTurnEventKind::Started,
                ChatTurnEventKind::Delta,
                ChatTurnEventKind::ToolCallStart,
                ChatTurnEventKind::ToolCallDelta,
                ChatTurnEventKind::ToolCallEnd,
                ChatTurnEventKind::Usage,
                ChatTurnEventKind::RoundFinished,
            ]
        );
        assert_eq!(events[2].tool_name.as_deref(), Some("get_holdings"));
        assert_eq!(
            events[4].tool_input.as_ref(),
            Some(&json!({"as_of": "today"}))
        );
        assert_eq!(events[5].usage.as_ref().expect("usage").total_tokens, 18);
        assert_eq!(events[6].metadata["status"], "requires_tool_results");
    }

    struct ScriptedToolProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmProvider for ScriptedToolProvider {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
            unreachable!("test uses stream")
        }

        async fn stream(&self, request: LlmRequest) -> Result<LlmEventStream, LlmError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let response = if call == 0 {
                LlmResponse {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    provider: request.provider,
                    model: request.model,
                    content: String::new(),
                    finish_reason: LlmFinishReason::ToolCall,
                    object: None,
                    usage: Some(LlmUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                        total_tokens: 2,
                    }),
                    metadata: json!({}),
                }
            } else {
                LlmResponse {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    provider: request.provider,
                    model: request.model,
                    content: "done".to_owned(),
                    finish_reason: LlmFinishReason::Stop,
                    object: None,
                    usage: None,
                    metadata: json!({}),
                }
            };
            let events = if call == 0 {
                vec![
                    Ok(LlmEvent {
                        kind: LlmEventKind::Started,
                        content: None,
                        response: None,
                        tool_call_id: None,
                        tool_name: None,
                        partial_input_json: None,
                        tool_input: None,
                        metadata: json!({}),
                    }),
                    Ok(LlmEvent {
                        kind: LlmEventKind::ToolCallStart,
                        content: None,
                        response: None,
                        tool_call_id: Some("call_1".to_owned()),
                        tool_name: Some("echo".to_owned()),
                        partial_input_json: None,
                        tool_input: None,
                        metadata: json!({}),
                    }),
                    Ok(LlmEvent {
                        kind: LlmEventKind::ToolCallEnd,
                        content: None,
                        response: None,
                        tool_call_id: Some("call_1".to_owned()),
                        tool_name: Some("echo".to_owned()),
                        partial_input_json: None,
                        tool_input: Some(json!({"value": "ok"})),
                        metadata: json!({}),
                    }),
                    Ok(LlmEvent {
                        kind: LlmEventKind::Finished,
                        content: None,
                        response: Some(response),
                        tool_call_id: None,
                        tool_name: None,
                        partial_input_json: None,
                        tool_input: None,
                        metadata: json!({}),
                    }),
                ]
            } else {
                vec![
                    Ok(LlmEvent {
                        kind: LlmEventKind::Started,
                        content: None,
                        response: None,
                        tool_call_id: None,
                        tool_name: None,
                        partial_input_json: None,
                        tool_input: None,
                        metadata: json!({}),
                    }),
                    Ok(LlmEvent {
                        kind: LlmEventKind::Delta,
                        content: Some("done".to_owned()),
                        response: None,
                        tool_call_id: None,
                        tool_name: None,
                        partial_input_json: None,
                        tool_input: None,
                        metadata: json!({}),
                    }),
                    Ok(LlmEvent {
                        kind: LlmEventKind::Finished,
                        content: None,
                        response: Some(response),
                        tool_call_id: None,
                        tool_name: None,
                        partial_input_json: None,
                        tool_input: None,
                        metadata: json!({}),
                    }),
                ]
            };
            Ok(Box::pin(stream::iter(events)))
        }
    }

    struct MetadataProvider {
        metadata: Mutex<Option<Value>>,
    }

    #[async_trait]
    impl LlmProvider for MetadataProvider {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
            unreachable!("test uses stream")
        }

        async fn stream(&self, request: LlmRequest) -> Result<LlmEventStream, LlmError> {
            *self.metadata.lock().expect("metadata lock") = Some(request.metadata.clone());
            let response = LlmResponse {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                provider: request.provider,
                model: request.model,
                content: "done".to_owned(),
                finish_reason: LlmFinishReason::Stop,
                object: None,
                usage: None,
                metadata: json!({}),
            };
            Ok(Box::pin(stream::iter(vec![
                Ok(LlmEvent {
                    kind: LlmEventKind::Started,
                    content: None,
                    response: None,
                    tool_call_id: None,
                    tool_name: None,
                    partial_input_json: None,
                    tool_input: None,
                    metadata: json!({}),
                }),
                Ok(LlmEvent {
                    kind: LlmEventKind::Delta,
                    content: Some("done".to_owned()),
                    response: None,
                    tool_call_id: None,
                    tool_name: None,
                    partial_input_json: None,
                    tool_input: None,
                    metadata: json!({}),
                }),
                Ok(LlmEvent {
                    kind: LlmEventKind::Finished,
                    content: None,
                    response: Some(response),
                    tool_call_id: None,
                    tool_name: None,
                    partial_input_json: None,
                    tool_input: None,
                    metadata: json!({}),
                }),
            ])))
        }
    }

    struct TestServices;

    #[async_trait]
    impl AgentServices for TestServices {
        async fn call_tool(&self, name: &str, input: Value) -> Result<Value, ToolError> {
            Ok(json!({"tool": name, "input": input}))
        }

        async fn emit_event(&self, _event: AgentEvent) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load_state(&self, _key: &str) -> Result<Option<Value>, AgentError> {
            Ok(None)
        }

        async fn save_state(&self, _key: &str, _value: Value) -> Result<(), AgentError> {
            Ok(())
        }
    }

    #[async_trait]
    impl agent_core::TraceSink for TestServices {
        async fn emit(&self, _event: TraceEvent) -> Result<(), AgentError> {
            Ok(())
        }
    }
}
