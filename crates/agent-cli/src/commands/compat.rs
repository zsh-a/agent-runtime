use std::sync::Arc;

use agent_core::{
    AgentProposalStore, AgentRunResult, PROTOCOL_VERSION, RunId, RunRequest, TriggerKind,
};
use agent_runtime::{AgentRunner, HookManager, RunOutcome};
use agent_store::{FileLockStore, FileProposalStore, FileRunStore};
use camino::Utf8PathBuf;
use clap::Subcommand;
use miette::{IntoDiagnostic, Result, miette};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    catalog::{read_catalog, registry_from_catalog},
    config::execution_policy,
    debug_bundle::write_debug_bundle,
    print_json,
    tools::{CliServices, tool_overrides},
    trace_store::{read_json, write_store_trace},
};

#[derive(Debug, Subcommand)]
pub(crate) enum CompatCommand {
    Check {
        #[arg(long)]
        catalog: Utf8PathBuf,
        #[arg(long = "tool-source", visible_alias = "tools", value_name = "PATH")]
        tool_source: Vec<Utf8PathBuf>,
        #[arg(long)]
        agent_id: String,
        #[arg(long)]
        run_input: Option<Utf8PathBuf>,
        #[arg(long)]
        proposal_input: Option<Utf8PathBuf>,
        #[arg(long, default_value = "schemas")]
        schema_root: Utf8PathBuf,
        #[arg(long)]
        store: Utf8PathBuf,
        #[arg(long)]
        debug_bundle_out: Option<Utf8PathBuf>,
        #[arg(long, default_value_t = 60)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = 0)]
        max_retries: u32,
        #[arg(long, default_value_t = 0)]
        retry_backoff_ms: u64,
    },
}

#[derive(Debug)]
struct CompatCheckOptions {
    catalog: Utf8PathBuf,
    tool_source: Vec<Utf8PathBuf>,
    agent_id: String,
    run_input: Option<Utf8PathBuf>,
    proposal_input: Option<Utf8PathBuf>,
    schema_root: Utf8PathBuf,
    store: Utf8PathBuf,
    debug_bundle_out: Option<Utf8PathBuf>,
    timeout_seconds: u64,
    max_retries: u32,
    retry_backoff_ms: u64,
}

#[derive(Debug, Serialize)]
struct CompatCheckReport {
    protocol_version: String,
    status: CompatStepStatus,
    catalog: String,
    store: String,
    steps: Vec<CompatCheckStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<CompatRunSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposal_run: Option<CompatRunSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    debug_bundle: Option<Value>,
}

#[derive(Debug, Serialize)]
struct CompatRunSummary {
    run_id: String,
    agent_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

#[derive(Debug, Serialize)]
struct CompatCheckStep {
    name: String,
    status: CompatStepStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CompatStepStatus {
    Passed,
    Failed,
    Skipped,
}

pub(crate) async fn run_compat_command(command: CompatCommand) -> Result<()> {
    match command {
        CompatCommand::Check {
            catalog,
            tool_source,
            agent_id,
            run_input,
            proposal_input,
            schema_root,
            store,
            debug_bundle_out,
            timeout_seconds,
            max_retries,
            retry_backoff_ms,
        } => {
            let options = CompatCheckOptions {
                catalog,
                tool_source,
                agent_id,
                run_input,
                proposal_input,
                schema_root,
                store,
                debug_bundle_out,
                timeout_seconds,
                max_retries,
                retry_backoff_ms,
            };
            let report = run_compat_check(options).await;
            print_json(&report)?;
            if report.status == CompatStepStatus::Failed {
                return Err(miette!("compatibility check failed"));
            }
            Ok(())
        }
    }
}

async fn run_compat_check(options: CompatCheckOptions) -> CompatCheckReport {
    let mut report = CompatCheckReport {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        status: CompatStepStatus::Passed,
        catalog: options.catalog.to_string(),
        store: options.store.to_string(),
        steps: Vec::new(),
        run: None,
        proposal_run: None,
        debug_bundle: None,
    };

    push_step(
        &mut report,
        validate_json_file(
            "catalog_schema",
            options.schema_root.join("catalog.schema.json"),
            options.catalog.clone(),
        )
        .await,
    );
    for source in &options.tool_source {
        push_step(
            &mut report,
            validate_json_file(
                "tool_source_schema",
                options.schema_root.join("tool-source-manifest.schema.json"),
                source.clone(),
            )
            .await,
        );
    }

    match validate_catalog_agent(&options).await {
        Ok(step) => push_step(&mut report, step),
        Err(error) => push_step(&mut report, failed_step("catalog_agent", error)),
    }

    let mut debug_run_id = None;
    if let Some(input) = &options.run_input {
        match execute_compat_run(&options, input.clone()).await {
            Ok(outcome) => {
                debug_run_id = Some(outcome.result.run_id.clone());
                report.run = Some(run_summary(&outcome.result));
                push_step(
                    &mut report,
                    passed_step(
                        "run_fixture",
                        json!({
                            "input": input,
                            "run_id": outcome.result.run_id.0,
                            "status": outcome.result.status,
                        }),
                    ),
                );
            }
            Err(error) => push_step(&mut report, failed_step("run_fixture", error)),
        }
    } else {
        push_step(
            &mut report,
            skipped_step("run_fixture", "no --run-input fixture provided"),
        );
    }

    if let Some(input) = &options.proposal_input {
        match execute_compat_run(&options, input.clone()).await {
            Ok(outcome) => {
                debug_run_id = Some(outcome.result.run_id.clone());
                report.proposal_run = Some(run_summary(&outcome.result));
                match proposals_for_run(&options.store, &outcome.result.run_id).await {
                    Ok(proposal_count) if proposal_count > 0 => push_step(
                        &mut report,
                        passed_step(
                            "proposal_fixture",
                            json!({
                                "input": input,
                                "run_id": outcome.result.run_id.0,
                                "proposal_count": proposal_count,
                            }),
                        ),
                    ),
                    Ok(_) => push_step(
                        &mut report,
                        failed_step(
                            "proposal_fixture",
                            miette!("proposal fixture completed without creating a proposal"),
                        ),
                    ),
                    Err(error) => push_step(&mut report, failed_step("proposal_fixture", error)),
                }
            }
            Err(error) => push_step(&mut report, failed_step("proposal_fixture", error)),
        }
    } else {
        push_step(
            &mut report,
            skipped_step("proposal_fixture", "no --proposal-input fixture provided"),
        );
    }

    match (&options.debug_bundle_out, debug_run_id) {
        (Some(out), Some(run_id)) => match verify_debug_bundle(&options, run_id, out.clone()).await
        {
            Ok((manifest, redaction_count)) => {
                report.debug_bundle = Some(manifest.clone());
                push_step(
                    &mut report,
                    passed_step(
                        "trace_redaction",
                        json!({
                            "bundle": out,
                            "manifest": manifest,
                            "redacted_path_count": redaction_count,
                        }),
                    ),
                );
            }
            Err(error) => push_step(&mut report, failed_step("trace_redaction", error)),
        },
        (Some(_), None) => push_step(
            &mut report,
            failed_step(
                "trace_redaction",
                miette!("debug bundle requested but no run fixture completed"),
            ),
        ),
        (None, _) => push_step(
            &mut report,
            skipped_step("trace_redaction", "no --debug-bundle-out path provided"),
        ),
    }

    if report
        .steps
        .iter()
        .any(|step| step.status == CompatStepStatus::Failed)
    {
        report.status = CompatStepStatus::Failed;
    }
    report
}

async fn validate_json_file(
    name: &str,
    schema_path: Utf8PathBuf,
    instance_path: Utf8PathBuf,
) -> CompatCheckStep {
    let schema = match read_json(schema_path.clone()).await {
        Ok(value) => value,
        Err(error) => return failed_step(name, error),
    };
    let instance = match read_json(instance_path.clone()).await {
        Ok(value) => value,
        Err(error) => return failed_step(name, error),
    };
    let validator = match jsonschema::validator_for(&schema) {
        Ok(validator) => validator,
        Err(error) => return failed_step(name, miette!("failed to compile JSON schema: {error}")),
    };
    let errors = validator
        .iter_errors(&instance)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        passed_step(
            name,
            json!({
                "schema": schema_path,
                "instance": instance_path,
            }),
        )
    } else {
        CompatCheckStep {
            name: name.to_owned(),
            status: CompatStepStatus::Failed,
            errors,
            details: Some(json!({
                "schema": schema_path,
                "instance": instance_path,
            })),
        }
    }
}

async fn validate_catalog_agent(options: &CompatCheckOptions) -> Result<CompatCheckStep> {
    let catalog = read_catalog(options.catalog.clone()).await?;
    let Some(agent) = catalog
        .agents
        .iter()
        .find(|agent| agent.id == options.agent_id)
    else {
        return Ok(failed_step(
            "catalog_agent",
            miette!("agent '{}' is not present in catalog", options.agent_id),
        ));
    };
    Ok(passed_step(
        "catalog_agent",
        json!({
            "agent_id": agent.id,
            "agent_version": agent.version,
            "catalog_agent_count": catalog.agents.len(),
        }),
    ))
}

async fn execute_compat_run(
    options: &CompatCheckOptions,
    input_path: Utf8PathBuf,
) -> Result<RunOutcome> {
    let input = read_json(input_path).await?;
    let catalog = read_catalog(options.catalog.clone()).await?;
    let mut tool_overrides =
        tool_overrides(Vec::new(), Vec::new(), options.tool_source.clone()).await?;
    tool_overrides.extend_tool_specs(catalog.tools.clone());
    let registry = registry_from_catalog(&catalog);
    let store = Arc::new(
        FileRunStore::new(options.store.clone())
            .await
            .into_diagnostic()?,
    );
    let lock_store = Arc::new(
        FileLockStore::new(options.store.clone())
            .await
            .into_diagnostic()?,
    );
    let proposal_store = Arc::new(
        FileProposalStore::new(options.store.clone())
            .await
            .into_diagnostic()?,
    );
    let services = Arc::new(CliServices::with_proposal_store(
        tool_overrides,
        proposal_store,
    ));
    let runner = AgentRunner::new(registry, store, services)
        .with_lock_store(lock_store)
        .with_hooks(HookManager::default())
        .with_policy(execution_policy(
            options.timeout_seconds,
            options.max_retries,
            options.retry_backoff_ms,
        ));
    let outcome = runner
        .run_once(
            &options.agent_id,
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input,
                user: None,
                scope: None,
                trigger: TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata: json!({"source": "compat_check"}),
            },
        )
        .await
        .into_diagnostic()?;
    write_store_trace(&options.store, &outcome.trace).await?;
    Ok(outcome)
}

async fn proposals_for_run(store: &camino::Utf8Path, run_id: &RunId) -> Result<usize> {
    let proposal_store = FileProposalStore::new(store.to_path_buf())
        .await
        .into_diagnostic()?;
    Ok(proposal_store
        .list_proposals(Some(run_id))
        .await
        .into_diagnostic()?
        .len())
}

async fn verify_debug_bundle(
    options: &CompatCheckOptions,
    run_id: RunId,
    out: Utf8PathBuf,
) -> Result<(Value, usize)> {
    let manifest = write_debug_bundle(
        run_id.0,
        options.store.clone(),
        out.clone(),
        Some(options.catalog.clone()),
        None,
        options.timeout_seconds,
        false,
        None,
    )
    .await?;
    let redactions = read_json(out.join("redactions.json")).await?;
    let redaction_count = redactions
        .get("redacted_paths")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Ok((manifest, redaction_count))
}

fn push_step(report: &mut CompatCheckReport, step: CompatCheckStep) {
    report.steps.push(step);
}

fn run_summary(result: &AgentRunResult) -> CompatRunSummary {
    CompatRunSummary {
        run_id: result.run_id.0.clone(),
        agent_id: result.agent_id.clone(),
        status: serde_json::to_value(&result.status)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| format!("{:?}", result.status)),
        summary: result.summary.clone(),
    }
}

fn passed_step(name: &str, details: Value) -> CompatCheckStep {
    CompatCheckStep {
        name: name.to_owned(),
        status: CompatStepStatus::Passed,
        errors: Vec::new(),
        details: Some(details),
    }
}

fn failed_step(name: &str, error: impl std::fmt::Display) -> CompatCheckStep {
    CompatCheckStep {
        name: name.to_owned(),
        status: CompatStepStatus::Failed,
        errors: vec![error.to_string()],
        details: None,
    }
}

fn skipped_step(name: &str, reason: &str) -> CompatCheckStep {
    CompatCheckStep {
        name: name.to_owned(),
        status: CompatStepStatus::Skipped,
        errors: Vec::new(),
        details: Some(json!({"reason": reason})),
    }
}
