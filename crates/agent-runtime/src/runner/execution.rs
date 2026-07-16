use super::*;

impl AgentRunner {
    pub async fn run_once_with_control(
        &self,
        agent_id: &str,
        request: RunRequest,
        control: RunControl,
    ) -> Result<RunOutcome, AgentError> {
        agent_core::validate_protocol_version(&request.protocol_version)
            .map_err(AgentError::validation)?;
        let run_timer = std::time::Instant::now();
        let _permit = if self.is_nested {
            self.subagent_concurrency
                .clone()
                .try_acquire_owned()
                .map_err(|_| AgentError::validation("subagent concurrency limit reached"))?
        } else {
            self.concurrency
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| AgentError::internal(format!("run concurrency limiter closed: {e}")))?
        };
        debug!(agent_id, "acquired run concurrency permit");
        let agent = self
            .registry
            .get_agent(agent_id)
            .await?
            .ok_or_else(|| AgentError::validation(format!("unknown agent '{agent_id}'")))?;
        let spec = agent.spec();
        agent_core::validate_protocol_version(&spec.protocol_version)
            .map_err(AgentError::validation)?;
        let run_id = request.run_id.clone().unwrap_or_else(RunId::new_v7);
        let started_at = OffsetDateTime::now_utc();
        let scope = request_scope(&request)?;
        let idempotency_key = run_idempotency_key(&spec.id, &scope, &request, &run_id);
        let lock_key = lock_key(&spec.id, &scope);
        let trace = Arc::new(match (control.trace_events, control.trace_event_buffer) {
            (Some(sender), buffer) => MemoryTraceSink::with_event_sender(sender, buffer),
            (None, Some(buffer)) => MemoryTraceSink::with_event_buffer(buffer),
            (None, None) => MemoryTraceSink::default(),
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
            if let Some(existing) = self
                .run_store
                .find_run_by_idempotency_key(&spec.id, &scope, &idempotency_key)
                .await
                .map_err(|e| AgentError::internal(e.to_string()))?
            {
                return Ok(deduplicated_outcome(existing, &spec));
            }
            let reason = format!("run skipped because active lease exists for {lock_key}");
            warn!(
                run_id = %run_id.0,
                agent_id = %spec.id,
                lock_key,
                "skipping agent run because active lease exists",
            );
            let mut result = AgentRunResult::skipped(
                run_id.clone(),
                spec.id.clone(),
                started_at,
                Some(reason.clone()),
            );
            result.workflow = request.workflow.clone();
            self.run_store
                .create_run(AgentRunRecord {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    version: 1,
                    run_id: run_id.clone(),
                    idempotency_key: Some(idempotency_key.clone()),
                    agent_id: spec.id.clone(),
                    status: AgentRunStatus::Skipped,
                    scope: scope.clone(),
                    started_at,
                    finished_at: Some(result.finished_at),
                    input: request.input.clone(),
                    output: result.output.clone(),
                    error: None,
                    workflow: request.workflow.clone(),
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
            let run_span = run_trace_span(
                &run_id,
                &spec.id,
                started_at,
                result.finished_at,
                &result.status,
            );
            let events = trace.events().await;
            let artifact_refs = artifact_refs_from_events(&events);
            let usage_summary = trace_usage_summary_from_events(&events);
            let trace_doc = AgentTrace {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                runtime_version: RUNTIME_VERSION.to_owned(),
                run_id,
                agent_id: spec.id,
                agent_version: spec.version,
                scope,
                started_at,
                finished_at: result.finished_at,
                input: request.input,
                output: result.output.clone(),
                workflow: request.workflow,
                usage_summary,
                spans: trace_spans_from_events(run_span, &events),
                events,
                artifact_refs,
            };
            return Ok(RunOutcome {
                result,
                trace: trace_doc,
                disposition: RunDisposition::Executed,
            });
        };
        debug!(
            run_id = %run_id.0,
            agent_id = %spec.id,
            lock_key,
            "run lease acquired",
        );
        let lease_run_id = run_id.clone();
        let lease_agent_id = spec.id.clone();
        let lease_renewer = spawn_lease_renewer(
            self.lock_store.clone(),
            lease.clone(),
            self.policy.lease_ttl(),
            "agent_run",
            run_id.0.clone(),
            Some(control.cancellation.clone()),
        );
        let leased_run: Result<RunOutcome, AgentError> = async {
            if let Some(existing) = self
                .run_store
                .find_run_by_idempotency_key(&spec.id, &scope, &idempotency_key)
                .await
                .map_err(|e| AgentError::internal(e.to_string()))?
            {
                return Ok(deduplicated_outcome(existing, &spec));
            }
            self.run_store
                .create_run(AgentRunRecord {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    version: 1,
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
                    workflow: request.workflow.clone(),
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
            let cancellation_watcher = spawn_persisted_cancellation_watcher(
                self.run_store.clone(),
                run_id.clone(),
                spec.id.clone(),
                control.cancellation.clone(),
            );

            let mut run_started_payload = json!({
                "run_id": run_id.0.clone(),
                "agent_id": spec.id.clone(),
                "trigger": request.trigger,
            });
            if let Some(envelope) = &request.trigger_envelope {
                insert_json_field(&mut run_started_payload, "trigger_envelope", envelope);
            }
            trace
                .emit(TraceEvent::new("run_started", run_started_payload))
                .await?;
            let mut hook_payload = json!({
                "run_id": run_id.0.clone(),
                "agent_id": spec.id.clone(),
                "trigger": request.trigger,
                "input": request.input,
                "metadata": request.metadata,
            });
            if let Some(envelope) = &request.trigger_envelope {
                insert_json_field(&mut hook_payload, "trigger_envelope", envelope);
            }
            self.hooks
                .observe(
                    HookEventName::RunStart,
                    Some(run_id.clone()),
                    Some(spec.id.clone()),
                    hook_payload,
                    trace.as_ref(),
                )
                .await?;

            let result = self
                .run_with_retries(
                    agent,
                    &spec,
                    run_id.clone(),
                    started_at,
                    request.clone(),
                    scope.clone(),
                    trace.clone(),
                    control.cancellation.clone(),
                )
                .await;
            cancellation_watcher.abort();
            let mut result = result?;
            result.finished_at = OffsetDateTime::now_utc();
            result.workflow = request.workflow.clone();

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

            let final_record = AgentRunRecord {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                version: 1,
                run_id: result.run_id.clone(),
                idempotency_key: Some(idempotency_key),
                agent_id: result.agent_id.clone(),
                status: result.status.clone(),
                scope: scope.clone(),
                started_at,
                finished_at: Some(result.finished_at),
                input: request.input.clone(),
                output: result.output.clone(),
                error: result.error.clone(),
                workflow: request.workflow.clone(),
                metadata: request.metadata.clone(),
            };
            update_running_run_with_retry(self.run_store.as_ref(), final_record)
                .await
                .inspect_err(|error| {
                    error!(
                        run_id = %result.run_id.0,
                        agent_id = %result.agent_id,
                        error = %error,
                        "failed to update run record",
                    );
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

            let events = trace.events().await;
            let artifact_refs = artifact_refs_from_events(&events);
            let usage_summary = trace_usage_summary_from_events(&events);
            let run_span = run_trace_span(
                &result.run_id,
                &result.agent_id,
                started_at,
                result.finished_at,
                &result.status,
            );
            let trace_doc = AgentTrace {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                runtime_version: RUNTIME_VERSION.to_owned(),
                run_id: result.run_id.clone(),
                agent_id: result.agent_id.clone(),
                agent_version: spec.version.clone(),
                scope,
                started_at,
                finished_at: result.finished_at,
                input: request.input,
                output: result.output.clone(),
                workflow: result.workflow.clone(),
                usage_summary,
                spans: trace_spans_from_events(run_span, &events),
                events,
                artifact_refs,
            };

            Ok(RunOutcome {
                result,
                trace: trace_doc,
                disposition: RunDisposition::Executed,
            })
        }
        .await;
        let leased_run = match leased_run {
            Ok(outcome) => Ok(outcome),
            Err(run_error) => {
                match self.run_store.get_run(&run_id).await {
                    Ok(Some(mut record)) if record.status == AgentRunStatus::Running => {
                        record.status = AgentRunStatus::Failed;
                        record.finished_at = Some(OffsetDateTime::now_utc());
                        record.error = Some((*run_error.record).clone());
                        if let Err(store_error) =
                            update_running_run_with_retry(self.run_store.as_ref(), record).await
                        {
                            error!(
                                run_id = %run_id.0,
                                agent_id = %spec.id,
                                error = %store_error,
                                "failed to finalize run after infrastructure error",
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(store_error) => error!(
                        run_id = %run_id.0,
                        agent_id = %spec.id,
                        error = %store_error,
                        "failed to load run for infrastructure-error finalization",
                    ),
                }
                Err(run_error)
            }
        };

        stop_lease_renewer(lease_renewer).await;
        let release_result = self.lock_store.release(lease).await.map_err(|e| {
            error!(
                run_id = %lease_run_id.0,
                agent_id = %lease_agent_id,
                error = %e,
                "failed to release run lease",
            );
            AgentError::internal(e.to_string())
        });
        if release_result.is_ok() {
            debug!(
                run_id = %lease_run_id.0,
                agent_id = %lease_agent_id,
                "run lease released",
            );
        }
        match (leased_run, release_result) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        }
    }
}
