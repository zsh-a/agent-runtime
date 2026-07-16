use super::*;

pub(super) async fn http_chat_turn(State(server): State<RuntimeServer>, body: Bytes) -> Response {
    let request = match decode_schema_json::<ChatTurnRequest>(
        &body,
        include_str!("../../../../../schemas/chat-turn-request.schema.json"),
        "chat-turn-request",
    ) {
        Ok(request) => request,
        Err(response) => return *response,
    };

    match server.stream_chat_turn(request).await {
        Ok(stream) => chat_sse_response(stream),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "chat_turn_failed", err),
    }
}

pub(super) async fn http_chat_resume(State(server): State<RuntimeServer>, body: Bytes) -> Response {
    let request = match decode_schema_json::<ChatResumeRequest>(
        &body,
        include_str!("../../../../../schemas/chat-resume-request.schema.json"),
        "chat-resume-request",
    ) {
        Ok(request) => request,
        Err(response) => return *response,
    };

    match server.stream_chat_resume(request).await {
        Ok(stream) => chat_sse_response(stream),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "chat_resume_failed", err),
    }
}

pub(super) fn chat_sse_response(stream: agent_chat::ChatEventStream) -> Response {
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

pub(super) fn chat_error_event(error: ChatError) -> ChatTurnEvent {
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
