use super::*;

#[test]
fn llm_cli_completes_with_mock_provider() {
    let output = agent_cmd()
        .args([
            "llm",
            "complete",
            "--prompt",
            "hello",
            "--model",
            "mock-fast",
            "--mock-response",
            "mocked answer",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: Value = serde_json::from_slice(&output).expect("llm response is JSON");
    assert_eq!(response["protocol_version"], "agent.v1");
    assert_eq!(response["provider"], "mock");
    assert_eq!(response["model"], "mock-fast");
    assert_eq!(response["content"], "mocked answer");
    assert_eq!(response["finish_reason"], "stop");
    assert!(
        response["usage"]["total_tokens"]
            .as_u64()
            .expect("usage count")
            > 0
    );
}

#[test]
fn llm_cli_completes_with_openai_compatible_provider() {
    let (port, request_handle) = spawn_openai_compatible_server();
    let output = agent_cmd()
        .env("TEST_OPENAI_API_KEY", "secret-key")
        .args([
            "llm",
            "complete",
            "--provider",
            "openai-compatible",
            "--prompt",
            "hello",
            "--model",
            "gpt-test",
            "--api-base-url",
            &format!("http://127.0.0.1:{port}"),
            "--api-key-env",
            "TEST_OPENAI_API_KEY",
            "--temperature",
            "0.2",
            "--max-output-tokens",
            "64",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: Value = serde_json::from_slice(&output).expect("llm response is JSON");
    assert_eq!(response["provider"], "openai-compatible");
    assert_eq!(response["model"], "gpt-test");
    assert_eq!(response["content"], "network answer");
    assert_eq!(response["usage"]["total_tokens"], 6);

    let request = request_handle.join().expect("request captured");
    assert!(request.starts_with("POST /chat/completions HTTP/1.1"));
    assert!(request.contains("authorization: Bearer secret-key"));
    assert!(request.contains(r#""model":"gpt-test""#));
    assert!(request.contains(r#""max_tokens":64"#));
}

#[test]
fn llm_cli_completes_with_anthropic_provider() {
    let (port, request_handle) = spawn_anthropic_server();
    let output = agent_cmd()
        .env("TEST_ANTHROPIC_API_KEY", "anthropic-key")
        .args([
            "llm",
            "complete",
            "--provider",
            "anthropic",
            "--prompt",
            "hello",
            "--model",
            "claude-test",
            "--api-base-url",
            &format!("http://127.0.0.1:{port}"),
            "--api-key-env",
            "TEST_ANTHROPIC_API_KEY",
            "--anthropic-version",
            "2023-06-01",
            "--max-output-tokens",
            "64",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: Value = serde_json::from_slice(&output).expect("llm response is JSON");
    assert_eq!(response["provider"], "anthropic");
    assert_eq!(response["model"], "claude-test");
    assert_eq!(response["content"], "anthropic answer");
    assert_eq!(response["usage"]["total_tokens"], 7);

    let request = request_handle.join().expect("request captured");
    assert!(request.starts_with("POST /messages HTTP/1.1"));
    assert!(request.contains("x-api-key: anthropic-key"));
    assert!(request.contains("anthropic-version: 2023-06-01"));
    assert!(request.contains(r#""model":"claude-test""#));
    assert!(request.contains(r#""max_tokens":64"#));
}

#[test]
fn llm_cli_completes_with_ollama_provider() {
    let (port, request_handle) = spawn_ollama_server();
    let output = agent_cmd()
        .args([
            "llm",
            "complete",
            "--provider",
            "ollama",
            "--prompt",
            "hello",
            "--model",
            "llama-test",
            "--api-base-url",
            &format!("http://127.0.0.1:{port}"),
            "--temperature",
            "0.3",
            "--max-output-tokens",
            "32",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: Value = serde_json::from_slice(&output).expect("llm response is JSON");
    assert_eq!(response["provider"], "ollama");
    assert_eq!(response["model"], "llama-test");
    assert_eq!(response["content"], "local answer");
    assert_eq!(response["usage"]["total_tokens"], 10);

    let request = request_handle.join().expect("request captured");
    assert!(request.starts_with("POST /api/chat HTTP/1.1"));
    assert!(request.contains(r#""model":"llama-test""#));
    assert!(request.contains(r#""stream":false"#));
    assert!(request.contains(r#""num_predict":32"#));
}
