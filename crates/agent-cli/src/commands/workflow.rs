use std::sync::Arc;

use agent_core::{
    AgentRunRecord, PROTOCOL_VERSION, RunId, RunRequest, TriggerKind, WorkflowRunRequest,
    WorkflowRunResult,
};
use agent_runtime::{AgentRunner, HookManager};
use camino::{Utf8Path, Utf8PathBuf};
use miette::{IntoDiagnostic, Result, miette};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;

use crate::config::RuntimeStoreBackend;
use crate::config::execution_policy;
use crate::runtime_config::{
    ResolvedRuntimeSources, RuntimeSourceOptions, RuntimeSources, compose_runtime_sources,
};
use crate::runtime_stores::RuntimeStores;
use crate::tools::{CliServices, ToolSelection};
use crate::trace_store::{read_json, write_json, write_text};

#[derive(Debug, Serialize)]
pub(crate) struct CommandCreateReport {
    id: String,
    run_id: String,
    agent_id: String,
    command_file: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CommandFrontmatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_run_status: Option<agent_core::AgentRunStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
}

struct CommandTemplate {
    frontmatter: CommandFrontmatter,
    input: Value,
}

pub(crate) struct CommandRunOptions {
    pub(crate) command_file: Utf8PathBuf,
    pub(crate) configured_sources: RuntimeSources,
    pub(crate) source_overrides: RuntimeSources,
    pub(crate) store: Utf8PathBuf,
    pub(crate) store_backend: RuntimeStoreBackend,
    pub(crate) tools: ToolSelection,
    pub(crate) trace_out: Option<Utf8PathBuf>,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_retries: u32,
    pub(crate) retry_backoff_ms: u64,
    pub(crate) hooks: HookManager,
}

pub(crate) struct WorkflowRunCliOptions {
    pub(crate) input: Utf8PathBuf,
    pub(crate) sources: ResolvedRuntimeSources,
    pub(crate) store: Utf8PathBuf,
    pub(crate) store_backend: RuntimeStoreBackend,
    pub(crate) tools: ToolSelection,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_retries: u32,
    pub(crate) retry_backoff_ms: u64,
    pub(crate) hooks: HookManager,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommandRunReport {
    command_file: String,
    agent_id: String,
    result: agent_core::AgentRunResult,
    trace: agent_core::AgentTrace,
}

pub(crate) async fn run_workflow_request(
    options: WorkflowRunCliOptions,
) -> Result<WorkflowRunResult> {
    let value = read_json(options.input.clone()).await?;
    validate_workflow_request(&value)?;
    let request = serde_json::from_value::<WorkflowRunRequest>(value)
        .map_err(|e| miette!("failed to parse workflow request at {}: {e}", options.input))?;
    let mut overrides = options.tools.load().await?;
    let composition = compose_runtime_sources(RuntimeSourceOptions {
        sources: options.sources,
        tool_overrides: overrides.clone(),
    })
    .await?;
    overrides.extend_tool_specs(composition.tool_specs.clone());
    let stores = RuntimeStores::open(options.store_backend, options.store).await?;
    let services = Arc::new(CliServices::with_stores(
        overrides,
        stores.state_store.clone(),
        stores.proposal_store.clone(),
    ));
    let runner =
        AgentRunner::new_with_factory(composition.registry, stores.run_store.clone(), services)
            .with_lock_store(stores.lock_store.clone())
            .with_hooks(options.hooks)
            .with_policy(execution_policy(
                options.timeout_seconds,
                options.max_retries,
                options.retry_backoff_ms,
            ));
    let result = runner.run_workflow(request).await.into_diagnostic()?;
    stores
        .trace_store
        .write_workflow_traces(&result)
        .await
        .into_diagnostic()?;
    Ok(result)
}

fn validate_workflow_request(value: &Value) -> Result<()> {
    let schema = serde_json::from_str::<Value>(include_str!(
        "../../../../schemas/workflow-run-request.schema.json"
    ))
    .into_diagnostic()?;
    let errors = crate::schema_validation::validation_errors(&schema, value)
        .map_err(|error| miette!("failed to compile workflow-run-request schema: {error}"))?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(miette!(
            "workflow request failed schema validation: {}",
            errors.join("; ")
        ))
    }
}

pub(crate) async fn create_command_from_run(
    run_id: String,
    store_path: Utf8PathBuf,
    store_backend: RuntimeStoreBackend,
    out: Utf8PathBuf,
    description: Option<String>,
    sources: RuntimeSources,
) -> Result<CommandCreateReport> {
    let stores = RuntimeStores::open(store_backend, store_path).await?;
    let run_id = RunId(run_id);
    let record = stores
        .run_store
        .get_run(&run_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("run '{}' was not found", run_id.0))?;

    let command_id = command_id_from_path(&out).unwrap_or_else(|| default_command_id(&record));
    let frontmatter = CommandFrontmatter {
        description: Some(description.unwrap_or_else(|| {
            format!(
                "Replay {} from captured run {}",
                record.agent_id, record.run_id.0
            )
        })),
        agent: record.agent_id.clone(),
        catalog: sources.catalog.map(|path| path.to_string()),
        registry: sources.registry.map(|path| path.to_string()),
        source_run_id: Some(record.run_id.0.clone()),
        source_run_status: Some(record.status.clone()),
        created_at: Some(
            time::OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc().to_string()),
        ),
    };
    let markdown = render_command_markdown(&frontmatter, &record.input)?;
    write_text(out.clone(), &markdown).await?;
    Ok(CommandCreateReport {
        id: command_id,
        run_id: record.run_id.0,
        agent_id: record.agent_id,
        command_file: out.to_string(),
    })
}

pub(crate) async fn run_command_template(options: CommandRunOptions) -> Result<CommandRunReport> {
    let text = fs_err::tokio::read_to_string(&options.command_file)
        .await
        .map_err(|e| miette!("failed to read command at {}: {e}", options.command_file))?;
    let template = parse_command_template(&text, &options.command_file)?;
    let sources = resolve_command_runtime_sources(
        &template.frontmatter,
        options.configured_sources,
        options.source_overrides,
    );
    let mut overrides = options.tools.load().await?;
    let composition = compose_runtime_sources(RuntimeSourceOptions {
        sources,
        tool_overrides: overrides.clone(),
    })
    .await?;
    overrides.extend_tool_specs(composition.tool_specs.clone());
    let stores = RuntimeStores::open(options.store_backend, options.store).await?;
    let services = Arc::new(CliServices::with_stores(
        overrides,
        stores.state_store.clone(),
        stores.proposal_store.clone(),
    ));
    let runner =
        AgentRunner::new_with_factory(composition.registry, stores.run_store.clone(), services)
            .with_lock_store(stores.lock_store.clone())
            .with_hooks(options.hooks)
            .with_policy(execution_policy(
                options.timeout_seconds,
                options.max_retries,
                options.retry_backoff_ms,
            ));
    let outcome = runner
        .run_once(
            &template.frontmatter.agent,
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input: template.input,
                user: None,
                scope: None,
                trigger: TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata: json!({
                    "source": "command_template",
                    "command_file": options.command_file.to_string(),
                    "source_run_id": template.frontmatter.source_run_id,
                }),
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
    if let Some(path) = options.trace_out {
        write_json(path, &outcome.trace).await?;
    }
    Ok(CommandRunReport {
        command_file: options.command_file.to_string(),
        agent_id: outcome.result.agent_id.clone(),
        result: outcome.result,
        trace: outcome.trace,
    })
}

fn resolve_command_runtime_sources(
    frontmatter: &CommandFrontmatter,
    mut sources: RuntimeSources,
    overrides: RuntimeSources,
) -> ResolvedRuntimeSources {
    sources.merge(RuntimeSources::new(
        frontmatter.registry.as_ref().map(Utf8PathBuf::from),
        frontmatter.catalog.as_ref().map(Utf8PathBuf::from),
    ));
    sources.merge(overrides);
    ResolvedRuntimeSources::from_sources(sources, "examples/agents.yaml")
}

fn render_command_markdown(frontmatter: &CommandFrontmatter, input: &Value) -> Result<String> {
    let frontmatter = serde_yaml::to_string(frontmatter).into_diagnostic()?;
    let input = serde_json::to_string_pretty(input).into_diagnostic()?;
    Ok(format!(
        "---\n{frontmatter}---\n\nRun the configured agent with this captured input. Replace or extend `$ARGUMENTS` when invoking the command to add run-specific instructions.\n\n```json\n{input}\n```\n"
    ))
}

fn parse_command_template(markdown: &str, path: &Utf8Path) -> Result<CommandTemplate> {
    let Some(rest) = markdown.strip_prefix("---\n") else {
        return Err(miette!(
            "command template at {path} must start with YAML frontmatter"
        ));
    };
    let Some((frontmatter, body)) = rest.split_once("\n---") else {
        return Err(miette!(
            "command template at {path} is missing closing frontmatter marker"
        ));
    };
    let frontmatter: CommandFrontmatter = serde_yaml::from_str(frontmatter)
        .map_err(|e| miette!("failed to parse command frontmatter at {path}: {e}"))?;
    if frontmatter.agent.trim().is_empty() {
        return Err(miette!("command frontmatter at {path} must include agent"));
    }
    let input_text = extract_json_fence(body)
        .ok_or_else(|| miette!("command template at {path} must include a json code fence"))?;
    let input = serde_json::from_str(input_text)
        .map_err(|e| miette!("failed to parse command input JSON at {path}: {e}"))?;
    Ok(CommandTemplate { frontmatter, input })
}

fn extract_json_fence(body: &str) -> Option<&str> {
    let (_, after_open) = body.split_once("```json")?;
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let (json, _) = after_open.split_once("```")?;
    Some(json.trim())
}

fn command_id_from_path(path: &Utf8Path) -> Option<String> {
    path.file_stem().map(sanitize_eval_id)
}

fn default_command_id(record: &AgentRunRecord) -> String {
    format!(
        "{}_{}",
        sanitize_eval_id(&record.agent_id),
        sanitize_eval_id(&record.run_id.0)
    )
}

fn sanitize_eval_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_markdown_has_frontmatter_and_captured_input() {
        let frontmatter = CommandFrontmatter {
            description: Some("Replay review".to_owned()),
            agent: "execution_review".to_owned(),
            catalog: Some("fixtures/contracts/catalog.valid.json".to_owned()),
            registry: None,
            source_run_id: Some("run_01".to_owned()),
            source_run_status: Some(agent_core::AgentRunStatus::Completed),
            created_at: Some("2026-06-28T00:00:00Z".to_owned()),
        };

        let markdown = render_command_markdown(&frontmatter, &json!({"message": "hello"})).unwrap();

        assert!(markdown.starts_with("---\n"));
        assert!(markdown.contains("agent: execution_review"));
        assert!(markdown.contains("source_run_id: run_01"));
        assert!(markdown.contains("```json\n{\n  \"message\": \"hello\"\n}\n```"));
    }

    #[test]
    fn command_template_parses_frontmatter_and_json_fence() {
        let markdown = r#"---
agent: echo_agent
registry: examples/agents.yaml
---

```json
{"message":"hello"}
```
"#;

        let template =
            parse_command_template(markdown, Utf8Path::new(".agent-runtime/commands/echo.md"))
                .unwrap();

        assert_eq!(template.frontmatter.agent, "echo_agent");
        assert_eq!(
            template.frontmatter.registry.as_deref(),
            Some("examples/agents.yaml")
        );
        assert_eq!(template.input, json!({"message": "hello"}));
    }
}
