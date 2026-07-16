use super::*;

pub fn run_idempotency_key(
    agent_id: &str,
    scope: &RunScope,
    request: &RunRequest,
    run_id: &RunId,
) -> String {
    let scheduled_for = request
        .metadata
        .get("scheduled_for")
        .cloned()
        .unwrap_or(Value::Null);
    let trigger_envelope = request
        .trigger_envelope
        .as_ref()
        .map(trigger_envelope_identity)
        .unwrap_or(Value::Null);
    let material = json!({
        "agent_id": agent_id,
        "scope": scope,
        "trigger_kind": &request.trigger,
        "scheduled_for": scheduled_for,
        "trigger_envelope": trigger_envelope,
        "request_identity": if scheduled_for.is_null() && trigger_envelope.is_null() {
            Value::String(run_id.0.clone())
        } else {
            Value::Null
        },
    });
    let bytes = serde_json::to_vec(&material).unwrap_or_else(|_| agent_id.as_bytes().to_vec());
    format!("idem_{}", blake3::hash(&bytes).to_hex())
}

pub(super) fn deduplicated_outcome(record: AgentRunRecord, spec: &AgentSpec) -> RunOutcome {
    let finished_at = record.finished_at.unwrap_or_else(OffsetDateTime::now_utc);
    let result = AgentRunResult {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: record.run_id.clone(),
        agent_id: record.agent_id.clone(),
        status: record.status.clone(),
        started_at: record.started_at,
        finished_at,
        summary: Some("deduplicated by idempotency key".to_owned()),
        output: record.output.clone(),
        error: record.error.clone(),
        workflow: record.workflow.clone(),
    };
    let event = TraceEvent::new(
        "run_deduplicated",
        json!({
            "run_id": record.run_id.0,
            "idempotency_key": record.idempotency_key,
            "status": record.status,
        }),
    );
    RunOutcome {
        result,
        trace: AgentTrace {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            runtime_version: RUNTIME_VERSION.to_owned(),
            run_id: record.run_id,
            agent_id: record.agent_id,
            agent_version: spec.version.clone(),
            scope: record.scope,
            started_at: record.started_at,
            finished_at,
            input: record.input,
            output: record.output,
            workflow: record.workflow,
            usage_summary: Some(TraceUsageSummary {
                llm_request_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
                cost_micros_by_currency: BTreeMap::new(),
                by_provider: Vec::new(),
            }),
            spans: Vec::new(),
            events: vec![event],
            artifact_refs: Vec::new(),
        },
        disposition: RunDisposition::Deduplicated,
    }
}
