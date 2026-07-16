use super::*;

pub(crate) async fn export_debug_bundle(options: DebugBundleOptions) -> Result<()> {
    let manifest = write_debug_bundle(options).await?;
    crate::print_json(&manifest)
}

pub(crate) async fn write_debug_bundle(options: DebugBundleOptions) -> Result<Value> {
    let replay_options = options.clone();
    let DebugBundleOptions {
        run_id,
        store_path,
        store_backend,
        out,
        catalog_path,
        trace_path,
        timeout_seconds: _,
        materialize_artifacts,
        artifact_resolver_path,
    } = options;
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
        &replay_options,
        DebugReplayAssets {
            prompt_manifest: prompt_manifest.is_some(),
            artifacts: !artifact_refs.is_empty(),
            artifact_materializations: materialization_manifest.is_some(),
        },
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

pub(super) async fn build_debug_state_snapshot(
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

pub(super) fn build_debug_replay_config(
    options: &DebugBundleOptions,
    included: DebugReplayAssets,
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
    if included.prompt_manifest {
        assets.insert(
            "prompt_manifest".to_owned(),
            "prompt_manifest.json".to_owned(),
        );
    }
    if included.artifacts {
        assets.insert("artifacts".to_owned(), "artifacts.json".to_owned());
    }
    if included.artifact_materializations {
        assets.insert(
            "artifact_materializations".to_owned(),
            "artifact_materializations.json".to_owned(),
        );
    }

    let mut replay_command = vec![
        "agent".to_owned(),
        "replay".to_owned(),
        "trace.json".to_owned(),
        "--mode".to_owned(),
        "live".to_owned(),
        "--store".to_owned(),
        options.store_path.to_string(),
        "--timeout-seconds".to_owned(),
        options.timeout_seconds.to_string(),
    ];
    if let Some(catalog_path) = &options.catalog_path {
        replay_command.push("--catalog".to_owned());
        replay_command.push(catalog_path.to_string());
    }

    DebugReplayConfig {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        runtime_version: RUNTIME_VERSION.to_owned(),
        run_id: record.run_id.0.clone(),
        agent_id: record.agent_id.clone(),
        replay_mode: "live".to_owned(),
        source_store: options.store_path.to_string(),
        source_trace: options.trace_path.as_ref().map(ToString::to_string),
        catalog: options.catalog_path.as_ref().map(ToString::to_string),
        timeout_seconds: options.timeout_seconds,
        assets,
        replay_command,
        run_request: run_request.clone(),
    }
}

pub(super) fn run_request_from_record(record: &AgentRunRecord) -> RunRequest {
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

pub(super) fn user_from_record(record: &AgentRunRecord) -> Option<UserContext> {
    match &record.scope {
        agent_core::RunScope::User(user_id) => Some(UserContext {
            user_id: user_id.clone(),
            metadata: json!({}),
        }),
        _ => None,
    }
}

pub(super) fn run_result_from_record(record: &AgentRunRecord) -> Result<AgentRunResult> {
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
