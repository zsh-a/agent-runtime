use agent_core::{
    AgentError, AgentRuntimeCatalog, EMBEDDED_SNAPSHOT_VERSION, EffectId, EmbeddedEffectResponse,
    EmbeddedHostEffect, EmbeddedRunLimits, EmbeddedRunProgress, EmbeddedRunSnapshot,
    EmbeddedRunStep, RunId, RunRequest, ToolSpec, protocol_version,
};
use serde_json::{Map, Value, json};

pub struct EffectStepLoop;

impl EffectStepLoop {
    /// Start a versioned embedded snapshot with runtime-owned effect and
    /// subagent limits.
    pub fn start_snapshot(
        catalog: &AgentRuntimeCatalog,
        request: RunRequest,
        agent_id: &str,
        limits: EmbeddedRunLimits,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        Self::start_snapshot_at_depth(catalog, request, agent_id, limits, 0, 0)
    }

    /// Continue a host-persisted snapshot. The runtime validates limits and
    /// checkpoint identity before consuming the host response.
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

    /// Cancel a non-terminal embedded snapshot without dispatching its pending
    /// host effect. Cancellation is recorded as a runtime-owned terminal step
    /// and does not consume the shared effect budget.
    pub fn cancel_snapshot(
        catalog: &AgentRuntimeCatalog,
        snapshot: EmbeddedRunSnapshot,
        agent_id: &str,
        reason: &str,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        validate_snapshot(&snapshot, agent_id)?;
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
        let response = EmbeddedEffectResponse {
            jsonrpc: "2.0".to_owned(),
            id: effect_id,
            result: Some(json!({
                "error": {
                    "code": "user_cancel",
                    "message": message,
                }
            })),
            error: None,
        };
        advance_snapshot(catalog, snapshot, response, agent_id, false)
    }

    /// Start the subagent currently requested by `parent`. The child inherits
    /// the parent's shared effect budget and advances subagent depth in Rust.
    pub fn start_requested_subagent(
        catalog: &AgentRuntimeCatalog,
        parent: &EmbeddedRunSnapshot,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        validate_snapshot(parent, &parent.step.agent_id)?;
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

    /// Resume a parent from a terminal child snapshot without asking the host
    /// to synthesize subagent bookkeeping or shared-budget counters.
    pub fn resume_parent_from_subagent(
        catalog: &AgentRuntimeCatalog,
        mut parent: EmbeddedRunSnapshot,
        child: EmbeddedRunSnapshot,
    ) -> Result<EmbeddedRunSnapshot, AgentError> {
        validate_snapshot(&parent, &parent.step.agent_id)?;
        validate_snapshot(&child, &child.step.agent_id)?;
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
        let response = EmbeddedEffectResponse {
            jsonrpc: "2.0".to_owned(),
            id: effect_id.clone(),
            result: Some(json!({
                "agent_id": child.step.agent_id,
                "terminal_step": child.step,
                "snapshot": child,
            })),
            error: None,
        };
        parent.progress.dispatched_effect_count = child.progress.dispatched_effect_count;
        parent.progress.effect_budget_exhausted |= child.progress.effect_budget_exhausted;
        parent.progress.subagent_depth_exceeded |= child.progress.subagent_depth_exceeded;
        let parent_agent_id = parent.step.agent_id.clone();
        advance_snapshot(catalog, parent, response, &parent_agent_id, false)
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

    /// Start an embedded run and return its typed, serializable checkpoint.
    ///
    /// Hosts should prefer this method. `start_step` remains available for
    /// agent.v1 consumers that still exchange untyped JSON values.
    pub fn start_typed(
        catalog: &AgentRuntimeCatalog,
        request: RunRequest,
        agent_id: &str,
    ) -> Result<EmbeddedRunStep, AgentError> {
        let value = Self::start_step(catalog, request, agent_id)?;
        serde_json::from_value(value).map_err(|error| {
            AgentError::internal(format!(
                "runtime produced an invalid embedded run step: {error}"
            ))
        })
    }

    /// Continue a typed embedded checkpoint with a typed host response.
    pub fn continue_typed(
        catalog: &AgentRuntimeCatalog,
        previous_step: EmbeddedRunStep,
        effect_response: EmbeddedEffectResponse,
        agent_id: &str,
    ) -> Result<EmbeddedRunStep, AgentError> {
        let previous_step = serde_json::to_value(previous_step).map_err(|error| {
            AgentError::internal(format!("failed to encode embedded run checkpoint: {error}"))
        })?;
        let effect_response = serde_json::to_value(effect_response).map_err(|error| {
            AgentError::internal(format!(
                "failed to encode embedded effect response: {error}"
            ))
        })?;
        let value = Self::continue_step(catalog, previous_step, effect_response, agent_id)?;
        serde_json::from_value(value).map_err(|error| {
            AgentError::internal(format!(
                "runtime produced an invalid embedded run step: {error}"
            ))
        })
    }

    pub fn start_step(
        catalog: &AgentRuntimeCatalog,
        mut request: RunRequest,
        agent_id: &str,
    ) -> Result<Value, AgentError> {
        require_catalog_contract(catalog)?;
        normalize_run_request_contract(&mut request)?;
        let agent = catalog_agent(catalog, agent_id)?;
        let run_id = request.run_id.clone().unwrap_or_else(RunId::new_v7);

        let mut response = match parse_initial_effect_request(&request.input)? {
            Some(effect_request) => {
                let continuation = effect_request.continuation();
                build_effect_requested_step(
                    catalog,
                    &agent.id,
                    &agent.version,
                    serde_json::to_value(&run_id)
                        .map_err(|error| AgentError::internal(error.to_string()))?,
                    effect_request.first,
                    continuation,
                )?
            }
            None => json!({
                "protocol_version": protocol_version(),
                "run_id": run_id,
                "agent_id": agent.id,
                "agent_version": agent.version,
                "step_index": 0,
                "status": "completed",
                "output": request.input,
            }),
        };
        attach_runtime_metadata(&mut response);
        Ok(response)
    }

    pub fn continue_step(
        catalog: &AgentRuntimeCatalog,
        previous_step: Value,
        effect_response: Value,
        agent_id: &str,
    ) -> Result<Value, AgentError> {
        require_catalog_contract(catalog)?;
        let agent = catalog_agent(catalog, agent_id)?;
        let previous_kind = previous_effect_kind(&previous_step)?;
        require_previous_step_protocol_version(&previous_step)?;
        require_previous_step_agent(&previous_step, &agent.id, &agent.version)?;
        let run_id = require_previous_step_run_id(&previous_step)?;
        let previous_step_index = require_previous_step_index(&previous_step)?;
        let effect_call = previous_effect_call(&previous_step, previous_kind)?.clone();
        require_previous_effect_call_id(&effect_call, previous_kind)?;
        require_previous_effect_catalog_entry(catalog, &agent.id, &effect_call, previous_kind)?;
        require_previous_step_runtime_metadata(&previous_step, &run_id, previous_step_index)?;
        require_effect_response_envelope(&effect_response)?;
        require_matching_effect_response_id(&effect_call, &effect_response, previous_kind)?;

        let previous_continuation =
            RuntimeContinuation::from_step(&previous_step, catalog, &agent.id)?;
        if let Some(continuation) = &previous_continuation {
            continuation.require_next_step_index(previous_step_index)?;
        }
        let mut effect_results = previous_continuation
            .as_ref()
            .map(|continuation| continuation.effect_results.clone())
            .unwrap_or_default();
        let terminal_status = effect_response_terminal_status(&effect_response);
        if terminal_status != Some("closed_early") {
            effect_results.push(EffectResultRecord::new(
                previous_kind,
                effect_call.clone(),
                effect_response.clone(),
            ));
        }

        let next_step_index = previous_step_index + 1;
        let mut response = match terminal_status {
            Some(status) => {
                let error = effect_response_error_payload(&effect_response).unwrap_or(Value::Null);
                let effect_results_json = EffectResultRecord::to_values(&effect_results);
                json!({
                    "protocol_version": protocol_version(),
                    "run_id": run_id,
                    "agent_id": agent.id,
                    "agent_version": agent.version,
                    "step_index": next_step_index,
                    "status": status,
                    "effect": effect_call,
                    "effect_response": effect_response,
                    "effect_results": effect_results_json,
                    "error": error,
                })
            }
            None => match next_effect_request_from_continuation(
                previous_continuation,
                effect_results.clone(),
            )? {
                Some(next) => {
                    let continuation = next.continuation();
                    build_effect_requested_step(
                        catalog,
                        &agent.id,
                        &agent.version,
                        run_id,
                        next.first,
                        continuation,
                    )?
                }
                None => {
                    let effect_results_json = EffectResultRecord::to_values(&effect_results);
                    terminal_completed_step(
                        &agent.id,
                        &agent.version,
                        run_id,
                        next_step_index,
                        effect_call,
                        effect_response,
                        effect_results_json,
                    )
                }
            },
        };
        attach_runtime_metadata(&mut response);
        Ok(response)
    }
}

fn validate_snapshot(snapshot: &EmbeddedRunSnapshot, agent_id: &str) -> Result<(), AgentError> {
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
    if snapshot.step.protocol_version != snapshot.protocol_version {
        return Err(validation(
            "embedded snapshot step protocol_version does not match snapshot",
        ));
    }
    if snapshot.step.agent_id != agent_id {
        return Err(validation(format!(
            "embedded snapshot agent '{}' does not match requested agent '{agent_id}'",
            snapshot.step.agent_id
        )));
    }
    if snapshot.progress.dispatched_effect_count > snapshot.limits.max_effect_steps {
        return Err(validation(
            "embedded dispatched effect count exceeds max_effect_steps",
        ));
    }
    Ok(())
}

fn advance_snapshot(
    catalog: &AgentRuntimeCatalog,
    mut snapshot: EmbeddedRunSnapshot,
    effect_response: EmbeddedEffectResponse,
    agent_id: &str,
    count_dispatch: bool,
) -> Result<EmbeddedRunSnapshot, AgentError> {
    validate_snapshot(&snapshot, agent_id)?;
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
    validate_snapshot(&snapshot, agent_id)?;
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
    let response = EmbeddedEffectResponse {
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
    };
    snapshot.step = EffectStepLoop::continue_typed(catalog, snapshot.step, response, agent_id)?;
    Ok(snapshot)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEffectKind {
    Tool,
    Subagent,
}

impl RunEffectKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Subagent => "subagent",
        }
    }

    fn id_field(self) -> &'static str {
        "effect_id"
    }

    fn trace_tool_name(self, call: &Value) -> Value {
        match self {
            Self::Tool => call.get("name").cloned().unwrap_or(Value::Null),
            Self::Subagent => Value::Null,
        }
    }

    fn trace_subagent_id(self, call: &Value) -> Value {
        match self {
            Self::Tool => Value::Null,
            Self::Subagent => call.get("agent_id").cloned().unwrap_or(Value::Null),
        }
    }
}

#[derive(Debug, Clone)]
enum RequestedEffect {
    Tool(RequestedToolEffect),
    Subagent(Box<RequestedSubagentEffect>),
}

#[derive(Debug, Clone)]
struct RequestedToolEffect {
    name: String,
    input: Value,
}

#[derive(Debug, Clone)]
struct RequestedSubagentEffect {
    agent_id: String,
    input: Value,
    run_id: Option<Value>,
    scope: Option<Value>,
    workflow: Option<Value>,
    metadata: Value,
}

#[derive(Debug, Clone)]
struct EffectResultRecord {
    kind: RunEffectKind,
    effect_call: Value,
    effect_response: Value,
}

impl EffectResultRecord {
    fn new(kind: RunEffectKind, effect_call: Value, effect_response: Value) -> Self {
        Self {
            kind,
            effect_call,
            effect_response,
        }
    }

    fn from_value(
        value: &Value,
        label: &str,
        catalog: &AgentRuntimeCatalog,
        agent_id: &str,
    ) -> Result<Self, AgentError> {
        let object = value
            .as_object()
            .ok_or_else(|| validation(format!("{label} must be an object")))?;
        let effect_call = object
            .get("effect")
            .ok_or_else(|| validation(format!("{label}.effect is required")))?;
        let kind = effect_kind_from_call(effect_call)?;
        require_previous_effect_catalog_entry(catalog, agent_id, effect_call, kind)?;
        let effect_response = object
            .get("effect_response")
            .ok_or_else(|| validation(format!("{label}.effect_response is required")))?;
        require_effect_response_envelope(effect_response).map_err(|error| {
            validation(format!("{label}.effect_response: {}", error.record.message))
        })?;
        require_matching_effect_response_id(effect_call, effect_response, kind).map_err(
            |error| validation(format!("{label}.effect_response: {}", error.record.message)),
        )?;
        Ok(Self::new(
            kind,
            effect_call.clone(),
            effect_response.clone(),
        ))
    }

    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            "kind".to_owned(),
            Value::String(match self.kind {
                RunEffectKind::Tool => "tool".to_owned(),
                RunEffectKind::Subagent => "subagent".to_owned(),
            }),
        );
        object.insert("effect".to_owned(), self.effect_call.clone());
        object.insert("effect_response".to_owned(), self.effect_response.clone());
        Value::Object(object)
    }

    fn to_values(records: &[Self]) -> Vec<Value> {
        records.iter().map(Self::to_value).collect()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RuntimeContinuationCounts {
    remaining_effect_count: usize,
    effect_result_count: usize,
}

#[derive(Debug, Clone)]
struct RuntimeContinuation {
    effects: Vec<Value>,
    effect_results: Vec<EffectResultRecord>,
    llm_response: Option<Value>,
    next_step_index: u64,
}

impl RuntimeContinuation {
    fn from_effect_request_state(state: &EffectRequestState) -> Option<Self> {
        if state.remaining.is_empty()
            && state.effect_results.is_empty()
            && state.llm_response.is_none()
            && state.step_index == 0
        {
            return None;
        }
        Some(Self {
            effects: state.remaining.clone(),
            effect_results: state.effect_results.clone(),
            llm_response: state.llm_response.clone(),
            next_step_index: state.step_index + 1,
        })
    }

    fn from_step(
        previous_step: &Value,
        catalog: &AgentRuntimeCatalog,
        agent_id: &str,
    ) -> Result<Option<Self>, AgentError> {
        let Some(value) = previous_step.get("continuation") else {
            return Ok(None);
        };
        let object = value
            .as_object()
            .ok_or_else(|| validation("continuation must be an object"))?;
        let effects = match object.get("effects") {
            Some(value) => value
                .as_array()
                .cloned()
                .ok_or_else(|| validation("continuation.effects must be an array"))?,
            None => Vec::new(),
        };
        let effect_results = match object.get("effect_results") {
            Some(value) => Self::effect_results_from_value(value, catalog, agent_id)?,
            None => Vec::new(),
        };
        let next_step_index = object
            .get("next_step_index")
            .ok_or_else(|| {
                validation(
                    "continuation.next_step_index must be present when continuation is present",
                )
            })?
            .as_u64()
            .ok_or_else(|| {
                validation("continuation.next_step_index must be a non-negative integer")
            })?;
        Ok(Some(Self {
            effects,
            effect_results,
            llm_response: object.get("llm_response").cloned(),
            next_step_index,
        }))
    }

    fn counts_from_step(step: &Value) -> RuntimeContinuationCounts {
        let Some(continuation) = step.get("continuation").and_then(Value::as_object) else {
            return RuntimeContinuationCounts::default();
        };
        RuntimeContinuationCounts {
            remaining_effect_count: continuation
                .get("effects")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0),
            effect_result_count: continuation
                .get("effect_results")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0),
        }
    }

    fn effect_results_from_value(
        value: &Value,
        catalog: &AgentRuntimeCatalog,
        agent_id: &str,
    ) -> Result<Vec<EffectResultRecord>, AgentError> {
        let results = value
            .as_array()
            .ok_or_else(|| validation("continuation.effect_results must be an array"))?;
        results
            .iter()
            .enumerate()
            .map(|(index, result)| {
                EffectResultRecord::from_value(
                    result,
                    &format!("continuation.effect_results[{index}]"),
                    catalog,
                    agent_id,
                )
            })
            .collect()
    }

    fn validate_effect_plan(
        &self,
        catalog: &AgentRuntimeCatalog,
        agent_id: &str,
    ) -> Result<(), AgentError> {
        for (index, effect) in self.effects.iter().enumerate() {
            let requested =
                parse_requested_effect(effect, &format!("continuation.effects[{index}]"))?;
            validate_requested_effect(catalog, agent_id, &requested).map_err(|error| {
                validation(format!(
                    "continuation.effects[{index}]: {}",
                    error.record.message
                ))
            })?;
        }
        Ok(())
    }

    fn require_next_step_index(&self, previous_step_index: u64) -> Result<(), AgentError> {
        let expected = previous_step_index + 1;
        if self.next_step_index != expected {
            return Err(validation(format!(
                "continuation.next_step_index {} must equal previous step_index + 1 ({expected})",
                self.next_step_index
            )));
        }
        Ok(())
    }

    fn requested_step_index(&self) -> u64 {
        self.next_step_index.saturating_sub(1)
    }

    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("effects".to_owned(), Value::Array(self.effects.clone()));
        object.insert(
            "effect_results".to_owned(),
            Value::Array(EffectResultRecord::to_values(&self.effect_results)),
        );
        if let Some(llm_response) = &self.llm_response {
            object.insert("llm_response".to_owned(), llm_response.clone());
        }
        object.insert(
            "next_step_index".to_owned(),
            Value::Number(serde_json::Number::from(self.next_step_index)),
        );
        Value::Object(object)
    }
}

#[derive(Debug)]
struct EffectRequestState {
    first: RequestedEffect,
    remaining: Vec<Value>,
    effect_results: Vec<EffectResultRecord>,
    llm_response: Option<Value>,
    step_index: u64,
}

impl EffectRequestState {
    fn continuation(&self) -> Option<RuntimeContinuation> {
        RuntimeContinuation::from_effect_request_state(self)
    }
}

fn parse_initial_effect_request(input: &Value) -> Result<Option<EffectRequestState>, AgentError> {
    if let Some(plan) = input.get("effects") {
        let plan = plan
            .as_array()
            .ok_or_else(|| validation("effects must be an array"))?;
        if plan.is_empty() {
            return Ok(None);
        }
        let first = parse_requested_effect(&plan[0], "effects[0]")?;
        return Ok(Some(EffectRequestState {
            first,
            remaining: plan[1..].to_vec(),
            effect_results: Vec::new(),
            llm_response: input.get("llm_response").cloned(),
            step_index: 0,
        }));
    }
    if let Some(effect) = input.get("effect") {
        return Ok(Some(EffectRequestState {
            first: parse_requested_effect(effect, "effect")?,
            remaining: Vec::new(),
            effect_results: Vec::new(),
            llm_response: input.get("llm_response").cloned(),
            step_index: 0,
        }));
    }
    Ok(None)
}

fn next_effect_request_from_continuation(
    continuation: Option<RuntimeContinuation>,
    effect_results: Vec<EffectResultRecord>,
) -> Result<Option<EffectRequestState>, AgentError> {
    let Some(continuation) = continuation else {
        return Ok(None);
    };
    if continuation.effects.is_empty() {
        return Ok(None);
    }
    let first = parse_requested_effect(&continuation.effects[0], "continuation.effects[0]")?;
    Ok(Some(EffectRequestState {
        first,
        remaining: continuation.effects[1..].to_vec(),
        effect_results,
        llm_response: continuation.llm_response,
        step_index: continuation.next_step_index,
    }))
}

fn parse_requested_effect(value: &Value, label: &str) -> Result<RequestedEffect, AgentError> {
    let object = value
        .as_object()
        .ok_or_else(|| validation(format!("{label} must be an object")))?;
    match object.get("kind").and_then(Value::as_str) {
        Some("tool") => parse_requested_tool_effect(value, label).map(RequestedEffect::Tool),
        Some("subagent") => parse_requested_subagent_effect(value, label)
            .map(Box::new)
            .map(RequestedEffect::Subagent),
        Some(kind) => Err(validation(format!(
            "{label}.kind '{kind}' is not supported"
        ))),
        None => Err(validation(format!("{label}.kind is required"))),
    }
}

fn parse_requested_tool_effect(
    value: &Value,
    label: &str,
) -> Result<RequestedToolEffect, AgentError> {
    let object = value
        .as_object()
        .ok_or_else(|| validation(format!("{label} must be an object")))?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| validation(format!("{label}.name is required")))?
        .to_owned();
    let input = match object.get("input") {
        Some(input) if input.is_object() => input.clone(),
        Some(_) => return Err(validation(format!("{label}.input must be an object"))),
        None => json!({}),
    };
    Ok(RequestedToolEffect { name, input })
}

fn parse_requested_subagent_effect(
    value: &Value,
    label: &str,
) -> Result<RequestedSubagentEffect, AgentError> {
    let object = value
        .as_object()
        .ok_or_else(|| validation(format!("{label} must be an object")))?;
    let agent_id = object
        .get("agent_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| validation(format!("{label}.agent_id is required")))?
        .trim()
        .to_owned();
    let metadata = object.get("metadata").cloned().unwrap_or_else(|| json!({}));
    if !metadata.is_object() {
        return Err(validation(format!("{label}.metadata must be an object")));
    }
    Ok(RequestedSubagentEffect {
        agent_id,
        input: object.get("input").cloned().unwrap_or_else(|| json!({})),
        run_id: object.get("run_id").cloned(),
        scope: object.get("scope").cloned(),
        workflow: object.get("workflow").cloned(),
        metadata,
    })
}

fn build_effect_requested_step(
    catalog: &AgentRuntimeCatalog,
    agent_id: &str,
    agent_version: &str,
    run_id: Value,
    requested: RequestedEffect,
    continuation: Option<RuntimeContinuation>,
) -> Result<Value, AgentError> {
    if let Some(continuation) = &continuation {
        continuation.validate_effect_plan(catalog, agent_id)?;
    }
    validate_requested_effect(catalog, agent_id, &requested)?;
    let call = build_effect_call(catalog, agent_id, requested)?;
    let mut step = json!({
        "protocol_version": protocol_version(),
        "run_id": run_id,
        "agent_id": agent_id,
        "agent_version": agent_version,
        "step_index": continuation
            .as_ref()
            .map(RuntimeContinuation::requested_step_index)
            .unwrap_or(0),
        "status": "effect_requested",
        "effect": call,
    });
    if let Some(continuation) = continuation {
        step.as_object_mut()
            .expect("step is an object")
            .insert("continuation".to_owned(), continuation.to_value());
    }
    attach_runtime_metadata(&mut step);
    Ok(step)
}

fn build_effect_call(
    catalog: &AgentRuntimeCatalog,
    agent_id: &str,
    requested: RequestedEffect,
) -> Result<Value, AgentError> {
    match requested {
        RequestedEffect::Tool(tool_effect) => {
            let tool = catalog_tool(catalog, agent_id, &tool_effect.name)?;
            Ok(json!({
                "effect_id": EffectId::new_v7(),
                "kind": "tool",
                "name": tool.name,
                "input": tool_effect.input,
                "risk": tool.risk,
                "metadata": tool.metadata,
            }))
        }
        RequestedEffect::Subagent(subagent_effect) => {
            let subagent_effect = *subagent_effect;
            catalog_agent(catalog, &subagent_effect.agent_id)?;
            let mut call = json!({
                "effect_id": EffectId::new_v7(),
                "kind": "subagent",
                "agent_id": subagent_effect.agent_id,
                "input": subagent_effect.input,
                "metadata": subagent_effect.metadata,
            });
            let object = call.as_object_mut().expect("subagent effect is an object");
            if let Some(run_id) = subagent_effect.run_id {
                object.insert("run_id".to_owned(), run_id);
            }
            if let Some(scope) = subagent_effect.scope {
                object.insert("scope".to_owned(), scope);
            }
            if let Some(workflow) = subagent_effect.workflow {
                object.insert("workflow".to_owned(), workflow);
            }
            Ok(call)
        }
    }
}

fn terminal_completed_step(
    agent_id: &str,
    agent_version: &str,
    run_id: Value,
    step_index: u64,
    effect_call: Value,
    effect_response: Value,
    effect_results: Vec<Value>,
) -> Value {
    let mode = if effect_results.len() > 1 {
        "frb_effect_loop"
    } else {
        "frb_effect_step"
    };
    let output = json!({
        "mode": mode,
        "effect": effect_call,
        "effect_result": effect_response.get("result").cloned().unwrap_or(Value::Null),
        "effect_response": effect_response,
        "effect_results": effect_results.clone(),
    });

    let mut step = json!({
        "protocol_version": protocol_version(),
        "run_id": run_id,
        "agent_id": agent_id,
        "agent_version": agent_version,
        "step_index": step_index,
        "status": "completed",
        "output": output,
    });
    let object = step.as_object_mut().expect("step is an object");
    object.insert("effect".to_owned(), effect_call.clone());
    object.insert("effect_response".to_owned(), effect_response.clone());
    object.insert(
        "effect_results".to_owned(),
        Value::Array(effect_results.clone()),
    );
    step
}

fn previous_effect_kind(previous_step: &Value) -> Result<RunEffectKind, AgentError> {
    match previous_step.get("status").and_then(Value::as_str) {
        Some("effect_requested") => {
            let effect = previous_step
                .get("effect")
                .ok_or_else(|| validation("previous step is missing effect"))?;
            effect_kind_from_call(effect)
        }
        _ => Err(validation(
            "previous step status must be 'effect_requested'",
        )),
    }
}

fn previous_effect_call(previous_step: &Value, kind: RunEffectKind) -> Result<&Value, AgentError> {
    let _ = kind;
    previous_step
        .get("effect")
        .ok_or_else(|| validation("previous step is missing effect"))
}

fn effect_kind_from_call(call: &Value) -> Result<RunEffectKind, AgentError> {
    let object = call
        .as_object()
        .ok_or_else(|| validation("effect must be an object"))?;
    match object.get("kind").and_then(Value::as_str) {
        Some("tool") => Ok(RunEffectKind::Tool),
        Some("subagent") => Ok(RunEffectKind::Subagent),
        Some(kind) => Err(validation(format!("effect kind '{kind}' is not supported"))),
        None => Err(validation("effect.kind is required")),
    }
}

fn require_previous_step_agent(
    previous_step: &Value,
    agent_id: &str,
    agent_version: &str,
) -> Result<(), AgentError> {
    let previous_agent_id = previous_step
        .get("agent_id")
        .and_then(Value::as_str)
        .ok_or_else(|| validation("previous step is missing agent_id"))?;
    if previous_agent_id != agent_id {
        return Err(validation(format!(
            "previous step agent_id '{previous_agent_id}' does not match requested agent '{agent_id}'"
        )));
    }
    let previous_agent_version = previous_step
        .get("agent_version")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| validation("previous step agent_version must be a non-empty string"))?;
    if previous_agent_version != agent_version {
        return Err(validation(format!(
            "previous step agent_version '{previous_agent_version}' does not match catalog agent version '{agent_version}'"
        )));
    }
    Ok(())
}

fn require_previous_step_protocol_version(previous_step: &Value) -> Result<(), AgentError> {
    let previous_protocol_version = previous_step
        .get("protocol_version")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| validation("previous step protocol_version must be a non-empty string"))?;
    let expected = protocol_version();
    if previous_protocol_version != expected {
        return Err(validation(format!(
            "previous step protocol_version '{previous_protocol_version}' does not match runtime protocol_version '{expected}'"
        )));
    }
    Ok(())
}

fn require_previous_step_run_id(previous_step: &Value) -> Result<Value, AgentError> {
    let run_id = previous_step
        .get("run_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| validation("previous step run_id must be a non-empty string"))?;
    Ok(Value::String(run_id.to_owned()))
}

fn require_previous_step_index(previous_step: &Value) -> Result<u64, AgentError> {
    previous_step
        .get("step_index")
        .and_then(Value::as_u64)
        .ok_or_else(|| validation("previous step_index must be a non-negative integer"))
}

fn require_previous_effect_catalog_entry(
    catalog: &AgentRuntimeCatalog,
    agent_id: &str,
    effect_call: &Value,
    kind: RunEffectKind,
) -> Result<(), AgentError> {
    match kind {
        RunEffectKind::Tool => {
            let name = effect_call
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| validation("previous step effect.name is required"))?;
            catalog_tool(catalog, agent_id, name)?;
            require_effect_call_input_object(effect_call, "previous step effect")?;
        }
        RunEffectKind::Subagent => {
            let subagent_id = effect_call
                .get("agent_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| validation("previous step effect.agent_id is required"))?;
            catalog_agent(catalog, subagent_id)?;
        }
    }
    Ok(())
}

fn require_previous_effect_call_id(
    effect_call: &Value,
    kind: RunEffectKind,
) -> Result<(), AgentError> {
    effect_call
        .get(kind.id_field())
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            validation(format!(
                "previous step effect.{} must be a non-empty string",
                kind.id_field()
            ))
        })?;
    Ok(())
}

fn require_previous_step_runtime_metadata(
    previous_step: &Value,
    run_id: &Value,
    step_index: u64,
) -> Result<(), AgentError> {
    let run_state = previous_step
        .get("run_state")
        .ok_or_else(|| validation("previous step run_state is required"))?;
    require_previous_step_run_state(previous_step, run_state, step_index).map_err(|error| {
        validation(format!("previous step run_state: {}", error.record.message))
    })?;
    let trace_event = previous_step
        .get("trace_event")
        .ok_or_else(|| validation("previous step trace_event is required"))?;
    require_previous_step_trace_event(previous_step, trace_event, run_id, step_index).map_err(
        |error| {
            validation(format!(
                "previous step trace_event: {}",
                error.record.message
            ))
        },
    )?;
    Ok(())
}

fn require_previous_step_run_state(
    previous_step: &Value,
    run_state: &Value,
    step_index: u64,
) -> Result<(), AgentError> {
    let run_state = run_state
        .as_object()
        .ok_or_else(|| validation("previous step run_state must be an object"))?;
    require_step_status(run_state, "status")?;
    require_matching_string(
        run_state,
        "status",
        previous_step
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    match run_state.get("step_index").and_then(Value::as_u64) {
        Some(value) if value == step_index => {}
        _ => {
            return Err(validation(
                "previous step run_state.step_index must match step_index",
            ));
        }
    }
    let continuation_counts = RuntimeContinuation::counts_from_step(previous_step);
    let expected_remaining = continuation_counts.remaining_effect_count as u64;
    let remaining = run_state
        .get("remaining_effect_count")
        .and_then(Value::as_u64);
    if remaining != Some(expected_remaining) {
        return Err(validation(
            "previous step run_state.remaining_effect_count must match continuation.effects",
        ));
    }
    let expected_results = continuation_counts.effect_result_count as u64;
    let results = run_state.get("effect_result_count").and_then(Value::as_u64);
    if results != Some(expected_results) {
        return Err(validation(
            "previous step run_state.effect_result_count must match continuation.effect_results",
        ));
    }
    require_terminal_reason(run_state, "terminal_reason")?;
    require_terminal_reason_matches_status(run_state)?;
    Ok(())
}

fn require_previous_step_trace_event(
    previous_step: &Value,
    trace_event: &Value,
    run_id: &Value,
    step_index: u64,
) -> Result<(), AgentError> {
    let trace_event = trace_event
        .as_object()
        .ok_or_else(|| validation("previous step trace_event must be an object"))?;
    require_matching_string(trace_event, "kind", "agent_runtime_step")?;
    require_matching_string(trace_event, "run_id", run_id.as_str().unwrap_or_default())?;
    require_matching_string(
        trace_event,
        "agent_id",
        previous_step
            .get("agent_id")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    require_matching_string(
        trace_event,
        "status",
        previous_step
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    match trace_event.get("step_index").and_then(Value::as_u64) {
        Some(value) if value == step_index => {}
        _ => {
            return Err(validation(
                "previous step trace_event.step_index must match step_index",
            ));
        }
    }
    let run_state = previous_step
        .get("run_state")
        .ok_or_else(|| validation("previous step run_state is required"))?;
    match trace_event.get("run_state") {
        Some(value) if value == run_state => {}
        _ => {
            return Err(validation(
                "previous step trace_event.run_state must match run_state",
            ));
        }
    }
    Ok(())
}

fn require_matching_effect_response_id(
    effect_call: &Value,
    effect_response: &Value,
    kind: RunEffectKind,
) -> Result<(), AgentError> {
    let expected_id = effect_call
        .get(kind.id_field())
        .and_then(Value::as_str)
        .ok_or_else(|| validation(format!("effect.{} is required", kind.id_field())))?;
    match effect_response.get("id").and_then(Value::as_str) {
        Some(value) if value == expected_id => Ok(()),
        Some(value) => Err(validation(format!(
            "effect response id '{value}' does not match requested {} '{expected_id}'",
            kind.id_field()
        ))),
        None => Err(validation("effect response id must be a string")),
    }
}

fn require_effect_response_envelope(effect_response: &Value) -> Result<(), AgentError> {
    let Some(object) = effect_response.as_object() else {
        return Err(validation("effect response must be an object"));
    };
    match object.get("jsonrpc").and_then(Value::as_str) {
        Some("2.0") => {}
        Some(_) => return Err(validation("effect response jsonrpc must be '2.0'")),
        None => return Err(validation("effect response jsonrpc must be '2.0'")),
    }
    if !object.contains_key("id") {
        return Err(validation("effect response id is required"));
    }
    match (object.contains_key("result"), object.contains_key("error")) {
        (true, false) | (false, true) => {}
        (true, true) => {
            return Err(validation(
                "effect response cannot contain both result and error",
            ));
        }
        (false, false) => {
            return Err(validation("effect response must contain result or error"));
        }
    }
    if let Some(error) = object.get("error") {
        let error = error
            .as_object()
            .ok_or_else(|| validation("effect response error must be an object"))?;
        if error.get("code").and_then(Value::as_i64).is_none() {
            return Err(validation("effect response error.code must be an integer"));
        }
        if error.get("message").and_then(Value::as_str).is_none() {
            return Err(validation("effect response error.message must be a string"));
        }
    }
    Ok(())
}

fn effect_response_terminal_status(effect_response: &Value) -> Option<&'static str> {
    let code = effect_response_error_code(effect_response);
    match code.as_deref() {
        Some("effect_budget_exhausted") => Some("closed_early"),
        Some("policy_denied") => Some("policy_denied"),
        Some("user_cancel" | "user_cancelled" | "cancelled") => Some("cancelled"),
        Some("tool_timeout" | "timeout" | "timed_out") => Some("timed_out"),
        Some(_) => Some("failed"),
        None if effect_response_error_payload(effect_response).is_some() => Some("failed"),
        None => None,
    }
}

fn effect_response_error_code(effect_response: &Value) -> Option<String> {
    effect_response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            effect_response
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| value.as_i64().map(|code| code.to_string()))
                })
        })
        .or_else(|| {
            effect_response
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            effect_response
                .get("result")
                .and_then(|result| result.get("error"))
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            effect_response
                .get("result")
                .and_then(|result| result.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

fn effect_response_error_payload(effect_response: &Value) -> Option<Value> {
    if let Some(error) = effect_response.get("error") {
        return Some(error.clone());
    }
    let result = effect_response.get("result")?;
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
    if let Some(code) = result.get("code").and_then(Value::as_str) {
        return Some(json!({ "code": code }));
    }
    None
}

fn attach_runtime_metadata(step: &mut Value) {
    attach_run_state(step);
    attach_trace_event(step);
}

fn attach_run_state(step: &mut Value) {
    let Some(object) = step.as_object_mut() else {
        return;
    };
    let continuation = object.get("continuation");
    let remaining_effect_count = continuation
        .and_then(|value| value.get("effects"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let continuation_result_count = continuation
        .and_then(|value| value.get("effect_results"))
        .and_then(Value::as_array)
        .map(Vec::len);
    let output_result_count = object
        .get("output")
        .and_then(|value| value.get("effect_results"))
        .and_then(Value::as_array)
        .map(Vec::len);
    let root_result_count = object
        .get("effect_results")
        .and_then(Value::as_array)
        .map(Vec::len);
    let effect_result_count = continuation_result_count
        .or(output_result_count)
        .or(root_result_count)
        .unwrap_or(0);
    let status = object.get("status").cloned().unwrap_or(Value::Null);
    let state = json!({
        "status": status,
        "step_index": object.get("step_index").cloned().unwrap_or(Value::Null),
        "remaining_effect_count": remaining_effect_count,
        "effect_result_count": effect_result_count,
        "terminal_reason": terminal_reason_for_status(object.get("status").and_then(Value::as_str)),
    });
    object.insert("run_state".to_owned(), state);
}

fn terminal_reason_for_status(status: Option<&str>) -> Value {
    match status {
        Some("completed") => Value::String("done".to_owned()),
        Some("failed") => Value::String("stream_error".to_owned()),
        Some("cancelled") => Value::String("user_cancel".to_owned()),
        Some("policy_denied") => Value::String("policy_denied".to_owned()),
        Some("closed_early") | Some("timed_out") => Value::String("closed_early".to_owned()),
        _ => Value::Null,
    }
}

fn attach_trace_event(step: &mut Value) {
    let Some(object) = step.as_object_mut() else {
        return;
    };
    let call = object
        .get("effect")
        .and_then(|call| effect_kind_from_call(call).ok().map(|kind| (kind, call)));
    let event = json!({
        "kind": "agent_runtime_step",
        "run_id": object.get("run_id").cloned().unwrap_or(Value::Null),
        "agent_id": object.get("agent_id").cloned().unwrap_or(Value::Null),
        "status": object.get("status").cloned().unwrap_or(Value::Null),
        "step_index": object.get("step_index").cloned().unwrap_or(Value::Null),
        "run_state": object.get("run_state").cloned().unwrap_or(Value::Null),
        "effect_id": call
            .and_then(|(_, call)| call.get("effect_id").cloned())
            .unwrap_or(Value::Null),
        "effect_kind": call
            .map(|(kind, _)| Value::String(kind.as_str().to_owned()))
            .unwrap_or(Value::Null),
        "tool_name": call
            .map(|(kind, call)| kind.trace_tool_name(call))
            .unwrap_or(Value::Null),
        "subagent_id": call
            .map(|(kind, call)| kind.trace_subagent_id(call))
            .unwrap_or(Value::Null),
    });
    object.insert("trace_event".to_owned(), event);
}

fn normalize_run_request_contract(request: &mut RunRequest) -> Result<(), AgentError> {
    agent_core::validate_protocol_version(&request.protocol_version).map_err(validation)?;
    if request.input.is_null() {
        request.input = json!({});
    } else if !request.input.is_object() {
        return Err(validation("run request input must be a JSON object"));
    }
    if request.metadata.is_null() {
        request.metadata = json!({});
    } else if !request.metadata.is_object() {
        return Err(validation("run request metadata must be a JSON object"));
    }
    if let Some(user) = &mut request.user {
        if user.user_id.trim().is_empty() {
            return Err(validation(
                "run request user.user_id must be a non-empty string",
            ));
        }
        if user.metadata.is_null() {
            user.metadata = json!({});
        } else if !user.metadata.is_object() {
            return Err(validation(
                "run request user.metadata must be a JSON object",
            ));
        }
    }
    Ok(())
}

fn require_catalog_contract(catalog: &AgentRuntimeCatalog) -> Result<(), AgentError> {
    catalog.validate_versions().map_err(validation)
}

fn validate_requested_effect(
    catalog: &AgentRuntimeCatalog,
    agent_id: &str,
    requested: &RequestedEffect,
) -> Result<(), AgentError> {
    match requested {
        RequestedEffect::Tool(tool_effect) => {
            catalog_tool(catalog, agent_id, &tool_effect.name)?;
            if !tool_effect.input.is_object() {
                return Err(validation("requested tool input must be an object"));
            }
            Ok(())
        }
        RequestedEffect::Subagent(subagent_effect) => {
            catalog_agent(catalog, &subagent_effect.agent_id)?;
            Ok(())
        }
    }
}

fn catalog_agent<'a>(
    catalog: &'a AgentRuntimeCatalog,
    agent_id: &str,
) -> Result<&'a agent_core::AgentSpec, AgentError> {
    catalog
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| validation(format!("agent '{agent_id}' is not present in the catalog")))
}

fn catalog_tool<'a>(
    catalog: &'a AgentRuntimeCatalog,
    agent_id: &str,
    name: &str,
) -> Result<&'a ToolSpec, AgentError> {
    catalog
        .tools
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| {
            validation(format!(
                "tool '{name}' requested by agent '{agent_id}' is not present in the catalog"
            ))
        })
}

fn require_effect_call_input_object(effect_call: &Value, label: &str) -> Result<(), AgentError> {
    if matches!(effect_call.get("input"), Some(input) if !input.is_object()) {
        return Err(validation(format!("{label}.input must be an object")));
    }
    Ok(())
}

fn require_non_empty_string(object: &Map<String, Value>, field: &str) -> Result<(), AgentError> {
    match object.get(field).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => Ok(()),
        _ => Err(validation(format!(
            "agent_runtime_step {field} must be a non-empty string"
        ))),
    }
}

fn require_matching_string(
    object: &Map<String, Value>,
    field: &str,
    expected: &str,
) -> Result<(), AgentError> {
    require_non_empty_string(object, field)?;
    match object.get(field).and_then(Value::as_str) {
        Some(value) if value == expected => Ok(()),
        _ => Err(validation(format!(
            "agent_runtime_step {field} must match trace {field}"
        ))),
    }
}

fn require_step_status(object: &Map<String, Value>, field: &str) -> Result<(), AgentError> {
    match object.get(field).and_then(Value::as_str) {
        Some(
            "effect_requested" | "completed" | "failed" | "cancelled" | "policy_denied"
            | "closed_early" | "timed_out",
        ) => Ok(()),
        _ => Err(validation(format!(
            "agent_runtime_step {field} is not a supported status"
        ))),
    }
}

fn require_terminal_reason(object: &Map<String, Value>, field: &str) -> Result<(), AgentError> {
    match object.get(field) {
        Some(Value::Null) => Ok(()),
        Some(Value::String(value))
            if matches!(
                value.as_str(),
                "done" | "stream_error" | "user_cancel" | "policy_denied" | "closed_early"
            ) =>
        {
            Ok(())
        }
        _ => Err(validation(format!(
            "agent_runtime_step {field} is not a supported terminal reason"
        ))),
    }
}

fn require_terminal_reason_matches_status(object: &Map<String, Value>) -> Result<(), AgentError> {
    let expected = match object.get("status").and_then(Value::as_str) {
        Some("effect_requested") => None,
        Some("completed") => Some("done"),
        Some("failed") => Some("stream_error"),
        Some("cancelled") => Some("user_cancel"),
        Some("policy_denied") => Some("policy_denied"),
        Some("closed_early" | "timed_out") => Some("closed_early"),
        _ => return Ok(()),
    };

    match (expected, object.get("terminal_reason")) {
        (None, Some(Value::Null)) => Ok(()),
        (Some(expected), Some(Value::String(value))) if value == expected => Ok(()),
        _ => Err(validation(
            "agent_runtime_step terminal_reason must match run_state.status",
        )),
    }
}

fn validation(message: impl Into<String>) -> AgentError {
    AgentError::validation(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{PROTOCOL_VERSION, ScheduleSpec, catalog_version};
    use serde_json::json;
    use time::OffsetDateTime;

    fn catalog() -> AgentRuntimeCatalog {
        AgentRuntimeCatalog {
            protocol_version: protocol_version(),
            catalog_version: catalog_version(),
            generated_at: OffsetDateTime::UNIX_EPOCH,
            active_domains: Vec::new(),
            agents: vec![
                agent_core::AgentSpec {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    id: "parent".to_owned(),
                    name: "Parent".to_owned(),
                    description: None,
                    version: "0.1.0".to_owned(),
                    schedule: ScheduleSpec::Manual,
                    capabilities: vec![],
                    metadata: json!({}),
                },
                agent_core::AgentSpec {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    id: "child".to_owned(),
                    name: "Child".to_owned(),
                    description: None,
                    version: "0.1.0".to_owned(),
                    schedule: ScheduleSpec::Manual,
                    capabilities: vec![],
                    metadata: json!({}),
                },
            ],
            tools: vec![ToolSpec {
                name: "read_first".to_owned(),
                description: "Read first".to_owned(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                risk: agent_core::ToolRisk::ReadOnly,
                metadata: json!({}),
            }],
            proposal_kinds: Vec::new(),
            prompt_blocks: Vec::new(),
        }
    }

    #[test]
    fn effect_step_loop_requests_and_completes_host_tool_effect() {
        let catalog = catalog();
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId("run_tool".to_owned())),
            input: json!({
                "effects": [
                    {"kind": "tool", "name": "read_first", "input": {"id": "first"}}
                ]
            }),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        };

        let first = EffectStepLoop::start_step(&catalog, request, "parent").expect("first step");
        assert_eq!(first["status"], "effect_requested");
        assert_eq!(first["effect"]["kind"], "tool");
        assert_eq!(first["effect"]["name"], "read_first");
        assert_eq!(first["run_state"]["remaining_effect_count"], 0);
        let id = first["effect"]["effect_id"].as_str().unwrap().to_owned();

        let terminal = EffectStepLoop::continue_step(
            &catalog,
            first,
            json!({"jsonrpc": "2.0", "id": id, "result": {"ok": true}}),
            "parent",
        )
        .expect("terminal step");
        assert_eq!(terminal["status"], "completed");
        assert_eq!(terminal["output"]["effect_result"], json!({"ok": true}));
        assert_eq!(
            terminal["trace_event"]["run_state"]["terminal_reason"],
            "done"
        );
    }

    #[test]
    fn effect_step_loop_exposes_typed_embedded_checkpoints() {
        let catalog = catalog();
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId("run_typed".to_owned())),
            input: json!({
                "effects": [
                    {"kind": "tool", "name": "read_first", "input": {"id": "first"}},
                    {"kind": "subagent", "agent_id": "child", "input": {}}
                ]
            }),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        };

        let first =
            EffectStepLoop::start_typed(&catalog, request, "parent").expect("typed first step");
        assert_eq!(
            first.status,
            agent_core::EmbeddedRunStepStatus::EffectRequested
        );
        assert!(first.requested_effect().unwrap().effect_id().len() > 1);
        assert_eq!(
            first
                .continuation
                .as_ref()
                .expect("remaining continuation")
                .effects
                .len(),
            1
        );
        let id = first.requested_effect().unwrap().effect_id().to_owned();
        let second = EffectStepLoop::continue_typed(
            &catalog,
            first,
            agent_core::EmbeddedEffectResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: Some(json!({"ok": true})),
                error: None,
            },
            "parent",
        )
        .expect("typed second step");
        assert_eq!(
            second.status,
            agent_core::EmbeddedRunStepStatus::EffectRequested
        );
        assert!(matches!(
            second.requested_effect(),
            Some(agent_core::EmbeddedHostEffect::Subagent { .. })
        ));
    }

    #[test]
    fn effect_step_loop_accepts_stable_embedded_control_error_codes() {
        let catalog = catalog();
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId("run_budget".to_owned())),
            input: json!({
                "effects": [
                    {"kind": "tool", "name": "read_first", "input": {}}
                ]
            }),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        };
        let first =
            EffectStepLoop::start_typed(&catalog, request, "parent").expect("typed first step");
        let id = first.requested_effect().unwrap().effect_id().to_owned();

        let terminal = EffectStepLoop::continue_typed(
            &catalog,
            first,
            agent_core::EmbeddedEffectResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: Some(json!({
                    "error": {
                        "code": "effect_budget_exhausted",
                        "message": "agent runtime effect budget exhausted"
                    }
                })),
                error: None,
            },
            "parent",
        )
        .expect("typed terminal step");
        assert_eq!(
            terminal.status,
            agent_core::EmbeddedRunStepStatus::ClosedEarly
        );
        assert_eq!(
            terminal.run_state.terminal_reason,
            Some(agent_core::EmbeddedTerminalReason::ClosedEarly)
        );
    }

    #[test]
    fn embedded_snapshot_closes_at_runtime_owned_effect_budget() {
        let catalog = catalog();
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId("run_snapshot_budget".to_owned())),
            input: json!({
                "effects": [
                    {"kind": "tool", "name": "read_first", "input": {"index": 1}},
                    {"kind": "tool", "name": "read_first", "input": {"index": 2}}
                ]
            }),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        };
        let first = EffectStepLoop::start_snapshot(
            &catalog,
            request,
            "parent",
            agent_core::EmbeddedRunLimits {
                max_effect_steps: 1,
                max_subagent_depth: 4,
            },
        )
        .expect("snapshot starts");
        assert_eq!(first.remaining_effect_steps(), 1);
        let id = first.requested_effect().unwrap().effect_id().to_owned();

        let terminal = EffectStepLoop::continue_snapshot(
            &catalog,
            first,
            agent_core::EmbeddedEffectResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: Some(json!({"ok": true})),
                error: None,
            },
            "parent",
        )
        .expect("snapshot advances and closes");
        assert_eq!(
            terminal.step.status,
            agent_core::EmbeddedRunStepStatus::ClosedEarly
        );
        assert_eq!(terminal.progress.dispatched_effect_count, 1);
        assert!(terminal.progress.effect_budget_exhausted);
        assert_eq!(terminal.remaining_effect_steps(), 0);
        assert_eq!(
            terminal.step.error.unwrap()["code"],
            "effect_budget_exhausted"
        );
    }

    #[test]
    fn embedded_snapshot_owns_subagent_depth_and_shared_effect_budget() {
        let catalog = catalog();
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId("run_snapshot_parent".to_owned())),
            input: json!({
                "effects": [{
                    "kind": "subagent",
                    "agent_id": "child",
                    "input": {
                        "effects": [{
                            "kind": "tool",
                            "name": "read_first",
                            "input": {"from": "child"}
                        }]
                    }
                }]
            }),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        };
        let parent = EffectStepLoop::start_snapshot(
            &catalog,
            request,
            "parent",
            agent_core::EmbeddedRunLimits {
                max_effect_steps: 3,
                max_subagent_depth: 2,
            },
        )
        .expect("parent starts");
        let child = EffectStepLoop::start_requested_subagent(&catalog, &parent)
            .expect("child starts from parent effect");
        assert_eq!(child.progress.subagent_depth, 1);
        assert_eq!(child.progress.dispatched_effect_count, 1);
        let child_effect_id = child.requested_effect().unwrap().effect_id().to_owned();
        let child = EffectStepLoop::continue_snapshot(
            &catalog,
            child,
            agent_core::EmbeddedEffectResponse {
                jsonrpc: "2.0".to_owned(),
                id: child_effect_id,
                result: Some(json!({"child": "done"})),
                error: None,
            },
            "child",
        )
        .expect("child completes");
        assert!(child.is_terminal());
        assert_eq!(child.progress.dispatched_effect_count, 2);

        let parent = EffectStepLoop::resume_parent_from_subagent(&catalog, parent, child)
            .expect("parent resumes from child");
        assert!(parent.is_terminal());
        assert_eq!(
            parent.step.status,
            agent_core::EmbeddedRunStepStatus::Completed
        );
        assert_eq!(parent.progress.dispatched_effect_count, 2);
        assert_eq!(parent.remaining_effect_steps(), 1);
    }

    #[test]
    fn embedded_snapshot_cancellation_is_terminal_without_consuming_budget() {
        let catalog = catalog();
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId("run_snapshot_cancelled".to_owned())),
            input: json!({
                "effect": {"kind": "tool", "name": "read_first", "input": {}}
            }),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        };
        let snapshot = EffectStepLoop::start_snapshot(
            &catalog,
            request,
            "parent",
            agent_core::EmbeddedRunLimits {
                max_effect_steps: 2,
                max_subagent_depth: 1,
            },
        )
        .expect("snapshot starts");

        let cancelled = EffectStepLoop::cancel_snapshot(
            &catalog,
            snapshot,
            "parent",
            "app moved to background",
        )
        .expect("snapshot cancels");

        assert_eq!(
            cancelled.step.status,
            agent_core::EmbeddedRunStepStatus::Cancelled
        );
        assert_eq!(
            cancelled.step.run_state.terminal_reason,
            Some(agent_core::EmbeddedTerminalReason::UserCancel)
        );
        assert_eq!(cancelled.progress.dispatched_effect_count, 0);
        assert_eq!(cancelled.remaining_effect_steps(), 2);
        assert_eq!(cancelled.step.error.unwrap()["code"], "user_cancel");
    }

    #[test]
    fn embedded_snapshot_rejects_tampered_progress() {
        let catalog = catalog();
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId("run_snapshot_tampered".to_owned())),
            input: json!({
                "effect": {"kind": "tool", "name": "read_first", "input": {}}
            }),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        };
        let mut snapshot = EffectStepLoop::start_snapshot(
            &catalog,
            request,
            "parent",
            agent_core::EmbeddedRunLimits {
                max_effect_steps: 1,
                max_subagent_depth: 1,
            },
        )
        .expect("snapshot starts");
        let id = snapshot.requested_effect().unwrap().effect_id().to_owned();
        snapshot.progress.dispatched_effect_count = 2;
        let error = EffectStepLoop::continue_snapshot(
            &catalog,
            snapshot,
            agent_core::EmbeddedEffectResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: Some(json!({})),
                error: None,
            },
            "parent",
        )
        .expect_err("tampered progress is rejected");
        assert!(
            error
                .record
                .message
                .contains("dispatched effect count exceeds")
        );
    }

    #[test]
    fn effect_step_loop_exposes_subagent_as_native_effect() {
        let catalog = catalog();
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId("run_subagent".to_owned())),
            input: json!({
                "effects": [
                    {
                        "kind": "subagent",
                        "agent_id": "child",
                        "input": {"from": "parent"}
                    }
                ]
            }),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        };

        let first = EffectStepLoop::start_step(&catalog, request, "parent").expect("first step");
        assert_eq!(first["status"], "effect_requested");
        assert_eq!(first["effect"]["kind"], "subagent");
        assert_eq!(first["effect"]["agent_id"], "child");
        assert_eq!(first["trace_event"]["subagent_id"], "child");
        let id = first["effect"]["effect_id"].as_str().unwrap().to_owned();

        let terminal = EffectStepLoop::continue_step(
            &catalog,
            first,
            json!({"jsonrpc": "2.0", "id": id, "result": {"result": {"status": "completed"}}}),
            "parent",
        )
        .expect("terminal step");
        assert_eq!(terminal["status"], "completed");
        assert_eq!(
            terminal["output"]["effect_result"]["result"]["status"],
            "completed"
        );
    }

    #[test]
    fn effect_step_loop_requires_current_step_metadata_and_json_rpc_response() {
        let catalog = catalog();
        let request = RunRequest {
            protocol_version: protocol_version(),
            run_id: Some(RunId("run_strict_contract".to_owned())),
            input: json!({
                "effects": [
                    {"kind": "tool", "name": "read_first", "input": {}}
                ]
            }),
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            workflow: None,
            metadata: json!({}),
        };

        let first = EffectStepLoop::start_step(&catalog, request, "parent").expect("first step");
        let id = first["effect"]["effect_id"]
            .as_str()
            .expect("effect id")
            .to_owned();

        let mut missing_metadata = first.clone();
        missing_metadata
            .as_object_mut()
            .expect("step object")
            .remove("run_state");
        let error = EffectStepLoop::continue_step(
            &catalog,
            missing_metadata,
            json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            "parent",
        )
        .expect_err("run_state is required");
        assert!(error.record.message.contains("run_state is required"));

        let error = EffectStepLoop::continue_step(
            &catalog,
            first,
            json!({"id": id, "result": {}}),
            "parent",
        )
        .expect_err("JSON-RPC envelope is required");
        assert!(error.record.message.contains("jsonrpc must be '2.0'"));
    }
}
