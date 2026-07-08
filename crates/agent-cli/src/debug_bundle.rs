use std::collections::{BTreeMap, BTreeSet};

use agent_core::{
    AgentProposalStore, AgentRunRecord, AgentRunResult, AgentSessionStore, PROTOCOL_VERSION,
    ProposalEnvelope, RunId, RunRequest, SessionId, SessionRecord, StepRecord, ThreadId,
    ThreadRecord, TriggerKind, UserContext,
};
use agent_runtime::RUNTIME_VERSION;
use camino::{Utf8Path, Utf8PathBuf};
use miette::{IntoDiagnostic, Result, miette};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::catalog::{build_prompt_manifest, read_catalog, string_metadata};
use crate::config::RuntimeStoreBackend;
use crate::runtime_stores::RuntimeStores;

#[derive(Debug, Serialize)]
struct DebugBundleManifest {
    bundle_version: String,
    protocol_version: String,
    runtime_version: String,
    run_id: String,
    agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_version: Option<String>,
    created_at: String,
    files: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize)]
struct RedactionReport {
    policy: String,
    replacement: String,
    redacted_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DebugStateSnapshot {
    protocol_version: String,
    runtime_version: String,
    captured_at: String,
    store_root: String,
    run_id: String,
    agent_id: String,
    run_status: agent_core::AgentRunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<SessionRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread: Option<ThreadRecord>,
    #[serde(default)]
    steps: Vec<StepRecord>,
    #[serde(default)]
    proposals: Vec<ProposalEnvelope>,
}

#[derive(Debug, Serialize)]
struct DebugReplayConfig {
    protocol_version: String,
    runtime_version: String,
    run_id: String,
    agent_id: String,
    replay_mode: String,
    source_store: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_trace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: Option<String>,
    timeout_seconds: u64,
    assets: BTreeMap<String, String>,
    replay_command: Vec<String>,
    run_request: RunRequest,
}

#[derive(Debug, Serialize)]
struct ArtifactMaterializationManifest {
    protocol_version: String,
    runtime_version: String,
    materialized_at: String,
    mode: String,
    records: Vec<ArtifactMaterializationRecord>,
}

#[derive(Debug, Serialize)]
struct ArtifactMaterializationRecord {
    artifact_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundled_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blake3: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DebugArtifactResolverManifest {
    protocol_version: String,
    #[serde(default)]
    resolvers: Vec<ArtifactStoreResolver>,
}

#[derive(Debug, Deserialize)]
struct ArtifactStoreResolver {
    provider: String,
    root: Utf8PathBuf,
}

impl DebugBundleManifest {
    fn new(
        record: &AgentRunRecord,
        agent_version: Option<String>,
        files: BTreeMap<String, String>,
    ) -> Self {
        Self {
            bundle_version: "debug_bundle.v1".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            runtime_version: RUNTIME_VERSION.to_owned(),
            run_id: record.run_id.0.clone(),
            agent_id: record.agent_id.clone(),
            agent_version,
            created_at: time::OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc().to_string()),
            files,
        }
    }
}

pub(crate) async fn export_debug_bundle(
    run_id: String,
    store_path: Utf8PathBuf,
    store_backend: RuntimeStoreBackend,
    out: Utf8PathBuf,
    catalog_path: Option<Utf8PathBuf>,
    trace_path: Option<Utf8PathBuf>,
    timeout_seconds: u64,
    materialize_artifacts: bool,
    artifact_resolver_path: Option<Utf8PathBuf>,
) -> Result<()> {
    let manifest = write_debug_bundle(
        run_id,
        store_path,
        store_backend,
        out,
        catalog_path,
        trace_path,
        timeout_seconds,
        materialize_artifacts,
        artifact_resolver_path,
    )
    .await?;
    crate::print_json(&manifest)
}

pub(crate) async fn write_debug_bundle(
    run_id: String,
    store_path: Utf8PathBuf,
    store_backend: RuntimeStoreBackend,
    out: Utf8PathBuf,
    catalog_path: Option<Utf8PathBuf>,
    trace_path: Option<Utf8PathBuf>,
    timeout_seconds: u64,
    materialize_artifacts: bool,
    artifact_resolver_path: Option<Utf8PathBuf>,
) -> Result<Value> {
    fs_err::tokio::create_dir_all(&out)
        .await
        .into_diagnostic()?;
    let stores = RuntimeStores::open(store_backend, store_path.clone()).await?;
    let run_id = RunId(run_id);
    let record = stores
        .run_store
        .get_run(&run_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("run '{}' was not found", run_id.0))?;

    let trace = match &trace_path {
        Some(path) => Some(read_json_file(path.clone()).await?),
        None => stores
            .trace_store
            .read_trace(&run_id)
            .await
            .into_diagnostic()?
            .map(|trace| serde_json::to_value(&trace))
            .transpose()
            .into_diagnostic()?,
    };
    let catalog = match &catalog_path {
        Some(path) => Some(read_catalog(path.clone()).await?),
        None => None,
    };
    let agent_spec = catalog.as_ref().and_then(|catalog| {
        catalog
            .agents
            .iter()
            .find(|spec| spec.id == record.agent_id)
            .cloned()
    });
    let prompt_manifest = match (&catalog, &agent_spec) {
        (Some(catalog), Some(_)) => Some(build_prompt_manifest(catalog, Some(&record.agent_id))?),
        _ => None,
    };

    let run_request = run_request_from_record(&record);
    let run_result = run_result_from_record(&record)?;
    let state_snapshot = build_debug_state_snapshot(
        &store_path,
        &record,
        stores.proposal_store.as_ref(),
        stores.session_store.as_ref(),
    )
    .await?;
    let artifact_refs = trace
        .as_ref()
        .map(artifact_ref_records_from_trace)
        .unwrap_or_default();
    let artifact_resolvers = match artifact_resolver_path {
        Some(path) => Some(read_artifact_resolver_manifest(path).await?),
        None => None,
    };
    let materialization_manifest = if materialize_artifacts && !artifact_refs.is_empty() {
        Some(materialize_artifact_refs(&out, &artifact_refs, artifact_resolvers.as_ref()).await?)
    } else {
        None
    };
    let replay_config = build_debug_replay_config(
        &store_path,
        catalog_path.as_ref(),
        trace_path.as_ref(),
        timeout_seconds,
        prompt_manifest.is_some(),
        !artifact_refs.is_empty(),
        materialization_manifest.is_some(),
        &record,
        &run_request,
    );
    let mut files = BTreeMap::new();
    let mut redactions = RedactionReport {
        policy: "builtin_sensitive_field_names.v1".to_owned(),
        replacement: "[REDACTED]".to_owned(),
        redacted_paths: Vec::new(),
    };

    write_redacted_bundle_json(
        &out,
        "run_record.json",
        &record,
        &mut files,
        &mut redactions,
    )
    .await?;
    write_redacted_bundle_json(
        &out,
        "run_request.json",
        &run_request,
        &mut files,
        &mut redactions,
    )
    .await?;
    write_redacted_bundle_json(
        &out,
        "run_result.json",
        &run_result,
        &mut files,
        &mut redactions,
    )
    .await?;
    write_redacted_bundle_json(
        &out,
        "replay_config.json",
        &replay_config,
        &mut files,
        &mut redactions,
    )
    .await?;
    if let Some(trace) = &trace {
        write_redacted_bundle_json(&out, "trace.json", trace, &mut files, &mut redactions).await?;
        let events = event_records_from_trace(trace);
        if !events.is_empty() {
            write_redacted_bundle_jsonl(&out, "events.jsonl", &events, &mut files, &mut redactions)
                .await?;
        }
        let tool_calls = tool_call_records_from_trace(trace);
        if !tool_calls.is_empty() {
            write_redacted_bundle_jsonl(
                &out,
                "tool_calls.jsonl",
                &tool_calls,
                &mut files,
                &mut redactions,
            )
            .await?;
        }
        if !artifact_refs.is_empty() {
            write_redacted_bundle_json(
                &out,
                "artifacts.json",
                &artifact_refs,
                &mut files,
                &mut redactions,
            )
            .await?;
        }
        if let Some(materialization_manifest) = &materialization_manifest {
            write_redacted_bundle_json(
                &out,
                "artifact_materializations.json",
                materialization_manifest,
                &mut files,
                &mut redactions,
            )
            .await?;
        }
    }
    if let Some(spec) = &agent_spec {
        write_redacted_bundle_json(&out, "agent_spec.json", spec, &mut files, &mut redactions)
            .await?;
    }
    if let Some(manifest) = &prompt_manifest {
        write_redacted_bundle_json(
            &out,
            "prompt_manifest.json",
            manifest,
            &mut files,
            &mut redactions,
        )
        .await?;
    }
    write_redacted_bundle_json(
        &out,
        "state_snapshot.json",
        &state_snapshot,
        &mut files,
        &mut redactions,
    )
    .await?;
    write_bundle_json(&out, "redactions.json", &redactions, &mut files).await?;
    files.insert("manifest".to_owned(), "manifest.json".to_owned());

    let manifest = DebugBundleManifest::new(
        &record,
        agent_spec.as_ref().map(|spec| spec.version.clone()),
        files,
    );
    write_json_file(out.join("manifest.json"), &manifest).await?;
    serde_json::to_value(manifest).into_diagnostic()
}

async fn build_debug_state_snapshot(
    store_path: &Utf8Path,
    record: &AgentRunRecord,
    proposal_store: &dyn AgentProposalStore,
    session_store: &dyn AgentSessionStore,
) -> Result<DebugStateSnapshot> {
    let proposals = proposal_store
        .list_proposals(Some(&record.run_id))
        .await
        .into_diagnostic()?;

    let session_id = string_metadata(&record.metadata, "session_id");
    let thread_id = string_metadata(&record.metadata, "thread_id");
    let session = match &session_id {
        Some(session_id) => session_store
            .get_session(&SessionId(session_id.clone()))
            .await
            .into_diagnostic()?,
        None => None,
    };
    let thread = match &thread_id {
        Some(thread_id) => session_store
            .get_thread(&ThreadId(thread_id.clone()))
            .await
            .into_diagnostic()?,
        None => None,
    };
    let steps = match &thread {
        Some(thread) => session_store
            .list_steps(&thread.thread_id)
            .await
            .into_diagnostic()?,
        None => Vec::new(),
    };
    let captured_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .into_diagnostic()?;

    Ok(DebugStateSnapshot {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: RUNTIME_VERSION.to_owned(),
        captured_at,
        store_root: store_path.to_string(),
        run_id: record.run_id.0.clone(),
        agent_id: record.agent_id.clone(),
        run_status: record.status.clone(),
        session_id,
        thread_id,
        session,
        thread,
        steps,
        proposals,
    })
}

fn build_debug_replay_config(
    store_path: &Utf8Path,
    catalog_path: Option<&Utf8PathBuf>,
    trace_path: Option<&Utf8PathBuf>,
    timeout_seconds: u64,
    include_prompt_manifest: bool,
    include_artifacts: bool,
    include_artifact_materializations: bool,
    record: &AgentRunRecord,
    run_request: &RunRequest,
) -> DebugReplayConfig {
    let mut assets = BTreeMap::new();
    assets.insert("run_request".to_owned(), "run_request.json".to_owned());
    assets.insert("trace".to_owned(), "trace.json".to_owned());
    assets.insert("events".to_owned(), "events.jsonl".to_owned());
    assets.insert("tool_calls".to_owned(), "tool_calls.jsonl".to_owned());
    assets.insert(
        "state_snapshot".to_owned(),
        "state_snapshot.json".to_owned(),
    );
    if include_prompt_manifest {
        assets.insert(
            "prompt_manifest".to_owned(),
            "prompt_manifest.json".to_owned(),
        );
    }
    if include_artifacts {
        assets.insert("artifacts".to_owned(), "artifacts.json".to_owned());
    }
    if include_artifact_materializations {
        assets.insert(
            "artifact_materializations".to_owned(),
            "artifact_materializations.json".to_owned(),
        );
    }

    let mut replay_command = vec![
        "agent".to_owned(),
        "replay".to_owned(),
        "trace.json".to_owned(),
        "--execute".to_owned(),
        "--store".to_owned(),
        store_path.to_string(),
        "--timeout-seconds".to_owned(),
        timeout_seconds.to_string(),
    ];
    if let Some(catalog_path) = catalog_path {
        replay_command.push("--catalog".to_owned());
        replay_command.push(catalog_path.to_string());
    }

    DebugReplayConfig {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: RUNTIME_VERSION.to_owned(),
        run_id: record.run_id.0.clone(),
        agent_id: record.agent_id.clone(),
        replay_mode: "trace_execute".to_owned(),
        source_store: store_path.to_string(),
        source_trace: trace_path.map(ToString::to_string),
        catalog: catalog_path.map(ToString::to_string),
        timeout_seconds,
        assets,
        replay_command,
        run_request: run_request.clone(),
    }
}

fn run_request_from_record(record: &AgentRunRecord) -> RunRequest {
    RunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: Some(record.run_id.clone()),
        input: record.input.clone(),
        user: user_from_record(record),
        scope: Some(record.scope.clone()),
        trigger: TriggerKind::Replay,
        trigger_envelope: None,
        workflow: record.workflow.clone(),
        metadata: json!({
            "source": "debug_bundle",
            "reconstructed_from": "run_record"
        }),
    }
}

fn user_from_record(record: &AgentRunRecord) -> Option<UserContext> {
    match &record.scope {
        agent_core::RunScope::User(user_id) => Some(UserContext {
            user_id: user_id.clone(),
            metadata: json!({}),
        }),
        _ => None,
    }
}

fn run_result_from_record(record: &AgentRunRecord) -> Result<AgentRunResult> {
    let finished_at = record.finished_at.unwrap_or(record.started_at);
    Ok(AgentRunResult {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: record.run_id.clone(),
        agent_id: record.agent_id.clone(),
        status: record.status.clone(),
        started_at: record.started_at,
        finished_at,
        summary: record.error.as_ref().map(|error| error.message.clone()),
        output: record.output.clone(),
        error: record.error.clone(),
        workflow: record.workflow.clone(),
    })
}

async fn write_bundle_json(
    out: &Utf8Path,
    name: &str,
    value: &impl Serialize,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    write_json_file(out.join(name), value).await?;
    files.insert(bundle_file_key(name), name.to_owned());
    Ok(())
}

async fn write_redacted_bundle_jsonl(
    out: &Utf8Path,
    name: &str,
    values: &[Value],
    files: &mut BTreeMap<String, String>,
    report: &mut RedactionReport,
) -> Result<()> {
    let mut lines = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let mut value = value.clone();
        redact_json_value(
            &mut value,
            &format!("$.{}[{index}]", bundle_file_key(name)),
            report,
        );
        lines.push(serde_json::to_string(&value).into_diagnostic()?);
    }
    fs_err::tokio::write(out.join(name), format!("{}\n", lines.join("\n")))
        .await
        .into_diagnostic()?;
    files.insert(bundle_file_key(name), name.to_owned());
    Ok(())
}

async fn write_redacted_bundle_json(
    out: &Utf8Path,
    name: &str,
    value: &impl Serialize,
    files: &mut BTreeMap<String, String>,
    report: &mut RedactionReport,
) -> Result<()> {
    let mut value = serde_json::to_value(value).into_diagnostic()?;
    redact_json_value(&mut value, "$", report);
    write_bundle_json(out, name, &value, files).await
}

fn bundle_file_key(name: &str) -> String {
    name.trim_end_matches(".json")
        .trim_end_matches(".jsonl")
        .to_owned()
}

fn redact_json_value(value: &mut Value, path: &str, report: &mut RedactionReport) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                let child_path = format!("{path}.{}", json_path_key(key));
                if is_sensitive_key(key) {
                    if !value.is_null() {
                        *value = Value::String(report.replacement.clone());
                        report.redacted_paths.push(child_path);
                    }
                } else {
                    redact_json_value(value, &child_path, report);
                }
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter_mut().enumerate() {
                redact_json_value(value, &format!("{path}[{index}]"), report);
            }
        }
        _ => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "authorization",
        "password",
        "passwd",
        "secret",
        "token",
        "access_token",
        "refresh_token",
        "api_key",
        "apikey",
        "jwt",
        "credential",
        "private_key",
        "local_path",
    ]
    .iter()
    .any(|marker| key == *marker || key.ends_with(marker) || key.contains(&format!("{marker}_")))
}

fn json_path_key(key: &str) -> String {
    if key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        key.to_owned()
    } else {
        format!("{key:?}")
    }
}

fn event_records_from_trace(trace: &Value) -> Vec<Value> {
    trace
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn tool_call_records_from_trace(trace: &Value) -> Vec<Value> {
    trace
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|event| {
            let kind = event.get("kind").and_then(Value::as_str)?;
            if !matches!(kind, "tool_call_finished" | "tool_call_failed") {
                return None;
            }
            event.get("payload").cloned()
        })
        .collect()
}

fn artifact_ref_records_from_trace(trace: &Value) -> Vec<Value> {
    let mut artifacts = trace
        .get("artifact_refs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if artifacts.is_empty() {
        artifacts = trace
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|event| event.get("kind").and_then(Value::as_str) == Some("artifact_published"))
            .filter_map(|event| {
                event
                    .get("payload")
                    .and_then(|payload| payload.get("artifact_ref"))
                    .cloned()
            })
            .collect();
    }
    artifacts
}

async fn materialize_artifact_refs(
    bundle_out: &Utf8Path,
    artifact_refs: &[Value],
    artifact_resolvers: Option<&DebugArtifactResolverManifest>,
) -> Result<ArtifactMaterializationManifest> {
    let artifact_dir = bundle_out.join("artifacts");
    let mut records = Vec::new();
    for (index, artifact) in artifact_refs.iter().enumerate() {
        records.push(
            materialize_artifact_ref(&artifact_dir, artifact, index, artifact_resolvers).await?,
        );
    }
    let materialized_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .into_diagnostic()?;
    Ok(ArtifactMaterializationManifest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: RUNTIME_VERSION.to_owned(),
        materialized_at,
        mode: if artifact_resolvers.is_some() {
            "local_files_and_artifact_store_resolvers".to_owned()
        } else {
            "local_files_only".to_owned()
        },
        records,
    })
}

async fn materialize_artifact_ref(
    artifact_dir: &Utf8Path,
    artifact: &Value,
    index: usize,
    artifact_resolvers: Option<&DebugArtifactResolverManifest>,
) -> Result<ArtifactMaterializationRecord> {
    let artifact_id = artifact_id_for_record(artifact, index);
    let Some((source, source_path)) = artifact_source_path(artifact, artifact_resolvers) else {
        return Ok(ArtifactMaterializationRecord {
            artifact_id,
            status: "skipped".to_owned(),
            source: None,
            bundled_path: None,
            size_bytes: None,
            blake3: None,
            reason: Some(
                "unsupported artifact source; expected file:// uri, metadata.local_path, or configured artifact store resolver"
                    .to_owned(),
            ),
        });
    };

    let filename = artifact_materialized_filename(&artifact_id, &source_path, index);
    let bundled_path = format!("artifacts/{filename}");
    let destination = artifact_dir.join(&filename);
    match copy_artifact_file(&source_path, &destination).await {
        Ok((size_bytes, blake3_hash)) => Ok(ArtifactMaterializationRecord {
            artifact_id,
            status: "materialized".to_owned(),
            source: Some(source),
            bundled_path: Some(bundled_path),
            size_bytes: Some(size_bytes),
            blake3: Some(blake3_hash),
            reason: None,
        }),
        Err(error) => Ok(ArtifactMaterializationRecord {
            artifact_id,
            status: "failed".to_owned(),
            source: Some(source),
            bundled_path: None,
            size_bytes: None,
            blake3: None,
            reason: Some(error.to_string()),
        }),
    }
}

fn artifact_source_path(
    artifact: &Value,
    artifact_resolvers: Option<&DebugArtifactResolverManifest>,
) -> Option<(String, Utf8PathBuf)> {
    artifact
        .get("metadata")
        .and_then(|metadata| metadata.get("local_path"))
        .and_then(Value::as_str)
        .map(|path| ("metadata.local_path".to_owned(), Utf8PathBuf::from(path)))
        .or_else(|| {
            artifact
                .get("uri")
                .and_then(Value::as_str)
                .and_then(file_uri_path)
                .map(|path| ("file_uri".to_owned(), path))
        })
        .or_else(|| artifact_store_resolver_path(artifact, artifact_resolvers))
}

fn artifact_store_resolver_path(
    artifact: &Value,
    artifact_resolvers: Option<&DebugArtifactResolverManifest>,
) -> Option<(String, Utf8PathBuf)> {
    let artifact_resolvers = artifact_resolvers?;
    let store = artifact.get("store")?;
    let provider = store.get("provider").and_then(Value::as_str)?;
    let provider = provider.trim();
    if provider.is_empty() {
        return None;
    }
    let resolver = artifact_resolvers
        .resolvers
        .iter()
        .find(|resolver| resolver.provider == provider)?;
    let key = store.get("key").and_then(Value::as_str)?;
    let bucket = store.get("bucket").and_then(Value::as_str);
    let path = artifact_store_local_path(&resolver.root, bucket, key)?;
    Some((format!("artifact_store:{provider}"), path))
}

fn artifact_store_local_path(
    root: &Utf8Path,
    bucket: Option<&str>,
    key: &str,
) -> Option<Utf8PathBuf> {
    let mut path = root.to_path_buf();
    if let Some(bucket) = bucket.filter(|bucket| !bucket.trim().is_empty()) {
        push_safe_relative_artifact_path(&mut path, bucket)?;
    }
    push_safe_relative_artifact_path(&mut path, key)?;
    Some(path)
}

fn push_safe_relative_artifact_path(path: &mut Utf8PathBuf, value: &str) -> Option<()> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('/') || value.contains('\\') {
        return None;
    }
    for segment in value.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") {
            return None;
        }
        path.push(segment);
    }
    Some(())
}

fn file_uri_path(uri: &str) -> Option<Utf8PathBuf> {
    let path = uri.strip_prefix("file://")?;
    let path = path.strip_prefix("localhost").unwrap_or(path);
    if !path.starts_with('/') {
        return None;
    }
    percent_decode_utf8(path).map(Utf8PathBuf::from)
}

fn percent_decode_utf8(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = *bytes.get(index + 1)?;
            let lo = *bytes.get(index + 2)?;
            decoded.push((hex_value(hi)? << 4) | hex_value(lo)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

async fn copy_artifact_file(source: &Utf8Path, destination: &Utf8Path) -> Result<(u64, String)> {
    if let Some(parent) = destination.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    let mut input = fs_err::tokio::File::open(source)
        .await
        .map_err(|e| miette!("failed to open artifact source {source}: {e}"))?;
    let mut output = fs_err::tokio::File::create(destination)
        .await
        .map_err(|e| miette!("failed to create materialized artifact {destination}: {e}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .await
            .map_err(|e| miette!("failed to read artifact source {source}: {e}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .await
            .map_err(|e| miette!("failed to write materialized artifact {destination}: {e}"))?;
        hasher.update(&buffer[..read]);
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    output
        .flush()
        .await
        .map_err(|e| miette!("failed to flush materialized artifact {destination}: {e}"))?;
    Ok((total, format!("blake3:{}", hasher.finalize().to_hex())))
}

fn artifact_id_for_record(artifact: &Value, index: usize) -> String {
    artifact
        .get("artifact_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("artifact_{}", index + 1))
}

fn artifact_materialized_filename(
    artifact_id: &str,
    source_path: &Utf8Path,
    index: usize,
) -> String {
    let mut name = sanitize_artifact_filename(artifact_id);
    if name.is_empty() {
        name = format!("artifact_{}", index + 1);
    }
    if !name.contains('.') {
        if let Some(extension) = source_path.extension() {
            name.push('.');
            name.push_str(extension);
        }
    }
    format!("{:03}_{name}", index + 1)
}

fn sanitize_artifact_filename(value: &str) -> String {
    value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('_')
            } else {
                None
            }
        })
        .collect()
}

async fn read_json_file(path: Utf8PathBuf) -> Result<Value> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read JSON at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse JSON at {path}: {e}"))
}

async fn read_artifact_resolver_manifest(
    path: Utf8PathBuf,
) -> Result<DebugArtifactResolverManifest> {
    let value = read_json_file(path.clone()).await?;
    let mut manifest: DebugArtifactResolverManifest = serde_json::from_value(value)
        .map_err(|e| miette!("failed to parse artifact resolver manifest at {path}: {e}"))?;
    if manifest.protocol_version != PROTOCOL_VERSION {
        return Err(miette!(
            "artifact resolver manifest at {path} uses unsupported protocol_version '{}'",
            manifest.protocol_version
        ));
    }

    let base_dir = path
        .parent()
        .filter(|parent| !parent.as_str().is_empty())
        .map(Utf8Path::to_path_buf)
        .unwrap_or_else(|| Utf8PathBuf::from("."));
    let mut providers = BTreeSet::new();
    for resolver in &mut manifest.resolvers {
        resolver.provider = resolver.provider.trim().to_owned();
        if resolver.provider.is_empty() {
            return Err(miette!(
                "artifact resolver manifest at {path} contains an empty provider"
            ));
        }
        if !providers.insert(resolver.provider.clone()) {
            return Err(miette!(
                "artifact resolver manifest at {path} contains duplicate provider '{}'",
                resolver.provider
            ));
        }
        if resolver.root.as_str().trim().is_empty() {
            return Err(miette!(
                "artifact resolver manifest at {path} contains an empty root for provider '{}'",
                resolver.provider
            ));
        }
        if resolver.root.is_relative() {
            resolver.root = base_dir.join(&resolver.root);
        }
    }

    Ok(manifest)
}

async fn write_json_file(path: Utf8PathBuf, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::tokio::create_dir_all(parent)
            .await
            .into_diagnostic()?;
    }
    let bytes = serde_json::to_vec_pretty(value).into_diagnostic()?;
    fs_err::tokio::write(&path, bytes)
        .await
        .map_err(|e| miette!("failed to write JSON at {path}: {e}"))
}
