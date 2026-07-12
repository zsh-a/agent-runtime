use std::collections::{HashMap, HashSet};

use agent_core::{
    AgentError, AgentRunStatus, PROTOCOL_VERSION, RunDependency, RunId, WorkflowInputMapping,
    WorkflowInputTransform, WorkflowRunNode, WorkflowRunNodeResult, WorkflowRunRequest,
    WorkflowRunResult,
};
use serde_json::{Map, Value, json};
use time::OffsetDateTime;

pub(super) fn workflow_execution_order(
    nodes: &[WorkflowRunNode],
) -> Result<Vec<usize>, AgentError> {
    if nodes.is_empty() {
        return Err(AgentError::validation(
            "workflow DAG requires at least one node",
        ));
    }
    let mut index_by_id = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        if node.node_id.trim().is_empty() {
            return Err(AgentError::validation(
                "workflow DAG node_id must not be empty",
            ));
        }
        if node.agent_id.trim().is_empty() {
            return Err(AgentError::validation(format!(
                "workflow DAG node '{}' agent_id must not be empty",
                node.node_id
            )));
        }
        if index_by_id.insert(node.node_id.clone(), index).is_some() {
            return Err(AgentError::validation(format!(
                "workflow DAG contains duplicate node_id '{}'",
                node.node_id
            )));
        }
    }
    for node in nodes {
        for dependency in &node.depends_on {
            if !index_by_id.contains_key(dependency) {
                return Err(AgentError::validation(format!(
                    "workflow DAG node '{}' depends on unknown node '{}'",
                    node.node_id, dependency
                )));
            }
        }
        for mapping in &node.input_mappings {
            if !index_by_id.contains_key(&mapping.from_node) {
                return Err(AgentError::validation(format!(
                    "workflow DAG node '{}' maps input from unknown node '{}'",
                    node.node_id, mapping.from_node
                )));
            }
            if !node
                .depends_on
                .iter()
                .any(|dependency| dependency == &mapping.from_node)
            {
                return Err(AgentError::validation(format!(
                    "workflow DAG node '{}' maps input from node '{}' but does not list it in depends_on",
                    node.node_id, mapping.from_node
                )));
            }
            validate_workflow_json_pointer(
                &mapping.from_path,
                &format!(
                    "workflow DAG node '{}' input mapping from_path",
                    node.node_id
                ),
            )?;
            validate_workflow_json_pointer(
                &mapping.to_path,
                &format!("workflow DAG node '{}' input mapping to_path", node.node_id),
            )?;
        }
    }

    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut order = Vec::new();
    for index in 0..nodes.len() {
        visit_workflow_node(
            index,
            nodes,
            &index_by_id,
            &mut visiting,
            &mut visited,
            &mut order,
        )?;
    }
    Ok(order)
}

pub(super) fn visit_workflow_node(
    index: usize,
    nodes: &[WorkflowRunNode],
    index_by_id: &HashMap<String, usize>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    order: &mut Vec<usize>,
) -> Result<(), AgentError> {
    let node_id = nodes[index].node_id.clone();
    if visited.contains(&node_id) {
        return Ok(());
    }
    if !visiting.insert(node_id.clone()) {
        return Err(AgentError::validation(format!(
            "workflow DAG contains a dependency cycle at node '{node_id}'"
        )));
    }
    for dependency in &nodes[index].depends_on {
        let dependency_index = *index_by_id
            .get(dependency)
            .expect("dependency existence validated before DFS");
        visit_workflow_node(
            dependency_index,
            nodes,
            index_by_id,
            visiting,
            visited,
            order,
        )?;
    }
    visiting.remove(&node_id);
    visited.insert(node_id);
    order.push(index);
    Ok(())
}

pub(super) fn planned_workflow_run_ids(nodes: &[WorkflowRunNode]) -> HashMap<String, RunId> {
    nodes
        .iter()
        .map(|node| {
            (
                node.node_id.clone(),
                node.run_id.clone().unwrap_or_else(RunId::new_v7),
            )
        })
        .collect()
}

pub(super) fn workflow_dependencies_resolved(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> bool {
    node.depends_on
        .iter()
        .all(|dependency| node_results.contains_key(dependency))
}

pub(super) fn blocked_workflow_dependencies(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> Vec<String> {
    node.depends_on
        .iter()
        .filter(|dependency| {
            node_results
                .get(*dependency)
                .is_none_or(|result| result.status != AgentRunStatus::Completed)
        })
        .cloned()
        .collect()
}

pub(super) fn skipped_workflow_node_result(
    node: &WorkflowRunNode,
    blocked_dependencies: Vec<String>,
) -> WorkflowRunNodeResult {
    WorkflowRunNodeResult {
        node_id: node.node_id.clone(),
        agent_id: node.agent_id.clone(),
        status: AgentRunStatus::Skipped,
        run_id: None,
        depends_on: node.depends_on.clone(),
        output: json!({}),
        error: None,
        trace: None,
        compensation: None,
        metadata: json!({
            "reason": "dependency_not_completed",
            "blocked_dependencies": blocked_dependencies,
        }),
    }
}

pub(super) fn skipped_workflow_result(
    request: WorkflowRunRequest,
    started_at: OffsetDateTime,
    reason: &str,
    metadata: Value,
) -> WorkflowRunResult {
    WorkflowRunResult {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        workflow_id: request.workflow_id,
        status: AgentRunStatus::Skipped,
        started_at,
        finished_at: OffsetDateTime::now_utc(),
        root_run_id: request.root_run_id,
        nodes: request
            .nodes
            .into_iter()
            .map(|node| WorkflowRunNodeResult {
                node_id: node.node_id,
                agent_id: node.agent_id,
                status: AgentRunStatus::Skipped,
                run_id: None,
                depends_on: node.depends_on,
                output: json!({}),
                error: None,
                trace: None,
                compensation: None,
                metadata: json!({
                    "reason": reason,
                    "workflow": metadata.clone(),
                }),
            })
            .collect(),
        metadata: request.metadata,
    }
}

pub(super) fn workflow_run_dependencies(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> Vec<RunDependency> {
    node.depends_on
        .iter()
        .filter_map(|dependency| {
            let result = node_results.get(dependency)?;
            Some(RunDependency {
                run_id: result.run_id.clone()?,
                edge: Some("depends_on".to_owned()),
                metadata: json!({
                    "workflow_node_id": dependency,
                }),
            })
        })
        .collect()
}

pub(super) fn workflow_node_input(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> Result<Value, AgentError> {
    let mut input = node.input.clone();
    for mapping in &node.input_mappings {
        let Some(source_result) = node_results.get(&mapping.from_node) else {
            return Err(AgentError::validation(format!(
                "workflow DAG node '{}' input mapping source node '{}' has not completed",
                node.node_id, mapping.from_node
            )));
        };
        let value = match json_pointer_get(&source_result.output, &mapping.from_path) {
            Some(value) => value.clone(),
            None => match &mapping.default {
                Some(value) => value.clone(),
                None => {
                    return Err(AgentError::validation(format!(
                        "workflow DAG node '{}' input mapping source path '{}' was not found in node '{}' output",
                        node.node_id, mapping.from_path, mapping.from_node
                    )));
                }
            },
        };
        let value = apply_workflow_input_transform(&value, mapping).map_err(|error| {
            AgentError::validation(format!(
                "workflow DAG node '{}' input mapping from node '{}' transform failed: {error}",
                node.node_id, mapping.from_node
            ))
        })?;
        json_pointer_insert(&mut input, &mapping.to_path, value).map_err(AgentError::validation)?;
    }
    Ok(input)
}

pub(super) fn apply_workflow_input_transform(
    value: &Value,
    mapping: &WorkflowInputMapping,
) -> Result<Value, String> {
    match mapping.transform {
        WorkflowInputTransform::None => Ok(value.clone()),
        WorkflowInputTransform::String => workflow_value_as_string(value)
            .map(Value::String)
            .ok_or_else(|| {
                format!(
                    "value at '{}' cannot be converted to string",
                    mapping.from_path
                )
            }),
        WorkflowInputTransform::Number => workflow_value_as_number(value)
            .map(Value::Number)
            .ok_or_else(|| {
                format!(
                    "value at '{}' cannot be converted to number",
                    mapping.from_path
                )
            }),
        WorkflowInputTransform::Integer => workflow_value_as_integer(value)
            .map(|value| Value::Number(value.into()))
            .ok_or_else(|| {
                format!(
                    "value at '{}' cannot be converted to integer",
                    mapping.from_path
                )
            }),
        WorkflowInputTransform::Boolean => workflow_value_as_boolean(value)
            .map(Value::Bool)
            .ok_or_else(|| {
                format!(
                    "value at '{}' cannot be converted to boolean",
                    mapping.from_path
                )
            }),
        WorkflowInputTransform::JsonString => serde_json::to_string(value)
            .map(Value::String)
            .map_err(|error| {
                format!(
                    "value at '{}' cannot be serialized as JSON: {error}",
                    mapping.from_path
                )
            }),
    }
}

pub(super) fn workflow_value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => Some("null".to_owned()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

pub(super) fn workflow_value_as_number(value: &Value) -> Option<serde_json::Number> {
    match value {
        Value::Number(value) => Some(value.clone()),
        Value::String(value) => value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .and_then(serde_json::Number::from_f64),
        _ => None,
    }
}

pub(super) fn workflow_value_as_integer(value: &Value) -> Option<i64> {
    match value {
        Value::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok())),
        Value::String(value) => value.parse::<i64>().ok(),
        _ => None,
    }
}

pub(super) fn workflow_value_as_boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn validate_workflow_json_pointer(pointer: &str, label: &str) -> Result<(), AgentError> {
    decode_json_pointer(pointer)
        .map(|_| ())
        .map_err(|error| AgentError::validation(format!("{label} {error}")))
}

pub(super) fn json_pointer_get<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        Some(value)
    } else {
        value.pointer(pointer)
    }
}

pub(super) fn json_pointer_insert(
    target: &mut Value,
    pointer: &str,
    value: Value,
) -> Result<(), String> {
    let segments = decode_json_pointer(pointer)?;
    if segments.is_empty() {
        *target = value;
        return Ok(());
    }
    let mut current = target;
    for segment in &segments[..segments.len() - 1] {
        if !current.is_object() {
            return Err(format!(
                "target path '{}' cannot create object segment '{}' under non-object value",
                pointer, segment
            ));
        }
        let object = current.as_object_mut().expect("object checked above");
        current = object
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if !current.is_object() {
        return Err(format!(
            "target path '{}' cannot set field under non-object value",
            pointer
        ));
    }
    current
        .as_object_mut()
        .expect("object checked above")
        .insert(segments.last().expect("non-empty segments").clone(), value);
    Ok(())
}

pub(super) fn decode_json_pointer(pointer: &str) -> Result<Vec<String>, String> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    if !pointer.starts_with('/') {
        return Err(format!("must be an RFC 6901 JSON Pointer, got '{pointer}'"));
    }
    pointer
        .split('/')
        .skip(1)
        .map(|segment| decode_json_pointer_segment(segment, pointer))
        .collect()
}

pub(super) fn decode_json_pointer_segment(segment: &str, pointer: &str) -> Result<String, String> {
    let mut decoded = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            Some(other) => {
                return Err(format!(
                    "contains invalid escape '~{other}' in JSON Pointer '{pointer}'"
                ));
            }
            None => {
                return Err(format!(
                    "contains trailing '~' escape in JSON Pointer '{pointer}'"
                ));
            }
        }
    }
    Ok(decoded)
}

pub(super) fn workflow_parent_from_dependencies(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> (Option<RunId>, Option<String>) {
    if node.depends_on.len() != 1 {
        return (None, None);
    }
    let Some(result) = node_results.get(&node.depends_on[0]) else {
        return (None, None);
    };
    if result.status != AgentRunStatus::Completed {
        return (None, None);
    }
    (result.run_id.clone(), Some(result.agent_id.clone()))
}

pub(super) fn workflow_status(results: &[WorkflowRunNodeResult]) -> AgentRunStatus {
    if results
        .iter()
        .any(|result| workflow_node_failed(&result.status))
    {
        AgentRunStatus::Failed
    } else if results
        .iter()
        .any(|result| result.status == AgentRunStatus::Skipped)
    {
        AgentRunStatus::Skipped
    } else {
        AgentRunStatus::Completed
    }
}

pub(super) fn workflow_needs_compensation(results: &[WorkflowRunNodeResult]) -> bool {
    results
        .iter()
        .any(|result| workflow_node_failed(&result.status))
}

pub(super) fn workflow_node_failed(status: &AgentRunStatus) -> bool {
    matches!(
        status,
        AgentRunStatus::Failed
            | AgentRunStatus::TimedOut
            | AgentRunStatus::Cancelled
            | AgentRunStatus::Abandoned
    )
}
