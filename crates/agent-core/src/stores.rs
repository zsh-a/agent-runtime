use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    Agent, AgentError, AgentRunRecord, AgentSpec, ProposalEnvelope, ProposalId, RunId, RunLease,
    RunScope, SessionId, SessionRecord, StepRecord, StoreError, ThreadId, ThreadRecord,
};

#[async_trait]
pub trait AgentRegistry: Send + Sync {
    async fn list_agents(&self) -> Result<Vec<AgentSpec>, AgentError>;
    async fn get_agent(&self, id: &str) -> Result<Option<Arc<dyn Agent>>, AgentError>;
}

#[async_trait]
pub trait AgentRunStore: Send + Sync {
    async fn create_run(&self, run: AgentRunRecord) -> Result<(), StoreError>;
    async fn update_run(&self, run: AgentRunRecord) -> Result<(), StoreError>;
    async fn get_run(&self, run_id: &RunId) -> Result<Option<AgentRunRecord>, StoreError>;
    async fn list_runs(
        &self,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError>;
    async fn last_run(
        &self,
        agent_id: &str,
        scope: &RunScope,
    ) -> Result<Option<AgentRunRecord>, StoreError>;
}

#[async_trait]
pub trait AgentLockStore: Send + Sync {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError>;
    async fn renew(&self, lease: &RunLease, ttl: Duration) -> Result<(), StoreError>;
    async fn release(&self, lease: RunLease) -> Result<(), StoreError>;
}

#[async_trait]
pub trait AgentStateStore: Send + Sync {
    async fn load(&self, agent_id: &str, key: &str) -> Result<Option<Value>, StoreError>;
    async fn save(&self, agent_id: &str, key: &str, value: Value) -> Result<(), StoreError>;
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
    async fn update_proposal(&self, proposal: ProposalEnvelope) -> Result<(), StoreError>;
    async fn get_proposal(
        &self,
        proposal_id: &ProposalId,
    ) -> Result<Option<ProposalEnvelope>, StoreError>;
    async fn list_proposals(
        &self,
        run_id: Option<&RunId>,
    ) -> Result<Vec<ProposalEnvelope>, StoreError>;
}
