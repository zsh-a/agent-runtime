use super::*;

impl RuntimeServer {
    pub(crate) async fn run_agent(
        &self,
        agent_id: String,
        params: HttpAgentRunParams,
    ) -> Result<AgentRunResponse> {
        let started_at = std::time::Instant::now();
        let HttpAgentRunParams {
            run_id,
            input,
            session_id,
            thread_id,
            trigger,
            trigger_envelope,
            workflow,
            user,
            scope,
            metadata,
        } = params;
        let run_id = match run_id {
            Some(value) if !value.trim().is_empty() => RunId(value),
            Some(_) => return Err(miette!("run_id cannot be empty")),
            None => RunId::new_v7(),
        };
        let metadata = merge_run_metadata(metadata, session_id.as_deref(), thread_id.as_deref());
        let cancellation = CancellationToken::new();
        let (events, _) = broadcast::channel(256);
        let event_buffer = Arc::new(TraceEventBuffer::default());
        let event_log_task =
            spawn_run_event_logger(self.event_store.clone(), run_id.clone(), events.subscribe());
        {
            let mut active = self.active_runs.lock().await;
            if active.contains_key(&run_id.0) {
                event_log_task.abort();
                return Err(miette!("run '{}' is already active", run_id.0));
            }
            active.insert(
                run_id.0.clone(),
                ActiveRun {
                    cancellation: cancellation.clone(),
                    events: events.clone(),
                    event_buffer: event_buffer.clone(),
                },
            );
        }
        info!(
            run_id = %run_id.0,
            agent_id = %agent_id,
            session_id = session_id.as_deref().unwrap_or("none"),
            thread_id = thread_id.as_deref().unwrap_or("none"),
            trigger = ?trigger,
            input_bytes = serialized_value_len(&input),
            "server run_agent requested",
        );
        let control = RunControl {
            cancellation,
            trace_events: Some(events),
            trace_event_buffer: Some(event_buffer),
        };
        let outcome = self
            .runner
            .run_once_with_control(
                &agent_id,
                RunRequest {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    run_id: Some(run_id.clone()),
                    input,
                    user,
                    scope,
                    trigger,
                    trigger_envelope,
                    workflow,
                    metadata,
                },
                control,
            )
            .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.active_runs.lock().await.remove(&run_id.0);
                stop_run_event_logger(event_log_task).await;
                return Err(error).into_diagnostic();
            }
        };
        let should_persist_trace = outcome.should_persist_trace();
        let trace_events = outcome.trace.events.clone();
        stop_run_event_logger(event_log_task).await;
        if should_persist_trace
            && let Err(err) = self
                .event_store
                .replace_run_events(&outcome.trace.run_id, trace_events)
                .await
        {
            warn!(
                run_id = %outcome.trace.run_id.0,
                error = %err,
                "failed to persist final run event log",
            );
        }
        let persist_result: Result<()> = async {
            record_session_step(self.session_store.as_ref(), thread_id.as_deref(), &outcome)
                .await?;
            if should_persist_trace {
                self.trace_store
                    .write_trace(outcome.trace.clone())
                    .await
                    .into_diagnostic()?;
            }
            Ok(())
        }
        .await;
        self.active_runs.lock().await.remove(&run_id.0);
        persist_result?;
        info!(
            run_id = %outcome.result.run_id.0,
            agent_id = %outcome.result.agent_id,
            status = ?outcome.result.status,
            duration_ms = started_at.elapsed().as_millis(),
            "server run_agent completed",
        );
        Ok(AgentRunResponse {
            result: outcome.result,
            trace: outcome.trace,
        })
    }

    pub(crate) async fn run_workflow(
        &self,
        request: WorkflowRunRequest,
    ) -> Result<WorkflowRunResult> {
        info!(
            workflow_id = %request.workflow_id,
            node_count = request.nodes.len(),
            trigger = ?request.trigger,
            "server workflow run requested",
        );
        let result = self.runner.run_workflow(request).await.into_diagnostic()?;
        let trace_count = self
            .trace_store
            .write_workflow_traces(&result)
            .await
            .into_diagnostic()?;
        info!(
            workflow_id = %result.workflow_id,
            status = ?result.status,
            trace_count,
            "server workflow run completed",
        );
        Ok(result)
    }

    pub(crate) async fn cancel_run(&self, run_id: RunId) -> Result<CancelRunResponse> {
        let active = self.active_runs.lock().await.get(&run_id.0).cloned();

        if let Some(active) = active {
            let persisted = self
                .persist_run_cancellation_request(&run_id, "http")
                .await?;
            active.cancellation.cancel();
            return Ok(CancelRunResponse {
                cancellation_requested: true,
                message: if persisted.is_some() {
                    "cancellation requested and persisted".to_owned()
                } else {
                    "cancellation requested; run record is not persisted yet".to_owned()
                },
                run_id: run_id.0,
                status: Some(AgentRunStatus::Running),
            });
        }

        if let Some(status) = self
            .persist_run_cancellation_request(&run_id, "http")
            .await?
        {
            if status == AgentRunStatus::Running {
                return Ok(CancelRunResponse {
                    cancellation_requested: true,
                    message: "cancellation intent persisted".to_owned(),
                    run_id: run_id.0,
                    status: Some(status),
                });
            }
            return Ok(CancelRunResponse {
                cancellation_requested: false,
                message: "run is not active".to_owned(),
                run_id: run_id.0,
                status: Some(status),
            });
        }

        let run = self.get_run(run_id.clone()).await?;
        Ok(CancelRunResponse {
            cancellation_requested: false,
            message: "run is not active".to_owned(),
            run_id: run_id.0,
            status: Some(run.status),
        })
    }

    async fn persist_run_cancellation_request(
        &self,
        run_id: &RunId,
        requested_by: &str,
    ) -> Result<Option<AgentRunStatus>> {
        const MAX_UPDATE_ATTEMPTS: usize = 8;
        for _ in 0..MAX_UPDATE_ATTEMPTS {
            let Some(mut run) = self.run_store.get_run(run_id).await.into_diagnostic()? else {
                return Ok(None);
            };
            let status = run.status.clone();
            if status != AgentRunStatus::Running || run.cancellation_requested() {
                return Ok(Some(status));
            }
            let expected_version = run.version;
            run.version = expected_version
                .checked_add(1)
                .ok_or_else(|| miette!("run record version overflow"))?;
            run.request_cancellation(OffsetDateTime::now_utc(), Some(requested_by.to_owned()));
            if self
                .run_store
                .update_run(run, expected_version)
                .await
                .into_diagnostic()?
            {
                return Ok(Some(status));
            }
        }
        Err(miette!("run cancellation update conflicted too many times"))
    }

    pub(crate) async fn subscribe_run_events(&self, run_id: &RunId) -> Option<ActiveRunEvents> {
        let (receiver, event_buffer) = {
            let active = self.active_runs.lock().await;
            let active = active.get(&run_id.0)?;
            (active.events.subscribe(), active.event_buffer.clone())
        };
        Some(ActiveRunEvents {
            receiver,
            replayed_events: event_buffer.events().await,
        })
    }

    pub(crate) async fn call_tool(&self, name: String, input: Value) -> Result<ToolCallResponse> {
        let started_at = std::time::Instant::now();
        info!(
            tool_name = %name,
            input_bytes = serialized_value_len(&input),
            "server tool call requested",
        );
        if let Err(err) = ensure_catalog_has_tool(&self.catalog, &name) {
            warn!(tool_name = %name, error = %err, "server rejected unknown tool");
            return Err(err);
        }
        let context = ExecutionContext {
            run_id: RunId::new_v7(),
            agent_id: "http.tool".to_owned(),
            scope: RunScope::Global,
            user: None,
            metadata: json!({"surface": "http"}),
        };
        let services = guarded_services(
            self.services.bind(context.clone()),
            context,
            self.hooks.clone(),
            CancellationToken::new(),
        );
        let output = services
            .call_tool(&name, input)
            .await
            .map_err(|err| miette!(err.record.message))?;
        info!(
            tool_name = %name,
            output_bytes = serialized_value_len(&output),
            duration_ms = started_at.elapsed().as_millis(),
            "server tool call completed",
        );
        Ok(ToolCallResponse { tool: name, output })
    }

    pub(crate) async fn get_run(&self, run_id: RunId) -> Result<AgentRunRecord> {
        debug!(run_id = %run_id.0, "server get_run requested");
        self.run_store
            .get_run(&run_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("run '{}' was not found", run_id.0))
    }

    pub(crate) async fn list_runs(
        &self,
        agent_id: Option<String>,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>> {
        debug!(
            agent_id = agent_id.as_deref().unwrap_or("all"),
            limit = ?limit,
            "server list_runs requested",
        );
        self.run_store
            .list_runs(agent_id.as_deref(), limit)
            .await
            .into_diagnostic()
    }

    pub(crate) async fn get_run_trace(&self, run_id: RunId) -> Result<Value> {
        debug!(run_id = %run_id.0, "server get_run_trace requested");
        let trace = self
            .trace_store
            .read_trace(&run_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("trace for run '{}' was not found", run_id.0))?;
        serde_json::to_value(trace).into_diagnostic()
    }

    pub(crate) async fn get_run_event_records_after(
        &self,
        run_id: RunId,
        after: RunEventCursor,
    ) -> Result<Option<Vec<RunEventRecord>>> {
        self.event_store
            .list_run_events_after(&run_id, after)
            .await
            .into_diagnostic()
    }

    pub(crate) async fn replay_run(&self, run_id: RunId) -> Result<ReplayExecutionReport> {
        info!(run_id = %run_id.0, "server replay_run requested");
        let trace_value = self.get_run_trace(run_id.clone()).await?;
        let source_trace: agent_core::AgentTrace = serde_json::from_value(trace_value)
            .map_err(|e| miette!("failed to parse trace for run '{}': {e}", run_id.0))?;
        replay_source_trace(
            self.runner.as_ref(),
            self.trace_store.as_ref(),
            source_trace,
            ReplayMode::Live,
        )
        .await
    }

    pub(crate) async fn metrics_summary(&self) -> Result<RuntimeMetricsSummary> {
        build_metrics_summary(
            &self.store_path,
            self.run_store.as_ref(),
            self.trace_store.as_ref(),
            self.proposal_store.as_ref(),
        )
        .await
    }
}
