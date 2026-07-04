use std::{collections::BTreeSet, sync::Arc};

use agent_core::{
    Agent, AgentRuntimeCatalog, AgentSpec, PROTOCOL_VERSION, PromptBlockSpec, ProposalKindSpec,
    ToolSpec, catalog_version,
};
use agent_llm::{LlmMessage, LlmRole};
use agent_runtime::InMemoryAgentRegistry;
use camino::{Utf8Path, Utf8PathBuf};
use miette::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tracing::warn;

use crate::{
    catalog::{agents_from_catalog, read_catalog},
    registry::load_registry,
    tools::{ToolOverrides, builtin_tools},
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RuntimeSources {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) registry: Option<Utf8PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) catalog: Option<Utf8PathBuf>,
}

impl RuntimeSources {
    pub(crate) fn new(registry: Option<Utf8PathBuf>, catalog: Option<Utf8PathBuf>) -> Self {
        Self { registry, catalog }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.registry.is_none() && self.catalog.is_none()
    }

    pub(crate) fn merge(&mut self, overlay: Self) {
        if overlay.registry.is_some() {
            self.registry = overlay.registry;
        }
        if overlay.catalog.is_some() {
            self.catalog = overlay.catalog;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ResolvedRuntimeSources {
    pub(crate) registry: Utf8PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) catalog: Option<Utf8PathBuf>,
}

impl ResolvedRuntimeSources {
    pub(crate) fn new(registry: Utf8PathBuf, catalog: Option<Utf8PathBuf>) -> Self {
        Self { registry, catalog }
    }

    pub(crate) fn from_sources(sources: RuntimeSources, default_registry: &str) -> Self {
        Self::new(
            sources
                .registry
                .unwrap_or_else(|| Utf8PathBuf::from(default_registry)),
            sources.catalog,
        )
    }

    pub(crate) fn catalog_path(&self) -> Option<&Utf8Path> {
        self.catalog.as_deref()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeSourceOptions {
    pub(crate) sources: ResolvedRuntimeSources,
    pub(crate) tool_overrides: ToolOverrides,
}

#[derive(Clone)]
pub(crate) struct RuntimeComposition {
    pub(crate) registry: Arc<InMemoryAgentRegistry>,
    pub(crate) agent_specs: Vec<AgentSpec>,
    pub(crate) catalog: Option<AgentRuntimeCatalog>,
    pub(crate) catalog_view: AgentRuntimeCatalog,
    pub(crate) tool_specs: Vec<ToolSpec>,
}

impl RuntimeComposition {
    pub(crate) fn chat_messages(
        &self,
        agent_id: &str,
        messages: Vec<LlmMessage>,
    ) -> Vec<LlmMessage> {
        if messages.first().is_some_and(|message| {
            message.role == LlmRole::System
                && message.metadata.get("source").and_then(Value::as_str) == Some("runtime_config")
        }) {
            return messages;
        }
        let Some(system_prompt) = self.system_prompt(agent_id) else {
            return messages;
        };
        let mut request_messages = Vec::with_capacity(messages.len() + 1);
        request_messages.push(LlmMessage {
            role: LlmRole::System,
            content: Value::String(system_prompt),
            name: None,
            metadata: json!({"source": "runtime_config"}),
        });
        request_messages.extend(messages);
        request_messages
    }

    pub(crate) fn system_prompt(&self, agent_id: &str) -> Option<String> {
        let catalog = self.catalog.as_ref()?;
        let agent = self
            .agent_specs
            .iter()
            .find(|agent| agent.id == agent_id)
            .or_else(|| catalog.agents.iter().find(|agent| agent.id == agent_id))?;

        let mut sections = vec![format!("You are {}.", agent.name)];
        if let Some(description) = agent
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
        {
            sections.push(description.to_owned());
        }
        let mut prompt_blocks = catalog.prompt_blocks.clone();
        prompt_blocks.sort_by_key(|block| block.index);
        for block in prompt_blocks {
            let text = block.text.trim();
            if !text.is_empty() {
                sections.push(text.to_owned());
            }
        }
        if !self.tool_specs.is_empty() {
            sections.push(
                "Use the provided tools when they are necessary. Keep normal replies concise and direct."
                    .to_owned(),
            );
        }
        Some(sections.join("\n\n"))
    }
}

pub(crate) async fn compose_runtime_sources(
    options: RuntimeSourceOptions,
) -> Result<RuntimeComposition> {
    let catalog = match options.sources.catalog.clone() {
        Some(path) => Some(read_catalog(path).await?),
        None => None,
    };

    let mut agents = Vec::<Arc<dyn Agent>>::new();
    let mut agent_specs = Vec::<AgentSpec>::new();
    let mut seen_agents = BTreeSet::<String>::new();
    match load_registry(options.sources.registry.clone()).await {
        Ok(registry_config) => {
            for agent in registry_config.into_agents() {
                let spec = agent.spec();
                if seen_agents.insert(spec.id.clone()) {
                    agent_specs.push(spec);
                    agents.push(agent);
                }
            }
        }
        Err(error) if catalog.is_some() => {
            warn!(
                registry = %options.sources.registry,
                error = %error,
                "skipping unavailable registry because a catalog source is configured",
            );
        }
        Err(error) => return Err(error),
    }
    if let Some(catalog) = &catalog {
        for agent in agents_from_catalog(catalog) {
            let spec = agent.spec();
            if seen_agents.insert(spec.id.clone()) {
                agent_specs.push(spec);
                agents.push(agent);
            }
        }
    }

    let tool_specs = runtime_tool_specs(catalog.as_ref(), &options.tool_overrides);
    let registry = InMemoryAgentRegistry::shared(agents);
    let catalog_view =
        runtime_catalog_view(catalog.as_ref(), agent_specs.clone(), tool_specs.clone());

    Ok(RuntimeComposition {
        registry,
        agent_specs,
        catalog,
        catalog_view,
        tool_specs,
    })
}

pub(crate) fn runtime_tool_specs(
    catalog: Option<&AgentRuntimeCatalog>,
    tool_overrides: &ToolOverrides,
) -> Vec<ToolSpec> {
    let mut tools = catalog
        .map(|catalog| catalog.tools.clone())
        .unwrap_or_default();
    tools.extend(tool_overrides.source_specs.clone());
    for tool in builtin_tools() {
        if !tools.iter().any(|existing| existing.name == tool.name) {
            tools.push(tool);
        }
    }
    tools
}

fn runtime_catalog_view(
    catalog: Option<&AgentRuntimeCatalog>,
    agents: Vec<AgentSpec>,
    tools: Vec<ToolSpec>,
) -> AgentRuntimeCatalog {
    let proposal_kinds = catalog
        .map(|catalog| catalog.proposal_kinds.clone())
        .unwrap_or_else(Vec::<ProposalKindSpec>::new);
    let prompt_blocks = catalog
        .map(|catalog| catalog.prompt_blocks.clone())
        .unwrap_or_else(Vec::<PromptBlockSpec>::new);
    AgentRuntimeCatalog {
        protocol_version: catalog
            .map(|catalog| catalog.protocol_version.clone())
            .unwrap_or_else(|| PROTOCOL_VERSION.to_owned()),
        catalog_version: catalog
            .map(|catalog| catalog.catalog_version.clone())
            .unwrap_or_else(catalog_version),
        generated_at: catalog
            .map(|catalog| catalog.generated_at)
            .unwrap_or_else(OffsetDateTime::now_utc),
        active_domains: catalog
            .map(|catalog| catalog.active_domains.clone())
            .unwrap_or_default(),
        agents,
        tools,
        proposal_kinds,
        prompt_blocks,
    }
}

pub(crate) fn tool_source_label(tool: &ToolSpec) -> String {
    if let Some(source) = tool
        .metadata
        .get("source")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        return source.to_owned();
    }
    match tool.name.as_str() {
        "echo" => "agent_cli_builtin".to_owned(),
        _ => "catalog".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_llm::user_message;

    #[tokio::test]
    async fn composition_merges_registry_and_catalog_agents() {
        let composition = compose_runtime_sources(RuntimeSourceOptions {
            sources: ResolvedRuntimeSources::new(
                Utf8PathBuf::from("../../examples/agents.yaml"),
                Some(Utf8PathBuf::from(
                    "../../fixtures/contracts/catalog.valid.json",
                )),
            ),
            tool_overrides: ToolOverrides::default(),
        })
        .await
        .expect("composition loads");

        let ids = composition
            .agent_specs
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"echo_agent"));
        assert!(ids.contains(&"ai_chat"));
    }

    #[tokio::test]
    async fn composition_injects_catalog_prompt_for_chat_frontends() {
        let composition = compose_runtime_sources(RuntimeSourceOptions {
            sources: ResolvedRuntimeSources::new(
                Utf8PathBuf::from("../../examples/agents.yaml"),
                Some(Utf8PathBuf::from(
                    "../../fixtures/contracts/catalog.valid.json",
                )),
            ),
            tool_overrides: ToolOverrides::default(),
        })
        .await
        .expect("composition loads");

        let messages = composition.chat_messages("ai_chat", vec![user_message("hello")]);

        assert_eq!(messages[0].role, LlmRole::System);
        assert!(
            messages[0]
                .content
                .as_str()
                .expect("system prompt is a string")
                .contains("default interactive agent in the Agent Runtime TUI")
        );
    }
}
