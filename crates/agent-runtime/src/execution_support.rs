use std::{sync::Arc, time::Duration};

use agent_core::{
    AgentError, AgentRunResult, AgentRunStatus, AgentRunStore, PROTOCOL_VERSION, RunId, TraceEvent,
    TraceSink,
};
use serde_json::json;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::{observability::run_status_name, trace::MemoryTraceSink};

const STORE_CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(25);

pub(super) fn result_is_retryable(result: &AgentRunResult) -> bool {
    if matches!(
        result.status,
        AgentRunStatus::Completed | AgentRunStatus::Skipped | AgentRunStatus::Cancelled
    ) {
        return false;
    }
    result.error.as_ref().is_some_and(|error| error.retryable)
}

pub(super) fn spawn_persisted_cancellation_watcher(
    run_store: Arc<dyn AgentRunStore>,
    run_id: RunId,
    agent_id: String,
    cancellation: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancellation.cancelled() => break,
                _ = tokio::time::sleep(STORE_CANCELLATION_POLL_INTERVAL) => {
                    match run_store.get_run(&run_id).await {
                        Ok(Some(record)) if record.cancellation_requested() => {
                            warn!(
                                run_id = %run_id.0,
                                agent_id = %agent_id,
                                "persisted cancellation intent observed for active run",
                            );
                            cancellation.cancel();
                            break;
                        }
                        Ok(Some(record)) if record.status != AgentRunStatus::Running => break,
                        Ok(Some(_)) | Ok(None) => {}
                        Err(error) => {
                            warn!(
                                run_id = %run_id.0,
                                agent_id = %agent_id,
                                error = %error,
                                "failed to poll persisted cancellation intent",
                            );
                        }
                    }
                }
            }
        }
    })
}

pub(super) async fn persisted_cancellation_requested(
    run_store: &dyn AgentRunStore,
    run_id: &RunId,
) -> Result<bool, AgentError> {
    Ok(run_store
        .get_run(run_id)
        .await
        .map_err(|error| AgentError::internal(error.to_string()))?
        .is_some_and(|run| run.cancellation_requested()))
}

pub(super) async fn update_running_run_with_retry(
    run_store: &dyn AgentRunStore,
    mut updated: agent_core::AgentRunRecord,
) -> Result<(), AgentError> {
    const MAX_UPDATE_ATTEMPTS: usize = 8;

    for _ in 0..MAX_UPDATE_ATTEMPTS {
        let existing = run_store
            .get_run(&updated.run_id)
            .await
            .map_err(|error| AgentError::internal(error.to_string()))?
            .ok_or_else(|| AgentError::internal("run disappeared before update"))?;
        if existing.status != AgentRunStatus::Running {
            return Err(AgentError::internal(format!(
                "run cannot transition from terminal status {}",
                run_status_name(&existing.status)
            )));
        }
        let expected_version = existing.version;
        updated.version = expected_version
            .checked_add(1)
            .ok_or_else(|| AgentError::internal("run record version overflow"))?;
        updated.merge_control_metadata_from(&existing);
        if run_store
            .update_run(updated.clone(), expected_version)
            .await
            .map_err(|error| AgentError::internal(error.to_string()))?
        {
            return Ok(());
        }
    }

    Err(AgentError::internal("run update conflicted too many times"))
}

pub(super) async fn emit_cancellation_events(
    trace: &MemoryTraceSink,
    run_id: &RunId,
    agent_id: &str,
    attempt: u32,
    reason: &str,
    include_request: bool,
) -> Result<(), AgentError> {
    let payload = json!({
        "run_id": run_id.0.clone(),
        "agent_id": agent_id,
        "attempt": attempt,
        "reason": reason,
    });
    if include_request {
        trace
            .emit(TraceEvent::new("run_cancel_requested", payload.clone()))
            .await?;
    }
    trace.emit(TraceEvent::new("run_cancelled", payload)).await
}

pub(super) fn failure_result(
    run_id: RunId,
    agent_id: &str,
    started_at: OffsetDateTime,
    err: AgentError,
) -> AgentRunResult {
    AgentRunResult {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id,
        agent_id: agent_id.to_owned(),
        status: match err.record.kind {
            agent_core::AgentErrorKind::Timeout => AgentRunStatus::TimedOut,
            agent_core::AgentErrorKind::Cancelled => AgentRunStatus::Cancelled,
            _ => AgentRunStatus::Failed,
        },
        started_at,
        finished_at: OffsetDateTime::now_utc(),
        summary: Some(err.record.message.clone()),
        output: json!({}),
        error: Some(*err.record),
        workflow: None,
    }
}
