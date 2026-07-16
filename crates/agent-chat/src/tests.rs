use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use agent_core::{
    AgentError, AgentEvent, AgentEventEmitter, AgentStateAccess, ArtifactPublisher, ContextPolicy,
    PROTOCOL_VERSION, ProposalCreator, SubagentRunner, ToolCaller, ToolError, TraceEvent,
};
use agent_llm::{
    LlmError, LlmEvent, LlmEventKind, LlmEventStream, LlmFinishReason, LlmMessage, LlmProvider,
    LlmRequest, LlmResponse, LlmRole, LlmUsage, MockLlmProvider, user_message,
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;

mod fixtures;
mod runner;
mod state;

#[test]
fn chat_request_requires_explicit_protocol_version() {
    let error = serde_json::from_value::<ChatTurnRequest>(json!({
        "provider": "mock",
        "model": "mock-model",
        "messages": []
    }))
    .expect_err("missing protocol version is rejected");
    assert!(error.to_string().contains("protocol_version"));
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

struct PendingStreamProvider;

#[async_trait]
impl LlmProvider for PendingStreamProvider {
    async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
        Err(LlmError::validation("pending provider does not complete"))
    }

    async fn stream(&self, _request: LlmRequest) -> Result<LlmEventStream, LlmError> {
        std::future::pending::<Result<LlmEventStream, LlmError>>().await
    }
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
impl ToolCaller for TestServices {
    async fn call_tool(&self, name: &str, input: Value) -> Result<Value, ToolError> {
        Ok(json!({"tool": name, "input": input}))
    }
}

#[async_trait]
impl AgentEventEmitter for TestServices {
    async fn emit_event(&self, _event: AgentEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait]
impl AgentStateAccess for TestServices {
    async fn load_state(&self, _key: &str) -> Result<Option<Value>, AgentError> {
        Ok(None)
    }

    async fn save_state(&self, _key: &str, _value: Value) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait]
impl ProposalCreator for TestServices {}

#[async_trait]
impl SubagentRunner for TestServices {}

#[async_trait]
impl ArtifactPublisher for TestServices {}

#[async_trait]
impl agent_core::TraceSink for TestServices {
    async fn emit(&self, _event: TraceEvent) -> Result<(), AgentError> {
        Ok(())
    }
}
