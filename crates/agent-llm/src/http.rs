use std::error::Error;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde_json::json;
use tokio::time::Instant;

use crate::{LlmError, LlmErrorKind, LlmEvent, LlmEventKind};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_ATTEMPTS: usize = 3;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
pub(crate) struct Timeouts {
    pub(crate) first_data: Duration,
    pub(crate) read_idle: Duration,
    pub(crate) model_progress: Duration,
    pub(crate) total: Duration,
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

#[derive(Clone, Copy)]
struct RequestTiming {
    started: Instant,
    timeouts: Timeouts,
}

pub(crate) fn client() -> Result<Client, LlmError> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .retry(reqwest::retry::never())
        .build()
        .map_err(|error| transport_error("http_client_build_failed", "client", error, 0))
}

/// Retry only request establishment, before any successful response body is
/// exposed. Never replay a stream, model tool call, or parsed response.
pub(crate) async fn send(request: RequestBuilder) -> Result<Response, LlmError> {
    send_with_timeouts(request, Timeouts::default()).await
}

pub(crate) async fn send_with_timeouts(
    request: RequestBuilder,
    timeouts: Timeouts,
) -> Result<Response, LlmError> {
    let timing = RequestTiming {
        started: Instant::now(),
        timeouts,
    };
    let (deadline, phase) = earlier(
        (timing.started + timeouts.first_data, "first_response"),
        (timing.started + timeouts.total, "total"),
    );
    for attempt in 1..=MAX_ATTEMPTS {
        let next = request
            .try_clone()
            .ok_or_else(|| LlmError::validation("LLM request body cannot be replayed"))?;
        let result = match tokio::time::timeout_at(deadline, next.send()).await {
            Ok(result) => result,
            Err(_) => return Err(timeout_error(phase, timing.started, 0)),
        };
        let delay = match &result {
            Ok(response) if retryable_status(response.status()) => retry_delay(response, attempt),
            Err(error) if retryable_transport(error) => Some(backoff(attempt)),
            _ => None,
        };
        if let Some(delay) = delay
            && attempt < MAX_ATTEMPTS
            && Instant::now() + delay < deadline
        {
            tracing::warn!(
                attempt,
                status = ?result.as_ref().ok().map(Response::status),
                delay_ms = delay.as_millis(),
                "retrying transient LLM request establishment failure",
            );
            // Drop the failed response before sleeping. Dropping this future
            // during cancellation also cancels the backoff and any next send.
            drop(result);
            tokio::time::sleep(delay).await;
            continue;
        }
        return result
            .map(|mut response| {
                response.extensions_mut().insert(timing);
                response
            })
            .map_err(|error| {
                transport_error("provider_request_failed", "request", error, attempt)
            });
    }
    unreachable!("bounded retry loop always returns its final result")
}

type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

/// Tracks wire activity separately from meaningful model progress. Each
/// successful non-empty read refreshes the idle timer, not the hard deadline.
pub(crate) struct ResponseBody {
    chunks: ByteStream,
    timing: RequestTiming,
    last_byte: Option<Instant>,
    last_progress: Option<Instant>,
    received_bytes: usize,
    finished: bool,
}

impl ResponseBody {
    pub(crate) fn new(response: Response, track_progress: bool) -> Self {
        let timing = response
            .extensions()
            .get::<RequestTiming>()
            .copied()
            .unwrap_or(RequestTiming {
                started: Instant::now(),
                timeouts: Timeouts::default(),
            });
        Self::from_parts(Box::pin(response.bytes_stream()), timing, track_progress)
    }

    fn from_parts(chunks: ByteStream, timing: RequestTiming, track_progress: bool) -> Self {
        Self {
            chunks,
            timing,
            last_byte: None,
            last_progress: track_progress.then_some(timing.started),
            received_bytes: 0,
            finished: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(chunks: ByteStream, timeouts: Timeouts, track_progress: bool) -> Self {
        Self::from_parts(
            chunks,
            RequestTiming {
                started: Instant::now(),
                timeouts,
            },
            track_progress,
        )
    }

    pub(crate) fn record_event(&mut self, event: &LlmEvent) {
        let meaningful = match event.kind {
            LlmEventKind::Delta
            | LlmEventKind::ThinkingDelta
            | LlmEventKind::ThinkingSignatureDelta => event
                .content
                .as_ref()
                .is_some_and(|content| !content.is_empty()),
            LlmEventKind::ToolCallDelta => event
                .partial_input_json
                .as_ref()
                .is_some_and(|input| !input.is_empty()),
            LlmEventKind::ToolCallStart | LlmEventKind::ToolCallEnd | LlmEventKind::Finished => {
                true
            }
            _ => false,
        };
        if meaningful && self.last_progress.is_some() {
            self.last_progress = Some(Instant::now());
        }
    }

    pub(crate) async fn read_chunk(&mut self) -> Option<Result<Bytes, LlmError>> {
        if self.finished {
            return None;
        }
        loop {
            let network = match self.last_byte {
                None => (
                    self.timing.started + self.timing.timeouts.first_data,
                    "first_byte",
                ),
                Some(last) => (last + self.timing.timeouts.read_idle, "read_idle"),
            };
            let mut deadline = earlier(
                network,
                (self.timing.started + self.timing.timeouts.total, "total"),
            );
            if let Some(last) = self.last_progress {
                deadline = earlier(
                    deadline,
                    (last + self.timing.timeouts.model_progress, "model_progress"),
                );
            }
            if Instant::now() >= deadline.0 {
                self.finished = true;
                return Some(Err(timeout_error(
                    deadline.1,
                    self.timing.started,
                    self.received_bytes,
                )));
            }
            let next = tokio::time::timeout_at(deadline.0, self.chunks.next()).await;
            return match next {
                Err(_) => {
                    self.finished = true;
                    Some(Err(timeout_error(
                        deadline.1,
                        self.timing.started,
                        self.received_bytes,
                    )))
                }
                Ok(Some(Ok(bytes))) if bytes.is_empty() => continue,
                Ok(Some(Ok(bytes))) => {
                    self.last_byte = Some(Instant::now());
                    self.received_bytes = self.received_bytes.saturating_add(bytes.len());
                    Some(Ok(bytes))
                }
                Ok(Some(Err(error))) => {
                    self.finished = true;
                    let code = if self.last_progress.is_some() {
                        "provider_stream_read_failed"
                    } else {
                        "provider_body_read_failed"
                    };
                    let mut error = transport_error(code, "response_body", error, 0);
                    error.record.details["received_data"] = json!(self.received_bytes > 0);
                    error.record.details["received_bytes"] = json!(self.received_bytes);
                    Some(Err(error))
                }
                Ok(None) => {
                    self.finished = true;
                    None
                }
            };
        }
    }
}

pub(crate) async fn read_text(response: Response) -> Result<String, LlmError> {
    let mut body = ResponseBody::new(response, false);
    let mut bytes = Vec::new();
    while let Some(chunk) = body.read_chunk().await {
        bytes.extend_from_slice(&chunk?);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn earlier(a: (Instant, &'static str), b: (Instant, &'static str)) -> (Instant, &'static str) {
    if a.0 <= b.0 { a } else { b }
}

fn timeout_error(phase: &str, started: Instant, received_bytes: usize) -> LlmError {
    let mut error = LlmError::provider(
        format!("provider_{phase}_timeout"),
        format!("LLM request timed out during {phase}"),
        true,
        json!({"transport_kind":"timeout", "phase":phase,
            "received_data":received_bytes > 0, "received_bytes":received_bytes,
            "elapsed_ms":started.elapsed().as_millis()}),
    );
    error.record.kind = LlmErrorKind::Timeout;
    error
}

fn retryable_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504 | 529)
}

fn retryable_transport(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect() || error.is_request()
}

fn backoff(attempt: usize) -> Duration {
    Duration::from_millis(500 * (1 << (attempt - 1)))
}

fn retry_delay(response: &Response, attempt: usize) -> Option<Duration> {
    match response.headers().get(reqwest::header::RETRY_AFTER) {
        None => Some(backoff(attempt)),
        Some(value) => {
            // A long or date-based Retry-After is surfaced to the caller,
            // rather than retrying sooner than the provider requested.
            let seconds = value.to_str().ok()?.parse::<u64>().ok()?;
            let delay = Duration::from_secs(seconds);
            (delay <= MAX_RETRY_DELAY).then_some(delay)
        }
    }
}

pub(crate) fn transport_error(
    code: &str,
    phase: &str,
    error: reqwest::Error,
    attempts: usize,
) -> LlmError {
    let timeout = error.is_timeout();
    let connecting = error.is_connect();
    let kind = if timeout {
        "timeout"
    } else if connecting {
        "connect"
    } else {
        "request"
    };
    // reqwest's outer Display often says only "error sending request". Keep
    // DNS/TLS/socket causes for local diagnostics, without URL query secrets.
    let error = error.without_url();
    let mut message = error.to_string();
    let mut source = error.source();
    for _ in 0..8 {
        let Some(cause) = source else { break };
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    let mut details =
        json!({"transport_kind": kind, "phase": if connecting { "connect" } else { phase }});
    if attempts > 0 {
        details["attempts"] = json!(attempts);
    }
    let mut error = LlmError::provider(
        if timeout && connecting {
            "provider_connect_timeout"
        } else if timeout {
            "provider_timeout"
        } else {
            code
        },
        message,
        code != "http_client_build_failed",
        details,
    );
    if timeout {
        error.record.kind = LlmErrorKind::Timeout;
    }
    error
}
