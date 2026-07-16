use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use agent_core::{
    AgentRunRecord, AgentRunStatus, AgentRunStore, AgentTrace, AgentTraceStore, PROTOCOL_VERSION,
    RunId, RunScope,
};
use agent_store::SqliteStore;
use assert_cmd::Command;
use serde_json::Value;
use time::OffsetDateTime;

#[path = "catalog.rs"]
mod catalog;
#[path = "llm.rs"]
mod llm;
#[path = "support.rs"]
mod support;
#[path = "tools.rs"]
mod tools;
#[path = "validation.rs"]
mod validation;

#[path = "compat.rs"]
mod compat;
#[path = "config_profiles.rs"]
mod config_profiles;
#[path = "config_stores.rs"]
mod config_stores;
#[path = "debug_bundle.rs"]
mod debug_bundle;
#[path = "eval.rs"]
mod eval;
#[path = "metrics.rs"]
mod metrics;
#[path = "proposal_approvals.rs"]
mod proposal_approvals;
#[path = "proposal_lifecycle.rs"]
mod proposal_lifecycle;
#[path = "proposal_policy.rs"]
mod proposal_policy;
#[path = "recovery.rs"]
mod recovery;
#[path = "replay.rs"]
mod replay;
#[path = "run.rs"]
mod run;
#[path = "server_chat.rs"]
mod server_chat;
#[path = "server_core.rs"]
mod server_core;
#[path = "server_proposals.rs"]
mod server_proposals;
#[path = "server_runs.rs"]
mod server_runs;
#[path = "server_sessions.rs"]
mod server_sessions;
#[path = "telemetry.rs"]
mod telemetry;
#[path = "tui.rs"]
mod tui;
#[path = "workflow.rs"]
mod workflow;

use support::*;
