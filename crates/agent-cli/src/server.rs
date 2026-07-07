use std::{collections::HashSet, convert::Infallible, net::SocketAddr};

use agent_chat::{ChatError, ChatResumeRequest, ChatTurnEvent, ChatTurnEventKind, ChatTurnRequest};
use agent_core::{ProposalId, RunId, SessionId, ToolSpec, TraceEvent, WorkflowRunRequest};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use futures::{StreamExt, stream};
use miette::{IntoDiagnostic, Result, miette};
use serde::Serialize;
use serde::de::DeserializeOwned;
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

#[derive(Debug, Serialize)]
struct HttpErrorBody {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

pub(crate) async fn serve_http(server: RuntimeServer, host: String, port: u16) -> Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| miette!("invalid listen address {host}:{port}: {e}"))?;
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

async fn http_healthz() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn http_catalog_summary(State(server): State<RuntimeServer>) -> Json<CatalogSummary> {
    Json(CatalogSummary::from_catalog(&server.catalog))
}

async fn http_tools(State(server): State<RuntimeServer>) -> Json<Vec<ToolSpec>> {
    Json(server.catalog.tools.clone())
}

async fn http_metrics_summary(State(server): State<RuntimeServer>) -> Response {
    match server.metrics_summary().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metrics_summary_failed",
            err,
        ),
    }
}

async fn http_chat_turn(State(server): State<RuntimeServer>, body: Bytes) -> Response {
    let request = match decode_schema_json::<ChatTurnRequest>(
        &body,
        include_str!("../../../schemas/chat-turn-request.schema.json"),
        "chat-turn-request",
    ) {
        Ok(request) => request,
        Err(response) => return response,
    };

    match server.stream_chat_turn(request).await {
        Ok(stream) => chat_sse_response(stream),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "chat_turn_failed", err),
    }
}

async fn http_chat_resume(State(server): State<RuntimeServer>, body: Bytes) -> Response {
    let request = match decode_schema_json::<ChatResumeRequest>(
        &body,
        include_str!("../../../schemas/chat-resume-request.schema.json"),
        "chat-resume-request",
    ) {
        Ok(request) => request,
        Err(response) => return response,
    };

    match server.stream_chat_resume(request).await {
        Ok(stream) => chat_sse_response(stream),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "chat_resume_failed", err),
    }
}

fn chat_sse_response(stream: agent_chat::ChatEventStream) -> Response {
    let stream = stream.map(|event| {
        let event = match event {
            Ok(event) => event,
            Err(error) => chat_error_event(error),
        };
        let data = serde_json::to_string(&event).unwrap_or_else(|err| {
            json!({
                "kind": "error",
                "content": format!("failed to encode chat event: {err}"),
                "round": event.round,
                "metadata": {}
            })
            .to_string()
        });
        Ok::<_, Infallible>(Event::default().event("chat_turn_event").data(data))
    });
    Sse::new(stream).into_response()
}

async fn http_agent_run(
    State(server): State<RuntimeServer>,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Response {
    let params = match decode_schema_json::<HttpAgentRunParams>(
        &body,
        include_str!("../../../schemas/http-agent-run-request.schema.json"),
        "http-agent-run-request",
    ) {
        Ok(params) => params,
        Err(response) => return response,
    };

    match server.run_agent(agent_id, params).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "agent_run_failed", err),
    }
}

async fn http_workflow_run(State(server): State<RuntimeServer>, body: Bytes) -> Response {
    let request = match decode_schema_json::<WorkflowRunRequest>(
        &body,
        include_str!("../../../schemas/workflow-run-request.schema.json"),
        "workflow-run-request",
    ) {
        Ok(request) => request,
        Err(response) => return response,
    };

    match server.run_workflow(request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "workflow_run_failed",
            err,
        ),
    }
}

async fn http_runs(
    State(server): State<RuntimeServer>,
    Query(params): Query<HttpRunListParams>,
) -> Response {
    match server.list_runs(params.agent_id, params.limit).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "run_list_failed", err),
    }
}

async fn http_run_inspect(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
) -> Response {
    match server.get_run(RunId(run_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "run_not_found", err),
    }
}

async fn http_run_trace(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
) -> Response {
    match server.get_run_trace(RunId(run_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "trace_not_found", err),
    }
}

async fn http_run_events(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
) -> Response {
    let run_id = RunId(run_id);
    if let Some(active_events) = server.subscribe_run_events(&run_id).await {
        let seen = active_events
            .replayed_events
            .iter()
            .filter_map(trace_event_dedupe_key)
            .collect::<HashSet<_>>();
        let replay_stream = stream::iter(
            active_events
                .replayed_events
                .into_iter()
                .map(|event| Ok::<_, Infallible>(trace_event_sse(event))),
        );
        let live_stream = stream::unfold(
            (active_events.receiver, seen),
            |(mut receiver, mut seen)| async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            if let Some(key) = trace_event_dedupe_key(&event)
                                && !seen.insert(key)
                            {
                                continue;
                            }
                            return Some((
                                Ok::<_, Infallible>(trace_event_sse(event)),
                                (receiver, seen),
                            ));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        );
        return Sse::new(replay_stream.chain(live_stream)).into_response();
    }

    match server.get_run_events(run_id.clone()).await {
        Ok(Some(events)) => {
            let stream = stream::iter(
                events
                    .into_iter()
                    .map(|event| Ok::<_, Infallible>(trace_event_sse(event))),
            );
            return Sse::new(stream).into_response();
        }
        Ok(None) => {}
        Err(err) => {
            return http_error(StatusCode::INTERNAL_SERVER_ERROR, "run_events_failed", err);
        }
    }

    match server.get_run_trace(run_id).await {
        Ok(trace) => {
            let events = event_records_from_trace(&trace);
            let stream = stream::iter(events.into_iter().map(|event| {
                let kind = event
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("trace_event")
                    .to_owned();
                let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_owned());
                Ok::<_, Infallible>(Event::default().event(kind).data(data))
            }));
            Sse::new(stream).into_response()
        }
        Err(err) => http_error(StatusCode::NOT_FOUND, "trace_not_found", err),
    }
}

async fn http_run_cancel(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
) -> Response {
    match server.cancel_run(RunId(run_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "run_not_found", err),
    }
}

async fn http_run_replay(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
) -> Response {
    match server.replay_run(RunId(run_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "run_replay_failed", err),
    }
}

async fn http_tool_call(
    State(server): State<RuntimeServer>,
    Path(tool_name): Path<String>,
    Json(params): Json<HttpToolCallParams>,
) -> Response {
    match server.call_tool(tool_name, params.input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "tool_call_failed", err),
    }
}

async fn http_proposal_create(
    State(server): State<RuntimeServer>,
    Json(params): Json<HttpProposalCreateParams>,
) -> Response {
    match server.create_proposal(params).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_report_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_create_failed",
            err,
        ),
    }
}

async fn http_proposals(
    State(server): State<RuntimeServer>,
    Query(params): Query<HttpProposalListParams>,
) -> Response {
    match server.list_proposals(params.run_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_list_failed",
            err,
        ),
    }
}

async fn http_proposal_inspect(
    State(server): State<RuntimeServer>,
    Path(proposal_id): Path<String>,
) -> Response {
    match server.get_proposal(ProposalId(proposal_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "proposal_not_found", err),
    }
}

async fn http_proposal_decide(
    State(server): State<RuntimeServer>,
    Path(proposal_id): Path<String>,
    Json(params): Json<HttpProposalDecisionParams>,
) -> Response {
    match server
        .decide_proposal(ProposalId(proposal_id), params)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_report_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_decide_failed",
            err,
        ),
    }
}

async fn http_proposal_apply(
    State(server): State<RuntimeServer>,
    Path(proposal_id): Path<String>,
) -> Response {
    match server.apply_proposal(ProposalId(proposal_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_report_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_apply_failed",
            err,
        ),
    }
}

async fn http_proposal_undo(
    State(server): State<RuntimeServer>,
    Path(proposal_id): Path<String>,
) -> Response {
    match server.undo_proposal(ProposalId(proposal_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_report_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_undo_failed",
            err,
        ),
    }
}

async fn http_session_create(
    State(server): State<RuntimeServer>,
    Json(params): Json<HttpSessionCreateParams>,
) -> Response {
    match server.create_session(params).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_create_failed",
            err,
        ),
    }
}

async fn http_sessions(State(server): State<RuntimeServer>) -> Response {
    match server.list_sessions().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_list_failed",
            err,
        ),
    }
}

async fn http_session_show(
    State(server): State<RuntimeServer>,
    Path(session_id): Path<String>,
) -> Response {
    match server.show_session(SessionId(session_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "session_not_found", err),
    }
}

async fn http_session_fork(
    State(server): State<RuntimeServer>,
    Path(session_id): Path<String>,
    Json(params): Json<HttpThreadForkParams>,
) -> Response {
    match server.fork_thread(SessionId(session_id), params).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_fork_failed",
            err,
        ),
    }
}

fn chat_error_event(error: ChatError) -> ChatTurnEvent {
    ChatTurnEvent {
        kind: ChatTurnEventKind::Error,
        content: Some(error.record.message.clone()),
        response: None,
        tool_call_id: None,
        tool_name: None,
        partial_input_json: None,
        tool_input: None,
        tool_output: None,
        usage: None,
        round: 0,
        metadata: json!({
            "code": error.record.code,
            "retryable": error.record.retryable,
            "details": error.record.details,
        }),
    }
}

fn trace_event_sse(event: TraceEvent) -> Event {
    let kind = event.kind.clone();
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_owned());

    Event::default().event(kind).data(data)
}

fn trace_event_dedupe_key(event: &TraceEvent) -> Option<String> {
    serde_json::to_string(event).ok()
}

fn decode_schema_json<T: DeserializeOwned>(
    body: &Bytes,
    schema_json: &str,
    schema_name: &str,
) -> std::result::Result<T, Response> {
    let body = if body.is_empty() {
        b"{}"
    } else {
        body.as_ref()
    };
    let value = match serde_json::from_slice::<Value>(body) {
        Ok(value) => value,
        Err(err) => {
            return Err(http_error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                format!("request body is not valid JSON: {err}"),
            ));
        }
    };
    let schema = match serde_json::from_str::<Value>(schema_json) {
        Ok(schema) => schema,
        Err(err) => {
            return Err(http_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "schema_load_failed",
                format!("failed to load {schema_name} schema: {err}"),
            ));
        }
    };
    let validator = match jsonschema::validator_for(&schema) {
        Ok(validator) => validator,
        Err(err) => {
            return Err(http_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "schema_compile_failed",
                format!("failed to compile {schema_name} schema: {err}"),
            ));
        }
    };
    let errors = validator
        .iter_errors(&value)
        .map(|error| format!("{}: {}", error.instance_path(), error))
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(http_error(
            StatusCode::BAD_REQUEST,
            "schema_validation_failed",
            format!(
                "{schema_name} request failed schema validation: {}",
                errors.join("; ")
            ),
        ));
    }

    serde_json::from_value(value).map_err(|err| {
        http_error(
            StatusCode::BAD_REQUEST,
            "request_decode_failed",
            format!("failed to decode {schema_name} request: {err}"),
        )
    })
}

fn http_error(status: StatusCode, code: &str, err: impl std::fmt::Display) -> Response {
    warn!(
        status = status.as_u16(),
        code,
        error = %err,
        "HTTP request failed",
    );
    (
        status,
        Json(HttpErrorBody {
            code: code.to_owned(),
            message: err.to_string(),
            details: None,
        }),
    )
        .into_response()
}

fn http_report_error(status: StatusCode, code: &str, err: miette::Report) -> Response {
    if let Some(error) = err.downcast_ref::<PolicyDeniedError>() {
        return http_error_body(
            StatusCode::FORBIDDEN,
            "policy_denied",
            error.message.clone(),
            Some(error.details.clone()),
        );
    }
    http_error(status, code, err)
}

fn http_error_body(
    status: StatusCode,
    code: &str,
    message: String,
    details: Option<Value>,
) -> Response {
    warn!(
        status = status.as_u16(),
        code,
        error = %message,
        "HTTP request failed",
    );
    (
        status,
        Json(HttpErrorBody {
            code: code.to_owned(),
            message,
            details,
        }),
    )
        .into_response()
}

async fn handle_stdio_line(server: &RuntimeServer, line: &str) -> StdioResponse {
    let request = match serde_json::from_str::<StdioRequest>(line) {
        Ok(request) => request,
        Err(err) => {
            warn!(error = %err, "stdio request parse failed");
            return stdio_error(None, -32700, format!("parse error: {err}"));
        }
    };
    debug!(
        method = %request.method,
        id = %request
            .id
            .as_ref()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        "stdio request received",
    );

    if request.jsonrpc.as_deref().is_some_and(|v| v != "2.0") {
        warn!(
            method = %request.method,
            jsonrpc = request.jsonrpc.as_deref().unwrap_or("none"),
            "stdio request has invalid jsonrpc version",
        );
        return stdio_error(request.id, -32600, "invalid jsonrpc version");
    }

    match request.method.as_str() {
        "catalog.summary" => stdio_result(
            request.id,
            serde_json::to_value(CatalogSummary::from_catalog(&server.catalog))
                .unwrap_or_else(|err| json!({"serialization_error": err.to_string()})),
        ),
        "agent.run" => {
            let params = match serde_json::from_value::<AgentRunParams>(request.params) {
                Ok(params) => params,
                Err(err) => {
                    warn!(method = %request.method, error = %err, "stdio params invalid");
                    return stdio_error(request.id, -32602, format!("invalid params: {err}"));
                }
            };
            let outcome = server.run_agent(params.agent_id, params.run).await;
            match outcome {
                Ok(outcome) => stdio_result(
                    request.id,
                    json!({
                        "result": outcome.result,
                        "trace": outcome.trace,
                    }),
                ),
                Err(err) => {
                    warn!(method = %request.method, error = %err, "stdio agent.run failed");
                    stdio_error(request.id, -32000, err.to_string())
                }
            }
        }
        "workflow.run" => {
            let params = match serde_json::from_value::<WorkflowRunRequest>(request.params) {
                Ok(params) => params,
                Err(err) => {
                    warn!(method = %request.method, error = %err, "stdio params invalid");
                    return stdio_error(request.id, -32602, format!("invalid params: {err}"));
                }
            };
            let outcome = server.run_workflow(params).await;
            match outcome {
                Ok(outcome) => stdio_result(
                    request.id,
                    serde_json::to_value(outcome)
                        .unwrap_or_else(|err| json!({"serialization_error": err.to_string()})),
                ),
                Err(err) => {
                    warn!(method = %request.method, error = %err, "stdio workflow.run failed");
                    stdio_error(request.id, -32000, err.to_string())
                }
            }
        }
        _ => {
            warn!(method = %request.method, "stdio method not found");
            stdio_error(request.id, -32601, "method not found")
        }
    }
}
