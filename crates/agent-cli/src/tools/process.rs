use std::process::Stdio;

use agent_core::ToolError;
use miette::Result;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;

use super::{
    error::{tool_error, tool_error_from_json},
    mcp,
};

#[derive(Debug, Clone)]
pub(crate) struct ProcessToolHost {
    pub(super) command: String,
    pub(super) args: Vec<String>,
}

impl ProcessToolHost {
    pub(super) fn new(command: String, args: Vec<String>) -> Self {
        Self { command, args }
    }

    pub(crate) async fn call(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        let mut child = TokioCommand::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| tool_error("tool_host_spawn_failed", e.to_string()))?;

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
        if !status.success() {
            return Err(tool_error(
                "tool_host_failed",
                format!("tool host exited with {status}"),
            ));
        }
        if let Some(error) = response.get("error") {
            return Err(tool_error_from_json("tool_host_error", error));
        }
        response.get("result").cloned().ok_or_else(|| {
            tool_error(
                "tool_host_missing_result",
                "tool host response missing result",
            )
        })
    }

    pub(super) async fn call_mcp_tool(
        &self,
        name: &str,
        input: Value,
    ) -> std::result::Result<Value, ToolError> {
        mcp::call_tool(self, name, input).await
    }
}

pub(super) fn process_tool_host(args: Vec<String>) -> Result<Option<ProcessToolHost>> {
    let Some((command, rest)) = args.split_first() else {
        return Ok(None);
    };
    Ok(Some(ProcessToolHost::new(command.clone(), rest.to_vec())))
}
