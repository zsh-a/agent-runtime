use std::sync::Arc;

use agent_core::{
    Agent, AgentContext, AgentError, AgentRegistry, AgentRunRecord, AgentRunResult, AgentRunStatus,
    AgentRunStore, AgentServices, AgentSpec, AgentTrace, HookEventName, PROTOCOL_VERSION, RunId,
    RunRequest, RunScope, TraceEvent, TraceSink,
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::{
    InMemoryLockStore, RUNTIME_VERSION,
    hooks::HookManager,
    lock::lock_key,
    policy::ExecutionPolicy,
    recovery::{RecoveryReport, recover_stale_runs},
    scheduler::AgentScheduler,
    services::TracedAgentServices,
    trace::MemoryTraceSink,
};

pub struct RunOutcome {
    pub result: AgentRunResult,
    pub trace: AgentTrace,
}

#[derive(Clone)]
pub struct RunControl {
    pub cancellation: CancellationToken,
    pub trace_events: Option<broadcast::Sender<TraceEvent>>,
}

impl Default for RunControl {
    fn default() -> Self {
        Self {
            cancellation: CancellationToken::new(),
            trace_events: None,
        }
    }
}

pub struct AgentRunner {
    registry: Arc<dyn AgentRegistry>,
    run_store: Arc<dyn AgentRunStore>,
    services: Arc<dyn AgentServices>,
    scheduler: AgentScheduler,
    policy: ExecutionPolicy,
    concurrency: Arc<Semaphore>,
    lock_store: Arc<dyn agent_core::AgentLockStore>,
    hooks: HookManager,
}

impl AgentRunner {
    pub fn new(
        registry: Arc<dyn AgentRegistry>,
        run_store: Arc<dyn AgentRunStore>,
        services: Arc<dyn AgentServices>,
    ) -> Self {
        Self {
            registry,
            run_store,
            services,
            scheduler: AgentScheduler,
            policy: ExecutionPolicy::default(),
            concurrency: Arc::new(Semaphore::new(
                ExecutionPolicy::default().max_concurrent_runs,
            )),
            lock_store: Arc::new(InMemoryLockStore::default()),
            hooks: HookManager::default(),
        }
    }

    pub fn with_policy(mut self, policy: ExecutionPolicy) -> Self {
        self.concurrency = Arc::new(Semaphore::new(policy.max_concurrent_runs.max(1)));
        self.policy = policy;
        self
    }

    pub fn with_lock_store(mut self, lock_store: Arc<dyn agent_core::AgentLockStore>) -> Self {
        self.lock_store = lock_store;
        self
    }

    pub fn with_hooks(mut self, hooks: HookManager) -> Self {
        self.hooks = hooks;
        self
    }

    pub(crate) fn nested_runner(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            run_store: self.run_store.clone(),
            services: self.services.clone(),
            scheduler: self.scheduler,
            policy: self.policy.clone(),
            concurrency: Arc::new(Semaphore::new(self.policy.max_concurrent_runs.max(1))),
            lock_store: self.lock_store.clone(),
            hooks: self.hooks.clone(),
        }
    }

    pub async fn recover_stale_runs(&self) -> Result<RecoveryReport, AgentError> {
        recover_stale_runs(self.run_store.as_ref(), &self.policy).await
    }

    pub async fn run_once(
        &self,
        agent_id: &str,
        request: RunRequest,
    ) -> Result<RunOutcome, AgentError> {
        self.run_once_with_control(agent_id, request, RunControl::default())
            .await
    }

    pub async fn run_once_with_control(
        &self,
        agent_id: &str,
        request: RunRequest,
        control: RunControl,
    ) -> Result<RunOutcome, AgentError> {
        let run_timer = std::time::Instant::now();
        let _permit = self
            .concurrency
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AgentError::internal(format!("run concurrency limiter closed: {e}")))?;
        debug!(agent_id, "acquired run concurrency permit");
        let agent = self
            .registry
            .get_agent(agent_id)
            .await?
            .ok_or_else(|| AgentError::validation(format!("unknown agent '{agent_id}'")))?;
        let spec = agent.spec();
        let run_id = request.run_id.clone().unwrap_or_else(RunId::new_v7);
        let started_at = OffsetDateTime::now_utc();
        let scope = request
            .user
            .as_ref()
            .map(|u| RunScope::User(u.user_id.clone()))
            .unwrap_or(RunScope::Global);
        let idempotency_key = run_idempotency_key(&spec.id, &scope, &request);
        let lock_key = lock_key(&spec.id, &scope);
        let trace = Arc::new(match control.trace_events {
            Some(sender) => MemoryTraceSink::with_event_sender(sender),
            None => MemoryTraceSink::default(),
        });
        info!(
            run_id = %run_id.0,
            agent_id = %spec.id,
            agent_version = %spec.version,
            trigger = ?request.trigger,
            scope = ?scope,
            timeout_ms = self.policy.timeout.as_millis(),
            max_retries = self.policy.max_retries,
            retry_backoff_ms = self.policy.retry_backoff.as_millis(),
            "starting agent run",
        );
        debug!(
            run_id = %run_id.0,
            agent_id = %spec.id,
            lock_key,
            lease_ttl_ms = self.policy.lease_ttl().as_millis(),
            "acquiring run lease",
        );
        let lease = self
            .lock_store
            .acquire(&lock_key, &run_id.0, self.policy.lease_ttl())
            .await
            .map_err(|e| AgentError::internal(e.to_string()))?;
        let Some(lease) = lease else {
            let reason = format!("run skipped because active lease exists for {lock_key}");
            warn!(
                run_id = %run_id.0,
                agent_id = %spec.id,
                lock_key,
                "skipping agent run because active lease exists",
            );
            let result = AgentRunResult::skipped(
                run_id.clone(),
                spec.id.clone(),
                started_at,
                Some(reason.clone()),
            );
            self.run_store
                .create_run(AgentRunRecord {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    run_id: run_id.clone(),
                    idempotency_key: Some(idempotency_key.clone()),
                    agent_id: spec.id.clone(),
                    status: AgentRunStatus::Skipped,
                    scope,
                    started_at,
                    finished_at: Some(result.finished_at),
                    input: request.input.clone(),
                    output: result.output.clone(),
                    error: None,
                    metadata: request.metadata.clone(),
                })
                .await
                .map_err(|e| {
                    error!(
                        run_id = %run_id.0,
                        agent_id = %spec.id,
                        error = %e,
                        "failed to create skipped run record",
                    );
                    AgentError::internal(e.to_string())
                })?;
            info!(
                run_id = %run_id.0,
                agent_id = %spec.id,
                duration_ms = run_timer.elapsed().as_millis(),
                "agent run skipped",
            );
            trace
                .emit(TraceEvent::new(
                    "run_skipped",
                    json!({"reason": reason, "lock_key": lock_key}),
                ))
                .await?;
            let trace_doc = AgentTrace {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                runtime_version: RUNTIME_VERSION.to_owned(),
                run_id,
                agent_id: spec.id,
                agent_version: spec.version,
                started_at,
                finished_at: result.finished_at,
                input: request.input,
                output: result.output.clone(),
                events: trace.events().await,
                artifact_refs: Vec::new(),
            };
            return Ok(RunOutcome {
                result,
                trace: trace_doc,
            });
        };
        debug!(
            run_id = %run_id.0,
            agent_id = %spec.id,
            lock_key,
            "run lease acquired",
        );
        self.run_store
            .create_run(AgentRunRecord {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: run_id.clone(),
                idempotency_key: Some(idempotency_key.clone()),
                agent_id: spec.id.clone(),
                status: AgentRunStatus::Running,
                scope: scope.clone(),
                started_at,
                finished_at: None,
                input: request.input.clone(),
                output: json!({}),
                error: None,
                metadata: request.metadata.clone(),
            })
            .await
            .map_err(|e| {
                error!(
                    run_id = %run_id.0,
                    agent_id = %spec.id,
                    error = %e,
                    "failed to create run record",
                );
                AgentError::internal(e.to_string())
            })?;
        debug!(
            run_id = %run_id.0,
            agent_id = %spec.id,
            idempotency_key,
            "run record created",
        );

        trace
            .emit(TraceEvent::new(
                "run_started",
                json!({"run_id": run_id.0.clone(), "agent_id": spec.id.clone(), "trigger": request.trigger}),
            ))
            .await?;
        self.hooks
            .observe(
                HookEventName::RunStart,
                Some(run_id.clone()),
                Some(spec.id.clone()),
                json!({
                    "run_id": run_id.0.clone(),
                    "agent_id": spec.id.clone(),
                    "trigger": request.trigger,
                    "input": request.input,
                    "metadata": request.metadata,
                }),
                trace.as_ref(),
            )
            .await?;

        let mut result = self
            .run_with_retries(
                agent,
                &spec,
                run_id.clone(),
                started_at,
                request.clone(),
                trace.clone(),
                control.cancellation.clone(),
            )
            .await?;
        result.finished_at = OffsetDateTime::now_utc();

        trace
            .emit(TraceEvent::new(
                "run_finished",
                json!({"run_id": result.run_id.0.clone(), "status": result.status.clone()}),
            ))
            .await?;
        self.hooks
            .observe(
                HookEventName::RunStop,
                Some(result.run_id.clone()),
                Some(result.agent_id.clone()),
                json!({
                    "run_id": result.run_id.0.clone(),
                    "agent_id": result.agent_id.clone(),
                    "status": result.status.clone(),
                    "output": result.output.clone(),
                    "error": result.error.clone(),
                }),
                trace.as_ref(),
            )
            .await?;

        self.run_store
            .update_run(AgentRunRecord {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: result.run_id.clone(),
                idempotency_key: Some(idempotency_key),
                agent_id: result.agent_id.clone(),
                status: result.status.clone(),
                scope,
                started_at,
                finished_at: Some(result.finished_at),
                input: request.input.clone(),
                output: result.output.clone(),
                error: result.error.clone(),
                metadata: request.metadata.clone(),
            })
            .await
            .map_err(|e| {
                error!(
                    run_id = %result.run_id.0,
                    agent_id = %result.agent_id,
                    error = %e,
                    "failed to update run record",
                );
                AgentError::internal(e.to_string())
            })?;

        let error_code = result.error.as_ref().map(|error| error.code.as_str());
        info!(
            run_id = %result.run_id.0,
            agent_id = %result.agent_id,
            status = ?result.status,
            error_code = error_code.unwrap_or("none"),
            duration_ms = run_timer.elapsed().as_millis(),
            "agent run finished",
        );

        let trace_doc = AgentTrace {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            runtime_version: RUNTIME_VERSION.to_owned(),
            run_id: result.run_id.clone(),
            agent_id: result.agent_id.clone(),
            agent_version: spec.version,
            started_at,
            finished_at: result.finished_at,
            input: request.input,
            output: result.output.clone(),
            events: trace.events().await,
            artifact_refs: Vec::new(),
        };

        self.lock_store.release(lease).await.map_err(|e| {
            error!(
                run_id = %result.run_id.0,
                agent_id = %result.agent_id,
                error = %e,
                "failed to release run lease",
            );
            AgentError::internal(e.to_string())
        })?;
        debug!(
            run_id = %result.run_id.0,
            agent_id = %result.agent_id,
            "run lease released",
        );

        Ok(RunOutcome {
            result,
            trace: trace_doc,
        })
    }

    async fn run_with_retries(
        &self,
        agent: Arc<dyn Agent>,
        spec: &AgentSpec,
        run_id: RunId,
        started_at: OffsetDateTime,
        request: RunRequest,
        trace: Arc<MemoryTraceSink>,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        let max_attempts = self.policy.max_retries.saturating_add(1);
        let trace_attempts = self.policy.max_retries > 0;
        let mut attempt = 1_u32;

        loop {
            if cancellation.is_cancelled() {
                warn!(
                    run_id = %run_id.0,
                    agent_id = %spec.id,
                    attempt,
                    "agent run cancelled before attempt started",
                );
                emit_cancellation_events(
                    trace.as_ref(),
                    &run_id,
                    &spec.id,
                    attempt,
                    "before_attempt",
                    true,
                )
                .await?;
                return Ok(failure_result(
                    run_id,
                    &spec.id,
                    started_at,
                    AgentError::cancelled("agent run cancelled before attempt started"),
                ));
            }
            if trace_attempts {
                trace
                    .emit(TraceEvent::new(
                        "run_attempt_started",
                        json!({
                            "run_id": run_id.0.clone(),
                            "agent_id": spec.id.clone(),
                            "attempt": attempt,
                            "max_attempts": max_attempts,
                        }),
                    ))
                    .await?;
            }
            debug!(
                run_id = %run_id.0,
                agent_id = %spec.id,
                attempt,
                max_attempts,
                "starting run attempt",
            );

            let ctx = AgentContext {
                run_id: run_id.clone(),
                now: started_at,
                user: request.user.clone(),
                input: request.input.clone(),
                services: Arc::new(TracedAgentServices {
                    inner: self.services.clone(),
                    trace: trace.clone(),
                    run_id: run_id.clone(),
                    agent_id: spec.id.clone(),
                    user: request.user.clone(),
                    hooks: self.hooks.clone(),
                    subagent_runner: Some(self.nested_runner()),
                    cancellation: cancellation.clone(),
                }),
                cancellation: cancellation.clone(),
                trace: trace.clone(),
            };

            let run_future = agent.run(ctx);
            let attempt_timer = std::time::Instant::now();
            let mut result = tokio::select! {
                _ = cancellation.cancelled() => {
                    warn!(
                        run_id = %run_id.0,
                        agent_id = %spec.id,
                        attempt,
                        duration_ms = attempt_timer.elapsed().as_millis(),
                        "run attempt cancelled",
                    );
                    emit_cancellation_events(
                        trace.as_ref(),
                        &run_id,
                        &spec.id,
                        attempt,
                        "during_attempt",
                        true,
                    )
                    .await?;
                    failure_result(
                        run_id.clone(),
                        &spec.id,
                        started_at,
                        AgentError::cancelled("agent run cancelled"),
                    )
                }
                outcome = tokio::time::timeout(self.policy.timeout, run_future) => match outcome {
                    Ok(Ok(mut result)) => {
                        result.run_id = run_id.clone();
                        result.agent_id = spec.id.clone();
                        result
                    }
                    Ok(Err(err)) => {
                        warn!(
                            run_id = %run_id.0,
                            agent_id = %spec.id,
                            attempt,
                            error_code = %err.record.code,
                            error_kind = ?err.record.kind,
                            retryable = err.record.retryable,
                            duration_ms = attempt_timer.elapsed().as_millis(),
                            "run attempt returned an agent error",
                        );
                        if matches!(err.record.kind, agent_core::AgentErrorKind::Cancelled) {
                            emit_cancellation_events(
                                trace.as_ref(),
                                &run_id,
                                &spec.id,
                                attempt,
                                "agent_returned_cancelled",
                                cancellation.is_cancelled(),
                            )
                            .await?;
                        }
                        failure_result(run_id.clone(), &spec.id, started_at, err)
                    }
                    Err(_) => {
                        warn!(
                            run_id = %run_id.0,
                            agent_id = %spec.id,
                            attempt,
                            timeout_ms = self.policy.timeout.as_millis(),
                            duration_ms = attempt_timer.elapsed().as_millis(),
                            "run attempt timed out",
                        );
                        failure_result(
                            run_id.clone(),
                            &spec.id,
                            started_at,
                            AgentError::timeout(self.policy.timeout),
                        )
                    }
                }
            };
            let retryable = result_is_retryable(&result);
            debug!(
                run_id = %run_id.0,
                agent_id = %spec.id,
                attempt,
                status = ?result.status,
                retryable,
                error_code = result
                    .error
                    .as_ref()
                    .map(|error| error.code.as_str())
                    .unwrap_or("none"),
                duration_ms = attempt_timer.elapsed().as_millis(),
                "run attempt finished",
            );
            if trace_attempts {
                trace
                    .emit(TraceEvent::new(
                        "run_attempt_finished",
                        json!({
                            "run_id": run_id.0.clone(),
                            "agent_id": spec.id.clone(),
                            "attempt": attempt,
                            "status": result.status.clone(),
                            "retryable": retryable,
                            "error": result.error.clone(),
                        }),
                    ))
                    .await?;
            }

            if !retryable || attempt >= max_attempts {
                if retryable && attempt >= max_attempts {
                    result.error = result.error.map(|mut error| {
                        error.details["attempts"] = json!(attempt);
                        error.details["retry_exhausted"] = json!(true);
                        error
                    });
                }
                return Ok(result);
            }

            let next_attempt = attempt + 1;
            warn!(
                run_id = %run_id.0,
                agent_id = %spec.id,
                attempt,
                next_attempt,
                backoff_ms = self.policy.retry_backoff.as_millis(),
                "scheduling run retry",
            );
            trace
                .emit(TraceEvent::new(
                    "run_retry_scheduled",
                    json!({
                        "run_id": run_id.0.clone(),
                        "agent_id": spec.id.clone(),
                        "attempt": attempt,
                        "next_attempt": next_attempt,
                        "backoff_ms": self.policy.retry_backoff.as_millis(),
                    }),
                ))
                .await?;
            if !self.policy.retry_backoff.is_zero() {
                tokio::select! {
                    _ = cancellation.cancelled() => {
                        emit_cancellation_events(
                            trace.as_ref(),
                            &run_id,
                            &spec.id,
                            attempt,
                            "retry_backoff",
                            true,
                        )
                        .await?;
                        return Ok(failure_result(
                            run_id,
                            &spec.id,
                            started_at,
                            AgentError::cancelled("agent run cancelled during retry backoff"),
                        ));
                    }
                    _ = tokio::time::sleep(self.policy.retry_backoff) => {}
                }
            }
            attempt = next_attempt;
        }
    }

    pub async fn tick(&self, request: RunRequest) -> Result<Vec<RunOutcome>, AgentError> {
        let now = OffsetDateTime::now_utc();
        let scope = request
            .user
            .as_ref()
            .map(|u| RunScope::User(u.user_id.clone()))
            .unwrap_or(RunScope::Global);
        let mut outcomes = Vec::new();
        let specs = self.registry.list_agents().await?;
        info!(
            agent_count = specs.len(),
            scope = ?scope,
            trigger = ?request.trigger,
            "evaluating scheduled agents",
        );
        for spec in specs {
            let last = self
                .run_store
                .last_run(&spec.id, &scope)
                .await
                .map_err(|e| AgentError::internal(e.to_string()))?;
            if self.scheduler.should_fire(&spec, now, last.as_ref()) {
                info!(
                    agent_id = %spec.id,
                    last_run_id = last
                        .as_ref()
                        .map(|run| run.run_id.0.as_str())
                        .unwrap_or("none"),
                    "scheduled agent is due",
                );
                outcomes.push(self.run_once(&spec.id, request.clone()).await?);
            } else {
                debug!(
                    agent_id = %spec.id,
                    last_run_id = last
                        .as_ref()
                        .map(|run| run.run_id.0.as_str())
                        .unwrap_or("none"),
                    "scheduled agent is not due",
                );
            }
        }
        info!(run_count = outcomes.len(), "scheduler tick finished");
        Ok(outcomes)
    }
}

pub fn run_idempotency_key(agent_id: &str, scope: &RunScope, request: &RunRequest) -> String {
    let scheduled_for = request
        .metadata
        .get("scheduled_for")
        .cloned()
        .unwrap_or(Value::Null);
    let material = json!({
        "agent_id": agent_id,
        "scope": scope,
        "trigger_kind": &request.trigger,
        "scheduled_for": scheduled_for,
    });
    let bytes = serde_json::to_vec(&material).unwrap_or_else(|_| agent_id.as_bytes().to_vec());
    format!("idem_{}", blake3::hash(&bytes).to_hex())
}

fn result_is_retryable(result: &AgentRunResult) -> bool {
    if matches!(
        result.status,
        AgentRunStatus::Completed | AgentRunStatus::Skipped | AgentRunStatus::Cancelled
    ) {
        return false;
    }
    result.error.as_ref().is_some_and(|error| error.retryable)
}

async fn emit_cancellation_events(
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

fn failure_result(
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
        error: Some(err.record),
    }
}
