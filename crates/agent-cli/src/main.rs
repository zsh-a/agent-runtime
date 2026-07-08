use std::{fs, io, sync::Mutex, time::Duration};

use agent_core::{ContextPolicy, HookSpec, RunId};
use agent_runtime::{ExecutionPolicy, HookManager, recover_stale_runs};
use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};
use miette::{IntoDiagnostic, Result, miette};
use serde::Serialize;

mod cancellation;
mod catalog;
mod chat;
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
mod server;
mod session;
mod shell_tool_host;
mod stdio_protocol;
mod tools;
mod trace_store;
mod tui;

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
use debug_bundle::export_debug_bundle;
use dev_stdio::{run_dev_mcp_server, run_dev_tool_host};
use eval::{create_eval_from_run, run_dev_score_hook, run_eval_path};
use metrics::build_metrics_summary;
use otel_export::{DEFAULT_OTLP_TIMEOUT_SECONDS, ExportOtelTraceOptions, export_otel_trace_file};
use replay::{ReplayMode, ReplayTraceOptions, replay_trace};
use runtime_config::{
    ResolvedRuntimeSources, RuntimeSourceOptions, RuntimeSources, compose_runtime_sources,
};
use runtime_server::RuntimeServer;
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
        if registry != Utf8PathBuf::from(DEFAULT_REGISTRY) || sources.registry.is_none() {
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

    fn require_file_store_backend(&self, command: &str) -> Result<()> {
        if self.store_backend() == RuntimeStoreBackend::File {
            return Ok(());
        }
        Err(miette!(
            "{command} does not support runtime.store_backend = \"sqlite\" yet; use runtime.store_backend = \"file\" for this file-oriented workflow"
        ))
    }

    fn eval_store(&self, value: Utf8PathBuf) -> Utf8PathBuf {
        if value == Utf8PathBuf::from(DEFAULT_EVAL_STORE) {
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

#[derive(Debug, Parser)]
#[command(name = "agent")]
#[command(about = "Schema-first Rust agent runtime CLI")]
struct Cli {
    #[arg(long, env = "AGENT_RUNTIME_CONFIG")]
    config: Option<Utf8PathBuf>,
    #[arg(long, env = "AGENT_RUNTIME_PROFILE")]
    profile: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Default, Args)]
struct ToolCliArgs {
    #[arg(
        long = "tool-host",
        visible_alias = "tool-cmd",
        num_args = 1..,
        value_name = "COMMAND"
    )]
    tool_host: Vec<String>,
    #[arg(
        long = "mock-tool",
        visible_alias = "mock",
        value_name = "NAME=JSON_OR_@PATH"
    )]
    mock_tool: Vec<String>,
    #[arg(long = "tool-source", visible_alias = "tools", value_name = "PATH")]
    tool_source: Vec<Utf8PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct ChatCliArgs {
    #[arg(long, env = "AGENT_LLM_PROVIDER", default_value = "mock")]
    provider: String,
    #[arg(long, default_value = "mock-model")]
    model: String,
    #[arg(long, default_value = "mock response")]
    mock_response: String,
    #[arg(long, env = "OPENAI_BASE_URL")]
    api_base_url: Option<String>,
    #[arg(long, default_value = "OPENAI_API_KEY")]
    api_key_env: String,
    #[arg(long, default_value = "2023-06-01")]
    anthropic_version: String,
    #[arg(long)]
    temperature: Option<f32>,
    #[arg(long)]
    max_output_tokens: Option<u32>,
    #[arg(long, default_value_t = DEFAULT_MAX_TOOL_ROUNDS)]
    max_tool_rounds: u32,
}

#[derive(Debug, Subcommand)]
enum Command {
    List {
        #[arg(long, default_value = DEFAULT_REGISTRY)]
        registry: Utf8PathBuf,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
    },
    Run {
        agent_id: String,
        #[arg(long, default_value = DEFAULT_REGISTRY)]
        registry: Utf8PathBuf,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[command(flatten)]
        tools: ToolCliArgs,
        #[arg(long)]
        input: Option<Utf8PathBuf>,
        #[arg(long)]
        trace_out: Option<Utf8PathBuf>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        thread: Option<String>,
        #[arg(long, value_name = "global|user:ID|tenant:ID")]
        scope: Option<String>,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = 0)]
        max_retries: u32,
        #[arg(long, default_value_t = 0)]
        retry_backoff_ms: u64,
    },
    Tick {
        #[arg(long, default_value = DEFAULT_REGISTRY)]
        registry: Utf8PathBuf,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
    },
    Replay {
        trace_file: Utf8PathBuf,
        #[arg(long, value_enum)]
        mode: Option<ReplayMode>,
        #[arg(long)]
        execute: bool,
        #[arg(long, default_value = DEFAULT_REGISTRY)]
        registry: Utf8PathBuf,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[command(flatten)]
        tools: ToolCliArgs,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
        #[arg(long)]
        trace_out: Option<Utf8PathBuf>,
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = 0)]
        max_retries: u32,
        #[arg(long, default_value_t = 0)]
        retry_backoff_ms: u64,
    },
    Inspect {
        run_id: String,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
    },
    Validate {
        schema: Utf8PathBuf,
        instance: Utf8PathBuf,
    },
    DebugBundle {
        #[command(subcommand)]
        command: DebugBundleCommand,
    },
    Metrics {
        #[command(subcommand)]
        command: MetricsCommand,
    },
    Trace {
        #[command(subcommand)]
        command: TraceCommand,
    },
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    Tool {
        #[command(subcommand)]
        command: ToolCommand,
    },
    Proposal {
        #[command(subcommand)]
        command: ProposalCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Llm {
        #[command(subcommand)]
        command: LlmCommand,
    },
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
    Compat {
        #[command(subcommand)]
        command: CompatCommand,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Recover {
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
    },
    Cmd {
        #[command(subcommand)]
        command: CmdCommand,
    },
    Serve {
        #[arg(long, default_value = DEFAULT_REGISTRY)]
        registry: Utf8PathBuf,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
        #[command(flatten)]
        tools: ToolCliArgs,
        #[arg(long)]
        stdio: bool,
        #[arg(long, default_value = DEFAULT_HOST)]
        host: String,
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        #[command(flatten)]
        chat: ChatCliArgs,
    },
    Tui {
        #[arg(long, default_value = DEFAULT_REGISTRY)]
        registry: Utf8PathBuf,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[arg(long)]
        trace: Option<Utf8PathBuf>,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
        #[command(flatten)]
        tools: ToolCliArgs,
        #[arg(
            long,
            help = "Block high-risk tools such as shell.exec in the TUI runtime"
        )]
        deny_high_risk_tools: bool,
        #[command(flatten)]
        chat: ChatCliArgs,
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = 0)]
        max_retries: u32,
        #[arg(long, default_value_t = 0)]
        retry_backoff_ms: u64,
        #[arg(
            long = "no-mouse",
            action = clap::ArgAction::SetFalse,
            default_value_t = true,
            help = "Disable mouse pane resizing, panel selection, and wheel events"
        )]
        mouse_capture: bool,
        #[arg(long)]
        once: bool,
    },
    Eval {
        eval_path: Utf8PathBuf,
        #[arg(long, default_value = DEFAULT_EVAL_STORE)]
        store: Utf8PathBuf,
        #[command(flatten)]
        tools: ToolCliArgs,
        #[arg(long)]
        update_golden: bool,
        #[arg(long)]
        from_run: Option<String>,
        #[arg(long)]
        out: Option<Utf8PathBuf>,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        golden_trace: Option<Utf8PathBuf>,
    },
    #[command(hide = true)]
    DevToolHost,
    #[command(hide = true)]
    DevMcpServer,
    #[command(hide = true)]
    DevScoreHook,
    #[command(hide = true)]
    ShellToolHost,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Show,
}

#[derive(Debug, Subcommand)]
enum CmdCommand {
    Create {
        #[arg(long)]
        from_run: String,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
        #[arg(long)]
        out: Utf8PathBuf,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[arg(long)]
        registry: Option<Utf8PathBuf>,
    },
    Run {
        command_file: Utf8PathBuf,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[arg(long)]
        registry: Option<Utf8PathBuf>,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
        #[command(flatten)]
        tools: ToolCliArgs,
        #[arg(long)]
        trace_out: Option<Utf8PathBuf>,
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = 0)]
        max_retries: u32,
        #[arg(long, default_value_t = 0)]
        retry_backoff_ms: u64,
    },
}

#[derive(Debug, Subcommand)]
enum DebugBundleCommand {
    Export {
        run_id: String,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
        #[arg(long)]
        out: Utf8PathBuf,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[arg(long)]
        trace: Option<Utf8PathBuf>,
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long)]
        materialize_artifacts: bool,
        #[arg(long, value_name = "PATH")]
        artifact_resolver: Option<Utf8PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum MetricsCommand {
    Summary {
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum WorkflowCommand {
    Run {
        input: Utf8PathBuf,
        #[arg(long, default_value = DEFAULT_REGISTRY)]
        registry: Utf8PathBuf,
        #[arg(long)]
        catalog: Option<Utf8PathBuf>,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
        #[command(flatten)]
        tools: ToolCliArgs,
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = 0)]
        max_retries: u32,
        #[arg(long, default_value_t = 0)]
        retry_backoff_ms: u64,
    },
}

#[derive(Debug, Subcommand)]
enum TraceCommand {
    ExportOtel {
        trace_file: Utf8PathBuf,
        #[arg(long)]
        out: Option<Utf8PathBuf>,
        #[arg(long, env = "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")]
        endpoint: Option<String>,
        #[arg(long = "header", value_name = "NAME=VALUE")]
        header: Vec<String>,
        #[arg(long, default_value_t = DEFAULT_OTLP_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
    },
}

#[derive(Debug, Subcommand)]
enum LlmCommand {
    Complete {
        #[arg(long)]
        prompt: String,
        #[arg(long, env = "AGENT_LLM_PROVIDER", default_value = "mock")]
        provider: String,
        #[arg(long, default_value = "mock-model")]
        model: String,
        #[arg(long, default_value = "mock response")]
        mock_response: String,
        #[arg(long, env = "OPENAI_BASE_URL")]
        api_base_url: Option<String>,
        #[arg(long, default_value = "OPENAI_API_KEY")]
        api_key_env: String,
        #[arg(long)]
        temperature: Option<f32>,
        #[arg(long)]
        max_output_tokens: Option<u32>,
        #[arg(long, default_value = "2023-06-01")]
        anthropic_version: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let context =
        AppContext::new(load_agent_config(cli.config.clone(), cli.profile.as_deref()).await?);
    init_logging(log_mode_for_command(&cli.command, &context));

    match cli.command {
        Command::List { registry, catalog } => {
            let sources = context.runtime_sources(registry, catalog);
            let composition = compose_runtime_sources(RuntimeSourceOptions {
                sources,
                tool_overrides: ToolOverrides::default(),
            })
            .await?;
            let specs = composition.agent_specs;
            println!(
                "{}",
                serde_json::to_string_pretty(&specs).into_diagnostic()?
            );
        }
        Command::Run {
            agent_id,
            registry,
            catalog,
            tools,
            input,
            trace_out,
            session,
            thread,
            scope,
            store,
            timeout_seconds,
            max_retries,
            retry_backoff_ms,
        } => {
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let execution = context.execution(timeout_seconds, max_retries, retry_backoff_ms);
            let hooks = context.hooks()?;
            run_agent_once(RunCliOptions {
                agent_id,
                sources,
                tool_overrides: context.tools(tools).load().await?,
                input,
                trace_out,
                session,
                thread,
                scope,
                store,
                store_backend: context.store_backend(),
                timeout_seconds: execution.timeout_seconds,
                max_retries: execution.max_retries,
                retry_backoff_ms: execution.retry_backoff_ms,
                hooks,
            })
            .await?;
        }
        Command::Tick {
            registry,
            catalog,
            store,
        } => {
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let hooks = context.hooks()?;
            tick_agents(TickCliOptions {
                sources,
                store,
                store_backend: context.store_backend(),
                hooks,
            })
            .await?;
        }
        Command::Replay {
            trace_file,
            mode,
            execute,
            registry,
            catalog,
            tools,
            store,
            trace_out,
            timeout_seconds,
            max_retries,
            retry_backoff_ms,
        } => {
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let execution = context.execution(timeout_seconds, max_retries, retry_backoff_ms);
            let mode = if execute {
                match mode {
                    Some(ReplayMode::View | ReplayMode::Deterministic) => {
                        return Err(miette!("--execute is only compatible with --mode live"));
                    }
                    Some(ReplayMode::Live) | None => ReplayMode::Live,
                }
            } else {
                mode.unwrap_or(ReplayMode::View)
            };
            match mode {
                ReplayMode::Live | ReplayMode::Deterministic => {
                    if mode == ReplayMode::Live || execute {
                        context.require_file_store_backend("replay")?;
                    }
                    replay_trace(ReplayTraceOptions {
                        trace_file,
                        mode,
                        sources,
                        tools: context.tools(tools),
                        store,
                        trace_out,
                        timeout_seconds: execution.timeout_seconds,
                        max_retries: execution.max_retries,
                        retry_backoff_ms: execution.retry_backoff_ms,
                        hooks: match mode {
                            ReplayMode::Live => context.hooks()?,
                            ReplayMode::Deterministic => HookManager::default(),
                            ReplayMode::View => unreachable!("view replay does not execute"),
                        },
                    })
                    .await?;
                }
                ReplayMode::View => {
                    let trace = read_json(trace_file).await?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&trace).into_diagnostic()?
                    );
                }
            }
        }
        Command::Inspect { run_id, store } => {
            let store = context.store(store);
            let stores = RuntimeStores::open(context.store_backend(), store).await?;
            let record = stores
                .run_store
                .get_run(&RunId(run_id.clone()))
                .await
                .into_diagnostic()?
                .ok_or_else(|| miette!("run '{run_id}' was not found"))?;
            print_json(&record)?;
        }
        Command::Validate { schema, instance } => {
            let report = validate_json(schema, instance).await?;
            print_json(&report)?;
            if !report.valid {
                return Err(miette!("JSON instance failed schema validation"));
            }
        }
        Command::DebugBundle { command } => match command {
            DebugBundleCommand::Export {
                run_id,
                store,
                out,
                catalog,
                trace,
                timeout_seconds,
                materialize_artifacts,
                artifact_resolver,
            } => {
                context.require_file_store_backend("debug-bundle export")?;
                export_debug_bundle(
                    run_id,
                    context.store(store),
                    out,
                    catalog,
                    trace,
                    timeout_seconds,
                    materialize_artifacts,
                    artifact_resolver,
                )
                .await?;
            }
        },
        Command::Metrics { command } => match command {
            MetricsCommand::Summary { store } => {
                let stores =
                    RuntimeStores::open(context.store_backend(), context.store(store)).await?;
                let summary = build_metrics_summary(
                    &stores.artifact_store_path,
                    stores.run_store.as_ref(),
                    stores.trace_store.as_ref(),
                    stores.proposal_store.as_ref(),
                )
                .await?;
                print_json(&summary)?;
            }
        },
        Command::Trace { command } => match command {
            TraceCommand::ExportOtel {
                trace_file,
                out,
                endpoint,
                header,
                timeout_seconds,
            } => {
                export_otel_trace_file(ExportOtelTraceOptions {
                    trace_file,
                    out,
                    endpoint,
                    header,
                    timeout_seconds,
                })
                .await?;
            }
        },
        Command::Workflow { command } => match command {
            WorkflowCommand::Run {
                input,
                registry,
                catalog,
                store,
                tools,
                timeout_seconds,
                max_retries,
                retry_backoff_ms,
            } => {
                let execution = context.execution(timeout_seconds, max_retries, retry_backoff_ms);
                let sources = context.runtime_sources(registry, catalog);
                let result = run_workflow_request(WorkflowRunCliOptions {
                    input,
                    sources,
                    store: context.store(store),
                    store_backend: context.store_backend(),
                    tools: context.tools(tools),
                    timeout_seconds: execution.timeout_seconds,
                    max_retries: execution.max_retries,
                    retry_backoff_ms: execution.retry_backoff_ms,
                    hooks: context.hooks()?,
                })
                .await?;
                print_json(&result)?;
            }
        },
        Command::Tool { command } => {
            run_tool_command(command).await?;
        }
        Command::Proposal { command } => {
            run_proposal_command(
                command,
                context.hooks()?,
                context.store_backend(),
                context.configured_store(),
            )
            .await?;
        }
        Command::Session { command } => {
            run_session_command(command, context.store_backend(), context.configured_store())
                .await?;
        }
        Command::Llm { command } => match command {
            LlmCommand::Complete {
                prompt,
                provider,
                model,
                mock_response,
                api_base_url,
                api_key_env,
                temperature,
                max_output_tokens,
                anthropic_version,
            } => {
                run_llm_complete(LlmCompleteOptions {
                    prompt,
                    provider,
                    model,
                    mock_response,
                    api_base_url,
                    api_key_env,
                    temperature,
                    max_output_tokens,
                    anthropic_version,
                })
                .await?;
            }
        },
        Command::Catalog { command } => {
            run_catalog_command(command).await?;
        }
        Command::Compat { command } => {
            context.require_file_store_backend("compat")?;
            run_compat_command(command).await?;
        }
        Command::Config { command } => match command {
            ConfigCommand::Show => {
                print_json(context.config())?;
            }
        },
        Command::Recover {
            store,
            timeout_seconds,
        } => {
            let store = context.store(store);
            let execution = context.execution(timeout_seconds, 0, 0);
            let stores = RuntimeStores::open(context.store_backend(), store).await?;
            let report = recover_stale_runs(
                stores.run_store.as_ref(),
                &ExecutionPolicy {
                    timeout: Duration::from_secs(execution.timeout_seconds),
                    max_retries: 0,
                    retry_backoff: Duration::ZERO,
                    max_concurrent_runs: 1,
                },
            )
            .await
            .into_diagnostic()?;
            print_json(&report)?;
        }
        Command::Cmd { command } => match command {
            CmdCommand::Create {
                from_run,
                store,
                out,
                description,
                catalog,
                registry,
            } => {
                let sources = context.command_runtime_sources(registry, catalog);
                let report = create_command_from_run(
                    from_run,
                    context.store(store),
                    context.store_backend(),
                    out,
                    description,
                    sources,
                )
                .await?;
                print_json(&report)?;
            }
            CmdCommand::Run {
                command_file,
                catalog,
                registry,
                store,
                tools,
                trace_out,
                timeout_seconds,
                max_retries,
                retry_backoff_ms,
            } => {
                let report = run_command_template(CommandRunOptions {
                    command_file,
                    configured_sources: context.configured_runtime_sources(),
                    source_overrides: RuntimeSources::new(registry, catalog),
                    store: context.store(store),
                    store_backend: context.store_backend(),
                    tools: context.tools(tools),
                    trace_out,
                    timeout_seconds,
                    max_retries,
                    retry_backoff_ms,
                    hooks: context.hooks()?,
                })
                .await?;
                print_json(&report)?;
            }
        },
        Command::Serve {
            registry,
            catalog,
            store,
            tools,
            stdio,
            host,
            port,
            chat,
        } => {
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let stdio = context.stdio(stdio);
            let host = context.host(host);
            let port = context.port(port);
            let hooks = context.hooks()?;
            let server = RuntimeServer::new(
                sources,
                store,
                context.store_backend(),
                context.tools(tools).load().await?,
                hooks,
                context.context_policy(),
                context.default_agent(),
                context.chat(chat),
            )
            .await?;
            if stdio {
                serve_stdio(server).await?;
            } else {
                serve_http(server, host, port).await?;
            }
        }
        Command::Tui {
            registry,
            catalog,
            trace,
            store,
            tools,
            deny_high_risk_tools,
            chat,
            timeout_seconds,
            max_retries,
            retry_backoff_ms,
            mouse_capture,
            once,
        } => {
            context.require_file_store_backend("tui")?;
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let execution = context.execution(timeout_seconds, max_retries, retry_backoff_ms);
            run_tui(TuiOptions {
                runtime_sources: sources,
                trace_path: trace,
                store_path: store,
                tool_overrides: context.tools(tools).load().await?,
                allow_high_risk_tools: !deny_high_risk_tools,
                chat: context.chat(chat),
                timeout_seconds: execution.timeout_seconds,
                max_retries: execution.max_retries,
                retry_backoff_ms: execution.retry_backoff_ms,
                hooks: context.hook_specs(),
                context_policy: context.context_policy(),
                default_agent: context.default_agent(),
                mouse_capture,
                once,
            })
            .await?;
        }
        Command::Eval {
            eval_path,
            store,
            tools,
            update_golden,
            from_run,
            out,
            catalog,
            id,
            golden_trace,
        } => {
            context.require_file_store_backend("eval")?;
            let store = context.eval_store(store);
            let catalog = context.catalog(catalog);
            let result = if eval_path.as_str() == "create" || from_run.is_some() {
                create_eval_from_run(
                    from_run.ok_or_else(|| miette!("--from-run is required"))?,
                    store,
                    out.ok_or_else(|| miette!("--out is required"))?,
                    catalog.ok_or_else(|| miette!("--catalog is required"))?,
                    id,
                    golden_trace,
                )
                .await?
            } else {
                run_eval_path(
                    eval_path,
                    store,
                    context.tools(tools).load().await?,
                    update_golden,
                )
                .await?
            };
            print_json(&result)?;
        }
        Command::DevToolHost => run_dev_tool_host().await?,
        Command::DevMcpServer => run_dev_mcp_server().await?,
        Command::DevScoreHook => run_dev_score_hook().await?,
        Command::ShellToolHost => run_shell_tool_host().await?,
    }
    Ok(())
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
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER));
    filter
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
    let validator = jsonschema::validator_for(&schema)
        .map_err(|e| miette!("failed to compile JSON schema: {e}"))?;
    let errors = validator
        .iter_errors(&instance)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();

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
