mod error;
mod http;
mod manifest;
mod mcp;
mod process;

use std::{collections::BTreeMap, sync::Arc};

use agent_core::{
    AgentError, AgentProposalStore, AgentServices, AgentStateStore, ProposalEnvelope, ToolError,
    ToolRisk, ToolSpec,
};
use agent_store::{FileProposalStore, InMemoryStateStore};
use async_trait::async_trait;
use camino::Utf8PathBuf;
use miette::{Result, miette};
use serde_json::{Value, json};

use manifest::{ToolSourceRuntime, load_tool_sources as read_tool_sources};
pub(crate) use manifest::{load_tool_source_specs, load_tool_sources, source_has_tool};
use process::{ProcessToolHost, process_tool_host};

#[derive(Debug, Clone, Default)]
pub(crate) struct ToolOverrides {
    pub(crate) mock_tools: BTreeMap<String, Value>,
    pub(crate) source_specs: Vec<ToolSpec>,
    pub(crate) source_tools: BTreeMap<String, ToolSourceRuntime>,
    pub(crate) tool_host: Option<ProcessToolHost>,
}

#[derive(Default)]
pub(crate) struct CliServices {
    state: InMemoryStateStore,
    pub(crate) tools: ToolOverrides,
    proposal_store: Option<Arc<FileProposalStore>>,
}

impl CliServices {
    pub(crate) fn new(tools: ToolOverrides) -> Self {
        Self {
            state: InMemoryStateStore::default(),
            tools,
            proposal_store: None,
        }
    }

    pub(crate) fn with_proposal_store(
        tools: ToolOverrides,
        proposal_store: Arc<FileProposalStore>,
    ) -> Self {
        Self {
            state: InMemoryStateStore::default(),
            tools,
            proposal_store: Some(proposal_store),
        }
    }
}

#[async_trait]
impl AgentServices for CliServices {
    async fn call_tool(&self, name: &str, input: Value) -> std::result::Result<Value, ToolError> {
        if let Some(output) = self.tools.mock_tools.get(name) {
            return Ok(output.clone());
        }
        if let Some(host) = self.tools.source_tools.get(name) {
            return host.call(name, input).await;
        }
        if let Some(host) = &self.tools.tool_host {
            return host.call(name, input).await;
        }
        match name {
            "echo" => Ok(json!({"echo": input})),
            _ => Err(error::tool_error(
                "unknown_tool",
                format!("unknown tool '{name}'"),
            )),
        }
    }

    async fn emit_event(
        &self,
        _event: agent_core::AgentEvent,
    ) -> std::result::Result<(), AgentError> {
        Ok(())
    }

    async fn load_state(&self, key: &str) -> std::result::Result<Option<Value>, AgentError> {
        self.state
            .load("cli", key)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }

    async fn save_state(&self, key: &str, value: Value) -> std::result::Result<(), AgentError> {
        self.state
            .save("cli", key, value)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }

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

pub(crate) async fn tool_overrides(
    tool_host: Vec<String>,
    mock_tool: Vec<String>,
    tool_source: Vec<Utf8PathBuf>,
) -> Result<ToolOverrides> {
    let mut mock_tools = BTreeMap::new();
    for spec in mock_tool {
        let (name, raw_value) = spec
            .split_once('=')
            .ok_or_else(|| miette!("mock tool must use NAME=JSON or NAME=@PATH: {spec}"))?;
        let name = name.trim();
        if name.is_empty() {
            return Err(miette!("mock tool name cannot be empty"));
        }
        let value = if let Some(path) = raw_value.strip_prefix('@') {
            if path.is_empty() {
                return Err(miette!("mock tool path cannot be empty for '{name}'"));
            }
            read_json_file(Utf8PathBuf::from(path)).await?
        } else {
            serde_json::from_str(raw_value)
                .map_err(|e| miette!("failed to parse mock tool '{name}' JSON: {e}"))?
        };
        mock_tools.insert(name.to_owned(), value);
    }

    let mut source_tools = BTreeMap::new();
    let mut source_specs = Vec::new();
    for source in read_tool_sources(tool_source).await? {
        let runtime = ToolSourceRuntime::from_source(&source)?;
        for tool in source.tools {
            if source_tools
                .insert(tool.name.clone(), runtime.clone())
                .is_some()
            {
                return Err(miette!("duplicate tool-source tool '{}'", tool.name));
            }
            source_specs.push(tool);
        }
    }

    Ok(ToolOverrides {
        mock_tools,
        source_specs,
        source_tools,
        tool_host: process_tool_host(tool_host)?,
    })
}

#[allow(dead_code)]
pub(crate) fn builtin_tools() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "echo".to_owned(),
        description: "Return the input unchanged inside an echo envelope.".to_owned(),
        input_schema: json!({"type": "object"}),
        output_schema: Some(json!({"type": "object"})),
        risk: ToolRisk::ReadOnly,
        metadata: json!({}),
    }]
}

async fn read_json_file(path: Utf8PathBuf) -> Result<Value> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read JSON at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse JSON at {path}: {e}"))
}
