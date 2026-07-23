use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use agent_chat::{ChatToolExecution, ChatTurnEventKind, ChatTurnRequest, ChatTurnRunner};
use agent_core::{
    AgentError, AgentEvent, AgentEventEmitter, AgentStateAccess, ArtifactPublisher,
    PROTOCOL_VERSION, ProposalCreator, SubagentRunner, ToolCaller, ToolError, ToolRisk, ToolSpec,
};
use agent_llm::{
    LlmError, LlmEvent, LlmEventKind, LlmEventStream, LlmFinishReason, LlmProvider, LlmRequest,
    LlmResponse, LlmUsage, user_message,
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde_json::{Value, json};

#[tokio::test]
async fn chat_runner_executes_multiround_tools_and_memory_as_independent_e2e() {
    let provider = Arc::new(ScriptedAgentProvider::default());
    let services = Arc::new(ChatCapabilityServices::default());
    let runner = ChatTurnRunner::new(provider.clone(), services.clone());

    let events = runner
        .stream(ChatTurnRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            turn_id: Some("turn_agent_capability_e2e".to_owned()),
            surface: Some("agent_runtime_e2e".to_owned()),
            mode: Some("long_task".to_owned()),
            session_id: Some("session_agent_capability_e2e".to_owned()),
            thread_id: Some("thread_agent_capability_e2e".to_owned()),
            agent_id: Some("runtime_capability_agent".to_owned()),
            provider: "scripted".to_owned(),
            model: "scripted-agent".to_owned(),
            messages: vec![user_message(
                "Plan a long task, remember the budget, then replan from memory.",
            )],
            temperature: Some(0.0),
            max_output_tokens: Some(512),
            tools: capability_tools(),
            context_blocks: vec![],
            metadata: json!({"case": "chat_agent_capabilities_e2e"}),
            context_policy: Default::default(),
            max_tool_rounds: 5,
            tool_execution: ChatToolExecution::Runtime,
        })
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("chat turn events are valid");

    assert_eq!(provider.request_count(), 3);
    assert_eq!(
        services.tool_names(),
        vec![
            "remember_fact",
            "create_task",
            "recall_memory",
            "record_progress",
        ]
    );
    assert_eq!(
        services.memory_value("budget").as_deref(),
        Some("LLM API budget is 80 USD")
    );

    let kinds = events
        .iter()
        .map(|event| event.kind.clone())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&ChatTurnEventKind::Started));
    assert!(kinds.contains(&ChatTurnEventKind::ContextSnapshot));
    assert!(kinds.contains(&ChatTurnEventKind::ToolCallStart));
    assert!(kinds.contains(&ChatTurnEventKind::ToolCallDelta));
    assert!(kinds.contains(&ChatTurnEventKind::ToolCallEnd));
    assert!(kinds.contains(&ChatTurnEventKind::ToolResult));
    assert!(kinds.contains(&ChatTurnEventKind::Usage));
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == ChatTurnEventKind::RoundFinished)
            .count(),
        3
    );
    assert_eq!(
        events.last().expect("done event").kind,
        ChatTurnEventKind::Done
    );
    assert_eq!(
        events.last().expect("done event").metadata["stop_reason"],
        "end_turn"
    );
    assert_eq!(events.last().expect("done event").round, 3);

    let final_text = events
        .iter()
        .filter(|event| event.kind == ChatTurnEventKind::Delta)
        .filter_map(|event| event.content.as_deref())
        .collect::<String>();
    assert!(final_text.contains("80 USD"));
    assert!(final_text.contains("replanned"));

    let requests = provider.requests();
    assert_eq!(requests[0].metadata["chat_turn"], true);
    assert_eq!(requests[0].metadata["turn_id"], "turn_agent_capability_e2e");
    assert_eq!(requests[0].temperature, Some(0.0));
    assert_eq!(requests[0].max_output_tokens, Some(512));
    assert!(tool_result_ids(&requests[1]).contains(&"remember_budget"));
    assert!(tool_result_ids(&requests[1]).contains(&"create_validation_task"));
    assert!(tool_result_ids(&requests[2]).contains(&"recall_budget"));
    assert!(tool_result_ids(&requests[2]).contains(&"record_day7_progress"));
}

#[derive(Default)]
struct ScriptedAgentProvider {
    calls: AtomicUsize,
    requests: Mutex<Vec<LlmRequest>>,
}

impl ScriptedAgentProvider {
    fn request_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

#[async_trait]
impl LlmProvider for ScriptedAgentProvider {
    async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
        unreachable!("E2E uses streaming")
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmEventStream, LlmError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .expect("requests lock")
            .push(request.clone());
        let provider = request.provider.clone();
        let model = request.model.clone();
        let events = match call {
            0 => vec![
                started(),
                tool_start("remember_budget", "remember_fact"),
                tool_delta("remember_budget", r#"{"key":"budget""#),
                tool_end(
                    "remember_budget",
                    "remember_fact",
                    json!({
                        "key": "budget",
                        "fact": "LLM API budget is 80 USD",
                    }),
                ),
                tool_start("create_validation_task", "create_task"),
                tool_end(
                    "create_validation_task",
                    "create_task",
                    json!({
                        "title": "Validate runtime loop, tools, memory, and replanning",
                        "due_day": 10,
                    }),
                ),
                finished(tool_response(provider, model, 8, 17)),
            ],
            1 => vec![
                started(),
                tool_start("recall_budget", "recall_memory"),
                tool_end("recall_budget", "recall_memory", json!({"key": "budget"})),
                tool_start("record_day7_progress", "record_progress"),
                tool_end(
                    "record_day7_progress",
                    "record_progress",
                    json!({
                        "phase": "day7",
                        "status": "replanning",
                    }),
                ),
                finished(tool_response(provider, model, 13, 21)),
            ],
            _ => vec![
                started(),
                Ok(LlmEvent {
                    kind: LlmEventKind::Delta,
                    content: Some(
                        "The plan remembered the 80 USD budget and replanned day7 work.".to_owned(),
                    ),
                    response: None,
                    tool_call_id: None,
                    tool_name: None,
                    partial_input_json: None,
                    tool_input: None,
                    metadata: json!({}),
                }),
                finished(stop_response(provider, model, 10, 12)),
            ],
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

#[derive(Default)]
struct ChatCapabilityServices {
    tool_calls: Mutex<Vec<String>>,
    memory: Mutex<HashMap<String, String>>,
    tasks: Mutex<Vec<Value>>,
    progress: Mutex<Vec<Value>>,
}

impl ChatCapabilityServices {
    fn tool_names(&self) -> Vec<String> {
        self.tool_calls.lock().expect("tool calls lock").clone()
    }

    fn memory_value(&self, key: &str) -> Option<String> {
        self.memory.lock().expect("memory lock").get(key).cloned()
    }
}

#[async_trait]
impl ToolCaller for ChatCapabilityServices {
    async fn call_tool(&self, name: &str, input: Value) -> Result<Value, ToolError> {
        self.tool_calls
            .lock()
            .expect("tool calls lock")
            .push(name.to_owned());
        match name {
            "remember_fact" => {
                let key = input
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| tool_error("remember_fact requires key"))?;
                let fact = input
                    .get("fact")
                    .and_then(Value::as_str)
                    .ok_or_else(|| tool_error("remember_fact requires fact"))?;
                self.memory
                    .lock()
                    .expect("memory lock")
                    .insert(key.to_owned(), fact.to_owned());
                Ok(json!({"remembered": true, "key": key}))
            }
            "create_task" => {
                self.tasks.lock().expect("tasks lock").push(input.clone());
                Ok(json!({"task_id": "task_runtime_e2e", "input": input}))
            }
            "recall_memory" => {
                let key = input
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| tool_error("recall_memory requires key"))?;
                let value = self.memory.lock().expect("memory lock").get(key).cloned();
                Ok(json!({"key": key, "value": value}))
            }
            "record_progress" => {
                self.progress
                    .lock()
                    .expect("progress lock")
                    .push(input.clone());
                Ok(json!({"recorded": true, "input": input}))
            }
            other => Err(tool_error(format!("unknown tool '{other}'"))),
        }
    }
}

#[async_trait]
impl AgentEventEmitter for ChatCapabilityServices {
    async fn emit_event(&self, _event: AgentEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait]
impl AgentStateAccess for ChatCapabilityServices {
    async fn load_state(&self, _key: &str) -> Result<Option<Value>, AgentError> {
        Ok(None)
    }

    async fn save_state(&self, _key: &str, _value: Value) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait]
impl ProposalCreator for ChatCapabilityServices {}

#[async_trait]
impl SubagentRunner for ChatCapabilityServices {}

#[async_trait]
impl ArtifactPublisher for ChatCapabilityServices {}

fn capability_tools() -> Vec<ToolSpec> {
    vec![
        tool("remember_fact", ToolRisk::Low),
        tool("create_task", ToolRisk::Low),
        tool("recall_memory", ToolRisk::ReadOnly),
        tool("record_progress", ToolRisk::Low),
    ]
}

fn tool(name: &str, risk: ToolRisk) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: format!("Synthetic {name} tool for chat agent capability E2E"),
        input_schema: json!({"type": "object"}),
        output_schema: None,
        replay_policy: if risk == ToolRisk::ReadOnly {
            agent_core::ToolReplayPolicy::SafeRetry
        } else {
            agent_core::ToolReplayPolicy::AtMostOnce
        },
        risk,
        metadata: json!({"test_only": true}),
    }
}

fn started() -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent {
        kind: LlmEventKind::Started,
        content: None,
        response: None,
        tool_call_id: None,
        tool_name: None,
        partial_input_json: None,
        tool_input: None,
        metadata: json!({}),
    })
}

fn tool_start(id: &str, name: &str) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent {
        kind: LlmEventKind::ToolCallStart,
        content: None,
        response: None,
        tool_call_id: Some(id.to_owned()),
        tool_name: Some(name.to_owned()),
        partial_input_json: None,
        tool_input: None,
        metadata: json!({}),
    })
}

fn tool_delta(id: &str, partial_input_json: &str) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent {
        kind: LlmEventKind::ToolCallDelta,
        content: None,
        response: None,
        tool_call_id: Some(id.to_owned()),
        tool_name: None,
        partial_input_json: Some(partial_input_json.to_owned()),
        tool_input: None,
        metadata: json!({}),
    })
}

fn tool_end(id: &str, name: &str, input: Value) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent {
        kind: LlmEventKind::ToolCallEnd,
        content: None,
        response: None,
        tool_call_id: Some(id.to_owned()),
        tool_name: Some(name.to_owned()),
        partial_input_json: None,
        tool_input: Some(input),
        metadata: json!({}),
    })
}

fn finished(response: LlmResponse) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent {
        kind: LlmEventKind::Finished,
        content: None,
        response: Some(response),
        tool_call_id: None,
        tool_name: None,
        partial_input_json: None,
        tool_input: None,
        metadata: json!({}),
    })
}

fn tool_response(provider: String, model: String, input: u32, output: u32) -> LlmResponse {
    response(
        provider,
        model,
        LlmFinishReason::ToolCall,
        "",
        input,
        output,
    )
}

fn stop_response(provider: String, model: String, input: u32, output: u32) -> LlmResponse {
    response(
        provider,
        model,
        LlmFinishReason::Stop,
        "The plan remembered the 80 USD budget and replanned day7 work.",
        input,
        output,
    )
}

fn response(
    provider: String,
    model: String,
    finish_reason: LlmFinishReason,
    content: &str,
    input_tokens: u32,
    output_tokens: u32,
) -> LlmResponse {
    LlmResponse {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        provider,
        model,
        content: content.to_owned(),
        finish_reason,
        object: None,
        usage: Some(LlmUsage {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
        }),
        metadata: json!({}),
    }
}

fn tool_error(message: impl Into<String>) -> ToolError {
    ToolError {
        record: AgentError::validation(message).record,
    }
}

fn tool_result_ids(request: &LlmRequest) -> Vec<&str> {
    request
        .messages
        .iter()
        .filter_map(|message| message.content.as_array())
        .flat_map(|blocks| blocks.iter())
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|block| block.get("tool_use_id").and_then(Value::as_str))
        .collect()
}
