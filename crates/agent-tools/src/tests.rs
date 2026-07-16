use super::*;

#[tokio::test]
async fn tool_overrides_validates_declared_input_schema() {
    let overrides = overrides_with_mock(
        strict_tool_spec(Some(json!({
            "type": "object",
            "required": ["account_id"],
            "properties": {
                "account_id": {"type": "string"}
            },
            "additionalProperties": false
        }))),
        json!({"ok": true}),
    );

    let error = overrides
        .call_tool("strict.lookup", json!({"account_id": 7}))
        .await
        .expect_err("input schema violation fails");

    assert_eq!(error.record.code, "tool_input_schema_validation_failed");
    assert_eq!(error.record.details["tool_name"], "strict.lookup");
    assert_eq!(error.record.details["phase"], "input");
}

#[tokio::test]
async fn tool_overrides_validates_declared_output_schema() {
    let overrides = overrides_with_mock(
        strict_tool_spec(Some(json!({
            "type": "object",
            "required": ["ok"],
            "properties": {
                "ok": {"type": "boolean"}
            },
            "additionalProperties": false
        }))),
        json!({"ok": "yes"}),
    );

    let error = overrides
        .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
        .await
        .expect_err("output schema violation fails");

    assert_eq!(error.record.code, "tool_output_schema_validation_failed");
    assert_eq!(error.record.details["tool_name"], "strict.lookup");
    assert_eq!(error.record.details["phase"], "output");
}

#[tokio::test]
async fn tool_overrides_accepts_values_that_match_declared_schemas() {
    let overrides = overrides_with_mock(
        strict_tool_spec(Some(json!({
            "type": "object",
            "required": ["ok"],
            "properties": {
                "ok": {"type": "boolean"}
            },
            "additionalProperties": false
        }))),
        json!({"ok": true}),
    );

    let output = overrides
        .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
        .await
        .expect("schema-valid tool call succeeds");

    assert_eq!(output, json!({"ok": true}));
}

#[tokio::test]
async fn builtin_tools_are_validated_against_their_specs() {
    let overrides = ToolOverrides::default();

    let error = overrides
        .call_tool("echo", json!("not an object"))
        .await
        .expect_err("builtin echo requires object input");

    assert_eq!(error.record.code, "tool_input_schema_validation_failed");
}

#[tokio::test]
async fn tool_source_timeout_is_enforced() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = dir.path().join("tool-source.json");
    write_tool_source_manifest(
        &manifest,
        json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "slow-source",
                "command": "sh",
                "args": ["-c", "sleep 1; printf '%s\\n' '{\"result\":{\"ok\":true}}'"],
                "timeout_ms": 10,
                "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
            }]
        }),
    );
    let overrides = tool_overrides(
        Vec::new(),
        Vec::new(),
        vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
    )
    .await
    .expect("tool source loads");

    let error = overrides
        .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
        .await
        .expect_err("slow source times out");

    assert_eq!(error.record.code, "tool_source_timeout");
    assert!(error.record.retryable);
    assert_eq!(error.record.details["tool_name"], "strict.lookup");
}

#[tokio::test]
async fn tool_source_retries_retryable_process_errors() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = dir.path().join("tool-source.json");
    let flag = dir.path().join("failed-once");
    write_tool_source_manifest(
        &manifest,
        json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "retry-source",
                "command": "sh",
                "args": [
                    "-c",
                    "read _; if [ ! -f \"$1\" ]; then touch \"$1\"; printf '%s\\n' '{\"error\":{\"code\":-32000,\"message\":\"try again\",\"data\":{\"retryable\":true}}}'; else printf '%s\\n' '{\"result\":{\"ok\":true}}'; fi",
                    "retry-source",
                    flag.to_str().expect("utf8 flag path")
                ],
                "max_retries": 1,
                "retry_backoff_ms": 0,
                "tools": [strict_tool_spec(Some(json!({
                    "type": "object",
                    "required": ["ok"],
                    "properties": {"ok": {"type": "boolean"}},
                    "additionalProperties": false
                })))]
            }]
        }),
    );
    let overrides = tool_overrides(
        Vec::new(),
        Vec::new(),
        vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
    )
    .await
    .expect("tool source loads");

    let output = overrides
        .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
        .await
        .expect("retryable source succeeds");

    assert_eq!(output, json!({"ok": true}));
    assert!(flag.exists());
}

#[tokio::test]
async fn tool_source_rejects_output_over_source_limit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = dir.path().join("tool-source.json");
    write_tool_source_manifest(
        &manifest,
        json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "bounded-source",
                "command": "sh",
                "args": ["-c", "read _; printf '%s\\n' '{\"result\":{\"blob\":\"abcdefghijklmnopqrstuvwxyz\"}}'"],
                "max_output_bytes": 16,
                "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
            }]
        }),
    );
    let overrides = tool_overrides(
        Vec::new(),
        Vec::new(),
        vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
    )
    .await
    .expect("tool source loads");

    let error = overrides
        .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
        .await
        .expect_err("oversized source output fails");

    assert_eq!(error.record.code, "tool_source_output_too_large");
    assert!(!error.record.retryable);
    assert_eq!(error.record.details["tool_name"], "strict.lookup");
    assert_eq!(error.record.details["max_output_bytes"], 16);
    assert!(error.record.details["output_bytes"].as_u64().unwrap() > 16);
}

#[tokio::test]
async fn tool_source_rejects_zero_output_limit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = dir.path().join("tool-source.json");
    write_tool_source_manifest(
        &manifest,
        json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "bad-limit-source",
                "command": "sh",
                "args": ["-c", "printf '%s\\n' '{\"result\":{\"ok\":true}}'"],
                "max_output_bytes": 0,
                "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
            }]
        }),
    );

    let error = tool_overrides(
        Vec::new(),
        Vec::new(),
        vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
    )
    .await
    .expect_err("zero output limit is invalid");

    assert!(
        error
            .to_string()
            .contains("max_output_bytes must be greater than zero")
    );
}

#[tokio::test]
async fn process_tool_source_can_set_cwd_and_clear_environment() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workdir = dir.path().join("tool-workdir");
    std::fs::create_dir_all(&workdir).expect("workdir creates");
    std::fs::write(workdir.join("marker"), "ok").expect("marker writes");
    let manifest = dir.path().join("tool-source.json");
    write_tool_source_manifest(
        &manifest,
        json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "bounded-process-source",
                "command": "/bin/sh",
                "args": [
                    "-c",
                    "read _; if [ -f marker ] && [ \"$VISIBLE\" = allowed ] && [ -z \"${HOME:-}\" ]; then printf '%s\\n' '{\"result\":{\"cwd_ok\":true,\"env_ok\":true,\"home_hidden\":true}}'; else printf '%s\\n' '{\"result\":{\"cwd_ok\":false,\"env_ok\":false,\"home_hidden\":false}}'; fi"
                ],
                "cwd": workdir.to_str().expect("utf8 workdir"),
                "inherit_env": false,
                "env": {"VISIBLE": "allowed"},
                "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
            }]
        }),
    );
    let overrides = tool_overrides(
        Vec::new(),
        Vec::new(),
        vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
    )
    .await
    .expect("tool source loads");

    let output = overrides
        .call_tool("strict.lookup", json!({"account_id": "acct_1"}))
        .await
        .expect("bounded process source succeeds");

    assert_eq!(output["cwd_ok"], true);
    assert_eq!(output["env_ok"], true);
    assert_eq!(output["home_hidden"], true);
}

#[tokio::test]
async fn process_tool_source_rejects_missing_env_placeholder() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = dir.path().join("tool-source.json");
    write_tool_source_manifest(
        &manifest,
        json!({
            "version": "tool_source.v1",
            "sources": [{
                "id": "missing-env-source",
                "command": "/bin/sh",
                "args": ["-c", "printf '%s\\n' '{\"result\":{\"ok\":true}}'"],
                "env": {
                    "TOKEN": "Bearer ${env:AGENT_RUNTIME_TEST_MISSING_PROCESS_ENV_TOKEN}"
                },
                "tools": [strict_tool_spec(Some(json!({"type": "object"})))]
            }]
        }),
    );

    let error = tool_overrides(
        Vec::new(),
        Vec::new(),
        vec![Utf8PathBuf::from_path_buf(manifest).expect("utf8 path")],
    )
    .await
    .expect_err("missing env placeholder fails");

    assert!(
            error.to_string().contains(
                "process env 'TOKEN' references missing environment variable 'AGENT_RUNTIME_TEST_MISSING_PROCESS_ENV_TOKEN'"
            )
        );
}

fn overrides_with_mock(spec: ToolSpec, output: Value) -> ToolOverrides {
    ToolOverrides {
        mock_tools: BTreeMap::from([(spec.name.clone(), output)]),
        source_specs: vec![spec],
        source_tools: BTreeMap::new(),
        tool_host: None,
    }
}

fn strict_tool_spec(output_schema: Option<Value>) -> ToolSpec {
    ToolSpec {
        name: "strict.lookup".to_owned(),
        description: "Strict lookup test tool".to_owned(),
        input_schema: json!({
            "type": "object",
            "required": ["account_id"],
            "properties": {
                "account_id": {"type": "string"}
            },
            "additionalProperties": false
        }),
        output_schema,
        risk: ToolRisk::ReadOnly,
        metadata: json!({}),
    }
}

fn write_tool_source_manifest(path: &std::path::Path, value: Value) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("manifest encodes"),
    )
    .expect("manifest writes");
}
