use super::*;

pub(super) async fn http_tool_call(
    State(server): State<RuntimeServer>,
    Path(tool_name): Path<String>,
    Json(params): Json<HttpToolCallParams>,
) -> Response {
    match server.call_tool(tool_name, params.input).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::INTERNAL_SERVER_ERROR, "tool_call_failed", err),
    }
}
