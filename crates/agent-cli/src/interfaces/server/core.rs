use super::*;

pub(super) async fn http_healthz() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

pub(super) async fn http_catalog_summary(
    State(server): State<RuntimeServer>,
) -> Json<CatalogSummary> {
    Json(CatalogSummary::from_catalog(&server.catalog))
}

pub(super) async fn http_tools(State(server): State<RuntimeServer>) -> Json<Vec<ToolSpec>> {
    Json(server.catalog.tools.clone())
}

pub(super) async fn http_metrics_summary(State(server): State<RuntimeServer>) -> Response {
    match server.metrics_summary().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metrics_summary_failed",
            err,
        ),
    }
}
