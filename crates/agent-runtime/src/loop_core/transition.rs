use agent_core::{
    AgentError, AgentRuntimeCatalog, EmbeddedEffectResponse, EmbeddedHostEffect,
    EmbeddedRunSnapshot,
};
use serde_json::json;

use super::EffectStepLoop;
use super::validation::{validate_snapshot, validation};

pub(super) fn advance_snapshot(
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

pub(super) fn close_snapshot_at_limits(
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
