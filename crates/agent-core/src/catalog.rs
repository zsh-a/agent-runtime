use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{
    AgentSpec, ProposalKindSpec, ToolReplayPolicy, protocol_version, validate_catalog_version,
    validate_protocol_version,
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    pub risk: ToolRisk,
    #[serde(default)]
    pub replay_policy: ToolReplayPolicy,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    ReadOnly,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentRuntimeCatalog {
    pub protocol_version: String,
    pub catalog_version: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    #[serde(default)]
    pub active_domains: Vec<String>,
    #[serde(default)]
    pub agents: Vec<AgentSpec>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub proposal_kinds: Vec<ProposalKindSpec>,
    #[serde(default)]
    pub prompt_blocks: Vec<PromptBlockSpec>,
}

impl AgentRuntimeCatalog {
    pub fn validate_versions(&self) -> Result<(), String> {
        validate_protocol_version(&self.protocol_version)?;
        validate_catalog_version(&self.catalog_version)?;
        for agent in &self.agents {
            validate_protocol_version(&agent.protocol_version)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PromptBlockSpec {
    pub index: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PromptManifest {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub id: String,
    pub version: String,
    pub agent_id: String,
    pub agent_version: String,
    pub catalog_version: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    pub model_family: String,
    pub provider: String,
    pub model: String,
    pub tool_schema_version: String,
    #[serde(default)]
    pub active_domains: Vec<String>,
    #[serde(default)]
    pub blocks: Vec<PromptManifestBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PromptManifestBlock {
    pub index: u32,
    pub source: String,
    pub content_hash: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_requires_explicit_protocol_versions() {
        let error = serde_json::from_value::<AgentRuntimeCatalog>(json!({
            "catalog_version": "agent_catalog.v1",
            "generated_at": "2026-01-01T00:00:00Z",
            "active_domains": [],
            "agents": [],
            "tools": [],
            "proposal_kinds": [],
            "prompt_blocks": []
        }))
        .expect_err("missing protocol version is rejected");
        assert!(error.to_string().contains("protocol_version"));
    }
}
