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
    Profile,
    Memory,
    CompactionSummary,
    Message,
    ToolSchema,
    Resource,
    Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceAuthority {
    UserConfirmed,
    SourceFact,
    DeterministicDerived,
    ModelDerived,
    LegacyUnknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm_version: Option<String>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub observed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextEvidence {
    pub authority: EvidenceAuthority,
    #[serde(default)]
    pub provenance: EvidenceProvenance,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub valid_from: Option<OffsetDateTime>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub valid_until: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
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
    /// Optional host-side hash. Runtime context planners must recompute this
    /// from [content] before using the block in a persisted snapshot; the
    /// default keeps host adapters from having to implement the runtime's hash
    /// algorithm.
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub content: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<ContextEvidence>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

pub struct ContextSnapshotInput {
    pub snapshot_id: String,
    pub content_hash: String,
    pub token_estimate: u32,
    pub max_input_tokens: u32,
    pub omitted_block_count: u32,
    pub compacted: bool,
    pub blocks: Vec<ContextBlock>,
    pub metadata: Value,
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
    pub fn new(input: ContextSnapshotInput) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            snapshot_id: input.snapshot_id,
            content_hash: input.content_hash,
            created_at: OffsetDateTime::now_utc(),
            token_estimate: input.token_estimate,
            max_input_tokens: input.max_input_tokens,
            omitted_block_count: input.omitted_block_count,
            compacted: input.compacted,
            blocks: input.blocks,
            metadata: input.metadata,
        }
    }
}
