use std::collections::HashSet;

use agent_core::{
    AgentError, AgentRuntimeCatalog, AgentSpec, EMBEDDED_SNAPSHOT_VERSION, EmbeddedEffectResponse,
    EmbeddedEffectResult, EmbeddedHostEffect, EmbeddedPendingHostEffect, EmbeddedRunContinuation,
    EmbeddedRunSnapshot, EmbeddedRunStep, EmbeddedRunStepStatus, RunRequest, ToolOutcomeStatus,
    ToolSpec, catalog_version, infer_tool_outcome, protocol_version,
};
use serde_json::{Value, json};

use super::effects::{
    derived_run_state, derived_trace_event, effect_kind, validate_pending_effect,
};

pub(super) fn validate_snapshot(
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

pub(super) fn validate_step(
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

pub(super) fn validate_continuation(
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

pub(super) fn validate_effect_response(
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
        (Some(result), None) => {
            if response
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.status == ToolOutcomeStatus::Ok)
                && infer_tool_outcome(result, false).is_error()
            {
                return Err(validation(
                    "effect response outcome conflicts with its result payload",
                ));
            }
            Ok(())
        }
        (None, Some(_)) => {
            if response
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.status == ToolOutcomeStatus::Ok)
            {
                return Err(validation(
                    "effect response with a JSON-RPC error cannot have an ok outcome",
                ));
            }
            Ok(())
        }
        (Some(_), Some(_)) => Err(validation(
            "effect response cannot contain both result and error",
        )),
        (None, None) => Err(validation("effect response must contain result or error")),
    }
}

pub(super) fn normalize_run_request(request: &mut RunRequest) -> Result<(), AgentError> {
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

pub(super) fn validate_catalog(catalog: &AgentRuntimeCatalog) -> Result<(), AgentError> {
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

pub(super) fn catalog_agent<'a>(
    catalog: &'a AgentRuntimeCatalog,
    agent_id: &str,
) -> Result<&'a AgentSpec, AgentError> {
    catalog
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| validation(format!("agent '{agent_id}' is not in the active catalog")))
}

pub(super) fn catalog_tool<'a>(
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

pub(super) fn require_object(value: &Value, label: &str) -> Result<(), AgentError> {
    if value.is_object() {
        Ok(())
    } else {
        Err(validation(format!("{label} must be a JSON object")))
    }
}

pub(super) fn validation(message: impl Into<String>) -> AgentError {
    AgentError::validation(message.into())
}
