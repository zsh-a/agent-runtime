use super::*;

#[derive(Debug, Parser)]
#[command(name = "agent")]
#[command(about = "Schema-first Rust agent runtime CLI")]
pub(crate) struct Cli {
    #[arg(long, env = "AGENT_RUNTIME_CONFIG")]
    pub(crate) config: Option<Utf8PathBuf>,
    #[arg(long, env = "AGENT_RUNTIME_PROFILE")]
    pub(crate) profile: Option<String>,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Clone, Default, Args)]
pub(crate) struct ToolCliArgs {
    #[arg(
        long = "tool-host",
        num_args = 1..,
        value_name = "COMMAND"
    )]
    pub(crate) tool_host: Vec<String>,
    #[arg(long = "mock-tool", value_name = "NAME=JSON_OR_@PATH")]
    pub(crate) mock_tool: Vec<String>,
    #[arg(long = "tool-source", value_name = "PATH")]
    pub(crate) tool_source: Vec<Utf8PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub(crate) struct ChatCliArgs {
    #[arg(long, env = "AGENT_LLM_PROVIDER", default_value = "mock")]
    pub(crate) provider: String,
    #[arg(long, default_value = "mock-model")]
    pub(crate) model: String,
    #[arg(long, default_value = "mock response")]
    pub(crate) mock_response: String,
    #[arg(long, env = "OPENAI_BASE_URL")]
    pub(crate) api_base_url: Option<String>,
    #[arg(long, default_value = "OPENAI_API_KEY")]
    pub(crate) api_key_env: String,
    #[arg(long, default_value = "2023-06-01")]
    pub(crate) anthropic_version: String,
    #[arg(long)]
    pub(crate) temperature: Option<f32>,
    #[arg(long)]
    pub(crate) max_output_tokens: Option<u32>,
    #[arg(long, default_value_t = DEFAULT_MAX_TOOL_ROUNDS)]
    pub(crate) max_tool_rounds: u32,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
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
        #[arg(long, value_enum, default_value_t = ReplayMode::View)]
        mode: ReplayMode,
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
pub(crate) enum ConfigCommand {
    Show,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CmdCommand {
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
pub(crate) enum DebugBundleCommand {
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
pub(crate) enum MetricsCommand {
    Summary {
        #[arg(long, default_value = DEFAULT_STORE)]
        store: Utf8PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorkflowCommand {
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
pub(crate) enum TraceCommand {
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
pub(crate) enum LlmCommand {
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
