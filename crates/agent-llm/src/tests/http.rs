use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::sync::{Mutex, Notify};

use super::*;

fn request() -> LlmRequest {
    LlmRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        provider: "openai".to_owned(),
        model: "test".to_owned(),
        messages: vec![user_message("hello")],
        temperature: None,
        max_output_tokens: Some(16384),
        tools: vec![],
        response_format: None,
        metadata: json!({}),
    }
}

fn completion() -> Value {
    json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]})
}

fn sse() -> Response {
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    ).into_response()
}

async fn serve(app: Router) -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

#[tokio::test]
async fn retries_gateway_failures_without_changing_multimodal_payload() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let observed = bodies.clone();
    let base = serve(Router::new().route(
        "/chat/completions",
        post(move |Json(body): Json<Value>| {
            let observed = observed.clone();
            async move {
                let mut bodies = observed.lock().await;
                bodies.push(body);
                if bodies.len() < 3 {
                    (StatusCode::BAD_GATEWAY, "upstream unavailable").into_response()
                } else {
                    Json(completion()).into_response()
                }
            }
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new("openai", base, "test-key").unwrap();
    let mut input = request();
    input.messages[0].content = json!([
        {"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"AQID"}},
        {"type":"text","text":"one meal"}
    ]);
    assert_eq!(provider.complete(input).await.unwrap().content, "ok");
    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 3);
    assert!(bodies.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(bodies[0]["max_tokens"], 16384);
    assert_eq!(bodies[0]["messages"][0]["content"][0]["type"], "image_url");
}

#[tokio::test]
async fn retries_stream_opening_but_does_not_duplicate_deltas() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let base = serve(Router::new().route(
        "/chat/completions",
        post(move || {
            let count = count.clone();
            async move {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                } else {
                    sse()
                }
            }
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new("openai", base, "test-key").unwrap();
    let events: Vec<_> = provider.stream(request()).await.unwrap().collect().await;
    let text: String = events
        .into_iter()
        .map(Result::unwrap)
        .filter(|event| event.kind == LlmEventKind::Delta)
        .filter_map(|event| event.content)
        .collect();
    assert_eq!(text, "ok");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn anthropic_uses_the_same_transient_gateway_recovery() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let base = serve(Router::new().route(
        "/messages",
        post(move || {
            let count = count.clone();
            async move {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    StatusCode::BAD_GATEWAY.into_response()
                } else {
                    Json(json!({"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn"}))
                        .into_response()
                }
            }
        }),
    ))
    .await;
    let provider = AnthropicProvider::new("anthropic", base, "test-key", "2023-06-01").unwrap();
    assert_eq!(provider.complete(request()).await.unwrap().content, "ok");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stops_after_bounded_attempts_and_preserves_http_status() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let base = serve(Router::new().route(
        "/chat/completions",
        post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::BAD_GATEWAY }
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new("openai", base, "test-key").unwrap();
    let error = provider.complete(request()).await.unwrap_err();
    assert_eq!(error.record.code, "provider_http_502");
    assert!(error.record.retryable);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn does_not_retry_configuration_errors_or_long_retry_after() {
    for status in [
        StatusCode::BAD_REQUEST,
        StatusCode::UNAUTHORIZED,
        StatusCode::TOO_MANY_REQUESTS,
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let base = serve(Router::new().route(
            "/chat/completions",
            post(move || {
                count.fetch_add(1, Ordering::SeqCst);
                async move { (status, [(header::RETRY_AFTER, "120")], "try later") }
            }),
        ))
        .await;
        let provider = OpenAiCompatibleProvider::new("openai", base, "test-key").unwrap();
        let error = provider.complete(request()).await.unwrap_err();
        assert_eq!(
            error.record.code,
            if status.as_u16() == 429 {
                "rate_limited".to_owned()
            } else {
                format!("provider_http_{}", status.as_u16())
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn does_not_replay_a_stream_after_content_was_emitted() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let base = serve(Router::new().route("/chat/completions", post(move || {
        count.fetch_add(1, Ordering::SeqCst);
        async { ([(header::CONTENT_TYPE, "text/event-stream")], "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\ndata: invalid-json\n\n") }
    }))).await;
    let provider = OpenAiCompatibleProvider::new("openai", base, "test-key").unwrap();
    let events: Vec<_> = provider.stream(request()).await.unwrap().collect().await;
    assert!(events.iter().any(|event| {
        event
            .as_ref()
            .ok()
            .and_then(|event| event.content.as_deref())
            == Some("partial")
    }));
    assert!(events.iter().any(|event| {
        event
            .as_ref()
            .err()
            .is_some_and(|error| error.record.code == "provider_stream_decode_failed")
    }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_during_backoff_stops_subsequent_requests() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let seen = Arc::new(Notify::new());
    let notify = seen.clone();
    let base = serve(Router::new().route(
        "/chat/completions",
        post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            notify.notify_one();
            async { StatusCode::BAD_GATEWAY }
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new("openai", base, "test-key").unwrap();
    let pending = tokio::spawn(async move { provider.complete(request()).await });
    seen.notified().await;
    pending.abort();
    let _ = pending.await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn preserves_transport_causes_without_leaking_url_query() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = reqwest::Client::new()
        .get(format!("http://{address}/?api_key=private-query"))
        .send()
        .await
        .unwrap_err();
    let error = crate::http::transport_error("provider_request_failed", "request", error, 3);
    assert_eq!(error.record.details["transport_kind"], "connect");
    assert_eq!(error.record.details["attempts"], 3);
    assert!(
        error
            .record
            .message
            .to_lowercase()
            .contains("connection refused")
    );
    assert!(!error.record.message.contains("private-query"));
    assert!(!error.record.message.contains("api_key"));
}

#[tokio::test]
async fn reasoning_requests_can_wait_beyond_a_minute_before_response_headers() {
    let base = serve(Router::new().route(
        "/chat/completions",
        post(|Json(body): Json<Value>| async move {
            tokio::time::sleep(Duration::from_secs(61)).await;
            if body["stream"] == true {
                sse()
            } else {
                Json(completion()).into_response()
            }
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new("openai", base, "test-key").unwrap();
    let (response, events) = tokio::join!(provider.complete(request()), provider.stream(request()));
    assert_eq!(response.unwrap().content, "ok");
    let events: Vec<_> = events.unwrap().collect().await;
    assert!(events.into_iter().all(|event| event.is_ok()));
}
