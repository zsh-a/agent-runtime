use std::collections::BTreeMap;

use agent_core::{
    AgentProposalStore, AgentRunStore, AgentTraceStore, PROTOCOL_VERSION, ProposalStatus,
};
use agent_runtime::RUNTIME_VERSION;
use camino::Utf8Path;
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Serialize)]
pub(crate) struct RuntimeMetricsSummary {
    protocol_version: String,
    runtime_version: String,
    generated_at: String,
    store_root: String,
    run_count: usize,
    runs_by_status: BTreeMap<String, usize>,
    successful_run_count: usize,
    skipped_run_count: usize,
    failed_run_count: usize,
    timeout_count: usize,
    total_run_latency_ms: u64,
    average_run_latency_ms: Option<f64>,
    tool_call_count: usize,
    failed_tool_call_count: usize,
    total_tool_call_latency_ms: u64,
    average_tool_call_latency_ms: Option<f64>,
    replay_count: usize,
    proposal_count: usize,
    proposals_by_status: BTreeMap<String, usize>,
    proposal_created_count: usize,
    proposal_approved_count: usize,
    proposal_denied_count: usize,
    proposal_applied_count: usize,
    artifact_ref_count: usize,
    llm_total_tokens: u64,
    runs_by_agent: BTreeMap<String, RuntimeAgentMetrics>,
    tool_calls_by_tool: BTreeMap<String, RuntimeToolMetrics>,
    llm_usage_by_provider: BTreeMap<String, RuntimeLlmProviderMetrics>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RuntimeAgentMetrics {
    run_count: usize,
    runs_by_status: BTreeMap<String, usize>,
    successful_run_count: usize,
    failed_run_count: usize,
    total_run_latency_ms: u64,
    average_run_latency_ms: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RuntimeToolMetrics {
    tool_call_count: usize,
    failed_tool_call_count: usize,
    total_tool_call_latency_ms: u64,
    average_tool_call_latency_ms: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RuntimeLlmProviderMetrics {
    request_count: usize,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    total_latency_ms: u64,
    average_latency_ms: Option<f64>,
    cost_micros_by_currency: BTreeMap<String, u64>,
}

#[derive(Debug, Default)]
struct AgentMetricsAccumulator {
    run_count: usize,
    runs_by_status: BTreeMap<String, usize>,
    total_run_latency_ms: u64,
    completed_latency_count: u64,
}

#[derive(Debug, Default)]
struct ToolMetricsAccumulator {
    tool_call_count: usize,
    failed_tool_call_count: usize,
    total_tool_call_latency_ms: u64,
}

#[derive(Debug, Default)]
struct LlmProviderMetricsAccumulator {
    request_count: usize,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    total_latency_ms: u64,
    cost_micros_by_currency: BTreeMap<String, u64>,
}

pub(crate) async fn build_metrics_summary(
    store_path: &Utf8Path,
    run_store: &dyn AgentRunStore,
    trace_store: &dyn AgentTraceStore,
    proposal_store: &dyn AgentProposalStore,
) -> Result<RuntimeMetricsSummary> {
    let runs = run_store.list_runs(None, None).await.into_diagnostic()?;
    let proposals = proposal_store
        .list_proposals(None)
        .await
        .into_diagnostic()?;
    let mut runs_by_status = BTreeMap::new();
    let mut total_run_latency_ms = 0_u64;
    let mut completed_latency_count = 0_u64;
    let mut tool_call_count = 0_usize;
    let mut failed_tool_call_count = 0_usize;
    let mut total_tool_call_latency_ms = 0_u64;
    let mut replay_count = 0_usize;
    let mut llm_total_tokens = 0_u64;
    let mut proposal_created_count = 0_usize;
    let mut proposal_approved_count = 0_usize;
    let mut proposal_denied_count = 0_usize;
    let mut proposal_applied_count = 0_usize;
    let mut artifact_ref_count = 0_usize;
    let mut runs_by_agent = BTreeMap::<String, AgentMetricsAccumulator>::new();
    let mut tool_calls_by_tool = BTreeMap::<String, ToolMetricsAccumulator>::new();
    let mut llm_usage_by_provider = BTreeMap::<String, LlmProviderMetricsAccumulator>::new();

    for run in &runs {
        let status_key = run_status_key(&run.status);
        *runs_by_status.entry(status_key.clone()).or_insert(0) += 1;
        let agent_metrics = runs_by_agent.entry(run.agent_id.clone()).or_default();
        agent_metrics.run_count += 1;
        *agent_metrics.runs_by_status.entry(status_key).or_insert(0) += 1;
        if let Some(finished_at) = run.finished_at {
            let latency_ms = (finished_at - run.started_at).whole_milliseconds();
            if latency_ms >= 0 {
                let latency_ms = u64::try_from(latency_ms).unwrap_or(0);
                total_run_latency_ms = total_run_latency_ms.saturating_add(latency_ms);
                completed_latency_count = completed_latency_count.saturating_add(1);
                agent_metrics.total_run_latency_ms = agent_metrics
                    .total_run_latency_ms
                    .saturating_add(latency_ms);
                agent_metrics.completed_latency_count =
                    agent_metrics.completed_latency_count.saturating_add(1);
            }
        }
        if let Some(trace_record) = trace_store
            .read_trace(&run.run_id)
            .await
            .into_diagnostic()?
        {
            let trace = serde_json::to_value(trace_record).into_diagnostic()?;
            let terminal_tool_keys = terminal_tool_event_keys(&trace);
            artifact_ref_count = artifact_ref_count.saturating_add(
                trace
                    .get("artifact_refs")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0),
            );
            if trace_started_by_replay(&trace) {
                replay_count += 1;
            }
            for event in event_records_from_trace(&trace) {
                let kind = event.get("kind").and_then(Value::as_str);
                let payload = event.get("payload").unwrap_or(&Value::Null);
                match kind {
                    Some("tool_call_finished") => {
                        tool_call_count += 1;
                        let duration_ms = payload_duration_ms(payload);
                        total_tool_call_latency_ms =
                            total_tool_call_latency_ms.saturating_add(duration_ms);
                        record_tool_metrics(
                            &mut tool_calls_by_tool,
                            payload_tool_name(payload),
                            duration_ms,
                            false,
                        );
                    }
                    Some("tool_call_failed") => {
                        tool_call_count += 1;
                        failed_tool_call_count += 1;
                        let duration_ms = payload_duration_ms(payload);
                        total_tool_call_latency_ms =
                            total_tool_call_latency_ms.saturating_add(duration_ms);
                        record_tool_metrics(
                            &mut tool_calls_by_tool,
                            payload_tool_name(payload),
                            duration_ms,
                            true,
                        );
                    }
                    Some("tool_call") => {
                        if payload_tool_key(payload)
                            .is_some_and(|key| terminal_tool_keys.contains(&key))
                        {
                            continue;
                        }
                        tool_call_count += 1;
                        let duration_ms = payload_duration_ms(payload);
                        total_tool_call_latency_ms =
                            total_tool_call_latency_ms.saturating_add(duration_ms);
                        record_tool_metrics(
                            &mut tool_calls_by_tool,
                            payload_tool_name(payload),
                            duration_ms,
                            false,
                        );
                    }
                    Some("llm_response") | Some("llm.round.finished") => {
                        llm_total_tokens =
                            llm_total_tokens.saturating_add(payload_total_tokens(payload));
                        record_llm_provider_metrics(&mut llm_usage_by_provider, payload);
                    }
                    Some("proposal_created") => {
                        proposal_created_count += 1;
                    }
                    Some("proposal_decided") => {
                        match payload.get("status").and_then(Value::as_str) {
                            Some("approved") => proposal_approved_count += 1,
                            Some("denied") => proposal_denied_count += 1,
                            Some("pending_approval") => {}
                            _ => match payload.get("decision").and_then(Value::as_str) {
                                Some("approve" | "approved") => proposal_approved_count += 1,
                                Some("deny" | "denied") => proposal_denied_count += 1,
                                _ => {}
                            },
                        }
                    }
                    Some("proposal_applied") => {
                        proposal_applied_count += 1;
                    }
                    Some("artifact_ref") => {
                        artifact_ref_count += 1;
                    }
                    _ => {}
                }
            }
        }
    }

    let mut proposals_by_status = BTreeMap::new();
    for proposal in &proposals {
        *proposals_by_status
            .entry(proposal_status_key(&proposal.status))
            .or_insert(0) += 1;
    }
    if proposal_created_count == 0 {
        proposal_created_count = proposals.len();
    }
    if proposal_approved_count == 0 {
        proposal_approved_count = count_run_status(&proposals_by_status, "approved");
    }
    if proposal_denied_count == 0 {
        proposal_denied_count = count_run_status(&proposals_by_status, "denied");
    }
    if proposal_applied_count == 0 {
        proposal_applied_count = count_run_status(&proposals_by_status, "applied");
    }
    let generated_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .into_diagnostic()?;
    Ok(RuntimeMetricsSummary {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: RUNTIME_VERSION.to_owned(),
        generated_at,
        store_root: store_path.to_string(),
        run_count: runs.len(),
        successful_run_count: count_run_status(&runs_by_status, "completed"),
        skipped_run_count: count_run_status(&runs_by_status, "skipped"),
        failed_run_count: count_failure_runs(&runs_by_status),
        timeout_count: count_run_status(&runs_by_status, "timed_out"),
        total_run_latency_ms,
        average_run_latency_ms: average_ms(total_run_latency_ms, completed_latency_count),
        tool_call_count,
        failed_tool_call_count,
        total_tool_call_latency_ms,
        average_tool_call_latency_ms: average_ms(
            total_tool_call_latency_ms,
            tool_call_count as u64,
        ),
        replay_count,
        proposal_count: proposals.len(),
        proposal_created_count,
        proposal_approved_count,
        proposal_denied_count,
        proposal_applied_count,
        artifact_ref_count,
        proposals_by_status,
        runs_by_status,
        llm_total_tokens,
        runs_by_agent: runs_by_agent
            .into_iter()
            .map(|(agent_id, metrics)| (agent_id, metrics.into_summary()))
            .collect(),
        tool_calls_by_tool: tool_calls_by_tool
            .into_iter()
            .map(|(tool_name, metrics)| (tool_name, metrics.into_summary()))
            .collect(),
        llm_usage_by_provider: llm_usage_by_provider
            .into_iter()
            .map(|(provider, metrics)| (provider, metrics.into_summary()))
            .collect(),
    })
}

fn run_status_key(status: &agent_core::AgentRunStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{status:?}"))
}

fn proposal_status_key(status: &ProposalStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{status:?}"))
}

fn count_run_status(counts: &BTreeMap<String, usize>, status: &str) -> usize {
    counts.get(status).copied().unwrap_or(0)
}

fn count_failure_runs(counts: &BTreeMap<String, usize>) -> usize {
    ["failed", "cancelled", "timed_out", "abandoned"]
        .iter()
        .map(|status| count_run_status(counts, status))
        .sum()
}

impl AgentMetricsAccumulator {
    fn into_summary(self) -> RuntimeAgentMetrics {
        RuntimeAgentMetrics {
            run_count: self.run_count,
            successful_run_count: count_run_status(&self.runs_by_status, "completed"),
            failed_run_count: count_failure_runs(&self.runs_by_status),
            total_run_latency_ms: self.total_run_latency_ms,
            average_run_latency_ms: average_ms(
                self.total_run_latency_ms,
                self.completed_latency_count,
            ),
            runs_by_status: self.runs_by_status,
        }
    }
}

impl ToolMetricsAccumulator {
    fn into_summary(self) -> RuntimeToolMetrics {
        RuntimeToolMetrics {
            tool_call_count: self.tool_call_count,
            failed_tool_call_count: self.failed_tool_call_count,
            total_tool_call_latency_ms: self.total_tool_call_latency_ms,
            average_tool_call_latency_ms: average_ms(
                self.total_tool_call_latency_ms,
                self.tool_call_count as u64,
            ),
        }
    }
}

impl LlmProviderMetricsAccumulator {
    fn into_summary(self) -> RuntimeLlmProviderMetrics {
        RuntimeLlmProviderMetrics {
            request_count: self.request_count,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            total_tokens: self.total_tokens,
            total_latency_ms: self.total_latency_ms,
            average_latency_ms: average_ms(self.total_latency_ms, self.request_count as u64),
            cost_micros_by_currency: self.cost_micros_by_currency,
        }
    }
}

fn average_ms(total: u64, count: u64) -> Option<f64> {
    (count > 0).then(|| total as f64 / count as f64)
}

fn record_tool_metrics(
    tool_calls_by_tool: &mut BTreeMap<String, ToolMetricsAccumulator>,
    tool_name: String,
    duration_ms: u64,
    failed: bool,
) {
    let metrics = tool_calls_by_tool.entry(tool_name).or_default();
    metrics.tool_call_count += 1;
    if failed {
        metrics.failed_tool_call_count += 1;
    }
    metrics.total_tool_call_latency_ms = metrics
        .total_tool_call_latency_ms
        .saturating_add(duration_ms);
}

fn record_llm_provider_metrics(
    llm_usage_by_provider: &mut BTreeMap<String, LlmProviderMetricsAccumulator>,
    payload: &Value,
) {
    let provider = payload_usage_str(payload, &["provider", "model_provider", "vendor"])
        .unwrap_or("unknown")
        .to_owned();
    let input_tokens = payload_usage_u64(payload, &["input_tokens", "prompt_tokens"]);
    let output_tokens = payload_usage_u64(payload, &["output_tokens", "completion_tokens"]);
    let total_tokens = payload_usage_u64(payload, &["total_tokens"])
        .max(input_tokens.saturating_add(output_tokens));
    let metrics = llm_usage_by_provider.entry(provider).or_default();
    metrics.request_count += 1;
    metrics.input_tokens = metrics.input_tokens.saturating_add(input_tokens);
    metrics.output_tokens = metrics.output_tokens.saturating_add(output_tokens);
    metrics.total_tokens = metrics.total_tokens.saturating_add(total_tokens);
    metrics.total_latency_ms = metrics
        .total_latency_ms
        .saturating_add(payload_duration_ms(payload));
    if let Some((currency, cost_micros)) = payload_cost_micros(payload) {
        let entry = metrics.cost_micros_by_currency.entry(currency).or_insert(0);
        *entry = entry.saturating_add(cost_micros);
    }
}

fn payload_duration_ms(payload: &Value) -> u64 {
    payload
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn payload_tool_name(payload: &Value) -> String {
    payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

fn terminal_tool_event_keys(trace: &Value) -> std::collections::BTreeSet<String> {
    event_records_from_trace(trace)
        .into_iter()
        .filter(|event| {
            matches!(
                event.get("kind").and_then(Value::as_str),
                Some("tool_call_finished" | "tool_call_failed")
            )
        })
        .filter_map(|event| event.get("payload").and_then(payload_tool_key))
        .collect()
}

fn payload_tool_key(payload: &Value) -> Option<String> {
    let tool_name = payload.get("tool_name").and_then(Value::as_str)?;
    let input_hash = payload.get("input_hash").and_then(Value::as_str)?;
    Some(format!("{tool_name}\0{input_hash}"))
}

fn payload_total_tokens(payload: &Value) -> u64 {
    payload
        .get("usage")
        .and_then(|usage| usage.get("total_tokens"))
        .or_else(|| payload.get("total_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn payload_usage_u64(payload: &Value, keys: &[&str]) -> u64 {
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

fn payload_usage_str<'a>(payload: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        payload
            .get("usage")
            .and_then(|usage| usage.get(*key))
            .or_else(|| payload.get(*key))
            .and_then(Value::as_str)
    })
}

fn payload_cost_micros(payload: &Value) -> Option<(String, u64)> {
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

fn trace_started_by_replay(trace: &Value) -> bool {
    trace
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("run_started")
                && event
                    .get("payload")
                    .and_then(|payload| payload.get("trigger"))
                    .and_then(Value::as_str)
                    == Some("replay")
        })
}

pub(crate) fn event_records_from_trace(trace: &Value) -> Vec<Value> {
    trace
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
