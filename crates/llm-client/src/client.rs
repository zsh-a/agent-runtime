use crate::{Bytes, HttpResponse, LlmEventStream, LlmProvider, Timeouts, Transport};
use agent_core::bounds::{MaybeSend, MaybeSync};
use futures::{
    Stream, StreamExt,
    future::{Either, select},
    stream,
};
use llm_protocol::*;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
pub struct CapabilityCache(Arc<Mutex<HashMap<CapabilityKey, StructuredMode>>>);
#[derive(PartialEq, Eq, Hash)]
struct CapabilityKey {
    protocol: Protocol,
    provider: String,
    base_url: String,
    version: String,
    model: String,
    credential: [u8; 32],
}
impl CapabilityKey {
    fn new(ep: &Endpoint, model: &str) -> Self {
        Self {
            protocol: ep.protocol,
            provider: ep.provider.clone(),
            base_url: ep.base_url.clone(),
            version: ep.anthropic_version.clone(),
            model: model.into(),
            credential: *blake3::hash(ep.api_key().expose().as_bytes()).as_bytes(),
        }
    }
}
impl std::fmt::Debug for CapabilityCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapabilityCache")
            .field("entries", &self.0.lock().map(|m| m.len()).unwrap_or(0))
            .finish()
    }
}
pub struct Client<T> {
    transport: Arc<T>,
    endpoint: Endpoint,
    timeouts: Timeouts,
    modes: CapabilityCache,
}
impl<T> Clone for Client<T> {
    fn clone(&self) -> Self {
        Self {
            transport: self.transport.clone(),
            endpoint: self.endpoint.clone(),
            timeouts: self.timeouts,
            modes: self.modes.clone(),
        }
    }
}
impl<T: Transport> Client<T> {
    pub fn new(endpoint: Endpoint, transport: T) -> Self {
        Self {
            transport: Arc::new(transport),
            endpoint,
            timeouts: Timeouts::default(),
            modes: CapabilityCache::default(),
        }
    }
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
    pub fn with_capability_cache(mut self, cache: CapabilityCache) -> Self {
        self.modes = cache;
        self
    }
    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }
    /// Every clone is bound to the same endpoint and credential. Capability
    /// decisions are per model and never shared with a different profile.
    async fn open(
        &self,
        request: &LlmRequest,
        streaming: bool,
        started: Duration,
    ) -> Result<(HttpResponse<T::Body>, Duration), LlmError> {
        let mut mode = self
            .modes
            .0
            .lock()
            .expect("capability cache poisoned")
            .get(&CapabilityKey::new(&self.endpoint, &request.model))
            .copied()
            .unwrap_or(StructuredMode::Native);
        loop {
            let encoded = encode(&self.endpoint, request, streaming, mode)?;
            let response = self.send_with_retries(encoded, started).await?;
            if (200..300).contains(&response.status) {
                if request.response_format.is_some() {
                    self.modes
                        .0
                        .lock()
                        .expect("capability cache poisoned")
                        .insert(CapabilityKey::new(&self.endpoint, &request.model), mode);
                }
                return Ok((response, started));
            }
            let status = response.status;
            let headers = response.headers.clone();
            let body = self.read_all(response.body, started).await?;
            if request.response_format.is_some()
                && rejects_structured_output(status, &body)
                && let Some(next) = mode.downgrade(self.endpoint.protocol)
            {
                mode = next;
                continue;
            }
            return Err(http_error(status, &headers, &body));
        }
    }
    async fn send_with_retries(
        &self,
        request: HttpRequest,
        started: Duration,
    ) -> Result<HttpResponse<T::Body>, LlmError> {
        let deadline = started + self.timeouts.first_data.min(self.timeouts.total);
        for attempt in 1..=3 {
            let response = with_deadline(
                self.transport.as_ref(),
                deadline,
                self.transport.send(request.clone()),
                "first_response",
                0,
            )
            .await?;
            let delay = match &response {
                Ok(response)
                    if matches!(response.status, 408 | 429 | 500 | 502 | 503 | 504 | 529) =>
                {
                    retry_delay(&response.headers, attempt)
                }
                Err(error) if error.record.retryable => Some(backoff(attempt)),
                _ => None,
            };
            if let Some(delay) = delay
                && attempt < 3
                && self.transport.now() + delay < deadline
            {
                drop(response);
                self.transport.sleep(delay).await;
                continue;
            }
            return response;
        }
        unreachable!("retry loop bounded")
    }
    async fn read_all(&self, body: T::Body, started: Duration) -> Result<Vec<u8>, LlmError> {
        let mut body =
            ResponseBody::new(self.transport.clone(), body, started, self.timeouts, false);
        let mut bytes = Vec::new();
        while let Some(next) = body.read_chunk().await {
            let next = next?;
            if bytes.len().saturating_add(next.len()) > 16 * 1024 * 1024 {
                return Err(LlmError::provider(
                    "provider_response_too_large",
                    "Provider response exceeds size limit",
                    false,
                    serde_json::json!({}),
                ));
            }
            bytes.extend_from_slice(&next);
        }
        Ok(bytes)
    }
    pub async fn complete(&self, mut request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let started = self.transport.now();
        for repair in 0..=1 {
            let (response, _) = self.open(&request, false, started).await?;
            let headers = response.headers;
            let status = response.status;
            let bytes = self.read_all(response.body, started).await?;
            match decode(&self.endpoint, &request, status, &headers, &bytes) {
                Ok(response) => return Ok(response),
                Err(error)
                    if repair == 0
                        && request.response_format.is_some()
                        && request.tools.is_empty()
                        && matches!(
                            error.record.code.as_str(),
                            "structured_output_parse_failed"
                                | "structured_output_not_object"
                                | "structured_output_schema_validation_failed"
                        ) =>
                {
                    // A bounded repair is a new explicit model turn, never a
                    // replay of a tool call, refused answer, or partial stream.
                    let value =
                        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default();
                    let previous = match self.endpoint.protocol {
                        Protocol::Anthropic => value["content"].as_array().map(|blocks| {
                            blocks
                                .iter()
                                .filter_map(|b| b["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("")
                        }),
                        Protocol::OpenAiCompatible => value
                            .pointer("/choices/0/message/content")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned),
                        Protocol::Ollama => value
                            .pointer("/message/content")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned),
                    };
                    if let Some(previous) = previous.filter(|s| s.len() <= 60000) {
                        request.messages.push(LlmMessage {
                            role: LlmRole::Assistant,
                            content: previous.into(),
                            name: None,
                            metadata: serde_json::json!({}),
                        });
                    }
                    request.messages.push(user_message("The previous answer failed JSON/schema validation. Return only a valid JSON value matching every constraint of the requested schema, without explanations or markdown. Correct the answer once."));
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("bounded repair always returns")
    }
    pub async fn extract<O: serde::de::DeserializeOwned>(
        &self,
        mut request: LlmRequest,
        schema: serde_json::Value,
    ) -> Result<O, LlmError> {
        request.response_format = Some(LlmResponseFormat::JsonSchema {
            name: "structured_output".into(),
            schema,
            strict: Some(true),
        });
        let response = self.complete(request).await?;
        if response.finish_reason != LlmFinishReason::Stop {
            return Err(LlmError::provider(
                "structured_output_incomplete",
                "Provider did not complete the structured answer",
                false,
                serde_json::json!({"finish_reason":response.finish_reason}),
            ));
        }
        let object = response.object.ok_or_else(|| {
            LlmError::provider(
                "structured_output_parse_failed",
                "Provider returned no structured result",
                false,
                serde_json::json!({}),
            )
        })?;
        serde_json::from_value(object).map_err(|_| {
            LlmError::provider(
                "structured_output_schema_validation_failed",
                "Structured result does not match the requested type",
                false,
                serde_json::json!({}),
            )
        })
    }
    /// The stream owns transport state. Dropping it cancels reads, retry waits,
    /// and any in-flight request; no detached tasks or secondary loop exists.
    pub async fn stream(
        &self,
        request: LlmRequest,
    ) -> Result<impl Stream<Item = Result<LlmEvent, LlmError>> + use<T>, LlmError> {
        let state = if self.endpoint.protocol == Protocol::Ollama {
            let response = self.complete(request.clone()).await?;
            let events = vec![
                Ok(event(LlmEventKind::Started, None, None)),
                Ok(event(
                    LlmEventKind::Delta,
                    Some(response.content.clone()),
                    None,
                )),
                Ok(event(LlmEventKind::Finished, None, Some(response))),
            ];
            StreamState::Synthetic(events.into())
        } else {
            let (response, started) = self.open(&request, true, self.transport.now()).await?;
            let decoder = StreamDecoder::new(&self.endpoint, &request)?;
            StreamState::Live {
                body: ResponseBody::new(
                    self.transport.clone(),
                    response.body,
                    started,
                    self.timeouts,
                    true,
                ),
                decoder: Box::new(decoder),
                pending: VecDeque::new(),
                done: false,
            }
        };
        Ok(stream::unfold(state, |mut state| async move {
            loop {
                match &mut state {
                    StreamState::Synthetic(events) => {
                        return events.pop_front().map(|e| (e, state));
                    }
                    StreamState::Live {
                        body,
                        decoder,
                        pending,
                        done,
                    } => {
                        if let Some(event) = pending.pop_front() {
                            if let Ok(event) = &event {
                                body.record_event(event);
                            }
                            if event.is_err() {
                                *done = true;
                                pending.clear();
                            }
                            return Some((event, state));
                        }
                        if *done {
                            return None;
                        }
                        match body.read_chunk().await {
                            Some(Ok(bytes)) => {
                                pending.extend(decoder.push(&bytes));
                                if decoder.is_finished() {
                                    *done = true;
                                }
                            }
                            Some(Err(error)) => {
                                *done = true;
                                return Some((Err(error), state));
                            }
                            None => {
                                pending.extend(decoder.finish());
                                *done = true;
                            }
                        }
                    }
                }
            }
        }))
    }
}
fn event(kind: LlmEventKind, content: Option<String>, response: Option<LlmResponse>) -> LlmEvent {
    LlmEvent {
        kind,
        content,
        response,
        tool_call_id: None,
        tool_name: None,
        partial_input_json: None,
        tool_input: None,
        metadata: serde_json::json!({}),
    }
}
enum StreamState<T: Transport> {
    Synthetic(VecDeque<Result<LlmEvent, LlmError>>),
    Live {
        body: ResponseBody<T>,
        decoder: Box<StreamDecoder>,
        pending: VecDeque<Result<LlmEvent, LlmError>>,
        done: bool,
    },
}

struct ResponseBody<T: Transport> {
    transport: Arc<T>,
    body: T::Body,
    started: Duration,
    timeouts: Timeouts,
    last_byte: Option<Duration>,
    last_progress: Option<Duration>,
    received: usize,
    done: bool,
}
impl<T: Transport> ResponseBody<T> {
    fn new(
        transport: Arc<T>,
        body: T::Body,
        started: Duration,
        timeouts: Timeouts,
        track_progress: bool,
    ) -> Self {
        Self {
            transport,
            body,
            started,
            timeouts,
            last_byte: None,
            last_progress: track_progress.then_some(started),
            received: 0,
            done: false,
        }
    }
    fn record_event(&mut self, event: &LlmEvent) {
        let progress = match event.kind {
            LlmEventKind::Delta
            | LlmEventKind::ThinkingDelta
            | LlmEventKind::ThinkingSignatureDelta => {
                event.content.as_ref().is_some_and(|s| !s.is_empty())
            }
            LlmEventKind::ToolCallDelta => event
                .partial_input_json
                .as_ref()
                .is_some_and(|s| !s.is_empty()),
            LlmEventKind::ToolCallStart | LlmEventKind::ToolCallEnd | LlmEventKind::Finished => {
                true
            }
            _ => false,
        };
        if progress && self.last_progress.is_some() {
            self.last_progress = Some(self.transport.now());
        }
    }
    async fn read_chunk(&mut self) -> Option<Result<Bytes, LlmError>> {
        if self.done {
            return None;
        }
        loop {
            let mut deadline = match self.last_byte {
                Some(last) => (last + self.timeouts.read_idle, "read_idle"),
                None => (self.started + self.timeouts.first_data, "first_byte"),
            };
            if self.started + self.timeouts.total < deadline.0 {
                deadline = (self.started + self.timeouts.total, "total");
            }
            if let Some(last) = self.last_progress
                && last + self.timeouts.model_progress < deadline.0
            {
                deadline = (last + self.timeouts.model_progress, "model_progress");
            }
            let next = with_deadline(
                self.transport.as_ref(),
                deadline.0,
                self.body.next(),
                deadline.1,
                self.received,
            )
            .await;
            match next {
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
                Ok(Some(Ok(bytes))) if bytes.is_empty() => continue,
                Ok(Some(Ok(bytes))) => {
                    self.last_byte = Some(self.transport.now());
                    self.received = self.received.saturating_add(bytes.len());
                    return Some(Ok(bytes));
                }
                Ok(Some(Err(mut error))) => {
                    self.done = true;
                    error.record.details["received_data"] = serde_json::json!(self.received > 0);
                    error.record.details["received_bytes"] = serde_json::json!(self.received);
                    return Some(Err(error));
                }
                Ok(None) => {
                    self.done = true;
                    return None;
                }
            }
        }
    }
}
async fn with_deadline<T: Transport, F: Future>(
    transport: &T,
    deadline: Duration,
    future: F,
    phase: &str,
    received: usize,
) -> Result<F::Output, LlmError> {
    let Some(remaining) = deadline
        .checked_sub(transport.now())
        .filter(|d| !d.is_zero())
    else {
        return Err(timeout(phase, received));
    };
    let sleep = transport.sleep(remaining);
    futures::pin_mut!(future, sleep);
    match select(future, sleep).await {
        Either::Left((result, _)) => Ok(result),
        Either::Right(_) => Err(timeout(phase, received)),
    }
}
fn timeout(phase: &str, received: usize) -> LlmError {
    let mut error = LlmError::provider(
        format!("provider_{phase}_timeout"),
        format!("LLM request timed out during {phase}"),
        true,
        serde_json::json!({"phase":phase,"transport_kind":"timeout","received_data":received>0,"received_bytes":received}),
    );
    error.record.kind = LlmErrorKind::Timeout;
    error
}
fn backoff(attempt: usize) -> Duration {
    Duration::from_millis(500 * (1 << (attempt - 1)))
}
fn retry_delay(headers: &[(String, String)], attempt: usize) -> Option<Duration> {
    match headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
    {
        None => Some(backoff(attempt)),
        Some((_, value)) => {
            let seconds = value.parse::<u64>().ok()?;
            let delay = Duration::from_secs(seconds);
            (delay <= Duration::from_secs(10)).then_some(delay)
        }
    }
}

#[cfg_attr(all(target_arch="wasm32",target_os="unknown"),async_trait::async_trait(?Send))]
#[cfg_attr(
    not(all(target_arch = "wasm32", target_os = "unknown")),
    async_trait::async_trait
)]
impl<T> LlmProvider for Client<T>
where
    T: Transport + MaybeSend + MaybeSync + 'static,
    T::Body: MaybeSend,
    for<'a> T::SendFuture<'a>: MaybeSend,
    for<'a> T::SleepFuture<'a>: MaybeSend,
{
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        Client::complete(self, request).await
    }
    async fn stream(&self, request: LlmRequest) -> Result<LlmEventStream, LlmError> {
        Ok(Box::pin(Client::stream(self, request).await?))
    }
}
