use std::collections::{BTreeMap, BTreeSet};

use agent_core::{
    AgentProposalStore, AgentRunRecord, AgentRunResult, AgentSessionStore, PROTOCOL_VERSION,
    ProposalEnvelope, RunId, RunRequest, SessionId, SessionRecord, StepRecord, ThreadId,
    ThreadRecord, TriggerKind, UserContext,
};
use agent_runtime::RUNTIME_VERSION;
use camino::{Utf8Path, Utf8PathBuf};
use miette::{IntoDiagnostic, Result, miette};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::catalog::{build_prompt_manifest, read_catalog, string_metadata};
use crate::config::RuntimeStoreBackend;
use crate::runtime_stores::RuntimeStores;

mod artifacts;
mod export;
mod io;
mod redaction;
mod trace_records;
mod types;

use artifacts::*;
pub(crate) use export::{export_debug_bundle, write_debug_bundle};
use io::*;
use redaction::*;
use trace_records::*;
pub(crate) use types::DebugBundleOptions;
use types::*;
