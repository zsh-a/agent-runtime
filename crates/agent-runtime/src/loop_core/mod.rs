mod effects;
mod transition;
mod validation;

use agent_core::{
    AgentError, AgentRuntimeCatalog, EMBEDDED_SNAPSHOT_VERSION, EmbeddedEffectResponse,
    EmbeddedEffectResult, EmbeddedHostEffect, EmbeddedRunContinuation, EmbeddedRunLimits,
    EmbeddedRunProgress, EmbeddedRunSnapshot, EmbeddedRunStep, EmbeddedRunStepStatus, RunId,
    RunRequest, protocol_version,
};
use serde_json::json;

use self::effects::{
    completed_effect_step, completed_passthrough_step, continuation_for, effect_kind,
    effect_requested_step, effect_response_error_payload, effect_response_terminal_status,
    parse_effect_plan, terminal_effect_step,
};
use self::transition::{advance_snapshot, close_snapshot_at_limits};
use self::validation::{
    catalog_agent, normalize_run_request, validate_catalog, validate_effect_response,
    validate_snapshot, validate_step, validation,
};

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

#[cfg(test)]
mod tests;
