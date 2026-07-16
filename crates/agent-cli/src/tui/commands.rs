use agent_core::{
    AgentRunRecord, AgentRunStatus, AgentTrace, ApprovalDecisionKind, ApprovalLevel,
    ProposalEnvelope, ProposalId, ProposalStatus, RunId, TraceEvent,
    WorkflowRunNodeCompensationResult, WorkflowRunNodeResult, WorkflowRunRequest,
    WorkflowRunResult,
};
use camino::Utf8PathBuf;
use miette::{IntoDiagnostic, Result, miette};
use serde_json::{Value, json};

use crate::proposal::{
    ProposalDecisionInput, append_proposal_decision_trace_event, decide_proposal_with_store,
};
use crate::runtime_stores::RuntimeStores;
use crate::trace_store::read_json as read_json_file;

use super::{
    approval::{
        approve_pending_tool_with_display, call_tool_or_request_approval,
        deny_pending_tool_with_display,
    },
    chat::run_natural_language_command,
    data::{
        TuiActivityItem, TuiActivityKind, TuiDetailKind, TuiFocusPanel, TuiProposalListSummary,
        TuiProposalSummary, TuiRunSummary, TuiState, TuiTraceEventItem, TuiTraceEventSummary,
        TuiWorkflowCompensationSummary, TuiWorkflowNodeSummary, TuiWorkflowSummary, read_trace,
    },
    format::{compact_json, pretty_json},
    runtime::{TuiCancelRunResult, TuiRuntime},
    tool_inventory::{TuiToolInventory, load_tui_tool_inventory},
};

mod actions;
mod arguments;
mod dispatch;
mod presentation;

use actions::*;
use arguments::*;
pub(super) use dispatch::execute_command;
use presentation::*;

#[cfg(test)]
#[path = "tests/commands.rs"]
mod tests;
