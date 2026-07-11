use std::collections::BTreeMap;

use agent_core::{PROTOCOL_VERSION, ToolError};
use miette::{Result, miette};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{retryable_tool_error, tool_error};

#[derive(Debug, Clone)]
pub(crate) struct HttpToolEndpoint {
    endpoint: String,
    headers: BTreeMap<String, String>,
}

impl HttpToolEndpoint {
    pub(crate) fn new(
        source_id: &str,
        endpoint: String,
        headers: BTreeMap<String, String>,
    ) -> Result<Self> {
        Ok(Self {
            endpoint,
            headers: resolve_headers(source_id, headers)?,
        })
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
        let response = request.send().await.map_err(|e| {
            retryable_tool_error(
                "http_tool_request_failed",
                e.to_string(),
                json!({"endpoint": self.endpoint}),
            )
        })?;
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
            if status.is_server_error() || status.as_u16() == 429 {
                return Err(retryable_tool_error(
                    "http_tool_status_failed",
                    format!("HTTP tool endpoint returned {status}: {body}"),
                    json!({
                        "endpoint": self.endpoint,
                        "status": status.as_u16(),
                    }),
                ));
            }
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

fn resolve_headers(
    source_id: &str,
    headers: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    headers
        .into_iter()
        .map(|(key, value)| {
            let resolved =
                resolve_header_value(source_id, &key, &value, |name| std::env::var(name).ok())?;
            Ok((key, resolved))
        })
        .collect()
}

fn resolve_header_value(
    source_id: &str,
    header_name: &str,
    raw: &str,
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> Result<String> {
    let mut resolved = String::new();
    let mut remaining = raw;
    while let Some(start) = remaining.find("${env:") {
        resolved.push_str(&remaining[..start]);
        let after_start = &remaining[start + "${env:".len()..];
        let Some(end) = after_start.find('}') else {
            return Err(miette!(
                "tool source '{source_id}' header '{header_name}' has an unterminated environment placeholder"
            ));
        };
        let env_name = &after_start[..end];
        if env_name.trim().is_empty() {
            return Err(miette!(
                "tool source '{source_id}' header '{header_name}' has an empty environment variable placeholder"
            ));
        }
        let env_value = lookup(env_name).ok_or_else(|| {
            miette!(
                "tool source '{source_id}' header '{header_name}' references missing environment variable '{env_name}'"
            )
        })?;
        resolved.push_str(&env_value);
        remaining = &after_start[end + 1..];
    }
    resolved.push_str(remaining);
    Ok(resolved)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_header_environment_placeholders() {
        let resolved = resolve_header_value(
            "http-dev",
            "authorization",
            "Bearer ${env:TOOL_TOKEN}",
            |name| (name == "TOOL_TOKEN").then(|| "secret-token".to_owned()),
        )
        .expect("placeholder resolves");

        assert_eq!(resolved, "Bearer secret-token");
    }

    #[test]
    fn rejects_missing_header_environment_placeholders() {
        let error = resolve_header_value(
            "http-dev",
            "authorization",
            "Bearer ${env:MISSING_TOKEN}",
            |_| None,
        )
        .expect_err("missing env var fails");

        assert!(
            error
                .to_string()
                .contains("references missing environment variable 'MISSING_TOKEN'")
        );
    }
}
