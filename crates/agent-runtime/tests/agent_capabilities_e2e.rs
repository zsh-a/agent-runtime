use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use agent_core::{
    Agent, AgentContext, AgentError, AgentEvent, AgentEventEmitter, AgentRunResult, AgentRunStatus,
    AgentSpec, AgentStateAccess, ArtifactPublisher, PROTOCOL_VERSION, ProposalCreator, RunRequest,
    ScheduleSpec, SubagentRequest, SubagentRunner, ToolCaller, ToolError, TriggerKind,
};
use agent_runtime::{AgentRunner, InMemoryAgentRegistry};
use async_trait::async_trait;
use serde_json::{Value, json};

#[tokio::test]
async fn agent_runner_executes_tools_state_and_subagent_as_independent_e2e() {
    let services = Arc::new(RuntimeE2eServices::default());
    let runner = AgentRunner::new(
        InMemoryAgentRegistry::shared(vec![
            Arc::new(RuntimePlannerAgent),
            Arc::new(RuntimeReviewerAgent),
        ]),
        agent_store::InMemoryRunStore::shared(),
        services.clone(),
    );

    let day1 = runner
        .run_once(
            "runtime_planner",
            run_request(json!({
                "phase": "day1",
                "constraint": "Reuse the shared run loop and keep the API host-neutral.",
                "topics": ["run_loop", "tools", "subagent", "memory"]
            })),
        )
        .await
        .expect("day1 run succeeds");

    assert_eq!(day1.result.status, AgentRunStatus::Completed);
    assert_eq!(day1.result.output["phase"], "day1");
    assert_eq!(day1.result.output["had_previous_memory"], false);
    assert_eq!(day1.result.output["subagent_status"], "completed");
    assert_eq!(day1.result.output["review_count"], 1);

    let day7 = runner
        .run_once(
            "runtime_planner",
            run_request(json!({
                "phase": "day7",
                "constraint": "Preserve day1 memory while replanning around a blocked subagent task.",
                "topics": ["replan", "state", "trace"]
            })),
        )
        .await
        .expect("day7 run succeeds");

    assert_eq!(day7.result.status, AgentRunStatus::Completed);
    assert_eq!(day7.result.output["phase"], "day7");
    assert_eq!(day7.result.output["had_previous_memory"], true);
    assert_eq!(day7.result.output["review_saw_previous_memory"], true);
    assert_eq!(day7.result.output["review_count"], 2);

    assert_eq!(
        services.tool_names(),
        vec![
            "research_context",
            "review_plan",
            "record_progress",
            "research_context",
            "review_plan",
            "record_progress",
        ]
    );

    let memory = services
        .state_value("project_memory")
        .expect("project memory was saved");
    assert_eq!(memory["phase"], "day7");
    assert_eq!(memory["previous_memory"]["phase"], "day1");
    assert_eq!(
        memory["constraint"],
        "Preserve day1 memory while replanning around a blocked subagent task."
    );

    let parent_trace_kinds = trace_event_kinds(&day7.trace.events);
    assert!(parent_trace_kinds.contains(&"run_started"));
    assert!(parent_trace_kinds.contains(&"state_read"));
    assert!(parent_trace_kinds.contains(&"tool_call"));
    assert!(parent_trace_kinds.contains(&"subagent_started"));
    assert!(parent_trace_kinds.contains(&"subagent_finished"));
    assert!(parent_trace_kinds.contains(&"state_write"));
    assert!(parent_trace_kinds.contains(&"agent_capability_checkpoint"));
    assert!(parent_trace_kinds.contains(&"run_finished"));

    let parent_span_names = day7
        .trace
        .spans
        .iter()
        .map(|span| span.name.as_str())
        .collect::<Vec<_>>();
    assert!(parent_span_names.contains(&"agent.run"));
    assert!(parent_span_names.contains(&"tool.research_context"));
    assert!(parent_span_names.contains(&"tool.record_progress"));
    assert!(parent_span_names.contains(&"state.read"));
    assert!(parent_span_names.contains(&"state.write"));

    let child_trace = memory["review"]["trace"]["events"]
        .as_array()
        .expect("subagent trace events are returned");
    let child_trace_kinds = child_trace
        .iter()
        .filter_map(|event| event.get("kind").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(child_trace_kinds.contains(&"state_read"));
    assert!(child_trace_kinds.contains(&"tool_call"));
    assert!(child_trace_kinds.contains(&"state_write"));
    assert!(child_trace_kinds.contains(&"run_finished"));
}

struct RuntimePlannerAgent;

#[async_trait]
impl Agent for RuntimePlannerAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "runtime_planner".to_owned(),
            name: "Runtime Planner".to_owned(),
            description: Some("Synthetic long-task planner for runtime E2E coverage".to_owned()),
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec![
                "runtime.tools".to_owned(),
                "runtime.state".to_owned(),
                "runtime.subagent".to_owned(),
            ],
            metadata: json!({"test_only": true}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let phase = json_string(&ctx.input, "phase", "unknown");
        let previous_memory = ctx.services.load_state("project_memory").await?;
        let had_previous_memory = previous_memory.is_some();
        let previous_memory_for_review = previous_memory.clone();
        let research = ctx
            .services
            .call_tool(
                "research_context",
                json!({
                    "phase": phase,
                    "topics": ctx.input.get("topics").cloned().unwrap_or_else(|| json!([])),
                }),
            )
            .await
            .map_err(agent_error_from_tool)?;
        let review = ctx
            .services
            .run_subagent(SubagentRequest {
                agent_id: "runtime_reviewer".to_owned(),
                input: json!({
                    "phase": phase,
                    "research": research,
                    "previous_memory": previous_memory_for_review,
                }),
                run_id: None,
                scope: None,
                workflow: None,
                metadata: json!({"purpose": "independent_runtime_e2e"}),
            })
            .await
            .map_err(agent_error_from_tool)?;
        let progress = ctx
            .services
            .call_tool(
                "record_progress",
                json!({
                    "phase": phase,
                    "subagent_status": review["result"]["status"].clone(),
                    "had_previous_memory": had_previous_memory,
                }),
            )
            .await
            .map_err(agent_error_from_tool)?;

        let memory = json!({
            "phase": phase,
            "constraint": ctx.input.get("constraint").cloned().unwrap_or(Value::Null),
            "had_previous_memory": had_previous_memory,
            "previous_memory": previous_memory,
            "research": research,
            "review": review,
            "progress": progress,
        });
        ctx.services
            .save_state("project_memory", memory.clone())
            .await?;
        ctx.services
            .emit_event(AgentEvent {
                kind: "agent_capability_checkpoint".to_owned(),
                occurred_at: ctx.now,
                payload: json!({
                    "phase": phase,
                    "memory_written": true,
                    "subagent_status": memory["review"]["result"]["status"].clone(),
                }),
            })
            .await?;

        Ok(AgentRunResult::completed(
            ctx.run_id,
            "runtime_planner",
            ctx.now,
            json!({
                "phase": phase,
                "had_previous_memory": had_previous_memory,
                "subagent_status": memory["review"]["result"]["status"].clone(),
                "review_count": memory["review"]["result"]["output"]["review_count"].clone(),
                "review_saw_previous_memory": memory["review"]["result"]["output"]["saw_previous_memory"].clone(),
            }),
            Some("runtime planner completed".to_owned()),
        ))
    }
}

struct RuntimeReviewerAgent;

#[async_trait]
impl Agent for RuntimeReviewerAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "runtime_reviewer".to_owned(),
            name: "Runtime Reviewer".to_owned(),
            description: Some("Synthetic subagent for runtime E2E coverage".to_owned()),
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["runtime.review".to_owned()],
            metadata: json!({"test_only": true}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let previous_count = ctx
            .services
            .load_state("review_count")
            .await?
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let review_count = previous_count + 1;
        let review = ctx
            .services
            .call_tool(
                "review_plan",
                json!({
                    "phase": ctx.input.get("phase").cloned().unwrap_or(Value::Null),
                    "research": ctx.input.get("research").cloned().unwrap_or(Value::Null),
                }),
            )
            .await
            .map_err(agent_error_from_tool)?;
        ctx.services
            .save_state("review_count", json!(review_count))
            .await?;

        Ok(AgentRunResult::completed(
            ctx.run_id,
            "runtime_reviewer",
            ctx.now,
            json!({
                "approved": review["approved"].clone(),
                "review_count": review_count,
                "saw_previous_memory": !ctx
                    .input
                    .get("previous_memory")
                    .is_none_or(Value::is_null),
            }),
            Some("runtime reviewer completed".to_owned()),
        ))
    }
}

#[derive(Default)]
struct RuntimeE2eServices {
    tool_calls: Mutex<Vec<String>>,
    state: Mutex<HashMap<String, Value>>,
    events: Mutex<Vec<AgentEvent>>,
}

impl RuntimeE2eServices {
    fn tool_names(&self) -> Vec<&'static str> {
        self.tool_calls
            .lock()
            .expect("tool calls lock")
            .iter()
            .map(|name| match name.as_str() {
                "research_context" => "research_context",
                "review_plan" => "review_plan",
                "record_progress" => "record_progress",
                _ => "unknown",
            })
            .collect()
    }

    fn state_value(&self, key: &str) -> Option<Value> {
        self.state.lock().expect("state lock").get(key).cloned()
    }
}

#[async_trait]
impl ToolCaller for RuntimeE2eServices {
    async fn call_tool(&self, name: &str, input: Value) -> Result<Value, ToolError> {
        self.tool_calls
            .lock()
            .expect("tool calls lock")
            .push(name.to_owned());
        match name {
            "research_context" => Ok(json!({
                "finding": "The shared run loop can orchestrate host tools and subagents.",
                "phase": input.get("phase").cloned().unwrap_or(Value::Null),
                "topics": input.get("topics").cloned().unwrap_or_else(|| json!([])),
            })),
            "review_plan" => Ok(json!({
                "approved": true,
                "feedback": "Keep the capability test host-neutral and deterministic.",
                "phase": input.get("phase").cloned().unwrap_or(Value::Null),
            })),
            "record_progress" => Ok(json!({
                "recorded": true,
                "phase": input.get("phase").cloned().unwrap_or(Value::Null),
                "had_previous_memory": input
                    .get("had_previous_memory")
                    .cloned()
                    .unwrap_or(Value::Bool(false)),
            })),
            other => Err(ToolError {
                record: AgentError::validation(format!("unknown tool '{other}'")).record,
            }),
        }
    }
}

#[async_trait]
impl AgentEventEmitter for RuntimeE2eServices {
    async fn emit_event(&self, event: AgentEvent) -> Result<(), AgentError> {
        self.events.lock().expect("events lock").push(event);
        Ok(())
    }
}

#[async_trait]
impl AgentStateAccess for RuntimeE2eServices {
    async fn load_state(&self, key: &str) -> Result<Option<Value>, AgentError> {
        Ok(self.state.lock().expect("state lock").get(key).cloned())
    }

    async fn save_state(&self, key: &str, value: Value) -> Result<(), AgentError> {
        self.state
            .lock()
            .expect("state lock")
            .insert(key.to_owned(), value);
        Ok(())
    }
}

#[async_trait]
impl ProposalCreator for RuntimeE2eServices {}

#[async_trait]
impl SubagentRunner for RuntimeE2eServices {}

#[async_trait]
impl ArtifactPublisher for RuntimeE2eServices {}

fn run_request(input: Value) -> RunRequest {
    RunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: None,
        input,
        user: None,
        scope: None,
        trigger: TriggerKind::Manual,
        trigger_envelope: None,
        workflow: None,
        metadata: json!({"case": "agent_capabilities_e2e"}),
    }
}

fn json_string(input: &Value, key: &str, fallback: &str) -> String {
    input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

fn agent_error_from_tool(error: ToolError) -> AgentError {
    AgentError {
        record: error.record,
    }
}

fn trace_event_kinds(events: &[agent_core::TraceEvent]) -> Vec<&str> {
    events.iter().map(|event| event.kind.as_str()).collect()
}
