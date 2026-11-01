use agent_core::bounds::MaybeBoxStream;
use bytes::Bytes;
use futures::StreamExt;
use reqwest::{Client, RequestBuilder, Response};
use serde_json::json;

use crate::{LlmError, LlmEvent};

pub(crate) fn client() -> Result<Client, LlmError> {
    Client::builder().build().map_err(|error| {
        LlmError::provider(
            "http_client_build_failed",
            error.to_string(),
            false,
            json!({}),
        )
    })
}

pub(crate) async fn send(request: RequestBuilder) -> Result<Response, LlmError> {
    request.send().await.map_err(|error| {
        LlmError::provider(
            "provider_request_failed",
            error.to_string(),
            true,
            json!({}),
        )
    })
}

pub(crate) async fn read_text(response: Response) -> Result<String, LlmError> {
    response.text().await.map_err(|error| {
        LlmError::provider(
            "provider_body_read_failed",
            error.to_string(),
            true,
            json!({}),
        )
    })
}

pub(crate) struct ResponseBody {
    chunks: MaybeBoxStream<'static, Result<Bytes, reqwest::Error>>,
}

impl ResponseBody {
    pub(crate) fn new(response: Response, _track_progress: bool) -> Self {
        Self {
            chunks: Box::pin(response.bytes_stream()),
        }
    }

    // Browser fetch does not use the native model-progress deadline.
    pub(crate) fn record_event(&mut self, _event: &LlmEvent) {}

    pub(crate) async fn read_chunk(&mut self) -> Option<Result<Bytes, LlmError>> {
        self.chunks.next().await.map(|chunk| {
            chunk.map_err(|error| {
                LlmError::provider(
                    "provider_stream_read_failed",
                    error.to_string(),
                    true,
                    json!({}),
                )
            })
        })
    }
}
