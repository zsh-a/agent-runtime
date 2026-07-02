use std::collections::BTreeMap;

use agent_core::{PROTOCOL_VERSION, ToolError};
use miette::{Result, miette};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::tool_error;

#[derive(Debug, Clone)]
pub(crate) struct HttpToolEndpoint {
    endpoint: String,
    headers: BTreeMap<String, String>,
}

impl HttpToolEndpoint {
    pub(crate) fn new(endpoint: String, headers: BTreeMap<String, String>) -> Self {
        Self { endpoint, headers }
    }

    pub(crate) async fn call(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        let started_at = std::time::Instant::now();
        info!(
            tool_name = name,
            endpoint = %self.endpoint,
            header_count = self.headers.len(),
            "starting HTTP tool call",
        );
        let client = reqwest::Client::new();
        let payload = json!({
            "protocol_version": PROTOCOL_VERSION,
            "method": "tool.call",
            "tool": name,
            "input": input,
        });
        let mut request = client.post(&self.endpoint).json(&payload);
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| tool_error("http_tool_request_failed", e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| tool_error("http_tool_response_read_failed", e.to_string()))?;
        debug!(
            tool_name = name,
            endpoint = %self.endpoint,
            status = %status,
            body_bytes = body.len(),
            duration_ms = started_at.elapsed().as_millis(),
            "HTTP tool response received",
        );
        if !status.is_success() {
            warn!(
                tool_name = name,
                endpoint = %self.endpoint,
                status = %status,
                body_preview = %truncate_for_log(&body),
                duration_ms = started_at.elapsed().as_millis(),
                "HTTP tool call failed with non-success status",
            );
            return Err(tool_error(
                "http_tool_status_failed",
                format!("HTTP tool endpoint returned {status}: {body}"),
            ));
        }
        let value: Value = serde_json::from_str(&body)
            .map_err(|e| tool_error("http_tool_response_decode_failed", e.to_string()))?;
        if let Some(error) = value.get("error") {
            warn!(
                tool_name = name,
                endpoint = %self.endpoint,
                error = %truncate_for_log(&error.to_string()),
                duration_ms = started_at.elapsed().as_millis(),
                "HTTP tool endpoint returned an error payload",
            );
            return Err(tool_error("http_tool_error", error.to_string()));
        }
        let output = value
            .get("output")
            .or_else(|| value.get("result"))
            .cloned()
            .unwrap_or(value);
        info!(
            tool_name = name,
            endpoint = %self.endpoint,
            status = %status,
            output_bytes = serde_json::to_vec(&output).map(|bytes| bytes.len()).unwrap_or(0),
            duration_ms = started_at.elapsed().as_millis(),
            "HTTP tool call completed",
        );
        Ok(output)
    }
}

pub(crate) fn validate_http_tool_endpoint(source_id: &str, endpoint: &str) -> Result<()> {
    if endpoint.trim().is_empty() {
        return Err(miette!(
            "tool source '{source_id}' endpoint cannot be empty"
        ));
    }
    let url = reqwest::Url::parse(endpoint)
        .map_err(|e| miette!("tool source '{source_id}' endpoint is not a valid URL: {e}"))?;
    match url.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(miette!(
            "tool source '{source_id}' endpoint must use http or https, got '{scheme}'"
        )),
    }
}

fn truncate_for_log(value: &str) -> String {
    const MAX_CHARS: usize = 500;
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
