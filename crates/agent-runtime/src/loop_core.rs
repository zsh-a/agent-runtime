use agent_core::{
    AgentError, AgentRuntimeCatalog, EffectId, RunId, RunRequest, ToolSpec, catalog_version,
    protocol_version,
};
use serde_json::{Map, Value, json};

pub struct EffectStepLoop;

#[deprecated(note = "use EffectStepLoop; this type is a protocol stepper, not the executor loop")]
pub type RunLoop = EffectStepLoop;

impl EffectStepLoop {
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
                    let effect_result_count = effect_results.len();
                    let effect_results_json = EffectResultRecord::to_values(&effect_results);
                    terminal_completed_step(
                        &agent.id,
                        &agent.version,
                        run_id,
                        next_step_index,
                        previous_kind,
                        effect_call,
                        effect_response,
                        effect_result_count,
                        effect_results_json,
                    )
                }
            },
        };
        attach_runtime_metadata(&mut response);
        Ok(response)
    }
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
    Subagent(RequestedSubagentEffect),
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
        Some("subagent") => {
            parse_requested_subagent_effect(value, label).map(RequestedEffect::Subagent)
        }
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
    previous_kind: RunEffectKind,
    effect_call: Value,
    effect_response: Value,
    effect_result_count: usize,
    effect_results: Vec<Value>,
) -> Value {
    let _ = previous_kind;
    let mode = if effect_result_count > 1 {
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
    if let Some(run_state) = previous_step.get("run_state") {
        require_previous_step_run_state(previous_step, run_state, step_index).map_err(|error| {
            validation(format!("previous step run_state: {}", error.record.message))
        })?;
    }
    if let Some(trace_event) = previous_step.get("trace_event") {
        require_previous_step_trace_event(previous_step, trace_event, run_id, step_index).map_err(
            |error| {
                validation(format!(
                    "previous step trace_event: {}",
                    error.record.message
                ))
            },
        )?;
    }
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
    if let Some(run_state) = previous_step.get("run_state") {
        match trace_event.get("run_state") {
            Some(value) if value == run_state => {}
            _ => {
                return Err(validation(
                    "previous step trace_event.run_state must match run_state",
                ));
            }
        }
    }
    Ok(())
}

fn require_matching_effect_response_id(
    effect_call: &Value,
    effect_response: &Value,
    kind: RunEffectKind,
) -> Result<(), AgentError> {
    let Some(expected_id) = effect_call.get(kind.id_field()).and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(response_id) = effect_response.get("id") else {
        return Ok(());
    };
    match response_id.as_str() {
        Some(value) if value == expected_id => Ok(()),
        Some(value) => Err(validation(format!(
            "effect response id '{value}' does not match requested {} '{expected_id}'",
            kind.id_field()
        ))),
        None => Err(validation(
            "effect response id must be a string when present",
        )),
    }
}

fn require_effect_response_envelope(effect_response: &Value) -> Result<(), AgentError> {
    let Some(object) = effect_response.as_object() else {
        return Err(validation("effect response must be an object"));
    };
    if let Some(jsonrpc) = object.get("jsonrpc") {
        match jsonrpc.as_str() {
            Some("2.0") => {}
            Some(_) => return Err(validation("effect response jsonrpc must be '2.0'")),
            None => return Err(validation("effect response jsonrpc must be a string")),
        }
        if !object.contains_key("id") {
            return Err(validation(
                "effect response id is required when jsonrpc is present",
            ));
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
    }
    if !object.contains_key("jsonrpc")
        && object.contains_key("result")
        && object.contains_key("error")
    {
        return Err(validation(
            "effect response cannot contain both result and error",
        ));
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
        .and_then(|error| error.get("code"))
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_i64().map(|code| code.to_string()))
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
    let expected = protocol_version();
    if request.protocol_version != expected {
        return Err(validation(format!(
            "run request protocol_version '{}' does not match runtime protocol_version '{expected}'",
            request.protocol_version
        )));
    }
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
    let expected_protocol = protocol_version();
    if catalog.protocol_version != expected_protocol {
        return Err(validation(format!(
            "catalog protocol_version '{}' does not match runtime protocol_version '{expected_protocol}'",
            catalog.protocol_version
        )));
    }
    let expected_catalog = catalog_version();
    if catalog.catalog_version != expected_catalog {
        return Err(validation(format!(
            "catalog catalog_version '{}' does not match runtime catalog_version '{expected_catalog}'",
            catalog.catalog_version
        )));
    }
    Ok(())
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
    use agent_core::{PROTOCOL_VERSION, ScheduleSpec};
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
    fn run_loop_requests_and_completes_host_tool_effect() {
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
    fn run_loop_exposes_subagent_as_native_effect() {
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
}
