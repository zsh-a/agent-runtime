use std::sync::Arc;

use agent_core::{
    HookEventName, PROTOCOL_VERSION, RunId, RunRequest, ToolError, ToolRisk, ToolSpec, TraceEvent,
    TraceSink, TriggerKind, UserContext,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{HookManager, MemoryTraceSink, RunControl, runner::AgentRunner};

pub const AGENT_RUN_TOOL_NAME: &str = "agent.run";

pub fn agent_run_tool_spec() -> ToolSpec {
    ToolSpec {
        name: AGENT_RUN_TOOL_NAME.to_owned(),
        description: "Run another runtime agent with JSON input and return its result and trace."
            .to_owned(),
        input_schema: json!({
            "type": "object",
            "required": ["agent_id"],
            "properties": {
                "agent_id": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Runtime agent id to run."
                },
                "input": {
                    "description": "JSON input passed to the subagent."
                },
                "run_id": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Optional deterministic run id."
                },
                "metadata": {
                    "type": "object",
                    "description": "Optional metadata merged into the child run metadata."
                }
            },
            "additionalProperties": false
        }),
        output_schema: Some(json!({
            "type": "object",
            "required": ["result", "trace"],
            "properties": {
                "result": {"type": "object"},
                "trace": {"type": "object"}
            },
            "additionalProperties": false
        })),
        risk: ToolRisk::High,
        metadata: json!({"source": "agent_runtime_builtin"}),
    }
}

#[derive(Clone)]
pub struct AgentRunToolContext {
    pub parent_run_id: Option<RunId>,
    pub parent_agent_id: Option<String>,
    pub user: Option<UserContext>,
    pub metadata: Value,
    pub trace: Option<Arc<dyn TraceSink>>,
    pub hooks: HookManager,
    pub cancellation: CancellationToken,
}

impl Default for AgentRunToolContext {
    fn default() -> Self {
        Self {
            parent_run_id: None,
            parent_agent_id: None,
            user: None,
            metadata: json!({}),
            trace: None,
            hooks: HookManager::default(),
            cancellation: CancellationToken::new(),
        }
    }
}

pub async fn call_agent_run_tool(
    runner: &AgentRunner,
    input: Value,
    context: AgentRunToolContext,
) -> Result<Value, ToolError> {
    let agent_id = input
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            ToolError::policy_denied(
                "agent.run requires a non-empty agent_id",
                json!({"tool_name": AGENT_RUN_TOOL_NAME}),
            )
        })?;
    let run_id = input
        .get("run_id")
        .and_then(Value::as_str)
        .map(|value| RunId(value.to_owned()));
    let child_input = input.get("input").cloned().unwrap_or_else(|| json!({}));
    let metadata = child_metadata(&input, &context);
    let trace = context
        .trace
        .clone()
        .unwrap_or_else(|| Arc::new(MemoryTraceSink::default()));
    let hook_input = json!({
        "run_id": context.parent_run_id.as_ref().map(|run_id| run_id.0.clone()),
        "agent_id": context.parent_agent_id,
        "subagent_id": agent_id,
        "input": child_input,
        "metadata": metadata,
    });
    let decision = context
        .hooks
        .authorize(
            HookEventName::SubagentStart,
            context.parent_run_id.clone(),
            context.parent_agent_id.clone(),
            hook_input.clone(),
            trace.as_ref(),
        )
        .await
        .map_err(ToolError::from_agent_error)?;
    if decision.is_denied() {
        return Err(ToolError::policy_denied(
            decision
                .reason
                .clone()
                .unwrap_or_else(|| format!("subagent '{agent_id}' denied by policy hook")),
            json!({
                "decision": decision,
                "event": "SubagentStart",
                "subagent_id": agent_id,
            }),
        ));
    }
    context
        .hooks
        .observe(
            HookEventName::SubagentStart,
            context.parent_run_id.clone(),
            context.parent_agent_id.clone(),
            hook_input,
            trace.as_ref(),
        )
        .await
        .map_err(ToolError::from_agent_error)?;
    trace
        .emit(TraceEvent::new(
            "subagent_started",
            json!({
                "run_id": context.parent_run_id.as_ref().map(|run_id| run_id.0.clone()),
                "agent_id": context.parent_agent_id,
                "subagent_id": agent_id,
            }),
        ))
        .await
        .map_err(ToolError::from_agent_error)?;
    let outcome = runner
        .run_once_with_control(
            &agent_id,
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id,
                input: child_input,
                user: context.user,
                trigger: TriggerKind::Manual,
                metadata,
            },
            RunControl {
                cancellation: context.cancellation,
                trace_events: None,
            },
        )
        .await
        .map_err(ToolError::from_agent_error)?;
    trace
        .emit(TraceEvent::new(
            "subagent_finished",
            json!({
                "run_id": context.parent_run_id.as_ref().map(|run_id| run_id.0.clone()),
                "agent_id": context.parent_agent_id,
                "subagent_id": agent_id,
                "subagent_run_id": outcome.result.run_id.0.clone(),
                "status": outcome.result.status.clone(),
            }),
        ))
        .await
        .map_err(ToolError::from_agent_error)?;
    context
        .hooks
        .observe(
            HookEventName::SubagentStop,
            context.parent_run_id,
            context.parent_agent_id,
            json!({
                "subagent_id": agent_id,
                "result": outcome.result.clone(),
            }),
            trace.as_ref(),
        )
        .await
        .map_err(ToolError::from_agent_error)?;
    Ok(json!({
        "result": outcome.result,
        "trace": outcome.trace,
    }))
}

fn child_metadata(input: &Value, context: &AgentRunToolContext) -> Value {
    let mut metadata = if context.metadata.is_object() {
        context.metadata.clone()
    } else {
        json!({})
    };
    if let Some(object) = metadata.as_object_mut() {
        object.insert("subagent".to_owned(), Value::Bool(true));
        if let Some(parent_run_id) = &context.parent_run_id {
            object.insert(
                "parent_run_id".to_owned(),
                Value::String(parent_run_id.0.clone()),
            );
        }
        if let Some(parent_agent_id) = &context.parent_agent_id {
            object.insert(
                "parent_agent_id".to_owned(),
                Value::String(parent_agent_id.clone()),
            );
        }
        if let Some(extra) = input.get("metadata").and_then(Value::as_object) {
            for (key, value) in extra {
                object.insert(key.clone(), value.clone());
            }
        }
    }
    metadata
}
