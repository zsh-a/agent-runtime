use std::collections::BTreeMap;

use agent_core::{PROTOCOL_VERSION, ToolError};
use miette::{Result, miette};
use serde_json::{Value, json};

use super::error::tool_error;

#[derive(Debug, Clone)]
pub(super) struct HttpToolEndpoint {
    endpoint: String,
    headers: BTreeMap<String, String>,
}

impl HttpToolEndpoint {
    pub(super) fn new(endpoint: String, headers: BTreeMap<String, String>) -> Self {
        Self { endpoint, headers }
    }

    pub(super) async fn call(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
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
        if !status.is_success() {
            return Err(tool_error(
                "http_tool_status_failed",
                format!("HTTP tool endpoint returned {status}: {body}"),
            ));
        }
        let value: Value = serde_json::from_str(&body)
            .map_err(|e| tool_error("http_tool_response_decode_failed", e.to_string()))?;
        if let Some(error) = value.get("error") {
            return Err(tool_error("http_tool_error", error.to_string()));
        }
        Ok(value
            .get("output")
            .or_else(|| value.get("result"))
            .cloned()
            .unwrap_or(value))
    }
}

pub(super) fn validate_http_tool_endpoint(source_id: &str, endpoint: &str) -> Result<()> {
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
