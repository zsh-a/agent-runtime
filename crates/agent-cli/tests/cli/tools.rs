use super::*;

#[test]
fn tool_cli_lists_catalog_tools_and_calls_process_tool_host() {
    let tools = agent_cmd()
        .args([
            "tool",
            "list",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let tools: Value = serde_json::from_slice(&tools).expect("tools are JSON");
    assert_eq!(tools[0]["name"], "propose_fake");

    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "propose_fake",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input-json",
            r#"{"value":42}"#,
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["host"], "dev-tool-host");
    assert_eq!(output["tool"], "propose_fake");
    assert_eq!(output["input"]["value"], 42);
}

#[test]
fn tool_cli_can_call_inline_mock_tool() {
    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "propose_fake",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input-json",
            r#"{"value":42}"#,
            "--mock-tool",
            r#"propose_fake={"mocked":true,"value":123}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["mocked"], true);
    assert_eq!(output["value"], 123);
}

#[test]
fn tool_cli_validates_catalog_tool_input_schema_for_inline_mock() {
    let stderr = agent_cmd()
        .args([
            "tool",
            "call",
            "propose_fake",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--input-json",
            r#""not an object""#,
            "--mock-tool",
            r#"propose_fake={"mocked":true}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(stderr).expect("stderr is utf8");

    assert!(stderr.contains("tool 'propose_fake' input failed schema validation"));
}

#[test]
fn tool_cli_rejects_tools_missing_from_catalog() {
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    agent_cmd()
        .args([
            "tool",
            "call",
            "missing_tool",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--tool-host",
            agent_bin.to_str().expect("utf8 agent bin"),
            "dev-tool-host",
        ])
        .assert()
        .failure();
}

#[test]
fn tool_cli_lists_and_calls_tool_source_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("tool-source.json");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "dev",
                "command": agent_bin.to_str().expect("utf8 agent bin"),
                "args": ["dev-tool-host"],
                "tools": [{
                    "name": "sourced_echo",
                    "description": "Echo through a configured tool source.",
                    "input_schema": {"type": "object"},
                    "output_schema": {"type": "object"},
                    "risk": "read_only",
                    "metadata": {"source": "test"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let tools = agent_cmd()
        .args([
            "tool",
            "list",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let tools: Value = serde_json::from_slice(&tools).expect("tools are JSON");
    assert!(
        tools
            .as_array()
            .expect("tools array")
            .iter()
            .any(|tool| tool["name"] == "sourced_echo")
    );

    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "sourced_echo",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"value":77}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["host"], "dev-tool-host");
    assert_eq!(output["tool"], "sourced_echo");
    assert_eq!(output["input"]["value"], 77);
}

#[test]
fn tool_cli_validates_tool_source_input_schema() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("tool-source.json");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "dev",
                "command": agent_bin.to_str().expect("utf8 agent bin"),
                "args": ["dev-tool-host"],
                "tools": [{
                    "name": "strict_echo",
                    "description": "Echo through a configured tool source with strict input.",
                    "input_schema": {
                        "type": "object",
                        "required": ["value"],
                        "properties": {
                            "value": {"type": "string"}
                        },
                        "additionalProperties": false
                    },
                    "output_schema": {"type": "object"},
                    "risk": "read_only",
                    "metadata": {"source": "test"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let stderr = agent_cmd()
        .args([
            "tool",
            "call",
            "strict_echo",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"value":77}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(stderr).expect("stderr is utf8");

    assert!(stderr.contains("tool 'strict_echo' input failed schema validation"));
}

#[test]
fn tool_cli_calls_mcp_stdio_tool_source_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("mcp-tool-source.json");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "mcp-dev",
                "protocol": "mcp_stdio",
                "command": agent_bin.to_str().expect("utf8 agent bin"),
                "args": ["dev-mcp-server"],
                "tools": [{
                    "name": "mcp_echo",
                    "description": "Echo through a dev MCP server.",
                    "input_schema": {"type": "object"},
                    "output_schema": {"type": "object"},
                    "risk": "read_only",
                    "metadata": {"source": "mcp-dev", "protocol": "mcp_stdio"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "mcp_echo",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"value":91}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["structuredContent"]["host"], "dev-mcp-server");
    assert_eq!(output["structuredContent"]["tool"], "mcp_echo");
    assert_eq!(output["structuredContent"]["input"]["value"], 91);
    assert_eq!(output["isError"], false);
}

#[test]
fn tool_cli_calls_http_json_tool_source_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("http-tool-source.json");
    let (port, request_handle) = spawn_http_tool_source_server();
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "http-dev",
                "protocol": "http_json",
                "endpoint": format!("http://127.0.0.1:{port}/tools/call"),
                "headers": {"x-agent-runtime-source": "http-dev"},
                "tools": [{
                    "name": "http_echo",
                    "description": "Echo through a dev HTTP endpoint.",
                    "input_schema": {"type": "object"},
                    "output_schema": {"type": "object"},
                    "risk": "read_only",
                    "metadata": {"source": "http-dev", "protocol": "http_json"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "http_echo",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"value":64}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["host"], "http-tool-source");
    assert_eq!(output["tool"], "http_echo");
    assert_eq!(output["input"]["value"], 64);

    let request = request_handle.join().expect("request captured");
    assert!(request.starts_with("POST /tools/call HTTP/1.1"));
    assert!(request.contains("x-agent-runtime-source: http-dev"));
    assert!(request.contains(r#""protocol_version":"agent.v1""#));
    assert!(request.contains(r#""method":"tool.call""#));
    assert!(request.contains(r#""tool":"http_echo""#));
    assert!(request.contains(r#""value":64"#));
}

#[test]
fn tool_cli_calls_shell_tool_source_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_path = dir.path().join("shell-tool-source.json");
    let agent_bin = assert_cmd::cargo::cargo_bin("agent");
    std::fs::write(
        &source_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "local-shell",
                "protocol": "jsonl_tool_call",
                "command": agent_bin.to_str().expect("utf8 agent bin"),
                "args": ["shell-tool-host"],
                "tools": [{
                    "name": "shell.exec",
                    "description": "Execute a shell command in a bounded local workspace directory.",
                    "input_schema": {
                        "type": "object",
                        "required": ["command"],
                        "properties": {
                            "command": {"type": "string", "minLength": 1},
                            "cwd": {"type": "string"},
                            "timeout_ms": {"type": "integer", "minimum": 1},
                            "max_output_bytes": {"type": "integer", "minimum": 1},
                            "env": {
                                "type": "object",
                                "additionalProperties": {"type": "string"}
                            }
                        },
                        "additionalProperties": false
                    },
                    "output_schema": {"type": "object"},
                    "risk": "high",
                    "metadata": {"source": "local-shell", "protocol": "jsonl_tool_call"}
                }]
            }]
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");

    let output = agent_cmd()
        .args([
            "tool",
            "call",
            "shell.exec",
            "--tool-source",
            source_path.to_str().expect("utf8 source path"),
            "--input-json",
            r#"{"command":"printf shell-ok","timeout_ms":5000}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output: Value = serde_json::from_slice(&output).expect("tool output is JSON");
    assert_eq!(output["exit_code"], 0);
    assert_eq!(output["timed_out"], false);
    assert_eq!(output["stdout"], "shell-ok");
    assert_eq!(output["stderr"], "");
}
