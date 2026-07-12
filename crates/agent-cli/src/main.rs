use std::{fs, io, sync::Mutex, time::Duration};

use agent_core::{ContextPolicy, HookSpec, RunId};
use agent_runtime::{ExecutionPolicy, HookManager, recover_stale_runs};
use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};
use miette::{IntoDiagnostic, Result, miette};
use serde::Serialize;

mod app;
mod cancellation;
mod catalog;
mod chat;
mod cli;
mod cli_input;
mod commands;
mod config;
mod debug_bundle;
mod dev_stdio;
mod eval;
mod metrics;
mod otel_export;
mod proposal;
mod registry;
mod replay;
mod runtime_config;
mod runtime_server;
mod runtime_stores;
mod schema_validation;
mod server;
mod session;
mod shell_tool_host;
mod stdio_protocol;
mod tools;
mod trace_store;
mod tui;

use cli::*;
use commands::catalog::{CatalogCommand, run_catalog_command};
use commands::compat::{CompatCommand, run_compat_command};
use commands::llm::{LlmCompleteOptions, run_llm_complete};
use commands::proposal::{ProposalCommand, run_proposal_command};
use commands::run::{RunCliOptions, TickCliOptions, run_agent_once, tick_agents};
use commands::session::{SessionCommand, run_session_command};
use commands::tool::{ToolCommand, run_tool_command};
use commands::workflow::{
    CommandRunOptions, WorkflowRunCliOptions, create_command_from_run, run_command_template,
    run_workflow_request,
};
use config::{RuntimeStoreBackend, load_agent_config};
use debug_bundle::{DebugBundleOptions, export_debug_bundle};
use dev_stdio::{run_dev_mcp_server, run_dev_tool_host};
use eval::{create_eval_from_run, run_dev_score_hook, run_eval_path};
use metrics::build_metrics_summary;
use otel_export::{DEFAULT_OTLP_TIMEOUT_SECONDS, ExportOtelTraceOptions, export_otel_trace_file};
use replay::{ReplayMode, ReplayTraceOptions, replay_trace};
use runtime_config::{
    ResolvedRuntimeSources, RuntimeSourceOptions, RuntimeSources, compose_runtime_sources,
};
use runtime_server::{RuntimeServer, RuntimeServerOptions};
use runtime_stores::RuntimeStores;
use server::{serve_http, serve_stdio};
use shell_tool_host::run_shell_tool_host;
use tools::{ToolOverrides, ToolSelection};
use trace_store::read_json;
use tui::{TuiOptions, run_tui};

const DEFAULT_LOG_FILTER: &str =
    "warn,agent_cli=info,agent_runtime=info,agent_chat=info,agent_llm=info";
const DEFAULT_REGISTRY: &str = "examples/agents.yaml";
const DEFAULT_STORE: &str = ".agent-runtime/store";
const DEFAULT_EVAL_STORE: &str = ".agent-runtime/eval-store";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8765;
const DEFAULT_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_LLM_PROVIDER: &str = "mock";
const DEFAULT_LLM_MODEL: &str = "mock-model";
const DEFAULT_MOCK_RESPONSE: &str = "mock response";
const DEFAULT_API_KEY_ENV: &str = "OPENAI_API_KEY";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOOL_ROUNDS: u32 = 4;
const TUI_LOG_FILE: &str = "tui.log";

#[derive(Debug)]
struct AppContext {
    config: config::EffectiveAgentConfig,
}

#[derive(Debug, Clone, Copy)]
struct ResolvedExecutionOptions {
    timeout_seconds: u64,
    max_retries: u32,
    retry_backoff_ms: u64,
}

impl AppContext {
    fn new(config: config::EffectiveAgentConfig) -> Self {
        Self { config }
    }

    fn config(&self) -> &config::EffectiveAgentConfig {
        &self.config
    }

    fn catalog(&self, value: Option<Utf8PathBuf>) -> Option<Utf8PathBuf> {
        value.or_else(|| self.config.runtime.sources.catalog.clone())
    }

    fn runtime_sources(
        &self,
        registry: Utf8PathBuf,
        catalog: Option<Utf8PathBuf>,
    ) -> ResolvedRuntimeSources {
        let mut sources = self.config.runtime.sources.clone();
        if registry != DEFAULT_REGISTRY || sources.registry.is_none() {
            sources.registry = Some(registry);
        }
        if catalog.is_some() {
            sources.catalog = catalog;
        }
        ResolvedRuntimeSources::from_sources(sources, DEFAULT_REGISTRY)
    }

    fn command_runtime_sources(
        &self,
        registry: Option<Utf8PathBuf>,
        catalog: Option<Utf8PathBuf>,
    ) -> RuntimeSources {
        let mut sources = self.config.runtime.sources.clone();
        sources.merge(RuntimeSources::new(registry, catalog));
        sources
    }

    fn configured_runtime_sources(&self) -> RuntimeSources {
        self.config.runtime.sources.clone()
    }

    fn default_agent(&self) -> Option<String> {
        self.config.runtime.default_agent.clone()
    }

    fn store(&self, value: Utf8PathBuf) -> Utf8PathBuf {
        config::configured_path(value, DEFAULT_STORE, self.config.runtime.store.as_ref())
    }

    fn configured_store(&self) -> Option<Utf8PathBuf> {
        self.config.runtime.store.clone()
    }

    fn store_backend(&self) -> RuntimeStoreBackend {
        self.config
            .runtime
            .store_backend
            .unwrap_or(RuntimeStoreBackend::File)
    }

    fn eval_store(&self, value: Utf8PathBuf) -> Utf8PathBuf {
        if value == DEFAULT_EVAL_STORE {
            self.config
                .runtime
                .eval_store
                .clone()
                .or_else(|| self.config.runtime.store.clone())
                .unwrap_or(value)
        } else {
            value
        }
    }

    fn tools(&self, args: ToolCliArgs) -> ToolSelection {
        ToolSelection {
            host: if args.tool_host.is_empty() {
                self.config.runtime.tools.host.clone().unwrap_or_default()
            } else {
                args.tool_host
            },
            mocks: if args.mock_tool.is_empty() {
                self.config.runtime.tools.mocks.clone().unwrap_or_default()
            } else {
                args.mock_tool
            },
            sources: if args.tool_source.is_empty() {
                self.config
                    .runtime
                    .tools
                    .sources
                    .clone()
                    .unwrap_or_default()
            } else {
                args.tool_source
            },
        }
    }

    fn hooks(&self) -> Result<HookManager> {
        config::hook_manager(self.hook_specs())
    }

    fn hook_specs(&self) -> Vec<HookSpec> {
        config::configured_hooks(self.config.runtime.hooks.as_ref())
    }

    fn context_policy(&self) -> ContextPolicy {
        config::context_policy(self.config.runtime.context.as_ref())
    }

    fn execution(
        &self,
        timeout_seconds: u64,
        max_retries: u32,
        retry_backoff_ms: u64,
    ) -> ResolvedExecutionOptions {
        ResolvedExecutionOptions {
            timeout_seconds: config::configured_u64(
                timeout_seconds,
                DEFAULT_TIMEOUT_SECONDS,
                self.config.runtime.timeout_seconds,
            ),
            max_retries: config::configured_u32(max_retries, 0, self.config.runtime.max_retries),
            retry_backoff_ms: config::configured_u64(
                retry_backoff_ms,
                0,
                self.config.runtime.retry_backoff_ms,
            ),
        }
    }

    fn stdio(&self, value: bool) -> bool {
        value || self.config.runtime.stdio.unwrap_or(false)
    }

    fn host(&self, value: String) -> String {
        config::configured_string(value, DEFAULT_HOST, self.config.runtime.host.as_ref())
    }

    fn port(&self, value: u16) -> u16 {
        config::configured_u16(value, DEFAULT_PORT, self.config.runtime.port)
    }

    fn chat(&self, args: ChatCliArgs) -> chat::ChatLlmOptions {
        let configured = &self.config.runtime.llm;
        chat::ChatLlmOptions {
            provider: config::configured_string(
                args.provider,
                DEFAULT_LLM_PROVIDER,
                configured.provider.as_ref(),
            ),
            model: config::configured_string(
                args.model,
                DEFAULT_LLM_MODEL,
                configured.model.as_ref(),
            ),
            mock_response: config::configured_string(
                args.mock_response,
                DEFAULT_MOCK_RESPONSE,
                configured.mock_response.as_ref(),
            ),
            api_base_url: args
                .api_base_url
                .or_else(|| configured.api_base_url.clone()),
            api_key_env: config::configured_string(
                args.api_key_env,
                DEFAULT_API_KEY_ENV,
                configured.api_key_env.as_ref(),
            ),
            anthropic_version: config::configured_string(
                args.anthropic_version,
                DEFAULT_ANTHROPIC_VERSION,
                configured.anthropic_version.as_ref(),
            ),
            temperature: args.temperature.or(configured.temperature),
            max_output_tokens: args.max_output_tokens.or(configured.max_output_tokens),
            max_tool_rounds: config::configured_u32(
                args.max_tool_rounds,
                DEFAULT_MAX_TOOL_ROUNDS,
                configured.max_tool_rounds,
            ),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    app::run().await
}

enum LogMode {
    Stderr,
    File(Utf8PathBuf),
}

fn log_mode_for_command(command: &Command, context: &AppContext) -> LogMode {
    match command {
        Command::Tui { store, once, .. }
            if !once && context.store_backend() == RuntimeStoreBackend::File =>
        {
            LogMode::File(context.store(store.clone()).join(TUI_LOG_FILE))
        }
        _ => LogMode::Stderr,
    }
}

fn init_logging(mode: LogMode) {
    match mode {
        LogMode::Stderr => {
            let filter = log_filter();
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(io::stderr)
                .try_init()
                .ok();
        }
        LogMode::File(path) => {
            if let Some(file) = open_log_file(&path) {
                let filter = log_filter();
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_ansi(false)
                    .with_writer(Mutex::new(file))
                    .try_init()
                    .ok();
            } else {
                let filter = log_filter();
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_ansi(false)
                    .with_writer(io::sink)
                    .try_init()
                    .ok();
            }
        }
    }
}

fn log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER))
}

fn open_log_file(path: &Utf8PathBuf) -> Option<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent.as_std_path()).ok()?;
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_std_path())
        .ok()
}

#[derive(Debug, Serialize)]
struct ValidationReport {
    schema: String,
    instance: String,
    valid: bool,
    errors: Vec<String>,
}

async fn validate_json(
    schema_path: Utf8PathBuf,
    instance_path: Utf8PathBuf,
) -> Result<ValidationReport> {
    let schema = read_json(schema_path.clone()).await?;
    let instance = read_json(instance_path.clone()).await?;
    let errors = schema_validation::validation_errors(&schema, &instance)
        .map_err(|error| miette!("failed to compile JSON schema: {error}"))?;

    Ok(ValidationReport {
        schema: schema_path.to_string(),
        instance: instance_path.to_string(),
        valid: errors.is_empty(),
        errors,
    })
}

pub(crate) fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value).into_diagnostic()?);
    Ok(())
}
