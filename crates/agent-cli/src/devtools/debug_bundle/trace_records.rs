use super::*;

pub(super) fn event_records_from_trace(trace: &Value) -> Vec<Value> {
    trace
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn tool_call_records_from_trace(trace: &Value) -> Vec<Value> {
    trace
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|event| {
            let kind = event.get("kind").and_then(Value::as_str)?;
            if !matches!(kind, "tool_call_finished" | "tool_call_failed") {
                return None;
            }
            event.get("payload").cloned()
        })
        .collect()
}

pub(super) fn artifact_ref_records_from_trace(trace: &Value) -> Vec<Value> {
    let mut artifacts = trace
        .get("artifact_refs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if artifacts.is_empty() {
        artifacts = trace
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|event| event.get("kind").and_then(Value::as_str) == Some("artifact_published"))
            .filter_map(|event| {
                event
                    .get("payload")
                    .and_then(|payload| payload.get("artifact_ref"))
                    .cloned()
            })
            .collect();
    }
    artifacts
}
