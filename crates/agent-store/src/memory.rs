use std::{collections::HashMap, sync::Arc};

use agent_core::{
    AgentProposalStore, AgentRunRecord, AgentRunStore, AgentSessionStore, AgentStateStore,
    ProposalEnvelope, ProposalId, RunId, RunScope, SessionId, SessionRecord, StepRecord,
    StoreError, ThreadId, ThreadRecord,
};
use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::util::{same_scope, sort_and_limit_runs};

#[derive(Default)]
pub struct InMemoryRunStore {
    runs: RwLock<HashMap<String, AgentRunRecord>>,
}

impl InMemoryRunStore {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl AgentRunStore for InMemoryRunStore {
    async fn create_run(&self, run: AgentRunRecord) -> Result<(), StoreError> {
        let mut runs = self.runs.write().await;
        if runs.contains_key(&run.run_id.0) {
            return Err(StoreError::new("run already exists"));
        }
        if let Some(key) = run.idempotency_key.as_deref()
            && runs.values().any(|existing| {
                existing.agent_id == run.agent_id
                    && same_scope(&existing.scope, &run.scope)
                    && existing.idempotency_key.as_deref() == Some(key)
            })
        {
            return Err(StoreError::new("run idempotency key already exists"));
        }
        runs.insert(run.run_id.0.clone(), run);
        Ok(())
    }

    async fn update_run(&self, run: AgentRunRecord) -> Result<(), StoreError> {
        let mut runs = self.runs.write().await;
        if !runs.contains_key(&run.run_id.0) {
            return Err(StoreError::new("run does not exist"));
        }
        runs.insert(run.run_id.0.clone(), run);
        Ok(())
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<AgentRunRecord>, StoreError> {
        Ok(self.runs.read().await.get(&run_id.0).cloned())
    }

    async fn find_run_by_idempotency_key(
        &self,
        agent_id: &str,
        scope: &RunScope,
        idempotency_key: &str,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        Ok(self
            .runs
            .read()
            .await
            .values()
            .find(|run| {
                run.agent_id == agent_id
                    && same_scope(&run.scope, scope)
                    && run.idempotency_key.as_deref() == Some(idempotency_key)
            })
            .cloned())
    }

    async fn list_runs(
        &self,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError> {
        let mut runs = self
            .runs
            .read()
            .await
            .values()
            .filter(|run| agent_id.is_none_or(|agent_id| run.agent_id == agent_id))
            .cloned()
            .collect::<Vec<_>>();
        sort_and_limit_runs(&mut runs, limit);
        Ok(runs)
    }

    async fn last_run(
        &self,
        agent_id: &str,
        scope: &RunScope,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        let mut runs = self
            .runs
            .read()
            .await
            .values()
            .filter(|run| run.agent_id == agent_id && same_scope(&run.scope, scope))
            .cloned()
            .collect::<Vec<_>>();
        runs.sort_by_key(|run| run.started_at);
        Ok(runs.pop())
    }
}

#[derive(Default)]
pub struct InMemoryStateStore {
    values: RwLock<HashMap<(String, RunScope, String), serde_json::Value>>,
}

impl InMemoryStateStore {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl AgentStateStore for InMemoryStateStore {
    async fn load(
        &self,
        agent_id: &str,
        scope: &RunScope,
        key: &str,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        Ok(self
            .values
            .read()
            .await
            .get(&(agent_id.to_owned(), scope.clone(), key.to_owned()))
            .cloned())
    }

    async fn save(
        &self,
        agent_id: &str,
        scope: &RunScope,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), StoreError> {
        self.values
            .write()
            .await
            .insert((agent_id.to_owned(), scope.clone(), key.to_owned()), value);
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryProposalStore {
    proposals: RwLock<HashMap<String, ProposalEnvelope>>,
}

impl InMemoryProposalStore {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl AgentProposalStore for InMemoryProposalStore {
    async fn create_proposal(&self, proposal: ProposalEnvelope) -> Result<(), StoreError> {
        let mut proposals = self.proposals.write().await;
        if proposals.contains_key(&proposal.proposal_id.0) {
            return Err(StoreError::new("proposal already exists"));
        }
        proposals.insert(proposal.proposal_id.0.clone(), proposal);
        Ok(())
    }

    async fn update_proposal(
        &self,
        proposal: ProposalEnvelope,
        expected_version: u64,
    ) -> Result<bool, StoreError> {
        let mut proposals = self.proposals.write().await;
        let Some(current) = proposals.get(&proposal.proposal_id.0) else {
            return Ok(false);
        };
        if current.version != expected_version || proposal.version != expected_version + 1 {
            return Ok(false);
        }
        proposals.insert(proposal.proposal_id.0.clone(), proposal);
        Ok(true)
    }

    async fn get_proposal(
        &self,
        proposal_id: &ProposalId,
    ) -> Result<Option<ProposalEnvelope>, StoreError> {
        Ok(self.proposals.read().await.get(&proposal_id.0).cloned())
    }

    async fn list_proposals(
        &self,
        run_id: Option<&RunId>,
    ) -> Result<Vec<ProposalEnvelope>, StoreError> {
        let mut proposals = self
            .proposals
            .read()
            .await
            .values()
            .filter(|proposal| match run_id {
                Some(run_id) => proposal.run_id == *run_id,
                None => true,
            })
            .cloned()
            .collect::<Vec<_>>();
        proposals.sort_by_key(|proposal| proposal.created_at);
        Ok(proposals)
    }
}

#[derive(Default)]
pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<String, SessionRecord>>,
    threads: RwLock<HashMap<String, ThreadRecord>>,
    steps: RwLock<HashMap<String, StepRecord>>,
}

impl InMemorySessionStore {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl AgentSessionStore for InMemorySessionStore {
    async fn create_session(&self, session: SessionRecord) -> Result<(), StoreError> {
        self.sessions
            .write()
            .await
            .insert(session.session_id.0.clone(), session);
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let mut sessions = self
            .sessions
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.updated_at);
        sessions.reverse();
        Ok(sessions)
    }

    async fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self.sessions.read().await.get(&session_id.0).cloned())
    }

    async fn create_thread(&self, thread: ThreadRecord) -> Result<(), StoreError> {
        self.threads
            .write()
            .await
            .insert(thread.thread_id.0.clone(), thread);
        Ok(())
    }

    async fn list_threads(&self, session_id: &SessionId) -> Result<Vec<ThreadRecord>, StoreError> {
        let mut threads = self
            .threads
            .read()
            .await
            .values()
            .filter(|thread| thread.session_id == *session_id)
            .cloned()
            .collect::<Vec<_>>();
        threads.sort_by_key(|thread| thread.created_at);
        Ok(threads)
    }

    async fn get_thread(&self, thread_id: &ThreadId) -> Result<Option<ThreadRecord>, StoreError> {
        Ok(self.threads.read().await.get(&thread_id.0).cloned())
    }

    async fn create_step(&self, step: StepRecord) -> Result<(), StoreError> {
        self.steps
            .write()
            .await
            .insert(step.step_id.0.clone(), step);
        Ok(())
    }

    async fn list_steps(&self, thread_id: &ThreadId) -> Result<Vec<StepRecord>, StoreError> {
        let mut steps = self
            .steps
            .read()
            .await
            .values()
            .filter(|step| step.thread_id == *thread_id)
            .cloned()
            .collect::<Vec<_>>();
        steps.sort_by_key(|step| step.created_at);
        Ok(steps)
    }
}
