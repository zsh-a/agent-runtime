use super::*;

pub(crate) async fn run_eval_path(
    eval_path: Utf8PathBuf,
    store_path: Utf8PathBuf,
    store_backend: RuntimeStoreBackend,
    tool_overrides: ToolOverrides,
    update_golden: bool,
) -> Result<Value> {
    if eval_path.is_dir() {
        let mut reports = Vec::new();
        for path in discover_eval_files(&eval_path)? {
            reports.push(
                run_eval(
                    path,
                    store_path.clone(),
                    store_backend,
                    tool_overrides.clone(),
                    update_golden,
                )
                .await?,
            );
        }
        let total = reports.len();
        let report = EvalSuiteReport {
            passed: true,
            total,
            passed_count: total,
            failed_count: 0,
            reports,
        };
        serde_json::to_value(report).into_diagnostic()
    } else {
        let report = run_eval(
            eval_path,
            store_path,
            store_backend,
            tool_overrides,
            update_golden,
        )
        .await?;
        serde_json::to_value(report).into_diagnostic()
    }
}

pub(crate) async fn create_eval_from_run(
    run_id: String,
    store_path: Utf8PathBuf,
    store_backend: RuntimeStoreBackend,
    out: Utf8PathBuf,
    catalog: Utf8PathBuf,
    id: Option<String>,
    golden_trace: Option<Utf8PathBuf>,
) -> Result<Value> {
    let stores = RuntimeStores::open(store_backend, store_path).await?;
    let run_id = RunId(run_id);
    let record = stores
        .run_store
        .get_run(&run_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("run '{}' was not found", run_id.0))?;
    let trace = stores
        .trace_store
        .read_trace(&run_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("trace for run '{}' was not found", run_id.0))?;
    let trace_value = serde_json::to_value(&trace).into_diagnostic()?;
    let catalog_path = absolutize_runtime_path(catalog)?;
    let catalog = read_catalog(catalog_path.clone()).await?;
    let prompt_manifest =
        eval_prompt_manifest_expectation(&build_prompt_manifest(&catalog, Some(&record.agent_id))?);
    let proposals = stores
        .proposal_store
        .list_proposals(Some(&run_id))
        .await
        .into_diagnostic()?;

    let eval_id = id.unwrap_or_else(|| default_eval_id(&record));
    let eval_dir = out.parent().unwrap_or_else(|| Utf8Path::new("."));
    let golden_trace =
        golden_trace.unwrap_or_else(|| Utf8PathBuf::from(format!("golden/{eval_id}.trace.json")));
    let golden_trace_abs = absolutize_eval_path(eval_dir, &golden_trace);
    let normalized_trace = normalized_trace_json(&trace)?;
    write_json_file(golden_trace_abs.clone(), &normalized_trace).await?;

    let case = EvalCase {
        id: eval_id.clone(),
        agent_id: record.agent_id.clone(),
        catalog: catalog_path,
        golden_trace: Some(golden_trace.clone()),
        input: record.input.clone(),
        expect: EvalExpect {
            status: record.status.clone(),
            agent_id: Some(record.agent_id.clone()),
            trace_events: trace_event_kinds(&trace_value),
            tool_calls: tool_call_sequence_from_trace_value(&trace_value),
            proposals: eval_proposal_expectation(&proposals),
            prompt_manifest: Some(prompt_manifest),
            output_mode: record
                .output
                .get("mode")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        scoring_hook: None,
    };

    write_yaml_file(out.clone(), &case).await?;
    let report = EvalCreateReport {
        id: eval_id,
        run_id: run_id.0,
        agent_id: record.agent_id,
        eval_file: out.to_string(),
        golden_trace: golden_trace_abs.to_string(),
    };
    serde_json::to_value(report).into_diagnostic()
}

pub(super) async fn run_eval(
    eval_file: Utf8PathBuf,
    store_path: Utf8PathBuf,
    store_backend: RuntimeStoreBackend,
    mut tool_overrides: ToolOverrides,
    update_golden: bool,
) -> Result<EvalReport> {
    let bytes = fs_err::tokio::read(&eval_file)
        .await
        .map_err(|e| miette!("failed to read eval at {eval_file}: {e}"))?;
    let case: EvalCase = serde_yaml::from_slice(&bytes)
        .map_err(|e| miette!("failed to parse eval at {eval_file}: {e}"))?;
    let base = eval_file.parent().unwrap_or_else(|| Utf8Path::new("."));
    let catalog_path = absolutize_eval_path(base, &case.catalog);
    let catalog = read_catalog(catalog_path).await?;
    let expected_prompt_manifest = match &case.expect.prompt_manifest {
        Some(_) => Some(build_prompt_manifest(&catalog, Some(&case.agent_id))?),
        None => None,
    };
    tool_overrides.extend_tool_specs(catalog.tools.clone());
    let registry = registry_from_catalog(&catalog);
    let stores = RuntimeStores::open(store_backend, store_path).await?;
    let services = Arc::new(CliServices::with_stores(
        tool_overrides,
        stores.state_store.clone(),
        stores.proposal_store.clone(),
    ));
    let runner = AgentRunner::new_with_factory(registry, stores.run_store.clone(), services)
        .with_lock_store(stores.lock_store.clone());
    let outcome = runner
        .run_once(
            &case.agent_id,
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input: case.input.clone(),
                user: None,
                scope: None,
                trigger: TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata: json!({"eval_id": case.id}),
            },
        )
        .await
        .into_diagnostic()?;
    if outcome.should_persist_trace() {
        stores
            .trace_store
            .write_trace(outcome.trace.clone())
            .await
            .into_diagnostic()?;
    }

    let mut checked = Vec::new();
    if outcome.result.status != case.expect.status {
        return Err(miette!(
            "eval {} expected status {:?}, got {:?}",
            case.id,
            case.expect.status,
            outcome.result.status
        ));
    }
    checked.push("status".to_owned());

    if let Some(expected_agent_id) = &case.expect.agent_id {
        if &outcome.result.agent_id != expected_agent_id {
            return Err(miette!(
                "eval {} expected agent_id {}, got {}",
                case.id,
                expected_agent_id,
                outcome.result.agent_id
            ));
        }
        checked.push("agent_id".to_owned());
    }

    if let Some(expected_mode) = &case.expect.output_mode {
        let actual = outcome.result.output.get("mode").and_then(Value::as_str);
        if actual != Some(expected_mode.as_str()) {
            return Err(miette!(
                "eval {} expected output mode {}, got {:?}",
                case.id,
                expected_mode,
                actual
            ));
        }
        checked.push("output_mode".to_owned());
    }

    if let (Some(expected), Some(manifest)) =
        (&case.expect.prompt_manifest, &expected_prompt_manifest)
    {
        check_prompt_manifest_expectation(&case.id, expected, manifest)?;
        checked.push("prompt_manifest".to_owned());
    }

    for expected_event in &case.expect.trace_events {
        let found = outcome
            .trace
            .events
            .iter()
            .any(|event| &event.kind == expected_event);
        if !found {
            return Err(miette!(
                "eval {} expected trace event {}",
                case.id,
                expected_event
            ));
        }
        checked.push(format!("trace_event:{expected_event}"));
    }

    if !case.expect.tool_calls.is_empty() {
        let actual_tool_calls = tool_call_sequence_from_trace(&outcome.trace);
        if actual_tool_calls != case.expect.tool_calls {
            return Err(miette!(
                "eval {} expected tool calls {:?}, got {:?}",
                case.id,
                case.expect.tool_calls,
                actual_tool_calls
            ));
        }
        checked.push("tool_calls".to_owned());
    }

    if let Some(expected_proposals) = &case.expect.proposals {
        let proposals = stores
            .proposal_store
            .list_proposals(Some(&outcome.result.run_id))
            .await
            .into_diagnostic()?;
        if let Some(min_count) = expected_proposals.min_count
            && proposals.len() < min_count
        {
            return Err(miette!(
                "eval {} expected at least {} proposals, got {}",
                case.id,
                min_count,
                proposals.len()
            ));
        }
        for kind in &expected_proposals.kinds {
            if !proposals.iter().any(|proposal| &proposal.kind == kind) {
                return Err(miette!("eval {} expected proposal kind {}", case.id, kind));
            }
        }
        for status in &expected_proposals.statuses {
            if !proposals.iter().any(|proposal| &proposal.status == status) {
                return Err(miette!(
                    "eval {} expected proposal status {:?}",
                    case.id,
                    status
                ));
            }
        }
        checked.push("proposals".to_owned());
    }

    if let Some(golden_path) = &case.golden_trace {
        let golden_path = absolutize_eval_path(base, golden_path);
        let actual = normalized_trace_json(&outcome.trace)?;
        if update_golden {
            write_json_file(golden_path, &actual).await?;
            checked.push("golden_trace:updated".to_owned());
        } else {
            let expected = read_json_file(golden_path.clone()).await?;
            if expected != actual {
                return Err(miette!(
                    "eval {} golden trace mismatch at {}",
                    case.id,
                    golden_path
                ));
            }
            checked.push("golden_trace".to_owned());
        }
    }

    let mut score = None;
    let mut scoring_comment = None;
    let mut hooks = Vec::new();
    if let Some(hook) = &case.scoring_hook {
        let hook_outcome = run_eval_scoring_hook(hook, &case, &outcome, &checked).await?;
        let scoring = hook_outcome.result;
        hooks.push(hook_outcome.hook_event);
        if let Some(min_score) = hook.min_score
            && scoring.score < min_score
        {
            return Err(miette!(
                "eval {} scoring hook returned score {} below threshold {}",
                case.id,
                scoring.score,
                min_score
            ));
        }
        if !scoring.passed {
            return Err(miette!(
                "eval {} scoring hook failed: {}",
                case.id,
                scoring.comment.as_deref().unwrap_or("no comment")
            ));
        }
        checked.push("scoring_hook".to_owned());
        score = Some(scoring.score);
        scoring_comment = scoring.comment;
    }

    Ok(EvalReport {
        id: case.id,
        passed: true,
        run_id: outcome.result.run_id.0,
        agent_id: outcome.result.agent_id,
        status: outcome.result.status,
        checked,
        score,
        scoring_comment,
        hooks,
    })
}
