use std::process::Stdio;

use agent_core::ToolError;
use miette::Result;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tracing::{debug, info, warn};

use crate::{
    error::{tool_error, tool_error_from_json},
    mcp,
};

#[derive(Debug, Clone)]
pub(crate) struct ProcessToolHost {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
}

impl ProcessToolHost {
    pub(crate) fn new(command: String, args: Vec<String>) -> Self {
        Self { command, args }
    }

    pub(crate) async fn call(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        let started_at = std::time::Instant::now();
        info!(
            tool_name = name,
            command = %self.command,
            arg_count = self.args.len(),
            "starting process tool host call",
        );
        let mut child = TokioCommand::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| tool_error("tool_host_spawn_failed", e.to_string()))?;
        let child_id = child.id().unwrap_or_default();
        debug!(
            tool_name = name,
            command = %self.command,
            child_id,
            "process tool host spawned",
        );
        let stderr_task = child.stderr.take().map(spawn_stderr_reader);

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| tool_error("tool_host_stdin_missing", "tool host stdin missing"))?;
        let request = json!({
            "jsonrpc": "2.0",
            "id": "tool_call",
            "method": "tool.call",
            "params": {
                "name": name,
                "input": input,
            }
        });
        let mut encoded = serde_json::to_vec(&request)
            .map_err(|e| tool_error("tool_host_encode_failed", e.to_string()))?;
        encoded.push(b'\n');
        stdin
            .write_all(&encoded)
            .await
            .map_err(|e| tool_error("tool_host_write_failed", e.to_string()))?;
        drop(stdin);

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| tool_error("tool_host_stdout_missing", "tool host stdout missing"))?;
        let mut lines = BufReader::new(stdout).lines();
        let line = lines
            .next_line()
            .await
            .map_err(|e| tool_error("tool_host_read_failed", e.to_string()))?
            .ok_or_else(|| {
                tool_error("tool_host_empty_response", "tool host returned no response")
            })?;
        let response: Value = serde_json::from_str(&line)
            .map_err(|e| tool_error("tool_host_decode_failed", e.to_string()))?;

        let status = child
            .wait()
            .await
            .map_err(|e| tool_error("tool_host_wait_failed", e.to_string()))?;
        let stderr = finish_stderr_reader(stderr_task).await;
        if !stderr.trim().is_empty() {
            debug!(
                tool_name = name,
                command = %self.command,
                child_id,
                stderr_bytes = stderr.len(),
                stderr_preview = %truncate_for_log(&stderr),
                "process tool host wrote stderr",
            );
        }
        if !status.success() {
            warn!(
                tool_name = name,
                command = %self.command,
                child_id,
                status = %status,
                duration_ms = started_at.elapsed().as_millis(),
                "process tool host failed",
            );
            return Err(tool_error(
                "tool_host_failed",
                format!("tool host exited with {status}"),
            ));
        }
        if let Some(error) = response.get("error") {
            warn!(
                tool_name = name,
                command = %self.command,
                child_id,
                error = %truncate_for_log(&error.to_string()),
                duration_ms = started_at.elapsed().as_millis(),
                "process tool host returned JSON-RPC error",
            );
            return Err(tool_error_from_json("tool_host_error", error));
        }
        let result = response.get("result").cloned().ok_or_else(|| {
            tool_error(
                "tool_host_missing_result",
                "tool host response missing result",
            )
        })?;
        info!(
            tool_name = name,
            command = %self.command,
            child_id,
            status = %status,
            duration_ms = started_at.elapsed().as_millis(),
            "process tool host call completed",
        );
        Ok(result)
    }

    pub(crate) async fn call_mcp_tool(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        mcp::call_tool(self, name, input).await
    }
}

pub(crate) fn process_tool_host(args: Vec<String>) -> Result<Option<ProcessToolHost>> {
    let Some((command, rest)) = args.split_first() else {
        return Ok(None);
    };
    Ok(Some(ProcessToolHost::new(command.clone(), rest.to_vec())))
}

type StderrTask = tokio::task::JoinHandle<std::io::Result<String>>;

fn spawn_stderr_reader(stderr: tokio::process::ChildStderr) -> StderrTask {
    tokio::spawn(async move {
        let mut output = String::new();
        BufReader::new(stderr).read_to_string(&mut output).await?;
        Ok(output)
    })
}

async fn finish_stderr_reader(task: Option<StderrTask>) -> String {
    let Some(task) = task else {
        return String::new();
    };
    task.await
        .ok()
        .and_then(std::result::Result::ok)
        .unwrap_or_default()
}

fn truncate_for_log(value: &str) -> String {
    const MAX_CHARS: usize = 500;
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
