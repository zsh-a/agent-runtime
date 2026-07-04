use std::sync::Arc;

use agent_core::{
    HookEventName, PROTOCOL_VERSION, RunId, RunRequest, RunScope, RunWorkflow, SubagentRequest,
    ToolError, TraceEvent, TraceSink, TriggerKind, UserContext,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{HookManager, MemoryTraceSink, RunControl, runner::AgentRunner};

#[derive(Clone)]
pub struct SubagentRunContext {
    pub parent_run_id: Option<RunId>,
    pub parent_agent_id: Option<String>,
    pub user: Option<UserContext>,
    pub scope: Option<RunScope>,
    pub workflow: Option<RunWorkflow>,
    pub metadata: Value,
    pub trace: Option<Arc<dyn TraceSink>>,
    pub hooks: HookManager,
    pub cancellation: CancellationToken,
}

impl Default for SubagentRunContext {
    fn default() -> Self {
        Self {
            parent_run_id: None,
            parent_agent_id: None,
            user: None,
            scope: None,
            workflow: None,
            metadata: json!({}),
            trace: None,
            hooks: HookManager::default(),
            cancellation: CancellationToken::new(),
        }
    }
}

pub async fn run_subagent(
    runner: &AgentRunner,
    request: SubagentRequest,
    context: SubagentRunContext,
) -> Result<Value, ToolError> {
    let agent_id = request.agent_id.trim().to_owned();
    if agent_id.is_empty() {
        return Err(ToolError::policy_denied(
            "subagent request requires a non-empty agent_id",
            json!({"effect": "subagent"}),
        ));
    }
    let child_input = request.input.clone();
    let scope = request.scope.clone().or_else(|| context.scope.clone());
    let metadata = child_metadata(&request, &context);
    let workflow = child_workflow(&request, &context);
    let trace = context
        .trace
        .clone()
        .unwrap_or_else(|| Arc::new(MemoryTraceSink::default()));
    let hook_input = json!({
        "run_id": context.parent_run_id.as_ref().map(|run_id| run_id.0.clone()),
        "agent_id": context.parent_agent_id,
        "subagent_id": agent_id,
        "input": child_input,
        "workflow": workflow.clone(),
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
                run_id: request.run_id,
                input: child_input,
                user: context.user,
                scope,
                trigger: TriggerKind::Manual,
                trigger_envelope: None,
                workflow,
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

fn child_metadata(request: &SubagentRequest, context: &SubagentRunContext) -> Value {
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
        if let Some(extra) = request.metadata.as_object() {
            for (key, value) in extra {
                object.insert(key.clone(), value.clone());
            }
        }
    }
    metadata
}

fn child_workflow(request: &SubagentRequest, context: &SubagentRunContext) -> Option<RunWorkflow> {
    let mut workflow = request
        .workflow
        .clone()
        .or_else(|| context.workflow.clone())
        .unwrap_or_else(|| RunWorkflow {
            workflow_id: None,
            root_run_id: None,
            parent_run_id: None,
            parent_agent_id: None,
            dependencies: Vec::new(),
            fanout_id: None,
            fanin_id: None,
            compensation: None,
            metadata: json!({}),
        });

    if let Some(parent_run_id) = &context.parent_run_id {
        workflow.parent_run_id = Some(parent_run_id.clone());
        if workflow.root_run_id.is_none() {
            workflow.root_run_id = context
                .workflow
                .as_ref()
                .and_then(|workflow| workflow.root_run_id.clone())
                .or_else(|| Some(parent_run_id.clone()));
        }
    }
    if workflow.parent_agent_id.is_none() {
        workflow.parent_agent_id = context.parent_agent_id.clone();
    }

    if workflow.workflow_id.is_some()
        || workflow.root_run_id.is_some()
        || workflow.parent_run_id.is_some()
        || workflow.parent_agent_id.is_some()
        || !workflow.dependencies.is_empty()
        || workflow.fanout_id.is_some()
        || workflow.fanin_id.is_some()
        || workflow.compensation.is_some()
    {
        Some(workflow)
    } else {
        None
    }
}
