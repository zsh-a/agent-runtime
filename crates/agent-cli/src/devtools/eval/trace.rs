use super::*;

pub(super) fn default_eval_id(record: &AgentRunRecord) -> String {
    format!(
        "{}_{}",
        sanitize_eval_id(&record.agent_id),
        sanitize_eval_id(&record.run_id.0)
    )
}

pub(super) fn sanitize_eval_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn trace_event_kinds(trace: &Value) -> Vec<String> {
    trace
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|event| event.get("kind").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

pub(super) fn tool_call_sequence_from_trace(trace: &agent_core::AgentTrace) -> Vec<String> {
    trace
        .events
        .iter()
        .filter(|event| {
            matches!(
                event.kind.as_str(),
                "tool_call_finished" | "tool_call_failed"
            )
        })
        .filter_map(|event| event.payload.get("tool_name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

pub(super) fn tool_call_sequence_from_trace_value(trace: &Value) -> Vec<String> {
    trace
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|event| {
            event
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| matches!(kind, "tool_call_finished" | "tool_call_failed"))
        })
        .filter_map(|event| {
            event
                .get("payload")
                .and_then(|payload| payload.get("tool_name"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned)
        .collect()
}

pub(super) fn discover_eval_files(root: &Utf8Path) -> Result<Vec<Utf8PathBuf>> {
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.into_diagnostic()?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = Utf8PathBuf::from_path_buf(entry.path().to_path_buf())
            .map_err(|path| miette!("non-UTF-8 eval path: {}", path.display()))?;
        let Some(ext) = path.extension() else {
            continue;
        };
        if ext == "yaml" || ext == "yml" {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

pub(super) fn normalized_trace_json(trace: &agent_core::AgentTrace) -> Result<Value> {
    let mut value = serde_json::to_value(trace).into_diagnostic()?;
    normalize_volatile_json(&mut value);
    Ok(value)
}

pub(super) fn normalize_volatile_json(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in [
                "run_id",
                "proposal_id",
                "started_at",
                "created_at",
                "finished_at",
                "occurred_at",
                "expires_at",
                "duration_ms",
                "runtime_version",
                "spans",
            ] {
                map.remove(key);
            }
            for value in map.values_mut() {
                normalize_volatile_json(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_volatile_json(item);
            }
        }
        _ => {}
    }
}
