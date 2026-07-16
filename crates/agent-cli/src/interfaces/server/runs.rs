use super::*;

pub(super) async fn http_agent_run(
    State(server): State<RuntimeServer>,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Response {
    let params = match decode_schema_json::<HttpAgentRunParams>(
        &body,
        include_str!("../../../../../schemas/http-agent-run-request.schema.json"),
        "http-agent-run-request",
    ) {
        Ok(params) => params,
        Err(response) => return *response,
    };

    match server.run_agent(agent_id, params).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "agent_run_failed", err),
    }
}

pub(super) async fn http_workflow_run(
    State(server): State<RuntimeServer>,
    body: Bytes,
) -> Response {
    let request = match decode_schema_json::<WorkflowRunRequest>(
        &body,
        include_str!("../../../../../schemas/workflow-run-request.schema.json"),
        "workflow-run-request",
    ) {
        Ok(request) => request,
        Err(response) => return *response,
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

pub(super) async fn http_runs(
    State(server): State<RuntimeServer>,
    Query(params): Query<HttpRunListParams>,
) -> Response {
    match server.list_runs(params.agent_id, params.limit).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "run_list_failed", err),
    }
}

pub(super) async fn http_run_inspect(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
) -> Response {
    match server.get_run(RunId(run_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "run_not_found", err),
    }
}

pub(super) async fn http_run_trace(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
) -> Response {
    match server.get_run_trace(RunId(run_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "trace_not_found", err),
    }
}

pub(super) async fn http_run_events(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
    Query(params): Query<RunEventsQuery>,
    headers: HeaderMap,
) -> Response {
    let run_id = RunId(run_id);
    let after = match run_event_cursor_from_request(&params, &headers) {
        Ok(after) => after,
        Err(response) => return *response,
    };
    let follow = params.follow.unwrap_or(true);
    if let Some(active_events) = server.subscribe_run_events(&run_id).await {
        let replayed_events = enumerate_trace_events_after(active_events.replayed_events, after);
        let seen = replayed_events
            .all_events
            .iter()
            .filter_map(trace_event_dedupe_key)
            .collect::<HashSet<_>>();
        let next_cursor = replayed_events.next_cursor;
        let replay_stream = stream::iter(
            replayed_events
                .records
                .into_iter()
                .map(|record| Ok::<_, Infallible>(trace_event_sse(record.cursor, record.event))),
        );
        if !follow {
            return Sse::new(replay_stream).into_response();
        }
        let live_stream = stream::unfold(
            (active_events.receiver, seen, next_cursor, after),
            |(mut receiver, mut seen, mut next_cursor, after)| async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            if let Some(key) = trace_event_dedupe_key(&event)
                                && !seen.insert(key)
                            {
                                continue;
                            }
                            let cursor = next_cursor;
                            next_cursor = next_cursor.saturating_add(1);
                            if cursor <= after {
                                continue;
                            }
                            return Some((
                                Ok::<_, Infallible>(trace_event_sse(cursor, event)),
                                (receiver, seen, next_cursor, after),
                            ));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return None,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        );
        return Sse::new(replay_stream.chain(live_stream)).into_response();
    }

    match server
        .get_run_event_records_after(run_id.clone(), after)
        .await
    {
        Ok(Some(records)) => {
            let last_cursor = records.last().map(|record| record.cursor).unwrap_or(after);
            let replay_stream =
                stream::iter(records.into_iter().map(|record| {
                    Ok::<_, Infallible>(trace_event_sse(record.cursor, record.event))
                }));
            if follow {
                let live_stream = persisted_run_event_stream(server, run_id, last_cursor);
                return Sse::new(replay_stream.chain(live_stream)).into_response();
            }
            return Sse::new(replay_stream).into_response();
        }
        Ok(None) => {}
        Err(err) => {
            return http_error(StatusCode::INTERNAL_SERVER_ERROR, "run_events_failed", err);
        }
    }

    if follow {
        match server.get_run(run_id.clone()).await {
            Ok(run) if run.status == AgentRunStatus::Running => {
                return Sse::new(persisted_run_event_stream(server, run_id, after)).into_response();
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    match server.get_run_trace(run_id).await {
        Ok(trace) => {
            let events = event_records_from_trace(&trace);
            let stream = stream::iter(
                enumerate_trace_values_after(events, after)
                    .into_iter()
                    .map(|(cursor, event)| Ok::<_, Infallible>(trace_value_sse(cursor, event))),
            );
            Sse::new(stream).into_response()
        }
        Err(err) => http_error(StatusCode::NOT_FOUND, "trace_not_found", err),
    }
}

struct PersistedRunEventStreamState {
    server: RuntimeServer,
    run_id: RunId,
    cursor: RunEventCursor,
    pending: VecDeque<RunEventRecord>,
}

pub(super) fn persisted_run_event_stream(
    server: RuntimeServer,
    run_id: RunId,
    after: RunEventCursor,
) -> impl futures::Stream<Item = Result<Event, Infallible>> {
    stream::unfold(
        PersistedRunEventStreamState {
            server,
            run_id,
            cursor: after,
            pending: VecDeque::new(),
        },
        |mut state| async move {
            loop {
                if let Some(record) = state.pending.pop_front() {
                    state.cursor = record.cursor;
                    return Some((Ok(trace_event_sse(record.cursor, record.event)), state));
                }

                match state
                    .server
                    .get_run_event_records_after(state.run_id.clone(), state.cursor)
                    .await
                {
                    Ok(Some(records)) if !records.is_empty() => {
                        state.pending.extend(records);
                        continue;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(
                            run_id = %state.run_id.0,
                            cursor = state.cursor,
                            error = %error,
                            "failed to poll persisted run events",
                        );
                    }
                }

                match state.server.get_run(state.run_id.clone()).await {
                    Ok(run) if run.status != AgentRunStatus::Running => return None,
                    Ok(_) => {}
                    Err(error) => {
                        warn!(
                            run_id = %state.run_id.0,
                            error = %error,
                            "failed to inspect run while following persisted events",
                        );
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        },
    )
}

pub(super) async fn http_run_cancel(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
) -> Response {
    match server.cancel_run(RunId(run_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "run_not_found", err),
    }
}

pub(super) async fn http_run_replay(
    State(server): State<RuntimeServer>,
    Path(run_id): Path<String>,
) -> Response {
    match server.replay_run(RunId(run_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "run_replay_failed", err),
    }
}

pub(super) fn run_event_cursor_from_request(
    params: &RunEventsQuery,
    headers: &HeaderMap,
) -> std::result::Result<RunEventCursor, Box<Response>> {
    if let Some(after) = params.after.as_deref() {
        return parse_run_event_cursor(after, "after");
    }
    let Some(last_event_id) = headers.get("last-event-id") else {
        return Ok(0);
    };
    let last_event_id = last_event_id.to_str().map_err(|err| {
        Box::new(http_error(
            StatusCode::BAD_REQUEST,
            "invalid_event_cursor",
            format!("Last-Event-ID must be valid UTF-8: {err}"),
        ))
    })?;
    parse_run_event_cursor(last_event_id, "Last-Event-ID")
}

pub(super) fn parse_run_event_cursor(
    value: &str,
    label: &str,
) -> std::result::Result<RunEventCursor, Box<Response>> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Box::new(http_error(
            StatusCode::BAD_REQUEST,
            "invalid_event_cursor",
            format!("{label} cursor cannot be empty"),
        )));
    }
    value.parse::<RunEventCursor>().map_err(|err| {
        Box::new(http_error(
            StatusCode::BAD_REQUEST,
            "invalid_event_cursor",
            format!("{label} cursor must be a non-negative integer: {err}"),
        ))
    })
}

pub(super) fn enumerate_trace_events_after(
    events: Vec<TraceEvent>,
    after: RunEventCursor,
) -> EnumeratedTraceEvents {
    let next_cursor = RunEventCursor::try_from(events.len())
        .unwrap_or(RunEventCursor::MAX)
        .saturating_add(1);
    let records = events
        .iter()
        .cloned()
        .enumerate()
        .filter_map(|(index, event)| {
            let cursor = RunEventCursor::try_from(index.saturating_add(1)).ok()?;
            (cursor > after).then_some(RunEventRecord { cursor, event })
        })
        .collect();
    EnumeratedTraceEvents {
        all_events: events,
        records,
        next_cursor,
    }
}

pub(super) fn enumerate_trace_values_after(
    events: Vec<Value>,
    after: RunEventCursor,
) -> Vec<(RunEventCursor, Value)> {
    events
        .into_iter()
        .enumerate()
        .filter_map(|(index, event)| {
            let cursor = RunEventCursor::try_from(index.saturating_add(1)).ok()?;
            (cursor > after).then_some((cursor, event))
        })
        .collect()
}

pub(super) fn trace_event_sse(cursor: RunEventCursor, event: TraceEvent) -> Event {
    let kind = event.kind.clone();
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_owned());

    Event::default()
        .id(cursor.to_string())
        .event(kind)
        .data(data)
}

pub(super) fn trace_value_sse(cursor: RunEventCursor, event: Value) -> Event {
    let kind = event
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("trace_event")
        .to_owned();
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_owned());

    Event::default()
        .id(cursor.to_string())
        .event(kind)
        .data(data)
}

pub(super) fn trace_event_dedupe_key(event: &TraceEvent) -> Option<String> {
    serde_json::to_string(event).ok()
}
