mod error;
mod http;
mod manifest;
mod mcp;
mod process;

use std::collections::BTreeMap;

use agent_core::{ToolError, ToolRisk, ToolSpec};
use camino::Utf8PathBuf;
use manifest::{ToolSourceRuntime, load_tool_sources as read_tool_sources};
use miette::{Result, miette};
use process::{ProcessToolHost, process_tool_host};
use serde_json::{Value, json};
use tracing::{info, warn};

pub use manifest::{
    ToolSourceDefinition, load_tool_source_specs, load_tool_sources, source_has_tool,
};

#[derive(Debug, Clone, Default)]
pub struct ToolOverrides {
    mock_tools: BTreeMap<String, Value>,
    pub source_specs: Vec<ToolSpec>,
    source_tools: BTreeMap<String, ToolSourceRuntime>,
    tool_host: Option<ProcessToolHost>,
}

impl ToolOverrides {
    pub async fn call_tool(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        let started_at = std::time::Instant::now();
        let input_hash = value_hash(&input);
        let input_bytes = serialized_value_len(&input);
        let (source, result) = if let Some(output) = self.mock_tools.get(name) {
            ("mock", Ok(output.clone()))
        } else if let Some(host) = self.source_tools.get(name) {
            ("tool_source", host.call(name, input).await)
        } else if let Some(host) = &self.tool_host {
            ("tool_host", host.call(name, input).await)
        } else if name == "echo" {
            ("builtin", Ok(json!({"echo": input})))
        } else {
            (
                "missing",
                Err(error::tool_error(
                    "unknown_tool",
                    format!("unknown tool '{name}'"),
                )),
            )
        };
        match &result {
            Ok(output) => {
                info!(
                    tool_name = name,
                    source,
                    input_hash,
                    input_bytes,
                    output_hash = %value_hash(output),
                    output_bytes = serialized_value_len(output),
                    duration_ms = started_at.elapsed().as_millis(),
                    "tool dispatch completed",
                );
            }
            Err(error) => {
                warn!(
                    tool_name = name,
                    source,
                    input_hash,
                    input_bytes,
                    error_code = %error.record.code,
                    error_kind = ?error.record.kind,
                    retryable = error.record.retryable,
                    duration_ms = started_at.elapsed().as_millis(),
                    "tool dispatch failed",
                );
            }
        }
        result
    }
}

pub async fn tool_overrides(
    tool_host: Vec<String>,
    mock_tool: Vec<String>,
    tool_source: Vec<Utf8PathBuf>,
) -> Result<ToolOverrides> {
    let mut mock_tools = BTreeMap::new();
    for spec in mock_tool {
        let (name, raw_value) = spec
            .split_once('=')
            .ok_or_else(|| miette!("mock tool must use NAME=JSON or NAME=@PATH: {spec}"))?;
        let name = name.trim();
        if name.is_empty() {
            return Err(miette!("mock tool name cannot be empty"));
        }
        let value = if let Some(path) = raw_value.strip_prefix('@') {
            if path.is_empty() {
                return Err(miette!("mock tool path cannot be empty for '{name}'"));
            }
            read_json_file(Utf8PathBuf::from(path)).await?
        } else {
            serde_json::from_str(raw_value)
                .map_err(|e| miette!("failed to parse mock tool '{name}' JSON: {e}"))?
        };
        mock_tools.insert(name.to_owned(), value);
    }

    let mut source_tools = BTreeMap::new();
    let mut source_specs = Vec::new();
    for source in read_tool_sources(tool_source).await? {
        let runtime = ToolSourceRuntime::from_source(&source)?;
        for tool in source.tools() {
            if source_tools
                .insert(tool.name.clone(), runtime.clone())
                .is_some()
            {
                return Err(miette!("duplicate tool-source tool '{}'", tool.name));
            }
            source_specs.push(tool.clone());
        }
    }

    Ok(ToolOverrides {
        mock_tools,
        source_specs,
        source_tools,
        tool_host: process_tool_host(tool_host)?,
    })
}

pub fn builtin_tools() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "echo".to_owned(),
        description: "Return the input unchanged inside an echo envelope.".to_owned(),
        input_schema: json!({"type": "object"}),
        output_schema: Some(json!({"type": "object"})),
        risk: ToolRisk::ReadOnly,
        metadata: json!({}),
    }]
}

async fn read_json_file(path: Utf8PathBuf) -> Result<Value> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read JSON at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse JSON at {path}: {e}"))
}

fn value_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("blake3:{}", blake3::hash(&bytes).to_hex())
}

fn serialized_value_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}
