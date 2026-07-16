use std::time::Duration;

use super::infrastructure::{LogMode, TUI_LOG_FILE, init_logging, validate_json};
use agent_core::{ContextPolicy, HookSpec, RunId};
use agent_runtime::{ExecutionPolicy, HookManager, recover_stale_runs};
use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};
use miette::{IntoDiagnostic, Result, miette};

use crate::commands::catalog::{CatalogCommand, run_catalog_command};
use crate::commands::compat::{CompatCommand, run_compat_command};
use crate::commands::llm::{LlmCompleteOptions, run_llm_complete};
use crate::commands::proposal::{ProposalCommand, run_proposal_command};
use crate::commands::run::{RunCliOptions, TickCliOptions, run_agent_once, tick_agents};
use crate::commands::session::{SessionCommand, run_session_command};
use crate::commands::tool::{ToolCommand, run_tool_command};
use crate::commands::workflow::{
    CommandRunOptions, WorkflowRunCliOptions, create_command_from_run, run_command_template,
    run_workflow_request,
};
use crate::config::{RuntimeStoreBackend, load_agent_config};
use crate::debug_bundle::{DebugBundleOptions, export_debug_bundle};
use crate::dev_stdio::{run_dev_mcp_server, run_dev_tool_host};
use crate::eval::{create_eval_from_run, run_dev_score_hook, run_eval_path};
use crate::metrics::build_metrics_summary;
use crate::otel_export::{
    DEFAULT_OTLP_TIMEOUT_SECONDS, ExportOtelTraceOptions, export_otel_trace_file,
};
use crate::replay::{ReplayMode, ReplayTraceOptions, replay_trace};
use crate::runtime_config::{
    ResolvedRuntimeSources, RuntimeSourceOptions, RuntimeSources, compose_runtime_sources,
};
use crate::runtime_server::{RuntimeServer, RuntimeServerOptions};
use crate::runtime_stores::RuntimeStores;
use crate::server::{serve_http, serve_stdio};
use crate::shell_tool_host::run_shell_tool_host;
use crate::tools::{ToolOverrides, ToolSelection};
use crate::trace_store::read_json;
use crate::tui::{TuiOptions, run_tui};
use crate::{chat, config, print_json};

mod args;
mod dispatch;

use args::*;
use dispatch::dispatch;

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

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let context =
        AppContext::new(load_agent_config(cli.config.clone(), cli.profile.as_deref()).await?);
    init_logging(log_mode_for_command(&cli.command, &context));
    dispatch(&context, cli.command).await
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
