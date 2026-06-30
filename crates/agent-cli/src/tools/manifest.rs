use std::collections::BTreeMap;

use agent_core::{ToolError, ToolSpec};
use camino::Utf8PathBuf;
use miette::{Result, miette};
use serde::Deserialize;
use serde_json::Value;

use super::{
    error::tool_error,
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
pub(crate) struct ToolSourceDefinition {
    id: String,
    #[serde(default)]
    protocol: ToolSourceProtocol,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) tools: Vec<ToolSpec>,
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
}

impl ToolSourceRuntime {
    pub(super) fn from_source(source: &ToolSourceDefinition) -> Result<Self> {
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
                Some(ProcessToolHost::new(command.clone(), source.args.clone()))
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
                    endpoint.clone(),
                    source.headers.clone(),
                ))
            }
            ToolSourceProtocol::JsonlToolCall | ToolSourceProtocol::McpStdio => None,
        };
        Ok(Self {
            protocol: source.protocol,
            host,
            http,
        })
    }

    pub(crate) async fn call(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        match self.protocol {
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
        }
    }
}

pub(crate) async fn load_tool_source_specs(paths: Vec<Utf8PathBuf>) -> Result<Vec<ToolSpec>> {
    Ok(load_tool_sources(paths)
        .await?
        .into_iter()
        .flat_map(|source| source.tools)
        .collect())
}

pub(crate) async fn load_tool_sources(
    paths: Vec<Utf8PathBuf>,
) -> Result<Vec<ToolSourceDefinition>> {
    let mut sources = Vec::new();
    for path in paths {
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
            sources.push(source);
        }
    }
    Ok(sources)
}

pub(crate) fn source_has_tool(sources: &[ToolSourceDefinition], name: &str) -> bool {
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
