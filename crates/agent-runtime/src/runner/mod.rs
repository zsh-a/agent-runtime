use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use agent_core::{
    Agent, AgentContext, AgentError, AgentRegistry, AgentRunRecord, AgentRunResult, AgentRunStatus,
    AgentRunStore, AgentServices, AgentServicesFactory, AgentSpec, AgentTrace, ExecutionContext,
    HookEventName, PROTOCOL_VERSION, RunCompensation, RunId, RunLease, RunRequest, RunScope,
    RunWorkflow, StaticAgentServicesFactory, TraceEvent, TraceSink, TraceUsageSummary,
    WorkflowRunNode, WorkflowRunNodeCompensationResult, WorkflowRunNodeResult, WorkflowRunRequest,
    WorkflowRunResult,
};
use serde::Serialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::sync::{Semaphore, broadcast};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::{
    InMemoryLockStore, RUNTIME_VERSION,
    cancellation::agent_cancellation,
    execution_support::*,
    hooks::HookManager,
    lock::{lock_key, workflow_lock_key},
    observability::*,
    policy::ExecutionPolicy,
    recovery::{RecoveryReport, recover_stale_runs},
    scheduler::AgentScheduler,
    services::TracedAgentServices,
    trace::{MemoryTraceSink, TraceEventBuffer},
    workflow::*,
};

mod control;
mod idempotency;

use control::*;
use idempotency::deduplicated_outcome;
pub use idempotency::run_idempotency_key;

pub struct RunOutcome {
    pub result: AgentRunResult,
    pub trace: AgentTrace,
    pub disposition: RunDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunDisposition {
    Executed,
    Deduplicated,
}

impl RunOutcome {
    pub fn should_persist_trace(&self) -> bool {
        self.disposition == RunDisposition::Executed
    }
}

#[derive(Clone)]
pub struct RunControl {
    pub cancellation: CancellationToken,
    pub trace_events: Option<broadcast::Sender<TraceEvent>>,
    pub trace_event_buffer: Option<Arc<TraceEventBuffer>>,
}

impl Default for RunControl {
    fn default() -> Self {
        Self {
            cancellation: CancellationToken::new(),
            trace_events: None,
            trace_event_buffer: None,
        }
    }
}

#[derive(Clone)]
struct RunExecution {
    agent: Arc<dyn Agent>,
    spec: AgentSpec,
    run_id: RunId,
    started_at: OffsetDateTime,
    request: RunRequest,
    scope: RunScope,
    trace: Arc<MemoryTraceSink>,
    cancellation: CancellationToken,
}

pub struct AgentRunner {
    registry: Arc<dyn AgentRegistry>,
    run_store: Arc<dyn AgentRunStore>,
    services: Arc<dyn AgentServicesFactory>,
    scheduler: AgentScheduler,
    policy: ExecutionPolicy,
    concurrency: Arc<Semaphore>,
    subagent_concurrency: Arc<Semaphore>,
    is_nested: bool,
    lock_store: Arc<dyn agent_core::AgentLockStore>,
    hooks: HookManager,
}

impl AgentRunner {
    pub fn new(
        registry: Arc<dyn AgentRegistry>,
        run_store: Arc<dyn AgentRunStore>,
        services: Arc<dyn AgentServices>,
    ) -> Self {
        Self::new_with_factory(
            registry,
            run_store,
            Arc::new(StaticAgentServicesFactory::new(services)),
        )
    }

    pub fn new_with_factory(
        registry: Arc<dyn AgentRegistry>,
        run_store: Arc<dyn AgentRunStore>,
        services: Arc<dyn AgentServicesFactory>,
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
            subagent_concurrency: Arc::new(Semaphore::new(
                ExecutionPolicy::default().max_concurrent_runs,
            )),
            is_nested: false,
            lock_store: Arc::new(InMemoryLockStore::default()),
            hooks: HookManager::default(),
        }
    }

    pub fn with_policy(mut self, policy: ExecutionPolicy) -> Self {
        self.concurrency = Arc::new(Semaphore::new(policy.max_concurrent_runs.max(1)));
        self.subagent_concurrency = Arc::new(Semaphore::new(policy.max_concurrent_runs.max(1)));
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
            subagent_concurrency: self.subagent_concurrency.clone(),
            is_nested: true,
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
            subagent_concurrency: self.subagent_concurrency.clone(),
            is_nested: self.is_nested,
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
        agent_core::validate_protocol_version(&request.protocol_version)
            .map_err(AgentError::validation)?;
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

        let lease_cancellation = CancellationToken::new();
        let lease_renewer = spawn_lease_renewer(
            self.lock_store.clone(),
            lease.clone(),
            self.policy.lease_ttl(),
            "workflow",
            workflow_id.clone(),
            Some(lease_cancellation.clone()),
        );
        let leased_workflow = tokio::select! {
            result = self.run_workflow_locked(request, started_at, order) => result,
            _ = lease_cancellation.cancelled() => {
                Err(AgentError::cancelled("workflow lease ownership was lost"))
            }
        };
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
                    error: Some(*error.record),
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
                error: Some(*error.record),
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
                error: Some(*error.record),
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
                .run_with_retries(RunExecution {
                    agent,
                    spec: spec.clone(),
                    run_id: run_id.clone(),
                    started_at,
                    request: request.clone(),
                    scope: scope.clone(),
                    trace: trace.clone(),
                    cancellation: control.cancellation.clone(),
                })
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

    async fn run_with_retries(
        &self,
        execution: RunExecution,
    ) -> Result<AgentRunResult, AgentError> {
        let step_execution = execution.clone();
        let RunExecution {
            agent: _,
            spec,
            run_id,
            started_at,
            request,
            scope: _,
            trace,
            cancellation,
        } = execution;
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

                self.execute_agent_step(&step_execution, attempt, attempt_timer)
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
        execution: &RunExecution,
        attempt: u32,
        attempt_timer: std::time::Instant,
    ) -> Result<AgentRunResult, AgentError> {
        let RunExecution {
            agent,
            spec,
            run_id,
            started_at,
            request,
            scope,
            trace,
            cancellation,
        } = execution;
        let ctx = AgentContext {
            run_id: run_id.clone(),
            now: *started_at,
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
                    *started_at,
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
                    failure_result(run_id.clone(), &spec.id, *started_at, err)
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
                        *started_at,
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
