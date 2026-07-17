use std::collections::BTreeMap;

use agent_core::{
    AgentError, AgentRuntimeCatalog, EffectId, EmbeddedEffectKind, EmbeddedEffectResponse,
    EmbeddedEffectResult, EmbeddedHostEffect, EmbeddedPendingHostEffect, EmbeddedRunContinuation,
    EmbeddedRunState, EmbeddedRunStep, EmbeddedRunStepStatus, EmbeddedStepTraceEvent,
    EmbeddedTerminalReason, RunId, ToolOutcomeStatus, protocol_version,
};
use serde_json::{Map, Value, json};

use super::validation::{
    catalog_agent, catalog_tool, require_object, validate_continuation, validation,
};

pub(super) fn parse_effect_plan(
    input: &Value,
) -> Result<(Vec<EmbeddedPendingHostEffect>, Option<Value>), AgentError> {
    let object = input
        .as_object()
        .ok_or_else(|| validation("run request input must be a JSON object"))?;
    let values = if let Some(effects) = object.get("effects") {
        effects
            .as_array()
            .ok_or_else(|| validation("effects must be an array"))?
            .clone()
    } else if let Some(effect) = object.get("effect") {
        vec![effect.clone()]
    } else {
        Vec::new()
    };
    let effects = values
        .into_iter()
        .enumerate()
        .map(|(index, value)| parse_pending_effect(value, &format!("effects[{index}]")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((effects, object.get("llm_response").cloned()))
}

fn parse_pending_effect(
    value: Value,
    label: &str,
) -> Result<EmbeddedPendingHostEffect, AgentError> {
    let effect: EmbeddedPendingHostEffect = serde_json::from_value(value)
        .map_err(|error| validation(format!("{label} is invalid: {error}")))?;
    validate_pending_effect(&effect, label)?;
    Ok(effect)
}

pub(super) fn validate_pending_effect(
    effect: &EmbeddedPendingHostEffect,
    label: &str,
) -> Result<(), AgentError> {
    match effect {
        EmbeddedPendingHostEffect::Tool { name, input } => {
            if name.trim().is_empty() {
                return Err(validation(format!("{label}.name is required")));
            }
            require_object(input, &format!("{label}.input"))?;
        }
        EmbeddedPendingHostEffect::Subagent {
            agent_id,
            input,
            metadata,
            ..
        } => {
            if agent_id.trim().is_empty() {
                return Err(validation(format!("{label}.agent_id is required")));
            }
            require_object(input, &format!("{label}.input"))?;
            require_object(metadata, &format!("{label}.metadata"))?;
        }
    }
    Ok(())
}

pub(super) fn continuation_for(
    effects: Vec<EmbeddedPendingHostEffect>,
    effect_results: Vec<EmbeddedEffectResult>,
    llm_response: Option<Value>,
    next_step_index: u64,
) -> Option<EmbeddedRunContinuation> {
    if effects.is_empty() && effect_results.is_empty() && llm_response.is_none() {
        return None;
    }
    Some(EmbeddedRunContinuation {
        effects,
        effect_results,
        llm_response,
        next_step_index,
    })
}

pub(super) fn effect_requested_step(
    catalog: &AgentRuntimeCatalog,
    run_id: RunId,
    agent_id: &str,
    agent_version: &str,
    step_index: u64,
    pending: EmbeddedPendingHostEffect,
    continuation: Option<EmbeddedRunContinuation>,
) -> Result<EmbeddedRunStep, AgentError> {
    if let Some(value) = &continuation {
        validate_continuation(catalog, value, agent_id, step_index)?;
    }
    let effect = promote_effect(catalog, agent_id, pending)?;
    Ok(build_step(StepParts {
        run_id,
        agent_id: agent_id.to_owned(),
        agent_version: agent_version.to_owned(),
        step_index,
        status: EmbeddedRunStepStatus::EffectRequested,
        effect: Some(effect),
        effect_response: None,
        effect_results: Vec::new(),
        continuation,
        output: None,
        error: None,
    }))
}

fn promote_effect(
    catalog: &AgentRuntimeCatalog,
    agent_id: &str,
    pending: EmbeddedPendingHostEffect,
) -> Result<EmbeddedHostEffect, AgentError> {
    validate_pending_effect(&pending, "pending effect")?;
    match pending {
        EmbeddedPendingHostEffect::Tool { name, input } => {
            let tool = catalog_tool(catalog, agent_id, &name)?;
            Ok(EmbeddedHostEffect::Tool {
                effect_id: EffectId::new_v7().0,
                name: tool.name.clone(),
                input,
                risk: tool.risk.clone(),
                metadata: tool.metadata.clone(),
            })
        }
        EmbeddedPendingHostEffect::Subagent {
            agent_id,
            input,
            run_id,
            scope,
            workflow,
            metadata,
        } => {
            catalog_agent(catalog, &agent_id)?;
            Ok(EmbeddedHostEffect::Subagent {
                effect_id: EffectId::new_v7().0,
                agent_id,
                input,
                run_id,
                scope,
                workflow,
                metadata,
            })
        }
    }
}

pub(super) fn completed_passthrough_step(
    run_id: RunId,
    agent_id: &str,
    agent_version: &str,
    output: Value,
) -> EmbeddedRunStep {
    build_step(StepParts {
        run_id,
        agent_id: agent_id.to_owned(),
        agent_version: agent_version.to_owned(),
        step_index: 0,
        status: EmbeddedRunStepStatus::Completed,
        effect: None,
        effect_response: None,
        effect_results: Vec::new(),
        continuation: None,
        output: Some(output),
        error: None,
    })
}

pub(super) fn completed_effect_step(
    run_id: RunId,
    agent_id: &str,
    agent_version: &str,
    step_index: u64,
    effect: EmbeddedHostEffect,
    effect_response: EmbeddedEffectResponse,
    effect_results: Vec<EmbeddedEffectResult>,
) -> EmbeddedRunStep {
    let mode = if effect_results.len() > 1 {
        "frb_effect_loop"
    } else {
        "frb_effect_step"
    };
    let output = json!({
        "mode": mode,
        "effect": effect,
        "effect_result": effect_response.result.clone().unwrap_or(Value::Null),
        "effect_response": effect_response,
        "effect_results": effect_results,
    });
    build_step(StepParts {
        run_id,
        agent_id: agent_id.to_owned(),
        agent_version: agent_version.to_owned(),
        step_index,
        status: EmbeddedRunStepStatus::Completed,
        effect: Some(effect),
        effect_response: Some(effect_response),
        effect_results,
        continuation: None,
        output: Some(output),
        error: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn terminal_effect_step(
    run_id: RunId,
    agent_id: &str,
    agent_version: &str,
    step_index: u64,
    status: EmbeddedRunStepStatus,
    effect: EmbeddedHostEffect,
    effect_response: EmbeddedEffectResponse,
    effect_results: Vec<EmbeddedEffectResult>,
    error: Option<Value>,
) -> EmbeddedRunStep {
    build_step(StepParts {
        run_id,
        agent_id: agent_id.to_owned(),
        agent_version: agent_version.to_owned(),
        step_index,
        status,
        effect: Some(effect),
        effect_response: Some(effect_response),
        effect_results,
        continuation: None,
        output: None,
        error,
    })
}

struct StepParts {
    run_id: RunId,
    agent_id: String,
    agent_version: String,
    step_index: u64,
    status: EmbeddedRunStepStatus,
    effect: Option<EmbeddedHostEffect>,
    effect_response: Option<EmbeddedEffectResponse>,
    effect_results: Vec<EmbeddedEffectResult>,
    continuation: Option<EmbeddedRunContinuation>,
    output: Option<Value>,
    error: Option<Value>,
}

fn build_step(parts: StepParts) -> EmbeddedRunStep {
    let run_state = derived_run_state(
        parts.status,
        parts.step_index,
        parts.continuation.as_ref(),
        &parts.effect_results,
    );
    let trace_event = derived_trace_event(
        &parts.run_id,
        &parts.agent_id,
        parts.status,
        parts.step_index,
        parts.effect.as_ref(),
        &run_state,
    );
    EmbeddedRunStep {
        protocol_version: protocol_version(),
        run_id: parts.run_id,
        agent_id: parts.agent_id,
        agent_version: parts.agent_version,
        step_index: parts.step_index,
        status: parts.status,
        effect: parts.effect,
        effect_response: parts.effect_response.clone(),
        effect_result: parts
            .effect_response
            .as_ref()
            .and_then(|response| response.result.clone()),
        effect_results: parts.effect_results,
        continuation: parts.continuation,
        output: parts.output,
        error: parts.error,
        proposal: None,
        run_state,
        trace_event,
        extensions: BTreeMap::new(),
    }
}

pub(super) fn derived_run_state(
    status: EmbeddedRunStepStatus,
    step_index: u64,
    continuation: Option<&EmbeddedRunContinuation>,
    effect_results: &[EmbeddedEffectResult],
) -> EmbeddedRunState {
    EmbeddedRunState {
        status,
        step_index,
        remaining_effect_count: continuation.map(|value| value.effects.len()).unwrap_or(0),
        effect_result_count: continuation
            .map(|value| value.effect_results.len())
            .unwrap_or(effect_results.len()),
        terminal_reason: terminal_reason(status),
    }
}

pub(super) fn derived_trace_event(
    run_id: &RunId,
    agent_id: &str,
    status: EmbeddedRunStepStatus,
    step_index: u64,
    effect: Option<&EmbeddedHostEffect>,
    run_state: &EmbeddedRunState,
) -> EmbeddedStepTraceEvent {
    EmbeddedStepTraceEvent {
        kind: "agent_runtime_step".to_owned(),
        run_id: run_id.clone(),
        agent_id: agent_id.to_owned(),
        status,
        step_index,
        effect_id: effect.map(|value| value.effect_id().to_owned()),
        effect_kind: effect.map(effect_kind),
        tool_name: effect.and_then(|value| match value {
            EmbeddedHostEffect::Tool { name, .. } => Some(name.clone()),
            EmbeddedHostEffect::Subagent { .. } => None,
        }),
        subagent_id: effect.and_then(|value| match value {
            EmbeddedHostEffect::Tool { .. } => None,
            EmbeddedHostEffect::Subagent { agent_id, .. } => Some(agent_id.clone()),
        }),
        run_state: run_state.clone(),
        extensions: BTreeMap::new(),
    }
}

fn terminal_reason(status: EmbeddedRunStepStatus) -> Option<EmbeddedTerminalReason> {
    match status {
        EmbeddedRunStepStatus::EffectRequested => None,
        EmbeddedRunStepStatus::Completed => Some(EmbeddedTerminalReason::Done),
        EmbeddedRunStepStatus::Failed => Some(EmbeddedTerminalReason::StreamError),
        EmbeddedRunStepStatus::Cancelled => Some(EmbeddedTerminalReason::UserCancel),
        EmbeddedRunStepStatus::PolicyDenied => Some(EmbeddedTerminalReason::PolicyDenied),
        EmbeddedRunStepStatus::ClosedEarly | EmbeddedRunStepStatus::TimedOut => {
            Some(EmbeddedTerminalReason::ClosedEarly)
        }
    }
}

pub(super) fn effect_kind(effect: &EmbeddedHostEffect) -> EmbeddedEffectKind {
    match effect {
        EmbeddedHostEffect::Tool { .. } => EmbeddedEffectKind::Tool,
        EmbeddedHostEffect::Subagent { .. } => EmbeddedEffectKind::Subagent,
    }
}

pub(super) fn effect_response_terminal_status(
    response: &EmbeddedEffectResponse,
) -> Option<EmbeddedRunStepStatus> {
    let outcome = response.effective_outcome();
    match effect_response_error_code(response).as_deref() {
        Some("effect_budget_exhausted" | "subagent_depth_exceeded") => {
            Some(EmbeddedRunStepStatus::ClosedEarly)
        }
        Some("policy_denied" | "runtime_not_allowed") => Some(EmbeddedRunStepStatus::PolicyDenied),
        Some("user_cancel" | "user_cancelled" | "cancelled") => {
            Some(EmbeddedRunStepStatus::Cancelled)
        }
        Some("tool_timeout" | "timeout" | "timed_out") => Some(EmbeddedRunStepStatus::TimedOut),
        Some(_) | None => match outcome.status {
            ToolOutcomeStatus::Ok => None,
            ToolOutcomeStatus::PolicyDenied | ToolOutcomeStatus::ApprovalRequired => {
                Some(EmbeddedRunStepStatus::PolicyDenied)
            }
            ToolOutcomeStatus::Cancelled => Some(EmbeddedRunStepStatus::Cancelled),
            ToolOutcomeStatus::Error => Some(EmbeddedRunStepStatus::Failed),
        },
    }
}

fn effect_response_error_code(response: &EmbeddedEffectResponse) -> Option<String> {
    response
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.code.clone())
        .or_else(|| {
            response
                .error
                .as_ref()
                .and_then(|error| error.data.as_ref())
                .and_then(|data| data.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| response.error.as_ref().map(|error| error.code.to_string()))
        .or_else(|| {
            response
                .result
                .as_ref()
                .and_then(|result| result.get("error"))
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            response
                .result
                .as_ref()
                .and_then(|result| result.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

pub(super) fn effect_response_error_payload(response: &EmbeddedEffectResponse) -> Option<Value> {
    let outcome = response.effective_outcome();
    if outcome.is_error() {
        return serde_json::to_value(outcome).ok();
    }
    if let Some(error) = &response.error {
        return serde_json::to_value(error).ok();
    }
    let result = response.result.as_ref()?;
    if let Some(error) = result.get("error") {
        if error.is_object() {
            return Some(error.clone());
        }
        let mut object = Map::new();
        if let Some(code) = result.get("code").and_then(Value::as_str) {
            object.insert("code".to_owned(), Value::String(code.to_owned()));
        }
        object.insert("message".to_owned(), error.clone());
        return Some(Value::Object(object));
    }
    result
        .get("code")
        .and_then(Value::as_str)
        .map(|code| json!({"code": code}))
}
