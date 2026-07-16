use std::{
    collections::{HashSet, VecDeque},
    convert::Infallible,
    net::SocketAddr,
    time::Duration,
};

use agent_chat::{ChatError, ChatResumeRequest, ChatTurnEvent, ChatTurnEventKind, ChatTurnRequest};
use agent_core::{
    AgentRunStatus, ProposalId, RunEventCursor, RunEventRecord, RunId, SessionId, ToolSpec,
    TraceEvent, WorkflowRunRequest,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use futures::{StreamExt, stream};
use miette::{IntoDiagnostic, Result, miette};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use crate::{
    catalog::CatalogSummary,
    metrics::event_records_from_trace,
    proposal::PolicyDeniedError,
    runtime_server::{
        AgentRunParams, HttpAgentRunParams, HttpProposalCreateParams, HttpProposalDecisionParams,
        HttpProposalListParams, HttpRunListParams, HttpToolCallParams, RuntimeServer,
    },
    session::{HttpSessionCreateParams, HttpThreadForkParams},
    stdio_protocol::{StdioRequest, StdioResponse, stdio_error, stdio_result},
};

mod chat;
mod core;
mod proposals;
mod runs;
mod sessions;
mod stdio;
mod support;
mod tools;

use chat::*;
use core::*;
use proposals::*;
use runs::*;
use sessions::*;
use stdio::*;
use support::*;
use tools::*;

#[derive(Debug, Serialize)]
struct HttpErrorBody {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
struct RunEventsQuery {
    after: Option<String>,
    follow: Option<bool>,
}

struct EnumeratedTraceEvents {
    all_events: Vec<TraceEvent>,
    records: Vec<RunEventRecord>,
    next_cursor: RunEventCursor,
}

pub(crate) async fn serve_http(server: RuntimeServer, host: String, port: u16) -> Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| miette!("invalid listen address {host}:{port}: {e}"))?;
    if !addr.ip().is_loopback() {
        return Err(miette!(
            "the embedded runtime server only binds loopback addresses; expose it through an authenticated host gateway"
        ));
    }
    let app = Router::new()
        .route("/healthz", get(http_healthz))
        .route("/catalog/summary", get(http_catalog_summary))
        .route("/metrics/summary", get(http_metrics_summary))
        .route("/chat/turn", post(http_chat_turn))
        .route("/chat/resume", post(http_chat_resume))
        .route("/workflows/run", post(http_workflow_run))
        .route("/agents/{agent_id}/run", post(http_agent_run))
        .route("/runs", get(http_runs))
        .route("/runs/{run_id}", get(http_run_inspect))
        .route("/runs/{run_id}/trace", get(http_run_trace))
        .route("/runs/{run_id}/events", get(http_run_events))
        .route("/runs/{run_id}/cancel", post(http_run_cancel))
        .route("/runs/{run_id}/replay", post(http_run_replay))
        .route("/tools", get(http_tools))
        .route("/tools/{tool_name}/call", post(http_tool_call))
        .route("/proposals", get(http_proposals).post(http_proposal_create))
        .route("/proposals/{proposal_id}", get(http_proposal_inspect))
        .route(
            "/proposals/{proposal_id}/decision",
            post(http_proposal_decide),
        )
        .route("/proposals/{proposal_id}/apply", post(http_proposal_apply))
        .route("/proposals/{proposal_id}/undo", post(http_proposal_undo))
        .route("/sessions", get(http_sessions).post(http_session_create))
        .route("/sessions/{session_id}", get(http_session_show))
        .route("/sessions/{session_id}/fork", post(http_session_fork))
        .with_state(server);
    let listener = TcpListener::bind(addr).await.into_diagnostic()?;
    info!(addr = %addr, "HTTP server listening");
    eprintln!("agent serve listening on http://{addr}");
    axum::serve(listener, app).await.into_diagnostic()
}

pub(crate) async fn serve_stdio(server: RuntimeServer) -> Result<()> {
    info!("stdio server listening");
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await.into_diagnostic()? {
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_stdio_line(&server, &line).await;
        let encoded = serde_json::to_vec(&response).into_diagnostic()?;
        stdout.write_all(&encoded).await.into_diagnostic()?;
        stdout.write_all(b"\n").await.into_diagnostic()?;
        stdout.flush().await.into_diagnostic()?;
    }
    Ok(())
}
