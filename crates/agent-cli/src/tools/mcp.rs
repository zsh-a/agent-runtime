use std::process::Stdio;

use agent_core::ToolError;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;

use super::{
    error::{tool_error, tool_error_from_json},
    process::ProcessToolHost,
};

pub(super) async fn call_tool(
    host: &ProcessToolHost,
    name: &str,
    input: Value,
) -> std::result::Result<Value, ToolError> {
    let mut child = TokioCommand::new(&host.command)
        .args(&host.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tool_error("mcp_spawn_failed", e.to_string()))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| tool_error("mcp_stdin_missing", "MCP stdin missing"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| tool_error("mcp_stdout_missing", "MCP stdout missing"))?;
    let mut lines = BufReader::new(stdout).lines();

    write_json_line(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": "initialize",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "agent-runtime", "version": "0.1.0"}
            }
        }),
        "mcp_initialize_write_failed",
    )
    .await?;
    read_json_rpc_response(&mut lines, "initialize", "mcp_initialize_failed").await?;

    write_json_line(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": "tools_call",
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": input,
            }
        }),
        "mcp_tool_call_write_failed",
    )
    .await?;
    let response = read_json_rpc_response(&mut lines, "tools_call", "mcp_tool_call_failed").await?;
    drop(stdin);
    let status = child
        .wait()
        .await
        .map_err(|e| tool_error("mcp_wait_failed", e.to_string()))?;
    if !status.success() {
        return Err(tool_error(
            "mcp_failed",
            format!("MCP server exited with {status}"),
        ));
    }
    Ok(response)
}

async fn write_json_line(
    stdin: &mut tokio::process::ChildStdin,
    value: Value,
    code: &str,
) -> std::result::Result<(), ToolError> {
    let mut encoded =
        serde_json::to_vec(&value).map_err(|e| tool_error("json_encode_failed", e.to_string()))?;
    encoded.push(b'\n');
    stdin
        .write_all(&encoded)
        .await
        .map_err(|e| tool_error(code, e.to_string()))
}

async fn read_json_rpc_response(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: &str,
    code: &str,
) -> std::result::Result<Value, ToolError> {
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|e| tool_error(code, e.to_string()))?
            .ok_or_else(|| tool_error(code, "JSON-RPC peer returned no response"))?;
        let response: Value =
            serde_json::from_str(&line).map_err(|e| tool_error(code, e.to_string()))?;
        if response.get("id").and_then(Value::as_str) != Some(id) {
            continue;
        }
        if let Some(error) = response.get("error") {
            return Err(tool_error_from_json(code, error));
        }
        return response
            .get("result")
            .cloned()
            .ok_or_else(|| tool_error(code, "JSON-RPC response missing result"));
    }
}
