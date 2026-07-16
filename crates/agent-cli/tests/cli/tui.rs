use super::*;

#[test]
fn tui_once_renders_catalog_and_trace_snapshot() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("tui-store");
    let output = agent_cmd()
        .args([
            "tui",
            "--catalog",
            "../../fixtures/contracts/catalog.valid.json",
            "--trace",
            "../../fixtures/contracts/trace.valid.json",
            "--tool-source",
            "../../fixtures/contracts/tool-source.example.json",
            "--store",
            store.to_str().expect("utf8 store path"),
            "--once",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).expect("stdout is utf8");
    assert!(output.contains("Agent Runtime"));
    assert!(output.contains("Chat"));
    assert!(output.contains("[Details]  Timeline"));
    assert!(output.contains("Message"));
    assert!(output.contains("agent  echo_agent@0.1.0"));
    assert!(output.contains("ai_chat"));
}

#[test]
fn tui_once_reads_unified_runtime_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("configured-tui-store");
    let config_path = dir.path().join("agent-runtime.toml");
    let registry = std::path::Path::new("../../examples/agents.yaml")
        .canonicalize()
        .expect("registry path");
    let catalog = std::path::Path::new("../../fixtures/contracts/catalog.valid.json")
        .canonicalize()
        .expect("catalog path");
    let tool_source = std::path::Path::new("../../fixtures/contracts/tool-source.example.json")
        .canonicalize()
        .expect("tool source path");
    let trace = std::path::Path::new("../../fixtures/contracts/trace.valid.json")
        .canonicalize()
        .expect("trace path");
    std::fs::write(
        &config_path,
        format!(
            r#"[runtime]
store = "{}"
default_agent = "ai_chat"
timeout_seconds = 5

[runtime.sources]
registry = "{}"
catalog = "{}"

[runtime.tools]
sources = ["{}"]

[runtime.llm]
provider = "mock"
model = "configured-model"
max_tool_rounds = 2
"#,
            store.display(),
            registry.display(),
            catalog.display(),
            tool_source.display()
        ),
    )
    .expect("config written");

    let output = agent_cmd()
        .args([
            "--config",
            config_path.to_str().expect("utf8 config path"),
            "tui",
            "--trace",
            trace.to_str().expect("utf8 trace path"),
            "--once",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).expect("stdout is utf8");
    assert!(output.contains("mock / configured-model"));
    assert!(output.contains("ai_chat"));
    assert!(output.contains("catalog  1 agents / 1 tools"));
    assert!(output.contains("tools  3 | high 0 | blocked 0"));
}
