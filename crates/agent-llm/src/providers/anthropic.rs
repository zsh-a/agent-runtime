use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::time::Duration;

use agent_core::{PROTOCOL_VERSION, ToolSpec};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use super::llm_content_as_text;
use crate::sse::{
    decode_json_value_or_null, sse_data, take_next_sse_frame, take_remaining_sse_frame,
};
use crate::structured::{structured_output_from_content, structured_output_instruction};
use crate::types::{
    LlmError, LlmEvent, LlmEventKind, LlmEventStream, LlmFinishReason, LlmMessage, LlmProvider,
    LlmRequest, LlmResponse, LlmResponseFormat, LlmRole, LlmUsage,
};

#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    provider: String,
    base_url: String,
    api_key: String,
    anthropic_version: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(
        provider: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        anthropic_version: impl Into<String>,
    ) -> Result<Self, LlmError> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        let api_key = api_key.into();
        let anthropic_version = anthropic_version.into();
        if base_url.is_empty() {
            return Err(LlmError::validation("Anthropic base URL is required"));
        }
        if api_key.is_empty() {
            return Err(LlmError::validation("Anthropic API key is required"));
        }
        if anthropic_version.is_empty() {
            return Err(LlmError::validation("Anthropic API version is required"));
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|err| {
                LlmError::provider(
                    "http_client_build_failed",
                    err.to_string(),
                    false,
                    json!({}),
                )
            })?;
        Ok(Self {
            provider: provider.into(),
            base_url,
            api_key,
            anthropic_version,
            client,
        })
    }

    fn messages_url(&self) -> String {
        format!("{}/messages", self.base_url)
    }
}

mod mapping;
mod provider;
mod sse;
mod types;

use mapping::*;
use types::*;
