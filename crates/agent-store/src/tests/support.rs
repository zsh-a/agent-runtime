use super::*;

#[cfg(feature = "sqlite")]
pub(super) fn sqlite_trace_record(run_id: RunId, agent_id: &str) -> AgentTrace {
    let now = time::OffsetDateTime::now_utc();
    AgentTrace {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: "test-runtime".to_owned(),
        run_id,
        agent_id: agent_id.to_owned(),
        agent_version: "test-agent".to_owned(),
        scope: RunScope::Global,
        started_at: now,
        finished_at: now,
        input: serde_json::json!({}),
        output: serde_json::json!({}),
        workflow: None,
        usage_summary: None,
        spans: Vec::new(),
        events: vec![TraceEvent::new(
            "run_started",
            serde_json::json!({"source": "sqlite_test"}),
        )],
        artifact_refs: Vec::new(),
    }
}

pub(super) fn temp_root() -> Utf8PathBuf {
    let temp = tempfile::tempdir().expect("tempdir");
    Utf8PathBuf::from_path_buf(temp.keep()).expect("utf8 temp path")
}
