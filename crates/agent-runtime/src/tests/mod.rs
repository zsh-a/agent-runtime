use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use agent_core::{
    Agent, AgentContext, AgentError, AgentErrorKind, AgentErrorRecord, AgentEvent,
    AgentEventEmitter, AgentLockStore, AgentRunRecord, AgentRunResult, AgentRunStatus,
    AgentRunStore, AgentSpec, AgentStateAccess, ArtifactKind, ArtifactPublishRequest,
    ArtifactPublisher, ArtifactRef, ArtifactStoreRef, HookEffect, HookEventName, HookKind,
    HookSpec, PROTOCOL_VERSION, PolicyDecision, ProposalCreator, RedactionClassification, RunId,
    RunLease, RunRequest, RunScope, ScheduleSpec, StoreError, SubagentRequest, SubagentRunner,
    ToolCaller, ToolError, WorkflowInputMapping, WorkflowInputTransform, WorkflowRunNode,
    WorkflowRunNodeCompensation, WorkflowRunRequest,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::{Notify, broadcast};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

use super::*;

struct EchoAgent;

#[async_trait]
impl Agent for EchoAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "echo".to_owned(),
            name: "Echo".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.echo".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "echo",
            ctx.now,
            ctx.input,
            Some("echoed input".to_owned()),
        ))
    }
}

struct ParentAgent;

#[async_trait]
impl Agent for ParentAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "parent".to_owned(),
            name: "Parent".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.subagent".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let output = ctx
            .services
            .run_subagent(SubagentRequest {
                agent_id: "echo".to_owned(),
                input: json!({"from": "parent"}),
                run_id: None,
                scope: None,
                workflow: None,
                metadata: json!({}),
            })
            .await
            .map_err(|error| AgentError {
                record: error.record,
            })?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "parent",
            ctx.now,
            output,
            Some("parent delegated".to_owned()),
        ))
    }
}

struct ToolAgent;

#[async_trait]
impl Agent for ToolAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "tool_user".to_owned(),
            name: "Tool User".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.tool".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let output = ctx
            .services
            .call_tool("lookup", ctx.input.clone())
            .await
            .map_err(|error| AgentError {
                record: error.record,
            })?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "tool_user",
            ctx.now,
            json!({"tool_output": output}),
            Some("tool call completed".to_owned()),
        ))
    }
}

struct ArtifactAgent;

#[async_trait]
impl Agent for ArtifactAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "artifact_agent".to_owned(),
            name: "Artifact Agent".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.artifact".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let artifact = ctx
            .services
            .publish_artifact(ArtifactPublishRequest {
                artifact_id: Some("artifact_test_pdf".to_owned()),
                kind: Some(ArtifactKind::Document),
                uri: "artifact://test/report.pdf".to_owned(),
                media_type: Some("application/pdf".to_owned()),
                size_bytes: Some(1024),
                sha256: Some(
                    "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd".to_owned(),
                ),
                redaction_classification: Some(RedactionClassification::Confidential),
                metadata: json!({"title": "Report"}),
            })
            .await?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "artifact_agent",
            ctx.now,
            json!({"artifact_id": artifact.artifact_id}),
            Some("artifact published".to_owned()),
        ))
    }
}

struct UsageAgent;

#[async_trait]
impl Agent for UsageAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "usage_agent".to_owned(),
            name: "Usage Agent".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.usage".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        ctx.services
            .emit_event(AgentEvent {
                kind: "llm_response".to_owned(),
                occurred_at: ctx.now,
                payload: json!({
                    "provider": "openai",
                    "model": "gpt-test",
                    "duration_ms": 42,
                    "usage": {
                        "input_tokens": 11,
                        "output_tokens": 7,
                        "total_tokens": 18,
                        "cost_micros": 123,
                        "cost_currency": "USD"
                    }
                }),
            })
            .await?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "usage_agent",
            ctx.now,
            json!({"ok": true}),
            Some("usage emitted".to_owned()),
        ))
    }
}

mod hooks;
mod idempotency;
mod lifecycle;
mod locking;
mod observability;
mod recovery;
mod scheduler;
mod subagent;
mod workflow;

struct StateAgent;

#[async_trait]
impl Agent for StateAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "stateful".to_owned(),
            name: "Stateful".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.state".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        ctx.services
            .save_state("last_input", ctx.input.clone())
            .await?;
        let loaded = ctx.services.load_state("last_input").await?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "stateful",
            ctx.now,
            json!({"loaded": loaded}),
            Some("stateful run completed".to_owned()),
        ))
    }
}

struct FlakyAgent {
    attempts: Arc<AtomicUsize>,
}

#[async_trait]
impl Agent for FlakyAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "flaky".to_owned(),
            name: "Flaky".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.flaky".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt == 1 {
            return Err(AgentError {
                record: Box::new(AgentErrorRecord {
                    kind: AgentErrorKind::TransientExternalError,
                    code: "transient_test_error".to_owned(),
                    message: "transient failure".to_owned(),
                    retryable: true,
                    details: json!({"attempt": attempt}),
                }),
            });
        }
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "flaky",
            ctx.now,
            json!({"attempt": attempt}),
            Some("flaky run completed".to_owned()),
        ))
    }
}

struct BlockingAgent;

#[async_trait]
impl Agent for BlockingAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "blocking".to_owned(),
            name: "Blocking".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.blocking".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        ctx.trace
            .emit(agent_core::TraceEvent::new("blocking.started", json!({})))
            .await?;
        sleep(Duration::from_secs(60)).await;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "blocking",
            ctx.now,
            json!({}),
            Some("blocking completed".to_owned()),
        ))
    }
}

struct CountingAgent {
    executions: Arc<AtomicUsize>,
}

#[async_trait]
impl Agent for CountingAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "counting".to_owned(),
            name: "Counting".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: Vec::new(),
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "counting",
            ctx.now,
            json!({}),
            None,
        ))
    }
}

#[derive(Default)]
struct ConcurrencyCounters {
    current: AtomicUsize,
    max_seen: AtomicUsize,
    completed: AtomicUsize,
}

struct SlowAgent {
    id: String,
    counters: Arc<ConcurrencyCounters>,
    started: Option<Arc<Notify>>,
}

impl SlowAgent {
    fn new(id: impl Into<String>, counters: Arc<ConcurrencyCounters>) -> Self {
        Self {
            id: id.into(),
            counters,
            started: None,
        }
    }

    fn with_started_notify(
        id: impl Into<String>,
        counters: Arc<ConcurrencyCounters>,
        started: Arc<Notify>,
    ) -> Self {
        Self {
            id: id.into(),
            counters,
            started: Some(started),
        }
    }
}

#[async_trait]
impl Agent for SlowAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: self.id.clone(),
            name: "Slow".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.slow".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let current = self.counters.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.counters.max_seen.fetch_max(current, Ordering::SeqCst);
        if let Some(started) = &self.started {
            started.notify_one();
        }
        sleep(Duration::from_millis(100)).await;
        self.counters.current.fetch_sub(1, Ordering::SeqCst);
        self.counters.completed.fetch_add(1, Ordering::SeqCst);
        Ok(AgentRunResult::completed(
            ctx.run_id,
            self.id.clone(),
            ctx.now,
            ctx.input,
            Some("slow run completed".to_owned()),
        ))
    }
}

struct LeaseProbeAgent;

#[async_trait]
impl Agent for LeaseProbeAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "lease_probe".to_owned(),
            name: "Lease Probe".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.lease".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        sleep(Duration::from_millis(250)).await;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "lease_probe",
            ctx.now,
            ctx.input,
            Some("lease probe completed".to_owned()),
        ))
    }
}

fn run_request() -> RunRequest {
    RunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: None,
        input: json!({}),
        user: None,
        scope: None,
        trigger: agent_core::TriggerKind::Manual,
        trigger_envelope: None,
        workflow: None,
        metadata: json!({}),
    }
}

fn lease_probe_workflow_request() -> WorkflowRunRequest {
    WorkflowRunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        workflow_id: "workflow_lease_probe".to_owned(),
        root_run_id: None,
        user: None,
        scope: Some(RunScope::Tenant("tenant_lease".to_owned())),
        trigger: agent_core::TriggerKind::Manual,
        trigger_envelope: None,
        nodes: vec![WorkflowRunNode {
            node_id: "lease_probe_node".to_owned(),
            agent_id: "lease_probe".to_owned(),
            run_id: None,
            input: json!({"workflow": "lease_probe"}),
            input_mappings: vec![],
            depends_on: vec![],
            compensation: None,
            metadata: json!({}),
        }],
        metadata: json!({}),
    }
}

fn slow_workflow_request() -> WorkflowRunRequest {
    WorkflowRunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        workflow_id: "workflow_slow".to_owned(),
        root_run_id: None,
        user: None,
        scope: Some(RunScope::Tenant("tenant_slow".to_owned())),
        trigger: agent_core::TriggerKind::Manual,
        trigger_envelope: None,
        nodes: vec![WorkflowRunNode {
            node_id: "slow_node".to_owned(),
            agent_id: "slow".to_owned(),
            run_id: None,
            input: json!({"workflow": "slow"}),
            input_mappings: vec![],
            depends_on: vec![],
            compensation: None,
            metadata: json!({}),
        }],
        metadata: json!({}),
    }
}

fn scheduled_spec(schedule: ScheduleSpec) -> AgentSpec {
    AgentSpec {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        id: "scheduled".to_owned(),
        name: "Scheduled".to_owned(),
        description: None,
        version: "0.1.0".to_owned(),
        schedule,
        capabilities: vec!["debug.schedule".to_owned()],
        metadata: json!({}),
    }
}

fn parse_rfc3339(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).expect("valid RFC3339 test timestamp")
}

fn run_record_started_at(started_at: OffsetDateTime) -> AgentRunRecord {
    AgentRunRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        version: 1,
        run_id: RunId("run_schedule_test".to_owned()),
        idempotency_key: Some("idem_schedule_test".to_owned()),
        agent_id: "scheduled".to_owned(),
        status: AgentRunStatus::Completed,
        scope: RunScope::Global,
        started_at,
        finished_at: Some(started_at + time::Duration::seconds(1)),
        input: json!({}),
        output: json!({}),
        error: None,
        workflow: None,
        metadata: json!({}),
    }
}

fn hook_spec(name: &str, event: HookEventName, effect: HookEffect) -> HookSpec {
    HookSpec {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        name: name.to_owned(),
        event,
        kind: HookKind::NativeRust,
        effect,
        command: None,
        timeout_ms: None,
        enabled: true,
        metadata: json!({}),
    }
}

#[derive(Default)]
struct CountingLockStore {
    release_count: AtomicUsize,
    renew_count: AtomicUsize,
    renewed_keys: Mutex<Vec<String>>,
}

struct LosingLockStore;

#[async_trait]
impl AgentLockStore for LosingLockStore {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError> {
        let now = OffsetDateTime::now_utc();
        Ok(Some(RunLease {
            key: key.to_owned(),
            owner: owner.to_owned(),
            acquired_at: now,
            expires_at: now + time::Duration::try_from(ttl).unwrap_or(time::Duration::MAX),
        }))
    }

    async fn renew(&self, _lease: &RunLease, _ttl: Duration) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn release(&self, _lease: RunLease) -> Result<(), StoreError> {
        Ok(())
    }
}

impl CountingLockStore {
    fn renewed_keys(&self) -> Vec<String> {
        self.renewed_keys
            .lock()
            .expect("renewed keys lock is not poisoned")
            .clone()
    }
}

#[async_trait]
impl AgentLockStore for CountingLockStore {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError> {
        let now = OffsetDateTime::now_utc();
        Ok(Some(RunLease {
            key: key.to_owned(),
            owner: owner.to_owned(),
            acquired_at: now,
            expires_at: now + time::Duration::try_from(ttl).unwrap_or(time::Duration::MAX),
        }))
    }

    async fn renew(&self, lease: &RunLease, _ttl: Duration) -> Result<bool, StoreError> {
        self.renew_count.fetch_add(1, Ordering::SeqCst);
        self.renewed_keys
            .lock()
            .expect("renewed keys lock is not poisoned")
            .push(lease.key.clone());
        Ok(true)
    }

    async fn release(&self, _lease: RunLease) -> Result<(), StoreError> {
        self.release_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct FailingUpdateRunStore;

#[async_trait]
impl AgentRunStore for FailingUpdateRunStore {
    async fn create_run(&self, _run: AgentRunRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn update_run(
        &self,
        _run: AgentRunRecord,
        _expected_version: u64,
    ) -> Result<bool, StoreError> {
        Err(StoreError::new("forced update failure"))
    }

    async fn get_run(&self, _run_id: &RunId) -> Result<Option<AgentRunRecord>, StoreError> {
        Ok(None)
    }

    async fn find_run_by_idempotency_key(
        &self,
        _agent_id: &str,
        _scope: &RunScope,
        _idempotency_key: &str,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        Ok(None)
    }

    async fn list_runs(
        &self,
        _agent_id: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError> {
        Ok(Vec::new())
    }

    async fn last_run(
        &self,
        _agent_id: &str,
        _scope: &RunScope,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        Ok(None)
    }
}

struct AllowHook;

struct FailingHook;

#[async_trait]
impl crate::hooks::HookHandler for FailingHook {
    async fn handle(&self, _invocation: crate::hooks::HookInvocation) -> Result<Value, AgentError> {
        Err(AgentError::internal("policy backend unavailable"))
    }
}

#[async_trait]
impl crate::hooks::HookHandler for AllowHook {
    async fn handle(&self, invocation: crate::hooks::HookInvocation) -> Result<Value, AgentError> {
        Ok(json!({
            "event": invocation.event,
            "input": invocation.input,
        }))
    }
}

struct DenyHook;

#[async_trait]
impl crate::hooks::HookHandler for DenyHook {
    async fn handle(&self, _invocation: crate::hooks::HookInvocation) -> Result<Value, AgentError> {
        serde_json::to_value(PolicyDecision::deny("state writes disabled for test"))
            .map_err(|error| AgentError::internal(error.to_string()))
    }
}

struct NoopServices {
    state_store: Arc<dyn agent_core::AgentStateStore>,
}

#[async_trait]
impl ToolCaller for NoopServices {
    async fn call_tool(&self, _name: &str, _input: Value) -> Result<Value, ToolError> {
        Ok(json!({}))
    }
}

#[async_trait]
impl AgentEventEmitter for NoopServices {
    async fn emit_event(&self, _event: AgentEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait]
impl AgentStateAccess for NoopServices {
    async fn load_state(&self, key: &str) -> Result<Option<Value>, AgentError> {
        self.state_store
            .load("echo", &RunScope::Global, key)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }

    async fn save_state(&self, key: &str, value: Value) -> Result<(), AgentError> {
        self.state_store
            .save("echo", &RunScope::Global, key, value)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }
}

#[async_trait]
impl ProposalCreator for NoopServices {}

#[async_trait]
impl SubagentRunner for NoopServices {}

#[async_trait]
impl ArtifactPublisher for NoopServices {
    async fn publish_artifact(
        &self,
        request: ArtifactPublishRequest,
    ) -> Result<ArtifactRef, AgentError> {
        Ok(ArtifactRef {
            artifact_id: request
                .artifact_id
                .unwrap_or_else(|| "artifact_test_generated".to_owned()),
            kind: request.kind.unwrap_or(ArtifactKind::Blob),
            uri: request.uri,
            media_type: request.media_type,
            size_bytes: request.size_bytes,
            sha256: request.sha256,
            redaction_classification: request
                .redaction_classification
                .unwrap_or(RedactionClassification::Internal),
            store: Some(ArtifactStoreRef {
                provider: "test_artifact_store".to_owned(),
                bucket: Some("test-bucket".to_owned()),
                key: Some("report.pdf".to_owned()),
                version: Some("v1".to_owned()),
                metadata: json!({}),
            }),
            metadata: request.metadata,
        })
    }
}
