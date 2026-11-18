use super::*;
use futures::{StreamExt, executor::block_on, stream};
use llm_protocol::*;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    future::Future,
    pin::Pin,
    rc::Rc,
    time::Duration,
};

type LocalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;
struct Plan {
    status: u16,
    headers: Vec<(String, String)>,
    chunks: Vec<Result<Bytes, LlmError>>,
}
fn reply(status: u16, value: Value) -> Plan {
    Plan {
        status,
        headers: vec![],
        chunks: vec![Ok(Bytes::from(value.to_string()))],
    }
}
fn answer(text: &str) -> Plan {
    reply(
        200,
        json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}]}),
    )
}
#[derive(Clone, Default)]
struct LocalTransport {
    plans: Rc<RefCell<VecDeque<Result<Plan, LlmError>>>>,
    requests: Rc<RefCell<Vec<HttpRequest>>>,
    clock: Rc<Cell<Duration>>,
}
impl Transport for LocalTransport {
    type Body = stream::Iter<std::vec::IntoIter<Result<Bytes, LlmError>>>;
    type SendFuture<'a> = LocalFuture<'a, Result<HttpResponse<Self::Body>, LlmError>>;
    type SleepFuture<'a> = LocalFuture<'a, ()>;
    fn send(&self, request: HttpRequest) -> Self::SendFuture<'_> {
        Box::pin(async move {
            self.requests.borrow_mut().push(request);
            let plan = self
                .plans
                .borrow_mut()
                .pop_front()
                .expect("scripted response")?;
            Ok(HttpResponse {
                status: plan.status,
                headers: plan.headers,
                body: stream::iter(plan.chunks),
            })
        })
    }
    fn sleep(&self, d: Duration) -> Self::SleepFuture<'_> {
        Box::pin(async move {
            self.clock.set(self.clock.get() + d);
        })
    }
    fn now(&self) -> Duration {
        self.clock.get()
    }
}
fn request() -> LlmRequest {
    serde_json::from_value(json!({"protocol_version":PROTOCOL_VERSION,"provider":"test","model":"model","messages":[{"role":"user","content":"Extract a result"}],"response_format":{"type":"json_schema","name":"test","schema":{"type":"object","required":["n"],"additionalProperties":false,"properties":{"n":{"type":"integer","minimum":1}}}}})).unwrap()
}
fn endpoint(key: &str) -> Endpoint {
    Endpoint::new(
        Protocol::OpenAiCompatible,
        "test",
        "https://example.test/v1",
        key,
    )
    .unwrap()
}
fn setup(plans: Vec<Plan>) -> (Client<LocalTransport>, LocalTransport) {
    let transport = LocalTransport::default();
    transport
        .plans
        .borrow_mut()
        .extend(plans.into_iter().map(Ok));
    (Client::new(endpoint("key"), transport.clone()), transport)
}
fn payload(transport: &LocalTransport, index: usize) -> Value {
    serde_json::from_slice(&transport.requests.borrow()[index].body).unwrap()
}

#[test]
fn local_non_send_transport_completes_without_runtime() {
    let (client, _) = setup(vec![answer("{\"n\":3}")]);
    let response = block_on(client.complete(request())).unwrap();
    assert_eq!(response.object.unwrap()["n"], 3);
}
#[test]
fn explicit_unsupported_options_downgrade_and_are_remembered() {
    let unsupported = reply(
        400,
        json!({"error":{"message":"response_format json_schema is not supported"}}),
    );
    let no_object = reply(
        400,
        json!({"error":{"message":"response_format json_object is unsupported"}}),
    );
    let (client, transport) = setup(vec![
        unsupported,
        no_object,
        answer("{\"n\":1}"),
        answer("{\"n\":2}"),
    ]);
    block_on(client.complete(request())).unwrap();
    block_on(client.complete(request())).unwrap();
    assert_eq!(
        payload(&transport, 0)["response_format"]["type"],
        "json_schema"
    );
    assert_eq!(
        payload(&transport, 1)["response_format"]["type"],
        "json_object"
    );
    assert!(payload(&transport, 2).get("response_format").is_none());
    assert!(payload(&transport, 3).get("response_format").is_none());
    assert!(
        payload(&transport, 1)["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("JSON")
    );
}
#[test]
fn shared_capability_cache_isolates_models_endpoints_and_credentials() {
    let cache = CapabilityCache::default();
    let (client, transport) = setup(vec![
        reply(
            400,
            json!({"error":{"message":"response_format is unsupported"}}),
        ),
        answer("{\"n\":1}"),
        answer("{\"n\":2}"),
        answer("{\"n\":3}"),
        answer("{\"n\":4}"),
    ]);
    let client = client.with_capability_cache(cache.clone());
    block_on(client.complete(request())).unwrap();
    let same = Client::new(endpoint("key"), transport.clone()).with_capability_cache(cache.clone());
    block_on(same.complete(request())).unwrap();
    let other = Client::new(endpoint("different key"), transport.clone())
        .with_capability_cache(cache.clone());
    block_on(other.complete(request())).unwrap();
    let mut other_model = request();
    other_model.model = "other-model".into();
    block_on(client.complete(other_model)).unwrap();
    assert_eq!(
        payload(&transport, 2)["response_format"]["type"],
        "json_object"
    );
    assert_eq!(
        payload(&transport, 3)["response_format"]["type"],
        "json_schema"
    );
    assert_eq!(
        payload(&transport, 4)["response_format"]["type"],
        "json_schema"
    );
    assert!(!format!("{cache:?}").contains("different key"));
}
#[test]
fn schema_validation_repairs_once_and_retains_original_validation() {
    let (client, transport) = setup(vec![answer("{\"n\":0}"), answer("{\"n\":2}")]);
    let response = block_on(client.complete(request())).unwrap();
    assert_eq!(response.object.unwrap()["n"], 2);
    assert_eq!(transport.requests.borrow().len(), 2);
    assert_eq!(
        payload(&transport, 1)["messages"].as_array().unwrap().len(),
        3
    );
    let (client, transport) = setup(vec![answer("invalid"), answer("invalid")]);
    let error = block_on(client.complete(request())).unwrap_err();
    assert_eq!(error.record.code, "structured_output_parse_failed");
    assert_eq!(transport.requests.borrow().len(), 2);
}
#[test]
fn refusal_truncation_and_tools_are_never_repaired() {
    for reason in ["length", "content_filter", "tool_calls"] {
        let (client, transport) = setup(vec![reply(
            200,
            json!({"choices":[{"message":{"content":"{"},"finish_reason":reason}]}),
        )]);
        let response = block_on(client.complete(request())).unwrap();
        assert_ne!(response.finish_reason, LlmFinishReason::Stop);
        assert_eq!(transport.requests.borrow().len(), 1);
    }
}
#[test]
fn extract_deserializes_only_valid_complete_result() {
    #[derive(serde::Deserialize)]
    struct Output {
        n: u32,
    }
    let (client, _) = setup(vec![answer("{\"n\":7}")]);
    let value: Output = block_on(client.extract(
        request(),
        json!({"type":"object","required":["n"],"properties":{"n":{"type":"integer"}}}),
    ))
    .unwrap();
    assert_eq!(value.n, 7);
}
#[test]
fn long_or_date_retry_after_is_surfaced_without_early_retry() {
    for value in ["20", "Fri, 13 Nov 2026 12:00:00 GMT"] {
        let mut plan = reply(429, json!({"error":{"message":"secret"}}));
        plan.headers.push(("Retry-After".into(), value.into()));
        let (client, transport) = setup(vec![plan]);
        let error = block_on(client.complete(request())).unwrap_err();
        assert_eq!(error.record.kind, LlmErrorKind::RateLimited);
        assert_eq!(error.record.details["retry_after"], value);
        assert_eq!(transport.requests.borrow().len(), 1);
        assert_eq!(transport.clock.get(), Duration::ZERO);
    }
}
#[test]
fn establishment_retries_but_never_a_partly_read_body() {
    let (client, transport) = setup(vec![reply(503, json!({})), answer("{\"n\":1}")]);
    block_on(client.complete(request())).unwrap();
    assert_eq!(transport.requests.borrow().len(), 2);
    assert_eq!(transport.clock.get(), Duration::from_millis(500));
    let mut plan = answer("unused");
    plan.chunks = vec![
        Ok(Bytes::from_static(b"{")),
        Err(LlmError::provider("body_failed", "safe", true, json!({}))),
    ];
    let (client, transport) = setup(vec![plan]);
    let error = block_on(client.complete(request())).unwrap_err();
    assert_eq!(error.record.details["received_data"], true);
    assert_eq!(transport.requests.borrow().len(), 1);
}
#[test]
fn timeout_budget_includes_establishment_retries() {
    let (client, transport) = setup(vec![reply(503, json!({})), reply(503, json!({}))]);
    let client = client.with_timeouts(Timeouts {
        total: Duration::from_millis(600),
        ..Default::default()
    });
    let error = block_on(client.complete(request())).unwrap_err();
    assert_eq!(error.record.code, "provider_server_error");
    assert_eq!(transport.requests.borrow().len(), 2);
    assert_eq!(transport.clock.get(), Duration::from_millis(500));
}
#[test]
fn streaming_preserves_order_and_rejects_bare_eof() {
    let mut plan = answer("unused");
    plan.chunks = vec![Ok(Bytes::from_static(
        b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\n",
    ))];
    let (client, _) = setup(vec![plan]);
    let mut req = request();
    req.response_format = None;
    let events = block_on(async { client.stream(req).await.unwrap().collect::<Vec<_>>().await });
    assert_eq!(
        events.last().unwrap().as_ref().unwrap().kind,
        LlmEventKind::Finished
    );
    let mut plan = answer("unused");
    plan.chunks = vec![Ok(Bytes::from_static(
        b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
    ))];
    let (client, transport) = setup(vec![plan]);
    let mut req = request();
    req.response_format = None;
    let events = block_on(async { client.stream(req).await.unwrap().collect::<Vec<_>>().await });
    assert_eq!(
        events.last().unwrap().as_ref().unwrap_err().record.code,
        "provider_stream_incomplete"
    );
    assert_eq!(transport.requests.borrow().len(), 1);
}

#[test]
fn responses_downgrade_cache_and_repair_preserve_native_items() {
    let output = |text: &str| json!([{"type":"reasoning","id":"rs_1","encrypted_content":"opaque","summary":[]},{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":text,"annotations":[]}]}]);
    let (old_client, transport) = setup(vec![
        reply(
            400,
            json!({"error":{"message":"text.format json_schema unsupported"}}),
        ),
        reply(
            400,
            json!({"error":{"message":"text.format json_object unsupported"}}),
        ),
        reply(
            200,
            json!({"status":"completed","output":output("{\"n\":0}")}),
        ),
        reply(
            200,
            json!({"status":"completed","output":output("{\"n\":2}")}),
        ),
        reply(
            200,
            json!({"status":"completed","output":output("{\"n\":3}")}),
        ),
    ]);
    drop(old_client);
    let ep = Endpoint::new(
        Protocol::OpenAiResponses,
        "test",
        "https://example.test/v1",
        "key",
    )
    .unwrap();
    let client = Client::new(ep, transport.clone());
    let response = block_on(client.complete(request())).unwrap();
    assert_eq!(response.object.unwrap()["n"], 2);
    assert_eq!(
        payload(&transport, 0)["text"]["format"]["type"],
        "json_schema"
    );
    assert_eq!(
        payload(&transport, 1)["text"]["format"]["type"],
        "json_object"
    );
    assert!(payload(&transport, 2).get("text").is_none());
    assert_eq!(payload(&transport, 3)["input"][1], output("{\"n\":0}")[0]);
    assert_eq!(payload(&transport, 3)["input"][2], output("{\"n\":0}")[1]);
    block_on(client.complete(request())).unwrap();
    assert!(payload(&transport, 4).get("text").is_none());
    for request in transport.requests.borrow().iter() {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["store"], false);
        assert!(request.url.ends_with("/responses"));
    }
}
