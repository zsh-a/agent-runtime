use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use miette::{IntoDiagnostic, Result};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command as TokioCommand,
};

use crate::stdio_protocol::{StdioResponse, stdio_error, stdio_result};

const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
struct ToolHostRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    input: Value,
}

#[derive(Debug, Deserialize)]
struct ShellExecInput {
    command: String,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_output_bytes: Option<usize>,
    #[serde(default)]
    env: Map<String, Value>,
}

pub(crate) async fn run_shell_tool_host() -> Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();
    let base_cwd = std::env::current_dir().into_diagnostic()?;
    while let Some(line) = lines.next_line().await.into_diagnostic()? {
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_shell_tool_host_line(&line, &base_cwd).await;
        let encoded = serde_json::to_vec(&response).into_diagnostic()?;
        stdout.write_all(&encoded).await.into_diagnostic()?;
        stdout.write_all(b"\n").await.into_diagnostic()?;
        stdout.flush().await.into_diagnostic()?;
    }
    Ok(())
}

async fn handle_shell_tool_host_line(line: &str, base_cwd: &Path) -> StdioResponse {
    let request = match serde_json::from_str::<ToolHostRequest>(line) {
        Ok(request) => request,
        Err(err) => return stdio_error(None, -32700, format!("parse error: {err}")),
    };
    if request.method != "tool.call" {
        return stdio_error(request.id, -32601, "method not found");
    }
    let params = match serde_json::from_value::<ToolCallParams>(request.params) {
        Ok(params) => params,
        Err(err) => return stdio_error(request.id, -32602, format!("invalid params: {err}")),
    };
    if params.name != "shell.exec" {
        return stdio_error(
            request.id,
            -32601,
            format!("unknown shell tool '{}'", params.name),
        );
    }
    let input = match serde_json::from_value::<ShellExecInput>(params.input) {
        Ok(input) => input,
        Err(err) => return stdio_error(request.id, -32602, format!("invalid shell input: {err}")),
    };
    match run_shell_exec(input, base_cwd).await {
        Ok(output) => stdio_result(request.id, output),
        Err(message) => stdio_error(request.id, -32000, message),
    }
}

async fn run_shell_exec(
    input: ShellExecInput,
    base_cwd: &Path,
) -> std::result::Result<Value, String> {
    let command = input.command.trim();
    if command.is_empty() {
        return Err("shell.exec command cannot be empty".to_owned());
    }
    let cwd = resolve_cwd(base_cwd, input.cwd.as_deref())?;
    let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).max(1));
    let max_output_bytes = input
        .max_output_bytes
        .unwrap_or(DEFAULT_MAX_OUTPUT_BYTES)
        .max(1);
    let started_at = std::time::Instant::now();
    let shell = std::env::var("AGENT_SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());
    let child = TokioCommand::new(&shell)
        .arg("-lc")
        .arg(command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .envs(string_env(input.env)?)
        .spawn()
        .map_err(|err| format!("failed to spawn shell: {err}"))?;

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(format!("failed to wait for shell: {err}")),
        Err(_) => {
            return Ok(json!({
                "command": command,
                "cwd": cwd,
                "exit_code": null,
                "timed_out": true,
                "duration_ms": started_at.elapsed().as_millis(),
                "stdout": "",
                "stderr": format!("shell command timed out after {}ms", timeout.as_millis()),
                "stdout_truncated": false,
                "stderr_truncated": false,
            }));
        }
    };
    let stdout = truncate_utf8(&output.stdout, max_output_bytes);
    let stderr = truncate_utf8(&output.stderr, max_output_bytes);
    Ok(json!({
        "command": command,
        "cwd": cwd,
        "exit_code": output.status.code(),
        "timed_out": false,
        "duration_ms": started_at.elapsed().as_millis(),
        "stdout": stdout.text,
        "stderr": stderr.text,
        "stdout_truncated": stdout.truncated,
        "stderr_truncated": stderr.truncated,
    }))
}

fn resolve_cwd(base_cwd: &Path, cwd: Option<&Path>) -> std::result::Result<PathBuf, String> {
    let requested = match cwd {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => base_cwd.join(path),
        None => base_cwd.to_path_buf(),
    };
    let resolved = requested
        .canonicalize()
        .map_err(|err| format!("invalid cwd '{}': {err}", requested.display()))?;
    let base = base_cwd
        .canonicalize()
        .map_err(|err| format!("invalid base cwd '{}': {err}", base_cwd.display()))?;
    if !resolved.starts_with(&base) {
        return Err(format!(
            "cwd '{}' is outside shell host base '{}'",
            resolved.display(),
            base.display()
        ));
    }
    Ok(resolved)
}

fn string_env(env: Map<String, Value>) -> std::result::Result<Vec<(String, String)>, String> {
    env.into_iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .ok_or_else(|| format!("env value for '{key}' must be a string"))?;
            Ok((key, value.to_owned()))
        })
        .collect()
}

struct TruncatedText {
    text: String,
    truncated: bool,
}

fn truncate_utf8(bytes: &[u8], max_bytes: usize) -> TruncatedText {
    let truncated = bytes.len() > max_bytes;
    let bytes = if truncated {
        &bytes[..max_bytes]
    } else {
        bytes
    };
    TruncatedText {
        text: String::from_utf8_lossy(bytes).to_string(),
        truncated,
    }
}
