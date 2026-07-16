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

mod attempts;
mod control;
mod execution;
mod idempotency;
mod scheduling;
mod workflow;

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
