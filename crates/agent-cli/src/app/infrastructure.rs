use std::{fs, io, sync::Mutex};

use camino::Utf8PathBuf;
use miette::{IntoDiagnostic, Result, miette};
use serde::Serialize;

use crate::trace_store::read_json;

const DEFAULT_LOG_FILTER: &str =
    "warn,agent_cli=info,agent_runtime=info,agent_chat=info,agent_llm=info";
pub(super) const TUI_LOG_FILE: &str = "tui.log";

pub(super) enum LogMode {
    Stderr,
    File(Utf8PathBuf),
}

pub(super) fn init_logging(mode: LogMode) {
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
pub(super) struct ValidationReport {
    pub(super) schema: String,
    pub(super) instance: String,
    pub(super) valid: bool,
    pub(super) errors: Vec<String>,
}

pub(super) async fn validate_json(
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
