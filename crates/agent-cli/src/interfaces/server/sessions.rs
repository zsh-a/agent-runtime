use super::*;

pub(super) async fn http_session_create(
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

pub(super) async fn http_sessions(State(server): State<RuntimeServer>) -> Response {
    match server.list_sessions().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_list_failed",
            err,
        ),
    }
}

pub(super) async fn http_session_show(
    State(server): State<RuntimeServer>,
    Path(session_id): Path<String>,
) -> Response {
    match server.show_session(SessionId(session_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "session_not_found", err),
    }
}

pub(super) async fn http_session_fork(
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
