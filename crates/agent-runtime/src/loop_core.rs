use std::collections::{BTreeMap, HashSet};

use agent_core::{
    AgentError, AgentRuntimeCatalog, EMBEDDED_SNAPSHOT_VERSION, EffectId, EmbeddedEffectKind,
    EmbeddedEffectResponse, EmbeddedEffectResult, EmbeddedHostEffect, EmbeddedPendingHostEffect,
    EmbeddedRunContinuation, EmbeddedRunLimits, EmbeddedRunProgress, EmbeddedRunSnapshot,
    EmbeddedRunState, EmbeddedRunStep, EmbeddedRunStepStatus, EmbeddedStepTraceEvent,
    EmbeddedTerminalReason, RunId, RunRequest, ToolSpec, catalog_version, protocol_version,
};
use serde_json::{Map, Value, json};

/// Typed embedded execution state machine for hosts that persist and resume
/// runtime-owned snapshots.
pub struct EffectStepLoop;

impl EffectStepLoop {
    pub fn start_snapshot(
        catalog: &AgentRuntimeCatalog,
        request: RunRequest,
        agent_id: &str,
        limits: EmbeddedRunLimits,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        Self::start_snapshot_at_depth(catalog, request, agent_id, limits, 0, 0)
    }

    pub fn continue_snapshot(
        catalog: &AgentRuntimeCatalog,
        snapshot: EmbeddedRunSnapshot,
        effect_response: EmbeddedEffectResponse,
        agent_id: &str,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        let snapshot = close_snapshot_at_limits(catalog, snapshot, agent_id)?;
        if snapshot.is_terminal() {
            return Ok(snapshot);
        }
        advance_snapshot(catalog, snapshot, effect_response, agent_id, true)
    }

    pub fn cancel_snapshot(
        catalog: &AgentRuntimeCatalog,
        snapshot: EmbeddedRunSnapshot,
        agent_id: &str,
        reason: &str,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        validate_snapshot(catalog, &snapshot, agent_id)?;
        if snapshot.is_terminal() {
            return Ok(snapshot);
        }
        let effect_id = snapshot
            .requested_effect()
            .ok_or_else(|| validation("embedded snapshot has no pending effect to cancel"))?
            .effect_id()
            .to_owned();
        let message = if reason.trim().is_empty() {
            "embedded run cancelled by host"
        } else {
            reason.trim()
        };
        advance_snapshot(
            catalog,
            snapshot,
            EmbeddedEffectResponse {
                jsonrpc: "2.0".to_owned(),
                id: effect_id,
                result: Some(json!({
                    "error": {
                        "code": "user_cancel",
                        "message": message,
                    }
                })),
                error: None,
            },
            agent_id,
            false,
        )
    }

    pub fn start_requested_subagent(
        catalog: &AgentRuntimeCatalog,
        parent: &EmbeddedRunSnapshot,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        validate_snapshot(catalog, parent, &parent.step.agent_id)?;
        let Some(EmbeddedHostEffect::Subagent {
            agent_id,
            input,
            run_id,
            scope,
            workflow,
            metadata,
            ..
        }) = parent.requested_effect()
        else {
            return Err(validation(
                "embedded snapshot does not request a subagent effect",
            ));
        };
        if parent.remaining_effect_steps() == 0 {
            return Err(validation("embedded effect budget is exhausted"));
        }
        if parent.progress.subagent_depth >= parent.limits.max_subagent_depth {
            return Err(validation("embedded subagent depth is exhausted"));
        }
        let dispatched_effect_count = parent
            .progress
            .dispatched_effect_count
            .checked_add(1)
            .ok_or_else(|| validation("embedded dispatched effect count overflowed"))?;
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: run_id.clone(),
            input: input.clone(),
            user: None,
            scope: scope.clone(),
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: workflow.as_deref().cloned(),
            metadata: metadata.clone(),
        };
        Self::start_snapshot_at_depth(
            catalog,
            request,
            agent_id,
            parent.limits,
            parent.progress.subagent_depth.saturating_add(1),
            dispatched_effect_count,
        )
    }

    pub fn resume_parent_from_subagent(
        catalog: &AgentRuntimeCatalog,
        mut parent: EmbeddedRunSnapshot,
        child: EmbeddedRunSnapshot,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        validate_snapshot(catalog, &parent, &parent.step.agent_id)?;
        validate_snapshot(catalog, &child, &child.step.agent_id)?;
        let Some(EmbeddedHostEffect::Subagent {
            effect_id,
            agent_id,
            ..
        }) = parent.requested_effect()
        else {
            return Err(validation(
                "embedded parent does not request a subagent effect",
            ));
        };
        if !child.is_terminal() {
            return Err(validation("embedded child snapshot is not terminal"));
        }
        if child.step.agent_id != *agent_id {
            return Err(validation(format!(
                "embedded child agent '{}' does not match requested subagent '{}'",
                child.step.agent_id, agent_id
            )));
        }
        if child.limits != parent.limits {
            return Err(validation(
                "embedded child limits do not match parent limits",
            ));
        }
        if child.progress.dispatched_effect_count <= parent.progress.dispatched_effect_count {
            return Err(validation(
                "embedded child did not consume the parent subagent effect budget",
            ));
        }
        let child_agent_id = child.step.agent_id.clone();
        let child_progress = child.progress;
        let child_step = serde_json::to_value(&child.step).map_err(|error| {
            validation(format!("failed to encode terminal subagent step: {error}"))
        })?;
        let child_snapshot = serde_json::to_value(&child).map_err(|error| {
            validation(format!(
                "failed to encode terminal subagent snapshot: {error}"
            ))
        })?;
        let response = EmbeddedEffectResponse {
            jsonrpc: "2.0".to_owned(),
            id: effect_id.clone(),
            result: Some(json!({
                "agent_id": child_agent_id,
                "terminal_step": child_step,
                "snapshot": child_snapshot,
            })),
            error: None,
        };
        parent.progress.dispatched_effect_count = child_progress.dispatched_effect_count;
        parent.progress.effect_budget_exhausted |= child_progress.effect_budget_exhausted;
        parent.progress.subagent_depth_exceeded |= child_progress.subagent_depth_exceeded;
        let parent_agent_id = parent.step.agent_id.clone();
        advance_snapshot(catalog, parent, response, &parent_agent_id, false)
    }

    pub fn start_typed(
        catalog: &AgentRuntimeCatalog,
        mut request: RunRequest,
        agent_id: &str,
    ) -> Result<EmbeddedRunStep, AgentError> {
        validate_catalog(catalog)?;
        normalize_run_request(&mut request)?;
        let agent = catalog_agent(catalog, agent_id)?;
        let run_id = request.run_id.clone().unwrap_or_else(RunId::new_v7);
        let (effects, llm_response) = parse_effect_plan(&request.input)?;
        if effects.is_empty() {
            return Ok(completed_passthrough_step(
                run_id,
                &agent.id,
                &agent.version,
                request.input,
            ));
        }
        let mut effects = effects.into_iter();
        let first = effects.next().expect("non-empty effect plan");
        let remaining = effects.collect::<Vec<_>>();
        let continuation = continuation_for(remaining, Vec::new(), llm_response, 1);
        effect_requested_step(
            catalog,
            run_id,
            &agent.id,
            &agent.version,
            0,
            first,
            continuation,
        )
    }

    pub fn continue_typed(
        catalog: &AgentRuntimeCatalog,
        previous_step: EmbeddedRunStep,
        effect_response: EmbeddedEffectResponse,
        agent_id: &str,
    ) -> Result<EmbeddedRunStep, AgentError> {
        validate_catalog(catalog)?;
        let agent = catalog_agent(catalog, agent_id)?;
        validate_step(catalog, &previous_step, &agent.id, &agent.version)?;
        if previous_step.status != EmbeddedRunStepStatus::EffectRequested {
            return Err(validation(
                "previous step status must be 'effect_requested'",
            ));
        }
        let effect = previous_step
            .effect
            .clone()
            .ok_or_else(|| validation("previous step is missing effect"))?;
        validate_effect_response(&effect, &effect_response)?;

        let terminal_status = effect_response_terminal_status(&effect_response);
        let mut effect_results = previous_step
            .continuation
            .as_ref()
            .map(|continuation| continuation.effect_results.clone())
            .unwrap_or_default();
        if terminal_status != Some(EmbeddedRunStepStatus::ClosedEarly) {
            effect_results.push(EmbeddedEffectResult {
                kind: effect_kind(&effect),
                effect: effect.clone(),
                effect_response: effect_response.clone(),
            });
        }
        let next_step_index = previous_step.step_index.saturating_add(1);

        if let Some(status) = terminal_status {
            return Ok(terminal_effect_step(
                previous_step.run_id,
                &agent.id,
                &agent.version,
                next_step_index,
                status,
                effect,
                effect_response.clone(),
                effect_results,
                effect_response_error_payload(&effect_response),
            ));
        }

        let mut continuation = previous_step
            .continuation
            .unwrap_or(EmbeddedRunContinuation {
                effects: Vec::new(),
                effect_results: Vec::new(),
                llm_response: None,
                next_step_index,
            });
        if continuation.effects.is_empty() {
            return Ok(completed_effect_step(
                previous_step.run_id,
                &agent.id,
                &agent.version,
                next_step_index,
                effect,
                effect_response,
                effect_results,
            ));
        }

        let first = continuation.effects.remove(0);
        let next_continuation = continuation_for(
            continuation.effects,
            effect_results,
            continuation.llm_response,
            next_step_index.saturating_add(1),
        );
        effect_requested_step(
            catalog,
            previous_step.run_id,
            &agent.id,
            &agent.version,
            next_step_index,
            first,
            next_continuation,
        )
    }

    fn start_snapshot_at_depth(
        catalog: &AgentRuntimeCatalog,
        request: RunRequest,
        agent_id: &str,
        limits: EmbeddedRunLimits,
        subagent_depth: u32,
        dispatched_effect_count: u32,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        let step = Self::start_typed(catalog, request, agent_id)?;
        let snapshot = EmbeddedRunSnapshot {
            protocol_version: protocol_version(),
            snapshot_version: EMBEDDED_SNAPSHOT_VERSION,
            step,
            limits,
            progress: EmbeddedRunProgress {
                dispatched_effect_count,
                subagent_depth,
                effect_budget_exhausted: false,
                subagent_depth_exceeded: false,
            },
        };
        close_snapshot_at_limits(catalog, snapshot, agent_id)
    }
}

fn advance_snapshot(
    catalog: &AgentRuntimeCatalog,
    mut snapshot: EmbeddedRunSnapshot,
    effect_response: EmbeddedEffectResponse,
    agent_id: &str,
    count_dispatch: bool,
) -> Result<EmbeddedRunSnapshot, AgentError> {
    validate_snapshot(catalog, &snapshot, agent_id)?;
    if snapshot.is_terminal() {
        return Err(validation("embedded snapshot is already terminal"));
    }
    if count_dispatch {
        snapshot.progress.dispatched_effect_count = snapshot
            .progress
            .dispatched_effect_count
            .checked_add(1)
            .ok_or_else(|| validation("embedded dispatched effect count overflowed"))?;
        if snapshot.progress.dispatched_effect_count > snapshot.limits.max_effect_steps {
            return Err(validation("embedded effect budget is exhausted"));
        }
    }
    snapshot.step =
        EffectStepLoop::continue_typed(catalog, snapshot.step, effect_response, agent_id)?;
    close_snapshot_at_limits(catalog, snapshot, agent_id)
}

fn close_snapshot_at_limits(
    catalog: &AgentRuntimeCatalog,
    mut snapshot: EmbeddedRunSnapshot,
    agent_id: &str,
) -> Result<EmbeddedRunSnapshot, AgentError> {
    validate_snapshot(catalog, &snapshot, agent_id)?;
    let Some(effect) = snapshot.requested_effect() else {
        return Ok(snapshot);
    };
    let effect_id = effect.effect_id().to_owned();
    let is_subagent = matches!(effect, EmbeddedHostEffect::Subagent { .. });
    let closure = if snapshot.remaining_effect_steps() == 0 {
        snapshot.progress.effect_budget_exhausted = true;
        Some((
            "effect_budget_exhausted",
            "agent runtime effect budget exhausted",
        ))
    } else if is_subagent && snapshot.progress.subagent_depth >= snapshot.limits.max_subagent_depth
    {
        snapshot.progress.subagent_depth_exceeded = true;
        Some((
            "subagent_depth_exceeded",
            "agent runtime subagent depth exhausted",
        ))
    } else {
        None
    };
    let Some((code, message)) = closure else {
        return Ok(snapshot);
    };
    snapshot.step = EffectStepLoop::continue_typed(
        catalog,
        snapshot.step,
        EmbeddedEffectResponse {
            jsonrpc: "2.0".to_owned(),
            id: effect_id,
            result: Some(json!({
                "error": {
                    "code": code,
                    "message": message,
                    "max_effect_steps": snapshot.limits.max_effect_steps,
                    "dispatched_effect_count": snapshot.progress.dispatched_effect_count,
                    "max_subagent_depth": snapshot.limits.max_subagent_depth,
                    "subagent_depth": snapshot.progress.subagent_depth,
                }
            })),
            error: None,
        },
        agent_id,
    )?;
    Ok(snapshot)
}

fn parse_effect_plan(
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

fn validate_pending_effect(
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

fn continuation_for(
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

fn effect_requested_step(
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

fn completed_passthrough_step(
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

fn completed_effect_step(
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
fn terminal_effect_step(
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

fn derived_run_state(
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

fn derived_trace_event(
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

fn validate_snapshot(
    catalog: &AgentRuntimeCatalog,
    snapshot: &EmbeddedRunSnapshot,
    agent_id: &str,
) -> Result<(), AgentError> {
    if snapshot.protocol_version != protocol_version() {
        return Err(validation(format!(
            "embedded snapshot protocol_version '{}' is not supported",
            snapshot.protocol_version
        )));
    }
    if snapshot.snapshot_version != EMBEDDED_SNAPSHOT_VERSION {
        return Err(validation(format!(
            "embedded snapshot version {} is not supported",
            snapshot.snapshot_version
        )));
    }
    let agent = catalog_agent(catalog, agent_id)?;
    validate_step(catalog, &snapshot.step, &agent.id, &agent.version)?;
    if snapshot.progress.dispatched_effect_count > snapshot.limits.max_effect_steps {
        return Err(validation(
            "embedded dispatched effect count exceeds max_effect_steps",
        ));
    }
    if snapshot.progress.subagent_depth > snapshot.limits.max_subagent_depth {
        return Err(validation(
            "embedded subagent depth exceeds max_subagent_depth",
        ));
    }
    Ok(())
}

fn validate_step(
    catalog: &AgentRuntimeCatalog,
    step: &EmbeddedRunStep,
    agent_id: &str,
    agent_version: &str,
) -> Result<(), AgentError> {
    if step.protocol_version != protocol_version() {
        return Err(validation(
            "embedded step protocol_version is not supported",
        ));
    }
    if step.run_id.0.trim().is_empty() {
        return Err(validation("embedded step run_id must be non-empty"));
    }
    if step.agent_id != agent_id || step.agent_version != agent_version {
        return Err(validation(
            "embedded step agent identity does not match the catalog",
        ));
    }
    match (step.status, step.effect.as_ref()) {
        (EmbeddedRunStepStatus::EffectRequested, None) => {
            return Err(validation("effect_requested step must contain an effect"));
        }
        (EmbeddedRunStepStatus::EffectRequested, Some(effect)) => {
            validate_host_effect(catalog, effect, agent_id)?;
        }
        (_, Some(effect)) => validate_host_effect(catalog, effect, agent_id)?,
        (_, None) => {}
    }
    if let Some(continuation) = &step.continuation {
        validate_continuation(catalog, continuation, agent_id, step.step_index)?;
    }
    for result in &step.effect_results {
        validate_effect_result(catalog, result, agent_id)?;
    }
    let expected_state = derived_run_state(
        step.status,
        step.step_index,
        step.continuation.as_ref(),
        &step.effect_results,
    );
    if step.run_state != expected_state {
        return Err(validation(
            "embedded step run_state does not match typed continuation state",
        ));
    }
    let expected_trace = derived_trace_event(
        &step.run_id,
        &step.agent_id,
        step.status,
        step.step_index,
        step.effect.as_ref(),
        &expected_state,
    );
    if step.trace_event.kind != expected_trace.kind
        || step.trace_event.run_id != expected_trace.run_id
        || step.trace_event.agent_id != expected_trace.agent_id
        || step.trace_event.status != expected_trace.status
        || step.trace_event.step_index != expected_trace.step_index
        || step.trace_event.effect_id != expected_trace.effect_id
        || step.trace_event.effect_kind != expected_trace.effect_kind
        || step.trace_event.tool_name != expected_trace.tool_name
        || step.trace_event.subagent_id != expected_trace.subagent_id
        || step.trace_event.run_state != expected_trace.run_state
    {
        return Err(validation(
            "embedded step trace_event does not match typed step state",
        ));
    }
    Ok(())
}

fn validate_continuation(
    catalog: &AgentRuntimeCatalog,
    continuation: &EmbeddedRunContinuation,
    agent_id: &str,
    step_index: u64,
) -> Result<(), AgentError> {
    if continuation.next_step_index != step_index.saturating_add(1) {
        return Err(validation(
            "continuation.next_step_index must equal step_index + 1",
        ));
    }
    for (index, effect) in continuation.effects.iter().enumerate() {
        validate_pending_effect(effect, &format!("continuation.effects[{index}]"))?;
        validate_pending_effect_catalog(catalog, agent_id, effect)?;
    }
    for result in &continuation.effect_results {
        validate_effect_result(catalog, result, agent_id)?;
    }
    Ok(())
}

fn validate_pending_effect_catalog(
    catalog: &AgentRuntimeCatalog,
    agent_id: &str,
    effect: &EmbeddedPendingHostEffect,
) -> Result<(), AgentError> {
    match effect {
        EmbeddedPendingHostEffect::Tool { name, .. } => {
            catalog_tool(catalog, agent_id, name)?;
        }
        EmbeddedPendingHostEffect::Subagent { agent_id, .. } => {
            catalog_agent(catalog, agent_id)?;
        }
    }
    Ok(())
}

fn validate_host_effect(
    catalog: &AgentRuntimeCatalog,
    effect: &EmbeddedHostEffect,
    parent_agent_id: &str,
) -> Result<(), AgentError> {
    if effect.effect_id().trim().is_empty() {
        return Err(validation("embedded effect_id must be non-empty"));
    }
    match effect {
        EmbeddedHostEffect::Tool {
            name,
            input,
            metadata,
            ..
        } => {
            catalog_tool(catalog, parent_agent_id, name)?;
            require_object(input, "embedded tool input")?;
            require_object(metadata, "embedded tool metadata")?;
        }
        EmbeddedHostEffect::Subagent {
            agent_id,
            input,
            metadata,
            ..
        } => {
            catalog_agent(catalog, agent_id)?;
            require_object(input, "embedded subagent input")?;
            require_object(metadata, "embedded subagent metadata")?;
        }
    }
    Ok(())
}

fn validate_effect_result(
    catalog: &AgentRuntimeCatalog,
    result: &EmbeddedEffectResult,
    agent_id: &str,
) -> Result<(), AgentError> {
    if result.kind != effect_kind(&result.effect) {
        return Err(validation(
            "embedded effect result kind does not match its effect",
        ));
    }
    validate_host_effect(catalog, &result.effect, agent_id)?;
    validate_effect_response(&result.effect, &result.effect_response)
}

fn validate_effect_response(
    effect: &EmbeddedHostEffect,
    response: &EmbeddedEffectResponse,
) -> Result<(), AgentError> {
    if response.jsonrpc != "2.0" {
        return Err(validation("effect response jsonrpc must be '2.0'"));
    }
    if response.id != effect.effect_id() {
        return Err(validation(format!(
            "effect response id '{}' does not match requested effect_id '{}'",
            response.id,
            effect.effect_id()
        )));
    }
    match (&response.result, &response.error) {
        (Some(_), None) | (None, Some(_)) => Ok(()),
        (Some(_), Some(_)) => Err(validation(
            "effect response cannot contain both result and error",
        )),
        (None, None) => Err(validation("effect response must contain result or error")),
    }
}

fn effect_kind(effect: &EmbeddedHostEffect) -> EmbeddedEffectKind {
    match effect {
        EmbeddedHostEffect::Tool { .. } => EmbeddedEffectKind::Tool,
        EmbeddedHostEffect::Subagent { .. } => EmbeddedEffectKind::Subagent,
    }
}

fn effect_response_terminal_status(
    response: &EmbeddedEffectResponse,
) -> Option<EmbeddedRunStepStatus> {
    match effect_response_error_code(response).as_deref() {
        Some("effect_budget_exhausted" | "subagent_depth_exceeded") => {
            Some(EmbeddedRunStepStatus::ClosedEarly)
        }
        Some("policy_denied") => Some(EmbeddedRunStepStatus::PolicyDenied),
        Some("user_cancel" | "user_cancelled" | "cancelled") => {
            Some(EmbeddedRunStepStatus::Cancelled)
        }
        Some("tool_timeout" | "timeout" | "timed_out") => Some(EmbeddedRunStepStatus::TimedOut),
        Some(_) => Some(EmbeddedRunStepStatus::Failed),
        None if effect_response_error_payload(response).is_some() => {
            Some(EmbeddedRunStepStatus::Failed)
        }
        None => None,
    }
}

fn effect_response_error_code(response: &EmbeddedEffectResponse) -> Option<String> {
    response
        .error
        .as_ref()
        .and_then(|error| error.data.as_ref())
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned)
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

fn effect_response_error_payload(response: &EmbeddedEffectResponse) -> Option<Value> {
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

fn normalize_run_request(request: &mut RunRequest) -> Result<(), AgentError> {
    agent_core::validate_protocol_version(&request.protocol_version).map_err(validation)?;
    if request.input.is_null() {
        request.input = json!({});
    }
    require_object(&request.input, "run request input")?;
    if request.metadata.is_null() {
        request.metadata = json!({});
    }
    require_object(&request.metadata, "run request metadata")?;
    if let Some(user) = &mut request.user {
        if user.user_id.trim().is_empty() {
            return Err(validation("run request user.user_id must be non-empty"));
        }
        if user.metadata.is_null() {
            user.metadata = json!({});
        }
        require_object(&user.metadata, "run request user.metadata")?;
    }
    Ok(())
}

fn validate_catalog(catalog: &AgentRuntimeCatalog) -> Result<(), AgentError> {
    catalog.validate_versions().map_err(validation)?;
    if catalog.catalog_version != catalog_version() {
        return Err(validation("catalog_version is not supported"));
    }
    let mut agent_ids = HashSet::new();
    for agent in &catalog.agents {
        if agent.id.trim().is_empty() || agent.version.trim().is_empty() {
            return Err(validation("catalog agent identity must be non-empty"));
        }
        if !agent_ids.insert(agent.id.as_str()) {
            return Err(validation(format!(
                "catalog agent '{}' is duplicated",
                agent.id
            )));
        }
    }
    let mut tool_names = HashSet::new();
    for tool in &catalog.tools {
        validate_tool_spec(tool)?;
        if !tool_names.insert(tool.name.as_str()) {
            return Err(validation(format!(
                "catalog tool '{}' is duplicated",
                tool.name
            )));
        }
    }
    Ok(())
}

fn validate_tool_spec(tool: &ToolSpec) -> Result<(), AgentError> {
    if tool.name.trim().is_empty() || tool.description.trim().is_empty() {
        return Err(validation(
            "catalog tool name and description must be non-empty",
        ));
    }
    require_object(&tool.input_schema, "catalog tool input_schema")?;
    if let Some(output) = &tool.output_schema {
        require_object(output, "catalog tool output_schema")?;
    }
    require_object(&tool.metadata, "catalog tool metadata")
}

fn catalog_agent<'a>(
    catalog: &'a AgentRuntimeCatalog,
    agent_id: &str,
) -> Result<&'a agent_core::AgentSpec, AgentError> {
    catalog
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| validation(format!("agent '{agent_id}' is not in the active catalog")))
}

fn catalog_tool<'a>(
    catalog: &'a AgentRuntimeCatalog,
    _agent_id: &str,
    tool_name: &str,
) -> Result<&'a ToolSpec, AgentError> {
    catalog
        .tools
        .iter()
        .find(|tool| tool.name == tool_name)
        .ok_or_else(|| validation(format!("tool '{tool_name}' is not in the active catalog")))
}

fn require_object(value: &Value, label: &str) -> Result<(), AgentError> {
    if value.is_object() {
        Ok(())
    } else {
        Err(validation(format!("{label} must be a JSON object")))
    }
}

fn validation(message: impl Into<String>) -> AgentError {
    AgentError::validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{AgentSpec, ScheduleSpec, ToolRisk};
    use time::OffsetDateTime;

    fn catalog() -> AgentRuntimeCatalog {
        AgentRuntimeCatalog {
            protocol_version: protocol_version(),
            catalog_version: catalog_version(),
            generated_at: OffsetDateTime::now_utc(),
            active_domains: vec!["test".to_owned()],
            agents: vec![
                AgentSpec {
                    protocol_version: protocol_version(),
                    id: "parent".to_owned(),
                    name: "Parent".to_owned(),
                    description: None,
                    version: "1.0.0".to_owned(),
                    schedule: ScheduleSpec::Manual,
                    capabilities: vec!["read_first".to_owned()],
                    metadata: json!({}),
                },
                AgentSpec {
                    protocol_version: protocol_version(),
                    id: "child".to_owned(),
                    name: "Child".to_owned(),
                    description: None,
                    version: "1.0.0".to_owned(),
                    schedule: ScheduleSpec::Manual,
                    capabilities: vec!["read_first".to_owned()],
                    metadata: json!({}),
                },
            ],
            tools: vec![ToolSpec {
                name: "read_first".to_owned(),
                description: "Read first".to_owned(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                risk: ToolRisk::ReadOnly,
                metadata: json!({}),
            }],
            proposal_kinds: Vec::new(),
            prompt_blocks: Vec::new(),
        }
    }

    fn request(run_id: &str, effects: Value) -> RunRequest {
        RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId(run_id.to_owned())),
            input: json!({"effects": effects}),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        }
    }

    fn ok_response(snapshot: &EmbeddedRunSnapshot, value: Value) -> EmbeddedEffectResponse {
        EmbeddedEffectResponse {
            jsonrpc: "2.0".to_owned(),
            id: snapshot
                .requested_effect()
                .expect("requested effect")
                .effect_id()
                .to_owned(),
            result: Some(value),
            error: None,
        }
    }

    #[test]
    fn typed_snapshot_runs_tool_plan_to_completion() {
        let catalog = catalog();
        let first = EffectStepLoop::start_snapshot(
            &catalog,
            request(
                "run_tool",
                json!([{"kind": "tool", "name": "read_first", "input": {}}]),
            ),
            "parent",
            EmbeddedRunLimits::default(),
        )
        .expect("snapshot starts");
        assert_eq!(first.step.status, EmbeddedRunStepStatus::EffectRequested);
        let terminal = EffectStepLoop::continue_snapshot(
            &catalog,
            first.clone(),
            ok_response(&first, json!({"ok": true})),
            "parent",
        )
        .expect("snapshot completes");
        assert_eq!(terminal.step.status, EmbeddedRunStepStatus::Completed);
        assert_eq!(terminal.progress.dispatched_effect_count, 1);
        assert_eq!(
            terminal.step.run_state.terminal_reason,
            Some(EmbeddedTerminalReason::Done)
        );
    }

    #[test]
    fn typed_snapshot_preserves_continuation_without_json_roundtrip() {
        let catalog = catalog();
        let first = EffectStepLoop::start_snapshot(
            &catalog,
            request(
                "run_multi",
                json!([
                    {"kind": "tool", "name": "read_first", "input": {"index": 1}},
                    {"kind": "tool", "name": "read_first", "input": {"index": 2}}
                ]),
            ),
            "parent",
            EmbeddedRunLimits::default(),
        )
        .expect("snapshot starts");
        let second = EffectStepLoop::continue_snapshot(
            &catalog,
            first.clone(),
            ok_response(&first, json!({"index": 1})),
            "parent",
        )
        .expect("second effect requested");
        assert_eq!(second.step.step_index, 1);
        assert_eq!(second.step.run_state.effect_result_count, 1);
        let terminal = EffectStepLoop::continue_snapshot(
            &catalog,
            second.clone(),
            ok_response(&second, json!({"index": 2})),
            "parent",
        )
        .expect("snapshot completes");
        assert_eq!(terminal.step.effect_results.len(), 2);
    }

    #[test]
    fn snapshot_cancellation_is_terminal_without_consuming_budget() {
        let catalog = catalog();
        let snapshot = EffectStepLoop::start_snapshot(
            &catalog,
            request(
                "run_cancel",
                json!([{"kind": "tool", "name": "read_first", "input": {}}]),
            ),
            "parent",
            EmbeddedRunLimits::default(),
        )
        .expect("snapshot starts");
        let cancelled =
            EffectStepLoop::cancel_snapshot(&catalog, snapshot, "parent", "user stopped the run")
                .expect("snapshot cancels");
        assert_eq!(cancelled.step.status, EmbeddedRunStepStatus::Cancelled);
        assert_eq!(cancelled.progress.dispatched_effect_count, 0);
    }

    #[test]
    fn snapshot_closes_at_effect_budget() {
        let catalog = catalog();
        let snapshot = EffectStepLoop::start_snapshot(
            &catalog,
            request(
                "run_budget",
                json!([{"kind": "tool", "name": "read_first", "input": {}}]),
            ),
            "parent",
            EmbeddedRunLimits {
                max_effect_steps: 0,
                max_subagent_depth: 1,
            },
        )
        .expect("snapshot closes");
        assert_eq!(snapshot.step.status, EmbeddedRunStepStatus::ClosedEarly);
        assert!(snapshot.progress.effect_budget_exhausted);
    }

    #[test]
    fn subagent_inherits_shared_budget() {
        let catalog = catalog();
        let parent = EffectStepLoop::start_snapshot(
            &catalog,
            request(
                "run_parent",
                json!([{
                    "kind": "subagent",
                    "agent_id": "child",
                    "input": {"effects": [{"kind": "tool", "name": "read_first", "input": {}}]},
                    "metadata": {}
                }]),
            ),
            "parent",
            EmbeddedRunLimits::default(),
        )
        .expect("parent starts");
        let child =
            EffectStepLoop::start_requested_subagent(&catalog, &parent).expect("child starts");
        assert_eq!(child.progress.dispatched_effect_count, 1);
        assert_eq!(child.progress.subagent_depth, 1);
        let child = EffectStepLoop::continue_snapshot(
            &catalog,
            child.clone(),
            ok_response(&child, json!({"ok": true})),
            "child",
        )
        .expect("child completes");
        let parent = EffectStepLoop::resume_parent_from_subagent(&catalog, parent, child)
            .expect("parent resumes");
        assert_eq!(parent.step.status, EmbeddedRunStepStatus::Completed);
        assert_eq!(parent.progress.dispatched_effect_count, 2);
    }

    #[test]
    fn tampered_derived_state_is_rejected() {
        let catalog = catalog();
        let mut snapshot = EffectStepLoop::start_snapshot(
            &catalog,
            request(
                "run_tampered",
                json!([{"kind": "tool", "name": "read_first", "input": {}}]),
            ),
            "parent",
            EmbeddedRunLimits::default(),
        )
        .expect("snapshot starts");
        snapshot.step.run_state.step_index = 99;
        let error = EffectStepLoop::continue_snapshot(
            &catalog,
            snapshot.clone(),
            ok_response(&snapshot, json!({})),
            "parent",
        )
        .expect_err("tampering is rejected");
        assert!(error.record.message.contains("run_state"));
    }
}
