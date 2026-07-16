use std::{process::Stdio, sync::Arc};

use agent_core::{
    AgentRunRecord, HookEvent, HookEventName, HookInvocationStatus, HookKind, PROTOCOL_VERSION,
    PromptManifest, ProposalEnvelope, ProposalStatus, RunId, RunRequest, TriggerKind,
};
use agent_runtime::{AgentRunner, RunOutcome};
use camino::{Utf8Path, Utf8PathBuf};
use miette::{IntoDiagnostic, Result, miette};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;

use crate::catalog::{build_prompt_manifest, read_catalog, registry_from_catalog};
use crate::config::RuntimeStoreBackend;
use crate::runtime_stores::RuntimeStores;
use crate::tools::{CliServices, ToolOverrides};

mod expectations;
mod io;
mod runner;
mod scoring;
mod trace;
mod types;

use expectations::*;
use io::*;
pub(crate) use runner::{create_eval_from_run, run_eval_path};
pub(crate) use scoring::run_dev_score_hook;
use scoring::*;
use trace::*;
use types::*;
