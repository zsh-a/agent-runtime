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
    pub fn extend_tool_specs(&mut self, specs: impl IntoIterator<Item = ToolSpec>) {
        for spec in specs {
            if !self
                .source_specs
                .iter()
                .any(|known| known.name == spec.name)
            {
                self.source_specs.push(spec);
            }
        }
    }

    pub async fn call_tool(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        let started_at = std::time::Instant::now();
        let input_hash = value_hash(&input);
        let input_bytes = serialized_value_len(&input);
        let spec = self.tool_spec(name);
        let validation = validate_tool_input(spec.as_ref(), name, &input);
        let (source, result) = if let Some(output) = self.mock_tools.get(name) {
            ("mock", validation.map(|()| output.clone()))
        } else if let Some(host) = self.source_tools.get(name) {
            (
                "tool_source",
                match validation {
                    Ok(()) => host.call(name, input).await,
                    Err(error) => Err(error),
                },
            )
        } else if let Some(host) = &self.tool_host {
            (
                "tool_host",
                match validation {
                    Ok(()) => host.call(name, input).await,
                    Err(error) => Err(error),
                },
            )
        } else if name == "echo" {
            ("builtin", validation.map(|()| json!({"echo": input})))
        } else {
            (
                "missing",
                Err(error::tool_error(
                    "unknown_tool",
                    format!("unknown tool '{name}'"),
                )),
            )
        };
        let result = result.and_then(|output| validate_tool_output(spec.as_ref(), name, output));
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

    fn tool_spec(&self, name: &str) -> Option<ToolSpec> {
        self.source_specs
            .iter()
            .find(|tool| tool.name == name)
            .cloned()
            .or_else(|| builtin_tools().into_iter().find(|tool| tool.name == name))
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

fn validate_tool_input(
    spec: Option<&ToolSpec>,
    name: &str,
    input: &Value,
) -> std::result::Result<(), ToolError> {
    let Some(spec) = spec else {
        return Ok(());
    };
    validate_json_schema(&spec.input_schema, name, "input", input)
}

fn validate_tool_output(
    spec: Option<&ToolSpec>,
    name: &str,
    output: Value,
) -> std::result::Result<Value, ToolError> {
    let Some(spec) = spec else {
        return Ok(output);
    };
    let Some(schema) = spec.output_schema.as_ref() else {
        return Ok(output);
    };
    validate_json_schema(schema, name, "output", &output)?;
    Ok(output)
}

fn validate_json_schema(
    schema: &Value,
    name: &str,
    phase: &str,
    value: &Value,
) -> std::result::Result<(), ToolError> {
    let validator = jsonschema::validator_for(schema).map_err(|error| {
        error::tool_error_with_details(
            "tool_schema_compile_failed",
            format!("tool '{name}' {phase} schema failed to compile: {error}"),
            json!({
                "tool_name": name,
                "phase": phase,
                "schema": schema,
            }),
        )
    })?;
    if validator.is_valid(value) {
        return Ok(());
    }
    let errors = validator
        .iter_errors(value)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    Err(error::tool_error_with_details(
        &format!("tool_{phase}_schema_validation_failed"),
        format!("tool '{name}' {phase} failed schema validation"),
        json!({
            "tool_name": name,
            "phase": phase,
            "errors": errors,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tool_overrides_validates_declared_input_schema() {
        let overrides = overrides_with_mock(
            strict_tool_spec(Some(json!({
                "type": "object",
                "required": ["account_id"],
                "properties": {
                    "account_id": {"type": "string"}
                },
                "additionalProperties": false
            }))),
            json!({"ok": true}),
        );

        let error = overrides
            .call_tool("strict.lookup", json!({"account_id": 7}))
            .await
            .expect_err("input schema violation fails");

        assert_eq!(error.record.code, "tool_input_schema_validation_failed");
        assert_eq!(error.record.details["tool_name"], "strict.lookup");
        assert_eq!(error.record.details["phase"], "input");
    }

    #[tokio::test]
    async fn tool_overrides_validates_declared_output_schema() {
        let overrides = overrides_with_mock(
            strict_tool_spec(Some(json!({
                "type": "object",
                "required": ["ok"],
                "properties": {
                    "ok": {"type": "boolean"}
                },
                "additionalProperties": false
            }))),
            json!({"ok": "yes"}),
        );

        let error = overrides
            .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
            .await
            .expect_err("output schema violation fails");

        assert_eq!(error.record.code, "tool_output_schema_validation_failed");
        assert_eq!(error.record.details["tool_name"], "strict.lookup");
        assert_eq!(error.record.details["phase"], "output");
    }

    #[tokio::test]
    async fn tool_overrides_accepts_values_that_match_declared_schemas() {
        let overrides = overrides_with_mock(
            strict_tool_spec(Some(json!({
                "type": "object",
                "required": ["ok"],
                "properties": {
                    "ok": {"type": "boolean"}
                },
                "additionalProperties": false
            }))),
            json!({"ok": true}),
        );

        let output = overrides
            .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
            .await
            .expect("schema-valid tool call succeeds");

        assert_eq!(output, json!({"ok": true}));
    }

    #[tokio::test]
    async fn builtin_tools_are_validated_against_their_specs() {
        let overrides = ToolOverrides::default();

        let error = overrides
            .call_tool("echo", json!("not an object"))
            .await
            .expect_err("builtin echo requires object input");

        assert_eq!(error.record.code, "tool_input_schema_validation_failed");
    }

    #[tokio::test]
    async fn tool_source_timeout_is_enforced() {
        let dir = tempfile::tempdir().expect("temp dir");
        let manifest = dir.path().join("tool-source.json");
        write_tool_source_manifest(
            &manifest,
            json!({
                "version": "tool_source.v1",
                "sources": [{
                    "id": "slow-source",
                    "command": "sh",
                    "args": ["-c", "sleep 1; printf '%s\\n' '{\"result\":{\"ok\":true}}'"],
                    "timeout_ms": 10,
                    "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
                }]
            }),
        );
        let overrides = tool_overrides(
            Vec::new(),
            Vec::new(),
            vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
        )
        .await
        .expect("tool source loads");

        let error = overrides
            .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
            .await
            .expect_err("slow source times out");

        assert_eq!(error.record.code, "tool_source_timeout");
        assert!(error.record.retryable);
        assert_eq!(error.record.details["tool_name"], "strict.lookup");
    }

    #[tokio::test]
    async fn tool_source_retries_retryable_process_errors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let manifest = dir.path().join("tool-source.json");
        let flag = dir.path().join("failed-once");
        write_tool_source_manifest(
            &manifest,
            json!({
                "version": "tool_source.v1",
                "sources": [{
                    "id": "retry-source",
                    "command": "sh",
                    "args": [
                        "-c",
                        "if [ ! -f \"$1\" ]; then touch \"$1\"; printf '%s\\n' '{\"error\":{\"code\":-32000,\"message\":\"try again\",\"data\":{\"retryable\":true}}}'; else printf '%s\\n' '{\"result\":{\"ok\":true}}'; fi",
                        "retry-source",
                        flag.to_str().expect("utf8 flag path")
                    ],
                    "max_retries": 1,
                    "retry_backoff_ms": 0,
                    "tools": [strict_tool_spec(Some(json!({
                        "type": "object",
                        "required": ["ok"],
                        "properties": {"ok": {"type": "boolean"}},
                        "additionalProperties": false
                    })))]
                }]
            }),
        );
        let overrides = tool_overrides(
            Vec::new(),
            Vec::new(),
            vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
        )
        .await
        .expect("tool source loads");

        let output = overrides
            .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
            .await
            .expect("retryable source succeeds");

        assert_eq!(output, json!({"ok": true}));
        assert!(flag.exists());
    }

    #[tokio::test]
    async fn tool_source_rejects_output_over_source_limit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let manifest = dir.path().join("tool-source.json");
        write_tool_source_manifest(
            &manifest,
            json!({
                "version": "tool_source.v1",
                "sources": [{
                    "id": "bounded-source",
                    "command": "sh",
                    "args": ["-c", "printf '%s\\n' '{\"result\":{\"blob\":\"abcdefghijklmnopqrstuvwxyz\"}}'"],
                    "max_output_bytes": 16,
                    "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
                }]
            }),
        );
        let overrides = tool_overrides(
            Vec::new(),
            Vec::new(),
            vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
        )
        .await
        .expect("tool source loads");

        let error = overrides
            .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
            .await
            .expect_err("oversized source output fails");

        assert_eq!(error.record.code, "tool_source_output_too_large");
        assert!(!error.record.retryable);
        assert_eq!(error.record.details["tool_name"], "strict.lookup");
        assert_eq!(error.record.details["max_output_bytes"], 16);
        assert!(error.record.details["output_bytes"].as_u64().unwrap() > 16);
    }

    #[tokio::test]
    async fn tool_source_rejects_zero_output_limit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let manifest = dir.path().join("tool-source.json");
        write_tool_source_manifest(
            &manifest,
            json!({
                "version": "tool_source.v1",
                "sources": [{
                    "id": "bad-limit-source",
                    "command": "sh",
                    "args": ["-c", "printf '%s\\n' '{\"result\":{\"ok\":true}}'"],
                    "max_output_bytes": 0,
                    "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
                }]
            }),
        );

        let error = tool_overrides(
            Vec::new(),
            Vec::new(),
            vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
        )
        .await
        .expect_err("zero output limit is invalid");

        assert!(
            error
                .to_string()
                .contains("max_output_bytes must be greater than zero")
        );
    }

    #[tokio::test]
    async fn process_tool_source_can_set_cwd_and_clear_environment() {
        let dir = tempfile::tempdir().expect("temp dir");
        let workdir = dir.path().join("tool-workdir");
        std::fs::create_dir_all(&workdir).expect("workdir creates");
        std::fs::write(workdir.join("marker"), "ok").expect("marker writes");
        let manifest = dir.path().join("tool-source.json");
        write_tool_source_manifest(
            &manifest,
            json!({
                "version": "tool_source.v1",
                "sources": [{
                    "id": "bounded-process-source",
                    "command": "/bin/sh",
                    "args": [
                        "-c",
                        "if [ -f marker ] && [ \"$VISIBLE\" = allowed ] && [ -z \"${HOME:-}\" ]; then printf '%s\\n' '{\"result\":{\"cwd_ok\":true,\"env_ok\":true,\"home_hidden\":true}}'; else printf '%s\\n' '{\"result\":{\"cwd_ok\":false,\"env_ok\":false,\"home_hidden\":false}}'; fi"
                    ],
                    "cwd": workdir.to_str().expect("utf8 workdir"),
                    "inherit_env": false,
                    "env": {"VISIBLE": "allowed"},
                    "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
                }]
            }),
        );
        let overrides = tool_overrides(
            Vec::new(),
            Vec::new(),
            vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
        )
        .await
        .expect("tool source loads");

        let output = overrides
            .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
            .await
            .expect("bounded process source succeeds");

        assert_eq!(output["cwd_ok"], true);
        assert_eq!(output["env_ok"], true);
        assert_eq!(output["home_hidden"], true);
    }

    #[tokio::test]
    async fn process_tool_source_rejects_missing_env_placeholder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let manifest = dir.path().join("tool-source.json");
        write_tool_source_manifest(
            &manifest,
            json!({
                "version": "tool_source.v1",
                "sources": [{
                    "id": "missing-env-source",
                    "command": "/bin/sh",
                    "args": ["-c", "printf '%s\\n' '{\"result\":{\"ok\":true}}'"],
                    "env": {
                        "TOKEN": "Bearer ${env:AGENT_RUNTIME_TEST_MISSING_PROCESS_ENV_TOKEN}"
                    },
                    "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
                }]
            }),
        );

        let error = tool_overrides(
            Vec::new(),
            Vec::new(),
            vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
        )
        .await
        .expect_err("missing env placeholder fails");

        assert!(
            error.to_string().contains(
                "process env 'TOKEN' references missing environment variable 'AGENT_RUNTIME_TEST_MISSING_PROCESS_ENV_TOKEN'"
            )
        );
    }

    fn overrides_with_mock(spec: ToolSpec, output: Value) -> ToolOverrides {
        ToolOverrides {
            mock_tools: BTreeMap::from([(spec.name.clone(), output)]),
            source_specs: vec![spec],
            source_tools: BTreeMap::new(),
            tool_host: None,
        }
    }

    fn strict_tool_spec(output_schema: Option<Value>) -> ToolSpec {
        ToolSpec {
            name: "strict.lookup".to_owned(),
            description: "Strict lookup test tool".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["account_id"],
                "properties": {
                    "account_id": {"type": "string"}
                },
                "additionalProperties": false
            }),
            output_schema,
            risk: ToolRisk::ReadOnly,
            metadata: json!({}),
        }
    }

    fn write_tool_source_manifest(path: &std::path::Path, value: Value) {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&value).expect("manifest encodes"),
        )
        .expect("manifest writes");
    }
}
