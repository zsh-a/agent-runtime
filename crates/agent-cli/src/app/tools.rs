use std::sync::Arc;

use agent_core::{
    AgentError, AgentEvent, AgentEventEmitter, AgentProposalStore, AgentServices,
    AgentServicesFactory, AgentStateAccess, AgentStateStore, ArtifactPublisher, ExecutionContext,
    ProposalCreator, ProposalEnvelope, RunScope, SubagentRunner, ToolCaller, ToolError,
};
use agent_store::InMemoryStateStore;
use async_trait::async_trait;
use camino::Utf8PathBuf;
use miette::Result;
use serde_json::Value;

pub(crate) use agent_tools::{
    ToolOverrides, builtin_tools, load_tool_source_specs, load_tool_sources, source_has_tool,
    tool_overrides,
};

#[derive(Debug, Clone, Default)]
pub(crate) struct ToolSelection {
    pub(crate) host: Vec<String>,
    pub(crate) mocks: Vec<String>,
    pub(crate) sources: Vec<Utf8PathBuf>,
}

impl ToolSelection {
    pub(crate) async fn load(self) -> Result<ToolOverrides> {
        tool_overrides(self.host, self.mocks, self.sources).await
    }
}

pub(crate) struct CliServices {
    state: Arc<dyn AgentStateStore>,
    pub(crate) tools: ToolOverrides,
    proposal_store: Option<Arc<dyn AgentProposalStore>>,
}

struct BoundCliServices {
    context: ExecutionContext,
    state: Arc<dyn AgentStateStore>,
    tools: ToolOverrides,
    proposal_store: Option<Arc<dyn AgentProposalStore>>,
}

impl Default for CliServices {
    fn default() -> Self {
        Self::new(ToolOverrides::default())
    }
}

impl AgentServicesFactory for CliServices {
    fn bind(&self, context: ExecutionContext) -> Arc<dyn AgentServices> {
        Arc::new(BoundCliServices {
            context,
            state: self.state.clone(),
            tools: self.tools.clone(),
            proposal_store: self.proposal_store.clone(),
        })
    }
}

impl CliServices {
    pub(crate) fn new(tools: ToolOverrides) -> Self {
        Self {
            state: Arc::new(InMemoryStateStore::default()),
            tools,
            proposal_store: None,
        }
    }

    pub(crate) fn with_stores(
        tools: ToolOverrides,
        state: Arc<dyn AgentStateStore>,
        proposal_store: Arc<dyn AgentProposalStore>,
    ) -> Self {
        Self {
            state,
            tools,
            proposal_store: Some(proposal_store),
        }
    }
}

#[async_trait]
impl ToolCaller for CliServices {
    async fn call_tool(&self, name: &str, input: Value) -> std::result::Result<Value, ToolError> {
        self.tools.call_tool(name, input).await
    }
}

#[async_trait]
impl AgentEventEmitter for CliServices {
    async fn emit_event(&self, _event: AgentEvent) -> std::result::Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait]
impl AgentStateAccess for CliServices {
    async fn load_state(&self, key: &str) -> std::result::Result<Option<Value>, AgentError> {
        self.state
            .load("cli", &RunScope::Global, key)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }

    async fn save_state(&self, key: &str, value: Value) -> std::result::Result<(), AgentError> {
        self.state
            .save("cli", &RunScope::Global, key, value)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }
}

#[async_trait]
impl ProposalCreator for CliServices {
    async fn create_proposal(
        &self,
        proposal: ProposalEnvelope,
    ) -> std::result::Result<(), AgentError> {
        let Some(store) = &self.proposal_store else {
            return Err(AgentError::validation(
                "proposal creation requires a configured proposal store",
            ));
        };
        store
            .create_proposal(proposal)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }
}

#[async_trait]
impl SubagentRunner for CliServices {}

#[async_trait]
impl ArtifactPublisher for CliServices {}

#[async_trait]
impl ToolCaller for BoundCliServices {
    async fn call_tool(&self, name: &str, input: Value) -> std::result::Result<Value, ToolError> {
        self.tools.call_tool(name, input).await
    }
}

#[async_trait]
impl AgentEventEmitter for BoundCliServices {
    async fn emit_event(&self, _event: AgentEvent) -> std::result::Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait]
impl AgentStateAccess for BoundCliServices {
    async fn load_state(&self, key: &str) -> std::result::Result<Option<Value>, AgentError> {
        self.state
            .load(&self.context.agent_id, &self.context.scope, key)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }

    async fn save_state(&self, key: &str, value: Value) -> std::result::Result<(), AgentError> {
        self.state
            .save(&self.context.agent_id, &self.context.scope, key, value)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }
}

#[async_trait]
impl ProposalCreator for BoundCliServices {
    async fn create_proposal(
        &self,
        proposal: ProposalEnvelope,
    ) -> std::result::Result<(), AgentError> {
        let store = self.proposal_store.as_ref().ok_or_else(|| {
            AgentError::validation("proposal creation requires a configured proposal store")
        })?;
        store
            .create_proposal(proposal)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }
}

#[async_trait]
impl SubagentRunner for BoundCliServices {}

#[async_trait]
impl ArtifactPublisher for BoundCliServices {}
