use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    Agent, AgentError, AgentRunRecord, AgentRunStatus, AgentSpec, AgentTrace, ProposalEnvelope,
    ProposalId, RunId, RunLease, RunScope, SessionId, SessionRecord, StepRecord, StoreError,
    ThreadId, ThreadRecord, TraceEvent, WorkflowRunResult,
};

#[async_trait]
pub trait AgentRegistry: Send + Sync {
    async fn list_agents(&self) -> Result<Vec<AgentSpec>, AgentError>;
    async fn get_agent(&self, id: &str) -> Result<Option<Arc<dyn Agent>>, AgentError>;
}

#[async_trait]
pub trait AgentRunStore: Send + Sync {
    async fn create_run(&self, run: AgentRunRecord) -> Result<(), StoreError>;
    /// Atomically update a run when its stored version matches
    /// `expected_version`. The supplied record must use the next version.
    /// Returns `false` when another writer won the version race.
    async fn update_run(
        &self,
        run: AgentRunRecord,
        expected_version: u64,
    ) -> Result<bool, StoreError>;
    async fn get_run(&self, run_id: &RunId) -> Result<Option<AgentRunRecord>, StoreError>;
    async fn find_run_by_idempotency_key(
        &self,
        agent_id: &str,
        scope: &RunScope,
        idempotency_key: &str,
    ) -> Result<Option<AgentRunRecord>, StoreError>;
    async fn list_runs(
        &self,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError>;
    async fn list_runs_by_status(
        &self,
        status: AgentRunStatus,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError> {
        let runs = self.list_runs(None, None).await?;
        let mut filtered = runs
            .into_iter()
            .filter(|run| run.status == status)
            .collect::<Vec<_>>();
        if let Some(limit) = limit {
            filtered.truncate(limit);
        }
        Ok(filtered)
    }
    async fn last_run(
        &self,
        agent_id: &str,
        scope: &RunScope,
    ) -> Result<Option<AgentRunRecord>, StoreError>;
}

pub type RunEventCursor = u64;

#[derive(Debug, Clone)]
pub struct RunEventRecord {
    pub cursor: RunEventCursor,
    pub event: TraceEvent,
}

#[async_trait]
pub trait AgentRunEventStore: Send + Sync {
    async fn append_run_event(&self, run_id: &RunId, event: TraceEvent) -> Result<(), StoreError>;
    async fn replace_run_events(
        &self,
        run_id: &RunId,
        events: Vec<TraceEvent>,
    ) -> Result<(), StoreError>;
    async fn list_run_events_after(
        &self,
        run_id: &RunId,
        after: RunEventCursor,
    ) -> Result<Option<Vec<RunEventRecord>>, StoreError>;
}

#[async_trait]
pub trait AgentTraceStore: Send + Sync {
    async fn write_trace(&self, trace: AgentTrace) -> Result<(), StoreError>;
    async fn read_trace(&self, run_id: &RunId) -> Result<Option<AgentTrace>, StoreError>;
    async fn write_workflow_traces(&self, result: &WorkflowRunResult) -> Result<usize, StoreError> {
        let mut written = 0;
        for node in &result.nodes {
            if let Some(trace) = node.trace.as_ref() {
                self.write_trace(trace.clone()).await?;
                written += 1;
            }
            if let Some(trace) = node
                .compensation
                .as_ref()
                .and_then(|compensation| compensation.trace.as_ref())
            {
                self.write_trace(trace.clone()).await?;
                written += 1;
            }
        }
        Ok(written)
    }
}

#[async_trait]
pub trait AgentLockStore: Send + Sync {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError>;
    async fn renew(&self, lease: &RunLease, ttl: Duration) -> Result<bool, StoreError>;
    async fn release(&self, lease: RunLease) -> Result<(), StoreError>;
}

#[async_trait]
pub trait AgentStateStore: Send + Sync {
    async fn load(
        &self,
        agent_id: &str,
        scope: &RunScope,
        key: &str,
    ) -> Result<Option<Value>, StoreError>;
    async fn save(
        &self,
        agent_id: &str,
        scope: &RunScope,
        key: &str,
        value: Value,
    ) -> Result<(), StoreError>;
}

#[async_trait]
pub trait AgentSessionStore: Send + Sync {
    async fn create_session(&self, session: SessionRecord) -> Result<(), StoreError>;
    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, StoreError>;
    async fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionRecord>, StoreError>;
    async fn create_thread(&self, thread: ThreadRecord) -> Result<(), StoreError>;
    async fn list_threads(&self, session_id: &SessionId) -> Result<Vec<ThreadRecord>, StoreError>;
    async fn get_thread(&self, thread_id: &ThreadId) -> Result<Option<ThreadRecord>, StoreError>;
    async fn create_step(&self, step: StepRecord) -> Result<(), StoreError>;
    async fn list_steps(&self, thread_id: &ThreadId) -> Result<Vec<StepRecord>, StoreError>;
}

#[async_trait]
pub trait AgentProposalStore: Send + Sync {
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), StoreError>;
    async fn update_proposal(
        &self,
        proposal: ProposalEnvelope,
        expected_version: u64,
    ) -> Result<bool, StoreError>;
    async fn get_proposal(
        &self,
        proposal_id: &ProposalId,
    ) -> Result<Option<ProposalEnvelope>, StoreError>;
    async fn list_proposals(
        &self,
        run_id: Option<&RunId>,
    ) -> Result<Vec<ProposalEnvelope>, StoreError>;
}
