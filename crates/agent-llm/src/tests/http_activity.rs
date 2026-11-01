use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures::stream;

use crate::http::{ResponseBody, Timeouts};

use super::*;

fn timeouts() -> Timeouts {
    Timeouts {
        first_data: Duration::from_millis(200),
        read_idle: Duration::from_millis(120),
        model_progress: Duration::from_millis(250),
        total: Duration::from_secs(1),
    }
}

fn event(kind: LlmEventKind) -> LlmEvent {
    LlmEvent {
        kind,
        content: Some("increment".to_owned()),
        response: None,
        tool_call_id: None,
        tool_name: None,
        partial_input_json: Some("{\"amount\":".to_owned()),
        tool_input: None,
        metadata: json!({}),
    }
}

#[tokio::test]
async fn no_response_headers_has_a_distinct_deadline_and_is_not_replayed() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/",
        post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Json(json!({"ok":true}))
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let error = crate::http::send_with_timeouts(
        crate::http::client()
            .unwrap()
            .post(format!("http://{address}/")),
        timeouts(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.record.code, "provider_first_response_timeout");
    assert_eq!(error.record.details["received_data"], false);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn headers_without_body_data_do_not_count_as_progress() {
    let mut body = ResponseBody::for_test(Box::pin(stream::pending()), timeouts(), true);
    let error = body.read_chunk().await.unwrap().unwrap_err();
    assert_eq!(error.record.code, "provider_first_byte_timeout");
    assert_eq!(error.record.details["received_data"], false);
    assert_eq!(error.record.details["received_bytes"], 0);
    assert!(body.read_chunk().await.is_none());
}

#[tokio::test]
async fn first_data_wait_can_be_longer_than_the_following_read_idle_limit() {
    let source = stream::once(async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(Bytes::from_static(b"first"))
    });
    let mut body = ResponseBody::for_test(Box::pin(source), timeouts(), true);
    assert_eq!(
        body.read_chunk().await.unwrap().unwrap(),
        Bytes::from_static(b"first")
    );
    assert!(body.read_chunk().await.is_none());
}

#[tokio::test]
async fn continuing_non_stream_body_reads_outlive_the_first_data_budget() {
    let source = stream::unfold(0, |index| async move {
        if index == 8 {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
        Some((Ok(Bytes::from_static(b"json")), index + 1))
    });
    let mut body = ResponseBody::for_test(Box::pin(source), timeouts(), false);
    let mut count = 0;
    while let Some(chunk) = body.read_chunk().await {
        chunk.unwrap();
        count += 1;
    }
    assert_eq!(count, 8);
}

#[tokio::test]
async fn text_reasoning_and_tool_deltas_keep_a_long_stream_alive() {
    for kind in [
        LlmEventKind::Delta,
        LlmEventKind::ThinkingDelta,
        LlmEventKind::ToolCallDelta,
    ] {
        let source = stream::unfold(0, |index| async move {
            if index == 8 {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
            Some((Ok(Bytes::from_static(b"data")), index + 1))
        });
        let mut body = ResponseBody::for_test(Box::pin(source), timeouts(), true);
        let mut count = 0;
        while let Some(chunk) = body.read_chunk().await {
            chunk.unwrap();
            body.record_event(&event(kind.clone()));
            count += 1;
        }
        assert_eq!(count, 8);
    }
}

#[tokio::test]
async fn data_followed_by_silence_reports_read_idle_and_keeps_received_bytes() {
    let source =
        stream::once(async { Ok(Bytes::from_static(b"partial")) }).chain(stream::pending());
    let mut body = ResponseBody::for_test(Box::pin(source), timeouts(), true);
    body.read_chunk().await.unwrap().unwrap();
    body.record_event(&event(LlmEventKind::Delta));
    let error = body.read_chunk().await.unwrap().unwrap_err();
    assert_eq!(error.record.code, "provider_read_idle_timeout");
    assert_eq!(error.record.details["received_data"], true);
    assert_eq!(error.record.details["received_bytes"], 7);
}

#[tokio::test]
async fn heartbeats_and_started_events_do_not_extend_model_progress_deadline() {
    let source = stream::unfold((), |()| async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Some((Ok(Bytes::from_static(b": heartbeat\n\n")), ()))
    });
    let mut body = ResponseBody::for_test(Box::pin(source), timeouts(), true);
    loop {
        match body.read_chunk().await.unwrap() {
            Ok(_) => body.record_event(&event(LlmEventKind::Started)),
            Err(error) => {
                assert_eq!(error.record.code, "provider_model_progress_timeout");
                assert_eq!(error.record.details["received_data"], true);
                break;
            }
        }
    }
}

#[tokio::test]
async fn hard_deadline_still_bounds_a_continuously_active_stream() {
    let source = stream::unfold((), |()| async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Some((Ok(Bytes::from_static(b"data")), ()))
    });
    let mut limits = timeouts();
    limits.total = Duration::from_millis(300);
    let mut body = ResponseBody::for_test(Box::pin(source), limits, true);
    loop {
        match body.read_chunk().await.unwrap() {
            Ok(_) => body.record_event(&event(LlmEventKind::ThinkingDelta)),
            Err(error) => {
                assert_eq!(error.record.code, "provider_total_timeout");
                assert_eq!(error.record.details["received_data"], true);
                break;
            }
        }
    }
}
