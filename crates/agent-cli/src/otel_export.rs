use std::time::Duration;

use agent_core::{AgentTrace, PROTOCOL_VERSION, RunScope, TraceSpan};
use camino::Utf8PathBuf;
use miette::{IntoDiagnostic, Result, miette};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Serialize;
use serde_json::Value;

pub(crate) const DEFAULT_OTLP_TIMEOUT_SECONDS: u64 = 10;
const MAX_ERROR_BODY_CHARS: usize = 512;

#[derive(Debug, Clone)]
pub(crate) struct ExportOtelTraceOptions {
    pub(crate) trace_file: Utf8PathBuf,
    pub(crate) out: Option<Utf8PathBuf>,
    pub(crate) endpoint: Option<String>,
    pub(crate) header: Vec<String>,
    pub(crate) timeout_seconds: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct OtlpTraceJsonExport {
    protocol_version: String,
    export_format: String,
    #[serde(rename = "resourceSpans")]
    resource_spans: Vec<OtlpResourceSpans>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OtlpResourceSpans {
    resource: OtlpResource,
    scope_spans: Vec<OtlpScopeSpans>,
}

#[derive(Debug, Serialize)]
struct OtlpResource {
    attributes: Vec<OtlpAttribute>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OtlpScopeSpans {
    scope: OtlpInstrumentationScope,
    spans: Vec<OtlpSpan>,
}

#[derive(Debug, Serialize)]
struct OtlpInstrumentationScope {
    name: String,
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OtlpSpan {
    trace_id: String,
    span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_span_id: Option<String>,
    name: String,
    kind: String,
    start_time_unix_nano: String,
    end_time_unix_nano: String,
    attributes: Vec<OtlpAttribute>,
    status: OtlpStatus,
}

#[derive(Debug, Serialize)]
struct OtlpStatus {
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct OtlpAttribute {
    key: String,
    value: OtlpAnyValue,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum OtlpAnyValue {
    StringValue(String),
    IntValue(String),
    DoubleValue(f64),
    BoolValue(bool),
}

#[derive(Debug, Serialize)]
struct OtlpPushReport {
    protocol_version: String,
    export_format: String,
    endpoint: String,
    status_code: u16,
    span_count: usize,
}

pub(crate) async fn export_otel_trace_file(options: ExportOtelTraceOptions) -> Result<()> {
    let trace = read_trace(options.trace_file).await?;
    let export = otlp_trace_json_export(&trace);
    let wrote_file = options.out.is_some();
    if let Some(path) = options.out {
        write_export(&path, &export).await?;
    }
    let endpoint = options.endpoint.or_else(default_otlp_traces_endpoint);
    if let Some(endpoint) = endpoint {
        let status_code = push_otlp_trace_json(
            &endpoint,
            &export,
            otlp_headers(options.header)?,
            options.timeout_seconds,
        )
        .await?;
        return crate::print_json(&OtlpPushReport {
            protocol_version: export.protocol_version.clone(),
            export_format: export.export_format.clone(),
            endpoint,
            status_code,
            span_count: export.span_count(),
        });
    }
    if !wrote_file {
        crate::print_json(&export)?;
    }
    Ok(())
}

async fn write_export(path: &Utf8PathBuf, export: &OtlpTraceJsonExport) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    fs_err::tokio::write(path, serde_json::to_vec_pretty(export).into_diagnostic()?)
        .await
        .map_err(|e| miette!("failed to write OTLP trace export at {path}: {e}"))
}

async fn push_otlp_trace_json(
    endpoint: &str,
    export: &OtlpTraceJsonExport,
    headers: HeaderMap,
    timeout_seconds: u64,
) -> Result<u16> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_seconds.max(1)))
        .build()
        .into_diagnostic()?;
    let response = client
        .post(endpoint)
        .headers(headers)
        .json(export)
        .send()
        .await
        .map_err(|e| miette!("failed to push OTLP trace export to {endpoint}: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let body = truncated_error_body(&body);
        return Err(miette!(
            "OTLP trace export push to {endpoint} failed with HTTP {status}: {body}"
        ));
    }
    Ok(status.as_u16())
}

fn otlp_headers(cli_headers: Vec<String>) -> Result<HeaderMap> {
    let mut values = Vec::new();
    if let Ok(raw) = std::env::var("OTEL_EXPORTER_OTLP_HEADERS") {
        values.extend(
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
        );
    }
    values.extend(cli_headers);

    let mut headers = HeaderMap::new();
    for raw in values {
        let (name, value) = raw
            .split_once('=')
            .ok_or_else(|| miette!("invalid OTLP header `{raw}`; expected NAME=VALUE"))?;
        let name_text = name.trim();
        if name_text.is_empty() {
            return Err(miette!("invalid OTLP header `{raw}`; header name is empty"));
        }
        let name = HeaderName::from_bytes(name_text.as_bytes())
            .map_err(|e| miette!("invalid OTLP header name `{name_text}`: {e}"))?;
        let value = HeaderValue::from_str(value.trim())
            .map_err(|e| miette!("invalid OTLP header value for `{name_text}`: {e}"))?;
        headers.insert(name, value);
    }
    Ok(headers)
}

fn default_otlp_traces_endpoint() -> Option<String> {
    std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            format!(
                "{}/v1/traces",
                value.trim_end_matches('/').trim_end_matches("/v1/traces")
            )
        })
}

fn truncated_error_body(body: &str) -> String {
    let mut chars = body.chars();
    let mut truncated: String = chars.by_ref().take(MAX_ERROR_BODY_CHARS).collect();
    if chars.next().is_some() {
        truncated.push_str("...");
    }
    truncated
}

impl OtlpTraceJsonExport {
    fn span_count(&self) -> usize {
        self.resource_spans
            .iter()
            .flat_map(|resource| resource.scope_spans.iter())
            .map(|scope| scope.spans.len())
            .sum()
    }
}

fn otlp_trace_json_export(trace: &AgentTrace) -> OtlpTraceJsonExport {
    let trace_id = otel_trace_id(&trace.run_id.0);
    let mut resource_attributes = vec![
        otlp_attribute("service.name", "agent-runtime"),
        otlp_attribute("service.version", trace.runtime_version.as_str()),
        otlp_attribute("agent.id", trace.agent_id.as_str()),
        otlp_attribute("agent.version", trace.agent_version.as_str()),
        otlp_attribute("run.id", trace.run_id.0.as_str()),
    ];
    resource_attributes.extend(scope_resource_attributes(&trace.scope));
    let spans = trace
        .spans
        .iter()
        .map(|span| otlp_span(trace, span, &trace_id))
        .collect();
    OtlpTraceJsonExport {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        export_format: "otlp_trace_json.v1".to_owned(),
        resource_spans: vec![OtlpResourceSpans {
            resource: OtlpResource {
                attributes: resource_attributes,
            },
            scope_spans: vec![OtlpScopeSpans {
                scope: OtlpInstrumentationScope {
                    name: "agent-runtime".to_owned(),
                    version: trace.runtime_version.clone(),
                },
                spans,
            }],
        }],
    }
}

fn scope_resource_attributes(scope: &RunScope) -> Vec<OtlpAttribute> {
    match scope {
        RunScope::Global => vec![otlp_attribute("run.scope.type", "global")],
        RunScope::User(user_id) => vec![
            otlp_attribute("run.scope.type", "user"),
            otlp_attribute("run.scope.id", user_id),
        ],
        RunScope::Tenant(tenant_id) => vec![
            otlp_attribute("run.scope.type", "tenant"),
            otlp_attribute("run.scope.id", tenant_id),
        ],
    }
}

fn otlp_span(trace: &AgentTrace, span: &TraceSpan, trace_id: &str) -> OtlpSpan {
    let mut attributes = vec![
        otlp_attribute("run.id", trace.run_id.0.as_str()),
        otlp_attribute("agent.id", trace.agent_id.as_str()),
    ];
    if let Some(map) = span.attributes.as_object() {
        for (key, value) in map {
            attributes.push(otlp_json_attribute(key, value));
        }
    }
    OtlpSpan {
        trace_id: trace_id.to_owned(),
        span_id: otel_span_id(&span.span_id),
        parent_span_id: span.parent_span_id.as_deref().map(otel_span_id),
        name: span.name.clone(),
        kind: "SPAN_KIND_INTERNAL".to_owned(),
        start_time_unix_nano: unix_nanos(span.started_at),
        end_time_unix_nano: unix_nanos(span.finished_at),
        attributes,
        status: otlp_status(&span.status),
    }
}

async fn read_trace(path: Utf8PathBuf) -> Result<AgentTrace> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read trace at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse trace at {path}: {e}"))
}

fn otlp_attribute(key: &str, value: &str) -> OtlpAttribute {
    OtlpAttribute {
        key: key.to_owned(),
        value: OtlpAnyValue::StringValue(value.to_owned()),
    }
}

fn otlp_json_attribute(key: &str, value: &Value) -> OtlpAttribute {
    let value = match value {
        Value::Bool(value) => OtlpAnyValue::BoolValue(*value),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                OtlpAnyValue::IntValue(value.to_string())
            } else if let Some(value) = value.as_u64() {
                OtlpAnyValue::IntValue(value.to_string())
            } else {
                OtlpAnyValue::DoubleValue(value.as_f64().unwrap_or_default())
            }
        }
        Value::String(value) => OtlpAnyValue::StringValue(value.clone()),
        other => OtlpAnyValue::StringValue(other.to_string()),
    };
    OtlpAttribute {
        key: key.to_owned(),
        value,
    }
}

fn otlp_status(status: &str) -> OtlpStatus {
    match status {
        "completed" => OtlpStatus {
            code: "STATUS_CODE_OK".to_owned(),
            message: None,
        },
        "skipped" | "running" => OtlpStatus {
            code: "STATUS_CODE_UNSET".to_owned(),
            message: Some(status.to_owned()),
        },
        other => OtlpStatus {
            code: "STATUS_CODE_ERROR".to_owned(),
            message: Some(other.to_owned()),
        },
    }
}

fn otel_trace_id(run_id: &str) -> String {
    blake3::hash(run_id.as_bytes()).to_hex()[..32].to_owned()
}

fn otel_span_id(span_id: &str) -> String {
    let material = span_id.strip_prefix("span_").unwrap_or(span_id);
    if material.len() >= 16 && material.chars().take(16).all(|ch| ch.is_ascii_hexdigit()) {
        material[..16].to_ascii_lowercase()
    } else {
        blake3::hash(span_id.as_bytes()).to_hex()[..16].to_owned()
    }
}

fn unix_nanos(value: time::OffsetDateTime) -> String {
    value.unix_timestamp_nanos().to_string()
}
