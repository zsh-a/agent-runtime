use super::*;

pub(super) async fn handle_stdio_line(server: &RuntimeServer, line: &str) -> StdioResponse {
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
