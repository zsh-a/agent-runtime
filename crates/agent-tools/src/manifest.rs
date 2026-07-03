use std::collections::BTreeMap;
use std::time::Duration;

use agent_core::{AgentErrorKind, AgentErrorRecord, ToolError, ToolSpec};
use camino::Utf8PathBuf;
use miette::{Result, miette};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::{
    error::{tool_error, tool_error_with_details},
    http::{HttpToolEndpoint, validate_http_tool_endpoint},
    process::ProcessToolHost,
};

#[derive(Debug, Clone, Deserialize)]
struct ToolSourceManifest {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    sources: Vec<ToolSourceDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolSourceDefinition {
    id: String,
    #[serde(default)]
    protocol: ToolSourceProtocol,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<Utf8PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
    #[serde(default = "default_inherit_env")]
    inherit_env: bool,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_retries: u32,
    #[serde(default)]
    retry_backoff_ms: Option<u64>,
    #[serde(default)]
    max_output_bytes: Option<usize>,
    #[serde(default)]
    tools: Vec<ToolSpec>,
}

impl ToolSourceDefinition {
    pub fn tools(&self) -> &[ToolSpec] {
        &self.tools
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ToolSourceProtocol {
    #[default]
    JsonlToolCall,
    McpStdio,
    HttpJson,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolSourceRuntime {
    protocol: ToolSourceProtocol,
    host: Option<ProcessToolHost>,
    http: Option<HttpToolEndpoint>,
    policy: ToolSourceExecutionPolicy,
}

impl ToolSourceRuntime {
    pub(crate) fn from_source(source: &ToolSourceDefinition) -> Result<Self> {
        let policy = ToolSourceExecutionPolicy::from_source(source)?;
        let host = match source.protocol {
            ToolSourceProtocol::JsonlToolCall | ToolSourceProtocol::McpStdio => {
                let command = source.command.as_ref().ok_or_else(|| {
                    miette!(
                        "tool source '{}' command is required for protocol {:?}",
                        source.id,
                        source.protocol
                    )
                })?;
                if command.trim().is_empty() {
                    return Err(miette!(
                        "tool source '{}' command cannot be empty",
                        source.id
                    ));
                }
                let cwd = validate_process_cwd(&source.id, source.cwd.clone())?;
                let env = resolve_process_env(&source.id, source.env.clone())?;
                Some(ProcessToolHost::with_execution_env(
                    command.clone(),
                    source.args.clone(),
                    cwd,
                    env,
                    source.inherit_env,
                ))
            }
            ToolSourceProtocol::HttpJson => None,
        };
        let http = match source.protocol {
            ToolSourceProtocol::HttpJson => {
                let endpoint = source.endpoint.as_ref().ok_or_else(|| {
                    miette!(
                        "tool source '{}' endpoint is required for http_json",
                        source.id
                    )
                })?;
                validate_http_tool_endpoint(&source.id, endpoint)?;
                Some(HttpToolEndpoint::new(
                    &source.id,
                    endpoint.clone(),
                    source.headers.clone(),
                )?)
            }
            ToolSourceProtocol::JsonlToolCall | ToolSourceProtocol::McpStdio => None,
        };
        Ok(Self {
            protocol: source.protocol,
            host,
            http,
            policy,
        })
    }

    pub(crate) async fn call(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        let mut attempt = 0_u32;
        loop {
            let result = self.call_once_with_timeout(name, input.clone()).await;
            match result {
                Ok(output) => return Ok(output),
                Err(error) if error.record.retryable && attempt < self.policy.max_retries => {
                    attempt += 1;
                    warn!(
                        tool_name = name,
                        attempt,
                        max_retries = self.policy.max_retries,
                        error_code = %error.record.code,
                        retry_backoff_ms = self.policy.retry_backoff.as_millis(),
                        "retrying tool source call after retryable failure",
                    );
                    if !self.policy.retry_backoff.is_zero() {
                        sleep(self.policy.retry_backoff).await;
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn call_once_with_timeout(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        let call = self.call_once(name, input);
        if let Some(timeout_duration) = self.policy.timeout {
            match timeout(timeout_duration, call).await {
                Ok(result) => result,
                Err(_) => Err(tool_timeout_error(name, timeout_duration)),
            }
        } else {
            call.await
        }
    }

    async fn call_once(&self, name: &str, input: Value) -> std::result::Result<Value, ToolError> {
        let output = match self.protocol {
            ToolSourceProtocol::JsonlToolCall => {
                self.host
                    .as_ref()
                    .ok_or_else(|| {
                        tool_error("tool_source_missing_host", "tool source host missing")
                    })?
                    .call(name, input)
                    .await
            }
            ToolSourceProtocol::McpStdio => {
                self.host
                    .as_ref()
                    .ok_or_else(|| {
                        tool_error("tool_source_missing_host", "tool source host missing")
                    })?
                    .call_mcp_tool(name, input)
                    .await
            }
            ToolSourceProtocol::HttpJson => {
                self.http
                    .as_ref()
                    .ok_or_else(|| {
                        tool_error(
                            "tool_source_missing_http_endpoint",
                            "HTTP tool endpoint missing",
                        )
                    })?
                    .call(name, input)
                    .await
            }
        }?;
        self.enforce_output_limit(name, output)
    }

    fn enforce_output_limit(
        &self,
        name: &str,
        output: Value,
    ) -> std::result::Result<Value, ToolError> {
        let Some(max_output_bytes) = self.policy.max_output_bytes else {
            return Ok(output);
        };
        let output_bytes = serde_json::to_vec(&output)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if output_bytes <= max_output_bytes {
            return Ok(output);
        }
        Err(tool_error_with_details(
            "tool_source_output_too_large",
            format!(
                "tool '{name}' output exceeded source max_output_bytes ({output_bytes} > {max_output_bytes})"
            ),
            json!({
                "tool_name": name,
                "output_bytes": output_bytes,
                "max_output_bytes": max_output_bytes,
            }),
        ))
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ToolSourceExecutionPolicy {
    timeout: Option<Duration>,
    max_retries: u32,
    retry_backoff: Duration,
    max_output_bytes: Option<usize>,
}

impl ToolSourceExecutionPolicy {
    fn from_source(source: &ToolSourceDefinition) -> Result<Self> {
        let timeout = source
            .timeout_ms
            .map(|timeout_ms| {
                if timeout_ms == 0 {
                    Err(miette!(
                        "tool source '{}' timeout_ms must be greater than zero",
                        source.id
                    ))
                } else {
                    Ok(Duration::from_millis(timeout_ms))
                }
            })
            .transpose()?;
        Ok(Self {
            timeout,
            max_retries: source.max_retries,
            retry_backoff: Duration::from_millis(source.retry_backoff_ms.unwrap_or(0)),
            max_output_bytes: source
                .max_output_bytes
                .map(|max_output_bytes| {
                    if max_output_bytes == 0 {
                        Err(miette!(
                            "tool source '{}' max_output_bytes must be greater than zero",
                            source.id
                        ))
                    } else {
                        Ok(max_output_bytes)
                    }
                })
                .transpose()?,
        })
    }
}

fn tool_timeout_error(name: &str, timeout_duration: Duration) -> ToolError {
    ToolError {
        record: AgentErrorRecord {
            kind: AgentErrorKind::Timeout,
            code: "tool_source_timeout".to_owned(),
            message: format!(
                "tool '{name}' timed out after {}ms",
                timeout_duration.as_millis()
            ),
            retryable: true,
            details: json!({
                "tool_name": name,
                "timeout_ms": timeout_duration.as_millis(),
            }),
        },
    }
}

fn default_inherit_env() -> bool {
    true
}

fn validate_process_cwd(source_id: &str, cwd: Option<Utf8PathBuf>) -> Result<Option<Utf8PathBuf>> {
    if let Some(cwd) = &cwd
        && cwd.as_str().trim().is_empty()
    {
        return Err(miette!("tool source '{source_id}' cwd cannot be empty"));
    }
    Ok(cwd)
}

fn resolve_process_env(
    source_id: &str,
    env: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    env.into_iter()
        .map(|(key, value)| {
            if key.trim().is_empty() {
                return Err(miette!(
                    "tool source '{source_id}' process env key cannot be empty"
                ));
            }
            let resolved = resolve_env_template(source_id, "process env", &key, &value, |name| {
                std::env::var(name).ok()
            })?;
            Ok((key, resolved))
        })
        .collect()
}

fn resolve_env_template(
    source_id: &str,
    field: &str,
    key: &str,
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
                "tool source '{source_id}' {field} '{key}' has an unterminated environment placeholder"
            ));
        };
        let env_name = &after_start[..end];
        if env_name.trim().is_empty() {
            return Err(miette!(
                "tool source '{source_id}' {field} '{key}' has an empty environment variable placeholder"
            ));
        }
        let env_value = lookup(env_name).ok_or_else(|| {
            miette!(
                "tool source '{source_id}' {field} '{key}' references missing environment variable '{env_name}'"
            )
        })?;
        resolved.push_str(&env_value);
        remaining = &after_start[end + 1..];
    }
    resolved.push_str(remaining);
    Ok(resolved)
}

pub async fn load_tool_source_specs(paths: Vec<Utf8PathBuf>) -> Result<Vec<ToolSpec>> {
    Ok(load_tool_sources(paths)
        .await?
        .into_iter()
        .flat_map(|source| source.tools)
        .collect())
}

pub async fn load_tool_sources(paths: Vec<Utf8PathBuf>) -> Result<Vec<ToolSourceDefinition>> {
    let mut sources = Vec::new();
    for path in paths {
        debug!(path = %path, "loading tool source manifest");
        let manifest = read_tool_source_manifest(path).await?;
        if let Some(version) = &manifest.version
            && version != "tool_source.v1"
        {
            return Err(miette!(
                "unsupported tool source manifest version '{version}'"
            ));
        }
        for source in manifest.sources {
            if source.id.trim().is_empty() {
                return Err(miette!("tool source id cannot be empty"));
            }
            info!(
                source_id = %source.id,
                protocol = ?source.protocol,
                tool_count = source.tools.len(),
                "loaded tool source",
            );
            sources.push(source);
        }
    }
    Ok(sources)
}

pub fn source_has_tool(sources: &[ToolSourceDefinition], name: &str) -> bool {
    sources
        .iter()
        .any(|source| source.tools.iter().any(|tool| tool.name == name))
}

async fn read_tool_source_manifest(path: Utf8PathBuf) -> Result<ToolSourceManifest> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read tool source at {path}: {e}"))?;
    match path.extension() {
        Some("yaml" | "yml") => serde_yaml::from_slice(&bytes)
            .map_err(|e| miette!("failed to parse tool source YAML at {path}: {e}")),
        _ => serde_json::from_slice(&bytes)
            .map_err(|e| miette!("failed to parse tool source JSON at {path}: {e}")),
    }
}
