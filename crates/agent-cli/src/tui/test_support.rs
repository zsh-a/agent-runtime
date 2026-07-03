use camino::{Utf8Path, Utf8PathBuf};

use crate::{chat::ChatLlmOptions, tools::ToolOverrides};

use super::data::{TuiOptions, TuiState};

pub(super) fn temp_store_path(dir: &tempfile::TempDir) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(dir.path().join("store")).expect("temp path should be utf8")
}

pub(super) fn test_chat_options(response: &str) -> ChatLlmOptions {
    ChatLlmOptions {
        provider: "mock".to_owned(),
        model: "mock-model".to_owned(),
        mock_response: response.to_owned(),
        api_base_url: None,
        api_key_env: "OPENAI_API_KEY".to_owned(),
        anthropic_version: "2023-06-01".to_owned(),
        temperature: None,
        max_output_tokens: None,
        max_tool_rounds: 4,
    }
}

pub(super) fn test_options(
    dir: &tempfile::TempDir,
    response: &str,
    allow_high_risk_tools: bool,
) -> TuiOptions {
    TuiOptions {
        catalog_path: None,
        trace_path: None,
        store_path: temp_store_path(dir),
        registry_path: Utf8PathBuf::from("../../examples/agents.yaml"),
        tool_overrides: ToolOverrides::default(),
        allow_high_risk_tools,
        chat: test_chat_options(response),
        timeout_seconds: 60,
        max_retries: 0,
        retry_backoff_ms: 0,
        hooks: Vec::new(),
        context_policy: Default::default(),
        mouse_capture: false,
        once: false,
    }
}

pub(super) fn catalog_options(
    dir: &tempfile::TempDir,
    response: &str,
    catalog_path: impl AsRef<Utf8Path>,
) -> TuiOptions {
    TuiOptions {
        catalog_path: Some(catalog_path.as_ref().to_owned()),
        ..test_options(dir, response, true)
    }
}

pub(super) async fn test_state(dir: &tempfile::TempDir, response: &str) -> TuiState {
    TuiState::load(test_options(dir, response, true))
        .await
        .expect("state loads")
}

pub(super) async fn test_state_with_policy(
    dir: &tempfile::TempDir,
    response: &str,
    allow_high_risk_tools: bool,
) -> TuiState {
    TuiState::load(test_options(dir, response, allow_high_risk_tools))
        .await
        .expect("state loads")
}
