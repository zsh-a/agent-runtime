use agent_core::AgentLockStore;
#[cfg(feature = "sqlite")]
use agent_core::{
    AgentRunEventStore, AgentRunRecord, AgentRunStatus, AgentRunStore, AgentStateStore, AgentTrace,
    AgentTraceStore, PROTOCOL_VERSION, RunId, RunScope, TraceEvent,
};
use camino::Utf8PathBuf;
#[cfg(feature = "sqlite")]
use serde_json::json;
use std::time::Duration;
#[cfg(feature = "sqlite")]
use time::OffsetDateTime;

use super::*;
use crate::testkit::{
    assert_lock_store_conformance, assert_proposal_store_conformance,
    assert_run_event_store_conformance, assert_run_store_conformance,
    assert_session_store_conformance, assert_state_store_conformance,
    assert_trace_store_conformance,
};

mod concurrency;
mod conformance;
mod locks;
mod migrations;
mod persistence;
mod support;

use support::*;
