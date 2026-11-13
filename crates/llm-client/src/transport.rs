use bytes::Bytes;
use futures::Stream;
use llm_protocol::{HttpRequest, LlmError};
use std::{future::Future, time::Duration};

/// No thread bound: a system fetch implementation may stay on its UI executor.
/// Send transports still produce Send completion futures and event streams.
pub trait Transport {
    type Body: Stream<Item = Result<Bytes, LlmError>> + Unpin + 'static;
    type SendFuture<'a>: Future<Output = Result<HttpResponse<Self::Body>, LlmError>>
    where
        Self: 'a;
    type SleepFuture<'a>: Future<Output = ()>
    where
        Self: 'a;
    fn send(&self, request: HttpRequest) -> Self::SendFuture<'_>;
    fn sleep(&self, duration: Duration) -> Self::SleepFuture<'_>;
    /// Monotonic elapsed time in this transport's clock domain.
    fn now(&self) -> Duration;
}
pub struct HttpResponse<B> {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: B,
}
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub first_data: Duration,
    pub read_idle: Duration,
    pub model_progress: Duration,
    pub total: Duration,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            first_data: Duration::from_secs(300),
            read_idle: Duration::from_secs(120),
            model_progress: Duration::from_secs(300),
            total: Duration::from_secs(900),
        }
    }
}
