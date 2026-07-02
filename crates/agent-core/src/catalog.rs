use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{AgentSpec, ProposalKindSpec, catalog_version, protocol_version};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    pub risk: ToolRisk,
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
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    #[serde(default = "catalog_version")]
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
