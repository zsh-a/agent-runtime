use super::*;

impl AgentRunner {
    pub(super) async fn run_with_retries(
        &self,
        agent: Arc<dyn Agent>,
        spec: &AgentSpec,
        run_id: RunId,
        started_at: OffsetDateTime,
        request: RunRequest,
        scope: RunScope,
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
            if persisted_cancellation_requested(self.run_store.as_ref(), &run_id).await? {
                cancellation.cancel();
                warn!(
                    run_id = %run_id.0,
                    agent_id = %spec.id,
                    attempt,
                    "agent run cancelled before attempt started by persisted cancellation intent",
                );
                emit_cancellation_events(
                    trace.as_ref(),
                    &run_id,
                    &spec.id,
                    attempt,
                    "persisted_cancel_request",
                    true,
                )
                .await?;
                return Ok(failure_result(
                    run_id,
                    &spec.id,
                    started_at,
                    AgentError::cancelled("agent run cancelled by persisted cancellation request"),
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

            let attempt_timer = std::time::Instant::now();
            let step_input = json!({
                "run_id": run_id.0.clone(),
                "agent_id": spec.id.clone(),
                "attempt": attempt,
                "max_attempts": max_attempts,
                "input": request.input.clone(),
                "metadata": request.metadata.clone(),
            });
            let decision = self
                .hooks
                .authorize(
                    HookEventName::BeforeAgentStep,
                    Some(run_id.clone()),
                    Some(spec.id.clone()),
                    step_input.clone(),
                    trace.as_ref(),
                )
                .await?;
            let step_started = !decision.is_denied();
            let mut result = if decision.is_denied() {
                failure_result(
                    run_id.clone(),
                    &spec.id,
                    started_at,
                    AgentError::policy_denied(
                        decision
                            .reason
                            .clone()
                            .unwrap_or_else(|| "agent step denied by policy hook".to_owned()),
                        json!({
                            "decision": decision,
                            "event": "BeforeAgentStep",
                            "attempt": attempt,
                        }),
                    ),
                )
            } else {
                self.hooks
                    .observe(
                        HookEventName::BeforeAgentStep,
                        Some(run_id.clone()),
                        Some(spec.id.clone()),
                        step_input,
                        trace.as_ref(),
                    )
                    .await?;

                self.execute_agent_step(
                    agent.clone(),
                    spec,
                    &run_id,
                    started_at,
                    &request,
                    &scope,
                    trace.clone(),
                    cancellation.clone(),
                    attempt,
                    attempt_timer,
                )
                .await?
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
            if step_started {
                self.hooks
                    .observe(
                        HookEventName::AfterAgentStep,
                        Some(run_id.clone()),
                        Some(spec.id.clone()),
                        json!({
                            "run_id": run_id.0.clone(),
                            "agent_id": spec.id.clone(),
                            "attempt": attempt,
                            "max_attempts": max_attempts,
                            "status": result.status.clone(),
                            "retryable": retryable,
                            "error": result.error.clone(),
                            "output": result.output.clone(),
                            "duration_ms": attempt_timer.elapsed().as_millis(),
                        }),
                        trace.as_ref(),
                    )
                    .await?;
            }
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

    async fn execute_agent_step(
        &self,
        agent: Arc<dyn Agent>,
        spec: &AgentSpec,
        run_id: &RunId,
        started_at: OffsetDateTime,
        request: &RunRequest,
        scope: &RunScope,
        trace: Arc<MemoryTraceSink>,
        cancellation: CancellationToken,
        attempt: u32,
        attempt_timer: std::time::Instant,
    ) -> Result<AgentRunResult, AgentError> {
        let ctx = AgentContext {
            run_id: run_id.clone(),
            now: started_at,
            user: request.user.clone(),
            scope: scope.clone(),
            input: request.input.clone(),
            services: Arc::new(TracedAgentServices {
                inner: self.services.bind(ExecutionContext {
                    run_id: run_id.clone(),
                    agent_id: spec.id.clone(),
                    scope: scope.clone(),
                    user: request.user.clone(),
                    metadata: request.metadata.clone(),
                }),
                trace: trace.clone(),
                run_id: run_id.clone(),
                agent_id: spec.id.clone(),
                user: request.user.clone(),
                scope: scope.clone(),
                hooks: self.hooks.clone(),
                subagent_runner: Some(self.nested_runner()),
                cancellation: cancellation.clone(),
                workflow: request.workflow.clone(),
            }),
            cancellation: agent_cancellation(cancellation.clone()),
            trace: trace.clone(),
        };
        let run_future = agent.run(ctx);
        let result = tokio::select! {
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
                    run_id,
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
                            run_id,
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
        Ok(result)
    }
}
