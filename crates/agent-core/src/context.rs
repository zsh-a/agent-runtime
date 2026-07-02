use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{PROTOCOL_VERSION, protocol_version};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextBlockKind {
    RuntimeInstructions,
    AgentInstructions,
    CommandInstructions,
    Memory,
    CompactionSummary,
    Message,
    ToolSchema,
    Resource,
    Metadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextBlock {
    pub block_id: String,
    pub kind: ContextBlockKind,
    pub source: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub token_estimate: u32,
    pub content_hash: String,
    #[serde(default)]
    pub content: Value,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextSnapshot {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub snapshot_id: String,
    pub content_hash: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default)]
    pub token_estimate: u32,
    #[serde(default)]
    pub max_input_tokens: u32,
    #[serde(default)]
    pub omitted_block_count: u32,
    #[serde(default)]
    pub compacted: bool,
    #[serde(default)]
    pub blocks: Vec<ContextBlock>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextPolicy {
    pub max_input_tokens: u32,
    pub reserve_output_tokens: u32,
    pub preserve_recent_messages: usize,
    pub compact_when_over_budget: bool,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            max_input_tokens: 128_000,
            reserve_output_tokens: 4_096,
            preserve_recent_messages: 12,
            compact_when_over_budget: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompactionRecord {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub before_snapshot_hash: String,
    pub after_snapshot_hash: String,
    pub omitted_block_count: u32,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub metadata: Value,
}

impl ContextSnapshot {
    pub fn new(
        snapshot_id: String,
        content_hash: String,
        token_estimate: u32,
        max_input_tokens: u32,
        omitted_block_count: u32,
        compacted: bool,
        blocks: Vec<ContextBlock>,
        metadata: Value,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            snapshot_id,
            content_hash,
            created_at: OffsetDateTime::now_utc(),
            token_estimate,
            max_input_tokens,
            omitted_block_count,
            compacted,
            blocks,
            metadata,
        }
    }
}
