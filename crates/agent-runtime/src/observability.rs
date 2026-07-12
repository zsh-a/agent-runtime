use std::collections::{BTreeMap, HashMap, HashSet};

use agent_core::{
    AgentRunStatus, ArtifactRef, RunId, TraceEvent, TraceSpan, TraceUsageProviderSummary,
    TraceUsageSummary,
};
use serde_json::{Map, Value, json};
use time::{Duration as TimeDuration, OffsetDateTime};

pub(super) fn run_trace_span(
    run_id: &RunId,
    agent_id: &str,
    started_at: OffsetDateTime,
    finished_at: OffsetDateTime,
    status: &AgentRunStatus,
) -> TraceSpan {
    let material = format!("{}:run", run_id.0);
    let span_id = format!("span_{}", blake3::hash(material.as_bytes()).to_hex());
    let duration_ms = timestamp_duration_ms(started_at, finished_at);
    let status = run_status_name(status).to_owned();
    TraceSpan {
        span_id,
        parent_span_id: None,
        name: "agent.run".to_owned(),
        started_at,
        finished_at,
        duration_ms,
        status: status.clone(),
        attributes: json!({
            "run_id": run_id.0.clone(),
            "agent_id": agent_id,
            "status": status,
        }),
    }
}

pub(super) fn trace_spans_from_events(
    run_span: TraceSpan,
    events: &[TraceEvent],
) -> Vec<TraceSpan> {
    let parent_span_id = run_span.span_id.clone();
    let started_tools = started_tool_events_by_id(events);
    let paired_tool_keys = paired_tool_terminal_keys(events);
    let mut spans = vec![run_span];
    for (index, event) in events.iter().enumerate() {
        if let Some(span) = event_trace_span(
            &parent_span_id,
            event,
            index,
            &started_tools,
            &paired_tool_keys,
        ) {
            spans.push(span);
        }
    }
    spans
}

pub(super) fn artifact_refs_from_events(events: &[TraceEvent]) -> Vec<ArtifactRef> {
    events
        .iter()
        .filter(|event| event.kind == "artifact_published")
        .filter_map(|event| event.payload.get("artifact_ref").cloned())
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

#[derive(Default)]
struct UsageAccumulator {
    request_count: u64,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    cost_micros_by_currency: BTreeMap<String, u64>,
}

impl UsageAccumulator {
    fn add_usage(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
        cost: Option<(String, u64)>,
    ) {
        self.request_count = self.request_count.saturating_add(1);
        self.input_tokens = self.input_tokens.saturating_add(input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(total_tokens);
        if let Some((currency, cost_micros)) = cost {
            let entry = self.cost_micros_by_currency.entry(currency).or_insert(0);
            *entry = entry.saturating_add(cost_micros);
        }
    }
}

pub(super) fn trace_usage_summary_from_events(events: &[TraceEvent]) -> Option<TraceUsageSummary> {
    let mut total = UsageAccumulator::default();
    let mut by_provider: BTreeMap<(String, Option<String>), UsageAccumulator> = BTreeMap::new();

    for event in events
        .iter()
        .filter(|event| matches!(event.kind.as_str(), "llm_response" | "llm.round.finished"))
    {
        let input_tokens = payload_usage_u64(&event.payload, &["input_tokens", "prompt_tokens"]);
        let output_tokens =
            payload_usage_u64(&event.payload, &["output_tokens", "completion_tokens"]);
        let total_tokens = payload_usage_u64(&event.payload, &["total_tokens"])
            .max(input_tokens.saturating_add(output_tokens));
        let cost = payload_cost_micros(&event.payload);
        total.add_usage(input_tokens, output_tokens, total_tokens, cost.clone());

        let provider = payload_usage_str(&event.payload, &["provider", "model_provider", "vendor"])
            .unwrap_or("unknown")
            .to_owned();
        let model = payload_usage_str(&event.payload, &["model"]).map(ToOwned::to_owned);
        by_provider.entry((provider, model)).or_default().add_usage(
            input_tokens,
            output_tokens,
            total_tokens,
            cost,
        );
    }

    if total.request_count == 0 {
        return None;
    }

    Some(TraceUsageSummary {
        llm_request_count: total.request_count,
        input_tokens: total.input_tokens,
        output_tokens: total.output_tokens,
        total_tokens: total.total_tokens,
        cost_micros_by_currency: total.cost_micros_by_currency,
        by_provider: by_provider
            .into_iter()
            .map(|((provider, model), usage)| TraceUsageProviderSummary {
                provider,
                model,
                request_count: usage.request_count,
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                total_tokens: usage.total_tokens,
                cost_micros_by_currency: usage.cost_micros_by_currency,
            })
            .collect(),
    })
}

pub(super) fn started_tool_events_by_id(events: &[TraceEvent]) -> HashMap<String, &TraceEvent> {
    events
        .iter()
        .filter(|event| event.kind == "tool_call_started")
        .filter_map(|event| {
            payload_str(&event.payload, "tool_call_id")
                .map(|tool_call_id| (tool_call_id.to_owned(), event))
        })
        .collect()
}

pub(super) fn paired_tool_terminal_keys(events: &[TraceEvent]) -> HashSet<String> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event.kind.as_str(),
                "tool_call_finished" | "tool_call_failed"
            )
        })
        .filter(|event| payload_str(&event.payload, "tool_call_id").is_some())
        .filter_map(|event| tool_span_key(&event.payload))
        .collect()
}

pub(super) fn event_trace_span(
    parent_span_id: &str,
    event: &TraceEvent,
    index: usize,
    started_tools: &HashMap<String, &TraceEvent>,
    paired_tool_keys: &HashSet<String>,
) -> Option<TraceSpan> {
    match event.kind.as_str() {
        "tool_call" | "tool_call_finished" | "tool_call_failed" => tool_event_trace_span(
            parent_span_id,
            event,
            index,
            started_tools,
            paired_tool_keys,
        ),
        "state_read" | "state_read_failed" => {
            state_event_trace_span(parent_span_id, event, index, "state.read")
        }
        "state_write" | "state_write_failed" => {
            state_event_trace_span(parent_span_id, event, index, "state.write")
        }
        "llm_response" | "llm_response_failed" | "llm.round.finished" | "llm.round.failed" => {
            llm_event_trace_span(parent_span_id, event, index)
        }
        _ => None,
    }
}

pub(super) fn tool_event_trace_span(
    parent_span_id: &str,
    event: &TraceEvent,
    index: usize,
    started_tools: &HashMap<String, &TraceEvent>,
    paired_tool_keys: &HashSet<String>,
) -> Option<TraceSpan> {
    if payload_str(&event.payload, "tool_call_id").is_none()
        && tool_span_key(&event.payload).is_some_and(|key| paired_tool_keys.contains(&key))
    {
        return None;
    }
    let tool_name = payload_str(&event.payload, "tool_name")?;
    let status = payload_str(&event.payload, "status").unwrap_or_else(|| {
        if event.kind == "tool_call_failed" {
            "failed"
        } else {
            "completed"
        }
    });
    let duration_ms = payload_duration_ms(&event.payload).unwrap_or(0);
    let finished_at = event.occurred_at;
    let started_at = payload_str(&event.payload, "tool_call_id")
        .and_then(|tool_call_id| started_tools.get(tool_call_id))
        .map(|event| event.occurred_at)
        .unwrap_or_else(|| subtract_duration_ms(finished_at, duration_ms));
    let identity = payload_str(&event.payload, "tool_call_id").unwrap_or(tool_name);

    let mut attributes = span_common_attributes(&event.payload, status);
    copy_payload_field(&mut attributes, &event.payload, "tool_call_id");
    copy_payload_field(&mut attributes, &event.payload, "tool_name");
    copy_payload_field(&mut attributes, &event.payload, "input_hash");
    copy_payload_field(&mut attributes, &event.payload, "input_bytes");
    copy_payload_field(&mut attributes, &event.payload, "output_hash");
    copy_payload_field(&mut attributes, &event.payload, "output_bytes");
    copy_error_attributes(&mut attributes, &event.payload);

    Some(TraceSpan {
        span_id: child_span_id(parent_span_id, "tool", identity, index),
        parent_span_id: Some(parent_span_id.to_owned()),
        name: format!("tool.{tool_name}"),
        started_at,
        finished_at,
        duration_ms,
        status: status.to_owned(),
        attributes: Value::Object(attributes),
    })
}

pub(super) fn state_event_trace_span(
    parent_span_id: &str,
    event: &TraceEvent,
    index: usize,
    name: &str,
) -> Option<TraceSpan> {
    let status = payload_str(&event.payload, "status").unwrap_or_else(|| {
        if event.kind.ends_with("_failed") {
            "failed"
        } else {
            "completed"
        }
    });
    let duration_ms = payload_duration_ms(&event.payload).unwrap_or(0);
    let finished_at = event.occurred_at;
    let started_at = subtract_duration_ms(finished_at, duration_ms);
    let identity = payload_str(&event.payload, "key").unwrap_or(name);

    let mut attributes = span_common_attributes(&event.payload, status);
    copy_payload_field(&mut attributes, &event.payload, "key");
    copy_payload_field(&mut attributes, &event.payload, "found");
    copy_payload_field(&mut attributes, &event.payload, "value_hash");
    copy_error_attributes(&mut attributes, &event.payload);

    Some(TraceSpan {
        span_id: child_span_id(parent_span_id, name, identity, index),
        parent_span_id: Some(parent_span_id.to_owned()),
        name: name.to_owned(),
        started_at,
        finished_at,
        duration_ms,
        status: status.to_owned(),
        attributes: Value::Object(attributes),
    })
}

pub(super) fn llm_event_trace_span(
    parent_span_id: &str,
    event: &TraceEvent,
    index: usize,
) -> Option<TraceSpan> {
    let provider = payload_usage_str(&event.payload, &["provider", "model_provider", "vendor"])
        .unwrap_or("unknown");
    let model = payload_usage_str(&event.payload, &["model"]);
    let status = payload_str(&event.payload, "status").unwrap_or_else(|| {
        if event.kind.ends_with("failed") {
            "failed"
        } else {
            "completed"
        }
    });
    let duration_ms = payload_duration_ms(&event.payload).unwrap_or(0);
    let finished_at = event.occurred_at;
    let started_at = subtract_duration_ms(finished_at, duration_ms);
    let identity = model
        .map(|model| format!("{provider}:{model}"))
        .unwrap_or_else(|| provider.to_owned());

    let input_tokens = payload_usage_u64(&event.payload, &["input_tokens", "prompt_tokens"]);
    let output_tokens = payload_usage_u64(&event.payload, &["output_tokens", "completion_tokens"]);
    let total_tokens = payload_usage_u64(&event.payload, &["total_tokens"])
        .max(input_tokens.saturating_add(output_tokens));

    let mut attributes = span_common_attributes(&event.payload, status);
    attributes.insert("provider".to_owned(), json!(provider));
    if let Some(model) = model {
        attributes.insert("model".to_owned(), json!(model));
    }
    if input_tokens > 0 {
        attributes.insert("input_tokens".to_owned(), json!(input_tokens));
    }
    if output_tokens > 0 {
        attributes.insert("output_tokens".to_owned(), json!(output_tokens));
    }
    if total_tokens > 0 {
        attributes.insert("total_tokens".to_owned(), json!(total_tokens));
    }
    if let Some((currency, cost_micros)) = payload_cost_micros(&event.payload) {
        attributes.insert("cost_currency".to_owned(), json!(currency));
        attributes.insert("cost_micros".to_owned(), json!(cost_micros));
    }
    copy_error_attributes(&mut attributes, &event.payload);

    Some(TraceSpan {
        span_id: child_span_id(parent_span_id, "llm", &identity, index),
        parent_span_id: Some(parent_span_id.to_owned()),
        name: format!("llm.{}", span_name_segment(provider)),
        started_at,
        finished_at,
        duration_ms,
        status: status.to_owned(),
        attributes: Value::Object(attributes),
    })
}

pub(super) fn span_common_attributes(payload: &Value, status: &str) -> Map<String, Value> {
    let mut attributes = Map::new();
    copy_payload_field(&mut attributes, payload, "run_id");
    copy_payload_field(&mut attributes, payload, "agent_id");
    attributes.insert("status".to_owned(), json!(status));
    attributes
}

pub(super) fn copy_payload_field(attributes: &mut Map<String, Value>, payload: &Value, key: &str) {
    if let Some(value) = payload.get(key) {
        attributes.insert(key.to_owned(), value.clone());
    }
}

pub(super) fn copy_error_attributes(attributes: &mut Map<String, Value>, payload: &Value) {
    let Some(error) = payload.get("error") else {
        return;
    };
    if let Some(code) = error.get("code") {
        attributes.insert("error_code".to_owned(), code.clone());
    }
    if let Some(kind) = error.get("kind") {
        attributes.insert("error_kind".to_owned(), kind.clone());
    }
    if let Some(retryable) = error.get("retryable") {
        attributes.insert("retryable".to_owned(), retryable.clone());
    }
}

pub(super) fn tool_span_key(payload: &Value) -> Option<String> {
    let tool_name = payload_str(payload, "tool_name")?;
    let input_hash = payload_str(payload, "input_hash")?;
    Some(format!("{tool_name}\0{input_hash}"))
}

pub(super) fn payload_str<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

pub(super) fn payload_duration_ms(payload: &Value) -> Option<u64> {
    payload.get("duration_ms").and_then(Value::as_u64)
}

pub(super) fn payload_usage_u64(payload: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| {
            payload
                .get("usage")
                .and_then(|usage| usage.get(*key))
                .or_else(|| payload.get(*key))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0)
}

pub(super) fn payload_usage_str<'a>(payload: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        payload
            .get("usage")
            .and_then(|usage| usage.get(*key))
            .or_else(|| payload.get(*key))
            .and_then(Value::as_str)
    })
}

pub(super) fn payload_cost_micros(payload: &Value) -> Option<(String, u64)> {
    let cost_micros = payload
        .get("usage")
        .and_then(|usage| usage.get("cost_micros"))
        .or_else(|| payload.get("cost_micros"))
        .and_then(Value::as_u64)?;
    let currency = payload_usage_str(payload, &["cost_currency", "currency"])
        .unwrap_or("unknown")
        .to_owned();
    Some((currency, cost_micros))
}

pub(super) fn span_name_segment(value: &str) -> String {
    let segment: String = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if segment.is_empty() {
        "unknown".to_owned()
    } else {
        segment
    }
}

pub(super) fn subtract_duration_ms(
    finished_at: OffsetDateTime,
    duration_ms: u64,
) -> OffsetDateTime {
    let duration = TimeDuration::milliseconds(i64::try_from(duration_ms).unwrap_or(i64::MAX));
    finished_at.checked_sub(duration).unwrap_or(finished_at)
}

pub(super) fn child_span_id(
    parent_span_id: &str,
    kind: &str,
    identity: &str,
    index: usize,
) -> String {
    let material = format!("{parent_span_id}:{kind}:{identity}:{index}");
    format!("span_{}", blake3::hash(material.as_bytes()).to_hex())
}

pub(super) fn timestamp_duration_ms(
    started_at: OffsetDateTime,
    finished_at: OffsetDateTime,
) -> u64 {
    let duration_ms = (finished_at - started_at).whole_milliseconds();
    if duration_ms <= 0 {
        0
    } else {
        u64::try_from(duration_ms).unwrap_or(u64::MAX)
    }
}

pub(super) fn run_status_name(status: &AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Running => "running",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Skipped => "skipped",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
        AgentRunStatus::TimedOut => "timed_out",
        AgentRunStatus::Abandoned => "abandoned",
    }
}
