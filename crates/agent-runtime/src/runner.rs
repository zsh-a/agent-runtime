use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use agent_core::{
    Agent, AgentContext, AgentError, AgentRegistry, AgentRunRecord, AgentRunResult, AgentRunStatus,
    AgentRunStore, AgentServices, AgentSpec, AgentTrace, ArtifactRef, HookEventName,
    PROTOCOL_VERSION, RunCompensation, RunDependency, RunId, RunLease, RunRequest, RunScope,
    RunWorkflow, TraceEvent, TraceSink, TraceSpan, TraceUsageProviderSummary, TraceUsageSummary,
    WorkflowInputMapping, WorkflowInputTransform, WorkflowRunNode,
    WorkflowRunNodeCompensationResult, WorkflowRunNodeResult, WorkflowRunRequest,
    WorkflowRunResult,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::sync::{Semaphore, broadcast};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::{
    InMemoryLockStore, RUNTIME_VERSION,
    hooks::HookManager,
    lock::{lock_key, workflow_lock_key},
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

const STORE_CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(25);

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

    fn workflow_task_runner(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            run_store: self.run_store.clone(),
            services: self.services.clone(),
            scheduler: self.scheduler,
            policy: self.policy.clone(),
            concurrency: self.concurrency.clone(),
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

    pub async fn run_workflow(
        &self,
        request: WorkflowRunRequest,
    ) -> Result<WorkflowRunResult, AgentError> {
        let started_at = OffsetDateTime::now_utc();
        let order = workflow_execution_order(&request.nodes)?;
        let scope = workflow_request_scope(&request)?;
        let workflow_id = request.workflow_id.clone();
        let lock_key = workflow_lock_key(&workflow_id, &scope);
        let lease_owner = format!("workflow:{}", RunId::new_v7().0);
        debug!(
            workflow_id = %workflow_id,
            scope = ?scope,
            lock_key,
            lease_ttl_ms = self.policy.lease_ttl().as_millis(),
            "acquiring workflow lease",
        );
        let Some(lease) = self
            .lock_store
            .acquire(&lock_key, &lease_owner, self.policy.lease_ttl())
            .await
            .map_err(|e| AgentError::internal(e.to_string()))?
        else {
            warn!(
                workflow_id = %workflow_id,
                scope = ?scope,
                lock_key,
                "skipping workflow run because active lease exists",
            );
            return Ok(skipped_workflow_result(
                request,
                started_at,
                "workflow_lease_active",
                json!({
                    "lock_key": lock_key,
                    "scope": scope,
                }),
            ));
        };

        let lease_renewer = spawn_lease_renewer(
            self.lock_store.clone(),
            lease.clone(),
            self.policy.lease_ttl(),
            "workflow",
            workflow_id.clone(),
        );
        let leased_workflow = self.run_workflow_locked(request, started_at, order).await;
        stop_lease_renewer(lease_renewer).await;
        let release_result = self.lock_store.release(lease).await.map_err(|e| {
            error!(
                workflow_id = %workflow_id,
                error = %e,
                "failed to release workflow lease",
            );
            AgentError::internal(e.to_string())
        });
        if release_result.is_ok() {
            debug!(
                workflow_id = %workflow_id,
                "workflow lease released",
            );
        }
        match (leased_workflow, release_result) {
            (Ok(result), Ok(())) => Ok(result),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        }
    }

    async fn run_workflow_locked(
        &self,
        request: WorkflowRunRequest,
        started_at: OffsetDateTime,
        order: Vec<usize>,
    ) -> Result<WorkflowRunResult, AgentError> {
        let planned_run_ids = planned_workflow_run_ids(&request.nodes);
        let root_run_id = request.root_run_id.clone().or_else(|| {
            order
                .first()
                .and_then(|index| planned_run_ids.get(&request.nodes[*index].node_id))
                .cloned()
        });
        let mut node_results: HashMap<String, WorkflowRunNodeResult> = HashMap::new();
        let mut pending: HashSet<usize> = order.iter().copied().collect();
        let mut running = JoinSet::new();
        let mut active_agent_ids = HashSet::new();

        while !pending.is_empty() || !running.is_empty() {
            let mut progressed = false;
            let mut resolved_indexes = Vec::new();

            for index in &order {
                if !pending.contains(index) {
                    continue;
                }
                let node = &request.nodes[*index];
                if !workflow_dependencies_resolved(node, &node_results) {
                    continue;
                }

                let blocked_dependencies = blocked_workflow_dependencies(node, &node_results);
                if !blocked_dependencies.is_empty() {
                    node_results.insert(
                        node.node_id.clone(),
                        skipped_workflow_node_result(node, blocked_dependencies),
                    );
                    resolved_indexes.push(*index);
                    progressed = true;
                    continue;
                }

                if active_agent_ids.contains(&node.agent_id) {
                    continue;
                }
                active_agent_ids.insert(node.agent_id.clone());
                resolved_indexes.push(*index);
                progressed = true;

                let runner = self.workflow_task_runner();
                let node = node.clone();
                let request = request.clone();
                let root_run_id = root_run_id.clone();
                let planned_run_ids = planned_run_ids.clone();
                let dependency_results = node_results.clone();
                running.spawn(async move {
                    let node_id = node.node_id.clone();
                    let agent_id = node.agent_id.clone();
                    let result = runner
                        .run_workflow_node(
                            &node,
                            &request,
                            root_run_id,
                            &planned_run_ids,
                            &dependency_results,
                        )
                        .await;
                    (node_id, agent_id, result)
                });
            }

            for index in resolved_indexes {
                pending.remove(&index);
            }

            if !progressed {
                let Some(joined) = running.join_next().await else {
                    if pending.is_empty() {
                        break;
                    }
                    return Err(AgentError::internal(
                        "workflow DAG scheduler stalled with pending nodes",
                    ));
                };
                let (node_id, agent_id, result) = joined.map_err(|error| {
                    AgentError::internal(format!("workflow node task failed: {error}"))
                })?;
                active_agent_ids.remove(&agent_id);
                node_results.insert(node_id, result);
            }
        }

        let mut ordered_results = Vec::with_capacity(order.len());
        for index in &order {
            let node = &request.nodes[*index];
            let result = node_results.get(&node.node_id).cloned().ok_or_else(|| {
                AgentError::internal(format!(
                    "workflow DAG scheduler did not produce a result for node '{}'",
                    node.node_id
                ))
            })?;
            ordered_results.push(result);
        }

        let mut status = workflow_status(&ordered_results);
        if workflow_needs_compensation(&ordered_results) {
            self.run_workflow_compensations(&request, root_run_id.clone(), &mut ordered_results)
                .await;
            status = workflow_status(&ordered_results);
        }
        Ok(WorkflowRunResult {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            workflow_id: request.workflow_id,
            status,
            started_at,
            finished_at: OffsetDateTime::now_utc(),
            root_run_id,
            nodes: ordered_results,
            metadata: request.metadata,
        })
    }

    async fn run_workflow_node(
        &self,
        node: &WorkflowRunNode,
        request: &WorkflowRunRequest,
        root_run_id: Option<RunId>,
        planned_run_ids: &HashMap<String, RunId>,
        node_results: &HashMap<String, WorkflowRunNodeResult>,
    ) -> WorkflowRunNodeResult {
        let run_id = planned_run_ids.get(&node.node_id).cloned();
        let dependencies = workflow_run_dependencies(node, node_results);
        let (parent_run_id, parent_agent_id) =
            workflow_parent_from_dependencies(node, node_results);
        let input = match workflow_node_input(node, node_results) {
            Ok(input) => input,
            Err(error) => {
                return WorkflowRunNodeResult {
                    node_id: node.node_id.clone(),
                    agent_id: node.agent_id.clone(),
                    status: AgentRunStatus::Failed,
                    run_id,
                    depends_on: node.depends_on.clone(),
                    output: json!({}),
                    error: Some(error.record),
                    trace: None,
                    compensation: None,
                    metadata: json!({
                        "reason": "input_mapping_failed",
                    }),
                };
            }
        };
        let workflow = RunWorkflow {
            workflow_id: Some(request.workflow_id.clone()),
            root_run_id,
            parent_run_id,
            parent_agent_id,
            dependencies,
            fanout_id: None,
            fanin_id: None,
            compensation: None,
            metadata: json!({
                "workflow_node_id": node.node_id.clone(),
                "workflow_metadata": request.metadata.clone(),
                "node_metadata": node.metadata.clone(),
            }),
        };
        let run_request = RunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: run_id.clone(),
            input,
            user: request.user.clone(),
            scope: request.scope.clone(),
            trigger: request.trigger.clone(),
            trigger_envelope: request.trigger_envelope.clone(),
            workflow: Some(workflow),
            metadata: json!({
                "source": "workflow_dag",
                "workflow_id": request.workflow_id.clone(),
                "workflow_node_id": node.node_id.clone(),
                "workflow_metadata": request.metadata.clone(),
                "node_metadata": node.metadata.clone(),
            }),
        };

        match self.run_once(&node.agent_id, run_request).await {
            Ok(outcome) => WorkflowRunNodeResult {
                node_id: node.node_id.clone(),
                agent_id: node.agent_id.clone(),
                status: outcome.result.status,
                run_id: Some(outcome.result.run_id),
                depends_on: node.depends_on.clone(),
                output: outcome.result.output,
                error: outcome.result.error,
                trace: Some(outcome.trace),
                compensation: None,
                metadata: json!({}),
            },
            Err(error) => WorkflowRunNodeResult {
                node_id: node.node_id.clone(),
                agent_id: node.agent_id.clone(),
                status: AgentRunStatus::Failed,
                run_id,
                depends_on: node.depends_on.clone(),
                output: json!({}),
                error: Some(error.record),
                trace: None,
                compensation: None,
                metadata: json!({}),
            },
        }
    }

    async fn run_workflow_compensations(
        &self,
        request: &WorkflowRunRequest,
        root_run_id: Option<RunId>,
        ordered_results: &mut [WorkflowRunNodeResult],
    ) {
        for index in (0..ordered_results.len()).rev() {
            let result = &ordered_results[index];
            if result.status != AgentRunStatus::Completed {
                continue;
            }
            let Some(compensated_run_id) = result.run_id.clone() else {
                continue;
            };
            let Some(node) = request
                .nodes
                .iter()
                .find(|node| node.node_id == result.node_id)
            else {
                continue;
            };
            let Some(compensation) = node.compensation.as_ref() else {
                continue;
            };
            ordered_results[index].compensation = Some(
                self.run_workflow_compensation_node(
                    request,
                    node,
                    compensation,
                    root_run_id.clone(),
                    compensated_run_id,
                    result.agent_id.clone(),
                )
                .await,
            );
        }
    }

    async fn run_workflow_compensation_node(
        &self,
        request: &WorkflowRunRequest,
        node: &WorkflowRunNode,
        compensation: &agent_core::WorkflowRunNodeCompensation,
        root_run_id: Option<RunId>,
        compensated_run_id: RunId,
        compensated_agent_id: String,
    ) -> WorkflowRunNodeCompensationResult {
        let run_id = compensation.run_id.clone().unwrap_or_else(RunId::new_v7);
        let workflow = RunWorkflow {
            workflow_id: Some(request.workflow_id.clone()),
            root_run_id,
            parent_run_id: Some(compensated_run_id.clone()),
            parent_agent_id: Some(compensated_agent_id),
            dependencies: Vec::new(),
            fanout_id: None,
            fanin_id: None,
            compensation: Some(RunCompensation {
                compensates_run_id: compensated_run_id.clone(),
                strategy: compensation.strategy.clone(),
                metadata: json!({
                    "workflow_node_id": node.node_id.clone(),
                    "compensation_metadata": compensation.metadata.clone(),
                }),
            }),
            metadata: json!({
                "workflow_node_id": node.node_id.clone(),
                "workflow_compensation": true,
                "workflow_metadata": request.metadata.clone(),
                "node_metadata": node.metadata.clone(),
            }),
        };
        let run_request = RunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: Some(run_id.clone()),
            input: compensation.input.clone(),
            user: request.user.clone(),
            scope: request.scope.clone(),
            trigger: request.trigger.clone(),
            trigger_envelope: request.trigger_envelope.clone(),
            workflow: Some(workflow),
            metadata: json!({
                "source": "workflow_compensation",
                "workflow_id": request.workflow_id.clone(),
                "workflow_node_id": node.node_id.clone(),
                "compensates_run_id": compensated_run_id.0.clone(),
                "workflow_metadata": request.metadata.clone(),
                "node_metadata": node.metadata.clone(),
                "compensation_metadata": compensation.metadata.clone(),
            }),
        };

        match self.run_once(&compensation.agent_id, run_request).await {
            Ok(outcome) => WorkflowRunNodeCompensationResult {
                agent_id: compensation.agent_id.clone(),
                status: outcome.result.status,
                run_id: Some(outcome.result.run_id),
                output: outcome.result.output,
                error: outcome.result.error,
                trace: Some(outcome.trace),
                metadata: json!({}),
            },
            Err(error) => WorkflowRunNodeCompensationResult {
                agent_id: compensation.agent_id.clone(),
                status: AgentRunStatus::Failed,
                run_id: Some(run_id),
                output: json!({}),
                error: Some(error.record),
                trace: None,
                metadata: json!({}),
            },
        }
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
        let scope = request_scope(&request)?;
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
        );
        let leased_run = async {
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

            let mut final_record = AgentRunRecord {
                protocol_version: PROTOCOL_VERSION.to_owned(),
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
            if let Some(existing) = self
                .run_store
                .get_run(&result.run_id)
                .await
                .map_err(|e| AgentError::internal(e.to_string()))?
            {
                final_record.merge_control_metadata_from(&existing);
            }
            self.run_store.update_run(final_record).await.map_err(|e| {
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
            })
        }
        .await;

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

    async fn run_with_retries(
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
                inner: self.services.clone(),
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
            cancellation: cancellation.clone(),
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

    pub async fn tick(&self, request: RunRequest) -> Result<Vec<RunOutcome>, AgentError> {
        let now = OffsetDateTime::now_utc();
        let scope = request_scope(&request)?;
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

fn workflow_execution_order(nodes: &[WorkflowRunNode]) -> Result<Vec<usize>, AgentError> {
    if nodes.is_empty() {
        return Err(AgentError::validation(
            "workflow DAG requires at least one node",
        ));
    }
    let mut index_by_id = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        if node.node_id.trim().is_empty() {
            return Err(AgentError::validation(
                "workflow DAG node_id must not be empty",
            ));
        }
        if node.agent_id.trim().is_empty() {
            return Err(AgentError::validation(format!(
                "workflow DAG node '{}' agent_id must not be empty",
                node.node_id
            )));
        }
        if index_by_id.insert(node.node_id.clone(), index).is_some() {
            return Err(AgentError::validation(format!(
                "workflow DAG contains duplicate node_id '{}'",
                node.node_id
            )));
        }
    }
    for node in nodes {
        for dependency in &node.depends_on {
            if !index_by_id.contains_key(dependency) {
                return Err(AgentError::validation(format!(
                    "workflow DAG node '{}' depends on unknown node '{}'",
                    node.node_id, dependency
                )));
            }
        }
        for mapping in &node.input_mappings {
            if !index_by_id.contains_key(&mapping.from_node) {
                return Err(AgentError::validation(format!(
                    "workflow DAG node '{}' maps input from unknown node '{}'",
                    node.node_id, mapping.from_node
                )));
            }
            if !node
                .depends_on
                .iter()
                .any(|dependency| dependency == &mapping.from_node)
            {
                return Err(AgentError::validation(format!(
                    "workflow DAG node '{}' maps input from node '{}' but does not list it in depends_on",
                    node.node_id, mapping.from_node
                )));
            }
            validate_workflow_json_pointer(
                &mapping.from_path,
                &format!(
                    "workflow DAG node '{}' input mapping from_path",
                    node.node_id
                ),
            )?;
            validate_workflow_json_pointer(
                &mapping.to_path,
                &format!("workflow DAG node '{}' input mapping to_path", node.node_id),
            )?;
        }
    }

    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut order = Vec::new();
    for index in 0..nodes.len() {
        visit_workflow_node(
            index,
            nodes,
            &index_by_id,
            &mut visiting,
            &mut visited,
            &mut order,
        )?;
    }
    Ok(order)
}

fn visit_workflow_node(
    index: usize,
    nodes: &[WorkflowRunNode],
    index_by_id: &HashMap<String, usize>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    order: &mut Vec<usize>,
) -> Result<(), AgentError> {
    let node_id = nodes[index].node_id.clone();
    if visited.contains(&node_id) {
        return Ok(());
    }
    if !visiting.insert(node_id.clone()) {
        return Err(AgentError::validation(format!(
            "workflow DAG contains a dependency cycle at node '{node_id}'"
        )));
    }
    for dependency in &nodes[index].depends_on {
        let dependency_index = *index_by_id
            .get(dependency)
            .expect("dependency existence validated before DFS");
        visit_workflow_node(
            dependency_index,
            nodes,
            index_by_id,
            visiting,
            visited,
            order,
        )?;
    }
    visiting.remove(&node_id);
    visited.insert(node_id);
    order.push(index);
    Ok(())
}

fn planned_workflow_run_ids(nodes: &[WorkflowRunNode]) -> HashMap<String, RunId> {
    nodes
        .iter()
        .map(|node| {
            (
                node.node_id.clone(),
                node.run_id.clone().unwrap_or_else(RunId::new_v7),
            )
        })
        .collect()
}

fn workflow_dependencies_resolved(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> bool {
    node.depends_on
        .iter()
        .all(|dependency| node_results.contains_key(dependency))
}

fn blocked_workflow_dependencies(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> Vec<String> {
    node.depends_on
        .iter()
        .filter(|dependency| {
            node_results
                .get(*dependency)
                .is_none_or(|result| result.status != AgentRunStatus::Completed)
        })
        .cloned()
        .collect()
}

fn skipped_workflow_node_result(
    node: &WorkflowRunNode,
    blocked_dependencies: Vec<String>,
) -> WorkflowRunNodeResult {
    WorkflowRunNodeResult {
        node_id: node.node_id.clone(),
        agent_id: node.agent_id.clone(),
        status: AgentRunStatus::Skipped,
        run_id: None,
        depends_on: node.depends_on.clone(),
        output: json!({}),
        error: None,
        trace: None,
        compensation: None,
        metadata: json!({
            "reason": "dependency_not_completed",
            "blocked_dependencies": blocked_dependencies,
        }),
    }
}

fn skipped_workflow_result(
    request: WorkflowRunRequest,
    started_at: OffsetDateTime,
    reason: &str,
    metadata: Value,
) -> WorkflowRunResult {
    WorkflowRunResult {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        workflow_id: request.workflow_id,
        status: AgentRunStatus::Skipped,
        started_at,
        finished_at: OffsetDateTime::now_utc(),
        root_run_id: request.root_run_id,
        nodes: request
            .nodes
            .into_iter()
            .map(|node| WorkflowRunNodeResult {
                node_id: node.node_id,
                agent_id: node.agent_id,
                status: AgentRunStatus::Skipped,
                run_id: None,
                depends_on: node.depends_on,
                output: json!({}),
                error: None,
                trace: None,
                compensation: None,
                metadata: json!({
                    "reason": reason,
                    "workflow": metadata.clone(),
                }),
            })
            .collect(),
        metadata: request.metadata,
    }
}

fn workflow_run_dependencies(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> Vec<RunDependency> {
    node.depends_on
        .iter()
        .filter_map(|dependency| {
            let result = node_results.get(dependency)?;
            Some(RunDependency {
                run_id: result.run_id.clone()?,
                edge: Some("depends_on".to_owned()),
                metadata: json!({
                    "workflow_node_id": dependency,
                }),
            })
        })
        .collect()
}

fn workflow_node_input(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> Result<Value, AgentError> {
    let mut input = node.input.clone();
    for mapping in &node.input_mappings {
        let Some(source_result) = node_results.get(&mapping.from_node) else {
            return Err(AgentError::validation(format!(
                "workflow DAG node '{}' input mapping source node '{}' has not completed",
                node.node_id, mapping.from_node
            )));
        };
        let value = match json_pointer_get(&source_result.output, &mapping.from_path) {
            Some(value) => value.clone(),
            None => match &mapping.default {
                Some(value) => value.clone(),
                None => {
                    return Err(AgentError::validation(format!(
                        "workflow DAG node '{}' input mapping source path '{}' was not found in node '{}' output",
                        node.node_id, mapping.from_path, mapping.from_node
                    )));
                }
            },
        };
        let value = apply_workflow_input_transform(&value, mapping).map_err(|error| {
            AgentError::validation(format!(
                "workflow DAG node '{}' input mapping from node '{}' transform failed: {error}",
                node.node_id, mapping.from_node
            ))
        })?;
        json_pointer_insert(&mut input, &mapping.to_path, value).map_err(AgentError::validation)?;
    }
    Ok(input)
}

fn apply_workflow_input_transform(
    value: &Value,
    mapping: &WorkflowInputMapping,
) -> Result<Value, String> {
    match mapping.transform {
        WorkflowInputTransform::None => Ok(value.clone()),
        WorkflowInputTransform::String => workflow_value_as_string(value)
            .map(Value::String)
            .ok_or_else(|| {
                format!(
                    "value at '{}' cannot be converted to string",
                    mapping.from_path
                )
            }),
        WorkflowInputTransform::Number => workflow_value_as_number(value)
            .map(Value::Number)
            .ok_or_else(|| {
                format!(
                    "value at '{}' cannot be converted to number",
                    mapping.from_path
                )
            }),
        WorkflowInputTransform::Integer => workflow_value_as_integer(value)
            .map(|value| Value::Number(value.into()))
            .ok_or_else(|| {
                format!(
                    "value at '{}' cannot be converted to integer",
                    mapping.from_path
                )
            }),
        WorkflowInputTransform::Boolean => workflow_value_as_boolean(value)
            .map(Value::Bool)
            .ok_or_else(|| {
                format!(
                    "value at '{}' cannot be converted to boolean",
                    mapping.from_path
                )
            }),
        WorkflowInputTransform::JsonString => serde_json::to_string(value)
            .map(Value::String)
            .map_err(|error| {
                format!(
                    "value at '{}' cannot be serialized as JSON: {error}",
                    mapping.from_path
                )
            }),
    }
}

fn workflow_value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => Some("null".to_owned()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn workflow_value_as_number(value: &Value) -> Option<serde_json::Number> {
    match value {
        Value::Number(value) => Some(value.clone()),
        Value::String(value) => value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .and_then(serde_json::Number::from_f64),
        _ => None,
    }
}

fn workflow_value_as_integer(value: &Value) -> Option<i64> {
    match value {
        Value::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok())),
        Value::String(value) => value.parse::<i64>().ok(),
        _ => None,
    }
}

fn workflow_value_as_boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn validate_workflow_json_pointer(pointer: &str, label: &str) -> Result<(), AgentError> {
    decode_json_pointer(pointer)
        .map(|_| ())
        .map_err(|error| AgentError::validation(format!("{label} {error}")))
}

fn json_pointer_get<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        Some(value)
    } else {
        value.pointer(pointer)
    }
}

fn json_pointer_insert(target: &mut Value, pointer: &str, value: Value) -> Result<(), String> {
    let segments = decode_json_pointer(pointer)?;
    if segments.is_empty() {
        *target = value;
        return Ok(());
    }
    let mut current = target;
    for segment in &segments[..segments.len() - 1] {
        if !current.is_object() {
            return Err(format!(
                "target path '{}' cannot create object segment '{}' under non-object value",
                pointer, segment
            ));
        }
        let object = current.as_object_mut().expect("object checked above");
        current = object
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if !current.is_object() {
        return Err(format!(
            "target path '{}' cannot set field under non-object value",
            pointer
        ));
    }
    current
        .as_object_mut()
        .expect("object checked above")
        .insert(segments.last().expect("non-empty segments").clone(), value);
    Ok(())
}

fn decode_json_pointer(pointer: &str) -> Result<Vec<String>, String> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    if !pointer.starts_with('/') {
        return Err(format!("must be an RFC 6901 JSON Pointer, got '{pointer}'"));
    }
    pointer
        .split('/')
        .skip(1)
        .map(|segment| decode_json_pointer_segment(segment, pointer))
        .collect()
}

fn decode_json_pointer_segment(segment: &str, pointer: &str) -> Result<String, String> {
    let mut decoded = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            Some(other) => {
                return Err(format!(
                    "contains invalid escape '~{other}' in JSON Pointer '{pointer}'"
                ));
            }
            None => {
                return Err(format!(
                    "contains trailing '~' escape in JSON Pointer '{pointer}'"
                ));
            }
        }
    }
    Ok(decoded)
}

fn workflow_parent_from_dependencies(
    node: &WorkflowRunNode,
    node_results: &HashMap<String, WorkflowRunNodeResult>,
) -> (Option<RunId>, Option<String>) {
    if node.depends_on.len() != 1 {
        return (None, None);
    }
    let Some(result) = node_results.get(&node.depends_on[0]) else {
        return (None, None);
    };
    if result.status != AgentRunStatus::Completed {
        return (None, None);
    }
    (result.run_id.clone(), Some(result.agent_id.clone()))
}

fn workflow_status(results: &[WorkflowRunNodeResult]) -> AgentRunStatus {
    if results
        .iter()
        .any(|result| workflow_node_failed(&result.status))
    {
        AgentRunStatus::Failed
    } else if results
        .iter()
        .any(|result| result.status == AgentRunStatus::Skipped)
    {
        AgentRunStatus::Skipped
    } else {
        AgentRunStatus::Completed
    }
}

fn workflow_needs_compensation(results: &[WorkflowRunNodeResult]) -> bool {
    results
        .iter()
        .any(|result| workflow_node_failed(&result.status))
}

fn workflow_node_failed(status: &AgentRunStatus) -> bool {
    matches!(
        status,
        AgentRunStatus::Failed
            | AgentRunStatus::TimedOut
            | AgentRunStatus::Cancelled
            | AgentRunStatus::Abandoned
    )
}

pub fn run_idempotency_key(agent_id: &str, scope: &RunScope, request: &RunRequest) -> String {
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
    });
    let bytes = serde_json::to_vec(&material).unwrap_or_else(|_| agent_id.as_bytes().to_vec());
    format!("idem_{}", blake3::hash(&bytes).to_hex())
}

fn spawn_lease_renewer(
    lock_store: Arc<dyn agent_core::AgentLockStore>,
    lease: RunLease,
    ttl: Duration,
    lease_kind: &'static str,
    subject_id: String,
) -> JoinHandle<()> {
    let interval_duration = lease_renewal_interval(ttl);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(interval_duration);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            match lock_store.renew(&lease, ttl).await {
                Ok(()) => {
                    debug!(
                        lease_kind,
                        subject_id = %subject_id,
                        lock_key = %lease.key,
                        renew_interval_ms = interval_duration.as_millis(),
                        lease_ttl_ms = ttl.as_millis(),
                        "lease renewed",
                    );
                }
                Err(error) => {
                    warn!(
                        lease_kind,
                        subject_id = %subject_id,
                        lock_key = %lease.key,
                        error = %error,
                        "failed to renew lease",
                    );
                }
            }
        }
    })
}

async fn stop_lease_renewer(handle: JoinHandle<()>) {
    handle.abort();
    let _ = handle.await;
}

fn lease_renewal_interval(ttl: Duration) -> Duration {
    let interval_ms = (ttl.as_millis() / 3).max(1).min(u64::MAX as u128) as u64;
    Duration::from_millis(interval_ms)
}

fn request_scope(request: &RunRequest) -> Result<RunScope, AgentError> {
    resolve_request_scope(&request.scope, request.user.as_ref(), "run")
}

fn workflow_request_scope(request: &WorkflowRunRequest) -> Result<RunScope, AgentError> {
    resolve_request_scope(&request.scope, request.user.as_ref(), "workflow")
}

fn resolve_request_scope(
    scope: &Option<RunScope>,
    user: Option<&agent_core::UserContext>,
    label: &str,
) -> Result<RunScope, AgentError> {
    match scope {
        Some(RunScope::Global) => Ok(RunScope::Global),
        Some(RunScope::User(user_id)) => {
            let user_id = user_id.trim();
            if user_id.is_empty() {
                Err(AgentError::validation(format!(
                    "{label} scope user id must not be empty"
                )))
            } else {
                Ok(RunScope::User(user_id.to_owned()))
            }
        }
        Some(RunScope::Tenant(tenant_id)) => {
            let tenant_id = tenant_id.trim();
            if tenant_id.is_empty() {
                Err(AgentError::validation(format!(
                    "{label} scope tenant id must not be empty"
                )))
            } else {
                Ok(RunScope::Tenant(tenant_id.to_owned()))
            }
        }
        None => match user {
            Some(user) => {
                let user_id = user.user_id.trim();
                if user_id.is_empty() {
                    Err(AgentError::validation(format!(
                        "{label} user_id must not be empty"
                    )))
                } else {
                    Ok(RunScope::User(user_id.to_owned()))
                }
            }
            None => Ok(RunScope::Global),
        },
    }
}

fn trigger_envelope_identity(envelope: &agent_core::TriggerEnvelope) -> Value {
    let payload_hash = if envelope.id.is_none() {
        serde_json::to_vec(&envelope.payload)
            .map(|bytes| format!("blake3:{}", blake3::hash(&bytes).to_hex()))
            .unwrap_or_else(|_| "blake3:unserializable".to_owned())
    } else {
        String::new()
    };

    json!({
        "source": envelope.source,
        "id": envelope.id,
        "payload_hash": if envelope.id.is_none() {
            Value::String(payload_hash)
        } else {
            Value::Null
        },
    })
}

fn insert_json_field<T: Serialize>(target: &mut Value, key: &str, value: &T) {
    let Some(object) = target.as_object_mut() else {
        return;
    };
    object.insert(
        key.to_owned(),
        serde_json::to_value(value).unwrap_or(Value::Null),
    );
}

fn run_trace_span(
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

fn trace_spans_from_events(run_span: TraceSpan, events: &[TraceEvent]) -> Vec<TraceSpan> {
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

fn artifact_refs_from_events(events: &[TraceEvent]) -> Vec<ArtifactRef> {
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

fn trace_usage_summary_from_events(events: &[TraceEvent]) -> Option<TraceUsageSummary> {
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

fn started_tool_events_by_id(events: &[TraceEvent]) -> HashMap<String, &TraceEvent> {
    events
        .iter()
        .filter(|event| event.kind == "tool_call_started")
        .filter_map(|event| {
            payload_str(&event.payload, "tool_call_id")
                .map(|tool_call_id| (tool_call_id.to_owned(), event))
        })
        .collect()
}

fn paired_tool_terminal_keys(events: &[TraceEvent]) -> HashSet<String> {
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

fn event_trace_span(
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

fn tool_event_trace_span(
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

fn state_event_trace_span(
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

fn llm_event_trace_span(
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

fn span_common_attributes(payload: &Value, status: &str) -> Map<String, Value> {
    let mut attributes = Map::new();
    copy_payload_field(&mut attributes, payload, "run_id");
    copy_payload_field(&mut attributes, payload, "agent_id");
    attributes.insert("status".to_owned(), json!(status));
    attributes
}

fn copy_payload_field(attributes: &mut Map<String, Value>, payload: &Value, key: &str) {
    if let Some(value) = payload.get(key) {
        attributes.insert(key.to_owned(), value.clone());
    }
}

fn copy_error_attributes(attributes: &mut Map<String, Value>, payload: &Value) {
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

fn tool_span_key(payload: &Value) -> Option<String> {
    let tool_name = payload_str(payload, "tool_name")?;
    let input_hash = payload_str(payload, "input_hash")?;
    Some(format!("{tool_name}\0{input_hash}"))
}

fn payload_str<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

fn payload_duration_ms(payload: &Value) -> Option<u64> {
    payload.get("duration_ms").and_then(Value::as_u64)
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

fn span_name_segment(value: &str) -> String {
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

fn subtract_duration_ms(finished_at: OffsetDateTime, duration_ms: u64) -> OffsetDateTime {
    let duration = TimeDuration::milliseconds(i64::try_from(duration_ms).unwrap_or(i64::MAX));
    finished_at.checked_sub(duration).unwrap_or(finished_at)
}

fn child_span_id(parent_span_id: &str, kind: &str, identity: &str, index: usize) -> String {
    let material = format!("{parent_span_id}:{kind}:{identity}:{index}");
    format!("span_{}", blake3::hash(material.as_bytes()).to_hex())
}

fn timestamp_duration_ms(started_at: OffsetDateTime, finished_at: OffsetDateTime) -> u64 {
    let duration_ms = (finished_at - started_at).whole_milliseconds();
    if duration_ms <= 0 {
        0
    } else {
        u64::try_from(duration_ms).unwrap_or(u64::MAX)
    }
}

fn run_status_name(status: &AgentRunStatus) -> &'static str {
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

fn result_is_retryable(result: &AgentRunResult) -> bool {
    if matches!(
        result.status,
        AgentRunStatus::Completed | AgentRunStatus::Skipped | AgentRunStatus::Cancelled
    ) {
        return false;
    }
    result.error.as_ref().is_some_and(|error| error.retryable)
}

fn spawn_persisted_cancellation_watcher(
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

async fn persisted_cancellation_requested(
    run_store: &dyn AgentRunStore,
    run_id: &RunId,
) -> Result<bool, AgentError> {
    Ok(run_store
        .get_run(run_id)
        .await
        .map_err(|error| AgentError::internal(error.to_string()))?
        .is_some_and(|run| run.cancellation_requested()))
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
        workflow: None,
    }
}
