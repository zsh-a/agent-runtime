//! Reqwest HTTP adapter; retries, decoding, and deadlines live in llm-client.
use bytes::Bytes;
use futures::{Stream, StreamExt};
use llm_client::{Client, HttpResponse, LlmEventStream, LlmProvider, Transport};
use llm_protocol::*;
use std::{future::Future, pin::Pin, time::Duration};

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
type SendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
type SendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type Body = Pin<Box<dyn Stream<Item = Result<Bytes, LlmError>> + Send>>;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub type Body = Pin<Box<dyn Stream<Item = Result<Bytes, LlmError>>>>;

#[derive(Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    epoch: std::time::Instant,
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    epoch: web_time::Instant,
}
impl ReqwestTransport {
    pub fn new() -> Result<Self, LlmError> {
        let builder = reqwest::Client::builder();
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let builder = builder
            .connect_timeout(Duration::from_secs(15))
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none());
        let client = builder.build().map_err(|_| {
            LlmError::provider(
                "http_client_build_failed",
                "Could not initialize HTTP transport",
                false,
                serde_json::json!({}),
            )
        })?;
        Ok(Self {
            client,
            #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
            epoch: std::time::Instant::now(),
            #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
            epoch: web_time::Instant::now(),
        })
    }
}
impl Transport for ReqwestTransport {
    type Body = Body;
    type SendFuture<'a> = SendFuture<'a, Result<HttpResponse<Self::Body>, LlmError>>;
    type SleepFuture<'a> = SendFuture<'a, ()>;
    fn send(&self, request: HttpRequest) -> Self::SendFuture<'_> {
        Box::pin(async move {
            let mut builder = self.client.post(&request.url).body(request.body);
            for (key, value) in request.headers {
                builder = builder.header(key, value);
            }
            let response = builder.send().await.map_err(transport_error)?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|v| (name.as_str().into(), v.into()))
                })
                .collect();
            let body: Body = Box::pin(
                response
                    .bytes_stream()
                    .map(|result| result.map_err(transport_error)),
            );
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        })
    }
    fn sleep(&self, duration: Duration) -> Self::SleepFuture<'_> {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        {
            Box::pin(tokio::time::sleep(duration))
        }
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            Box::pin(gloo_timers::future::TimeoutFuture::new(
                duration.as_millis().min(u32::MAX as u128) as u32,
            ))
        }
    }
    fn now(&self) -> Duration {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        {
            self.epoch.elapsed()
        }
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            self.epoch.elapsed()
        }
    }
}
fn transport_error(error: reqwest::Error) -> LlmError {
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    let connecting = error.is_connect();
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    let connecting = false;
    let (kind, code) = if error.is_timeout() {
        ("timeout", "provider_timeout")
    } else if connecting {
        ("connect", "provider_connect_failed")
    } else {
        ("request", "provider_request_failed")
    };
    let mut mapped = LlmError::provider(
        code,
        "Provider network request failed",
        error.is_timeout() || connecting || error.is_request(),
        serde_json::json!({"transport_kind":kind}),
    );
    if error.is_timeout() {
        mapped.record.kind = LlmErrorKind::Timeout;
    }
    mapped
}

#[derive(Clone)]
pub struct OpenAiCompatibleProvider(Client<ReqwestTransport>);
impl OpenAiCompatibleProvider {
    pub fn new(
        provider: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, LlmError> {
        Ok(Self(Client::new(
            Endpoint::new(Protocol::OpenAiCompatible, provider, base_url, api_key)?,
            ReqwestTransport::new()?,
        )))
    }
}
#[derive(Clone)]
pub struct AnthropicProvider(Client<ReqwestTransport>);
impl AnthropicProvider {
    pub fn new(
        provider: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        anthropic_version: impl Into<String>,
    ) -> Result<Self, LlmError> {
        let mut ep = Endpoint::new(Protocol::Anthropic, provider, base_url, api_key)?;
        ep.anthropic_version = anthropic_version.into();
        if ep.anthropic_version.trim().is_empty() {
            return Err(LlmError::validation("Anthropic API version is required"));
        }
        Ok(Self(Client::new(ep, ReqwestTransport::new()?)))
    }
}
#[derive(Clone)]
pub struct OllamaProvider(Client<ReqwestTransport>);
impl OllamaProvider {
    pub fn new(provider: impl Into<String>, base_url: impl Into<String>) -> Result<Self, LlmError> {
        Ok(Self(Client::new(
            Endpoint::new(Protocol::Ollama, provider, base_url, "")?,
            ReqwestTransport::new()?,
        )))
    }
}
macro_rules! provider {
    ($name:ident) => {
        #[cfg_attr(all(target_arch="wasm32",target_os="unknown"),async_trait::async_trait(?Send))]
        #[cfg_attr(
            not(all(target_arch = "wasm32", target_os = "unknown")),
            async_trait::async_trait
        )]
        impl LlmProvider for $name {
            async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
                self.0.complete(request).await
            }
            async fn stream(&self, request: LlmRequest) -> Result<LlmEventStream, LlmError> {
                Ok(Box::pin(self.0.stream(request).await?))
            }
        }
    };
}
provider!(OpenAiCompatibleProvider);
provider!(AnthropicProvider);
provider!(OllamaProvider);

#[cfg(test)]
mod tests;
