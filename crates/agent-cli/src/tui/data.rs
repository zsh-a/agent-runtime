use std::{collections::VecDeque, time::Instant};

use agent_chat::{ChatToolCall, ChatTurnState};
use agent_core::{AgentRunRecord, AgentSpec, ContextPolicy, HookSpec};
use agent_llm::LlmMessage;
use camino::{Utf8Path, Utf8PathBuf};
use miette::{IntoDiagnostic, Result, miette};
use serde_json::Value;

use crate::{
    catalog::{CatalogSummary, read_catalog},
    chat::ChatLlmOptions,
    config::RuntimeStoreBackend,
    runtime_config::{ResolvedRuntimeSources, RuntimeSourceOptions, compose_runtime_sources},
    runtime_stores::RuntimeStores,
    tools::ToolOverrides,
};

use super::{
    policy::TuiToolRisk,
    tool_inventory::{TuiToolInventory, load_tui_tool_inventory},
};

const MAX_EVENT_LINES: usize = 160;
const MAX_HISTORY_ITEMS: usize = 80;
const SCROLL_LINES: u16 = 4;

mod loading;
mod models;
mod state;

pub(super) use loading::*;
pub(crate) use models::TuiOptions;
pub(super) use models::{
    TranscriptItem, TranscriptRole, TuiActivityItem, TuiActivityKind, TuiAgentSummary,
    TuiApprovalSelection, TuiCompletionItem, TuiCompletionMenu, TuiContextStatus, TuiDetailKind,
    TuiFocusPanel, TuiPaneSizing, TuiPendingApproval, TuiPendingApprovalAction,
    TuiProposalListSummary, TuiProposalSummary, TuiRunSummary, TuiSelectionPoint, TuiState,
    TuiTextSelection, TuiTraceEventItem, TuiTraceEventSummary, TuiUpdate,
    TuiWorkflowCompensationSummary, TuiWorkflowNodeSummary, TuiWorkflowSummary,
};

#[cfg(test)]
#[path = "tests/data.rs"]
mod tests;
