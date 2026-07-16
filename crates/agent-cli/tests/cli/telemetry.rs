use super::*;

#[test]
fn trace_export_otel_converts_trace_spans() {
    let output = agent_cmd()
        .args([
            "trace",
            "export-otel",
            "../../fixtures/contracts/trace.valid.json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let export: Value = serde_json::from_slice(&output).expect("otel export is JSON");
    assert_eq!(export["protocol_version"], "agent.v1");
    assert_eq!(export["export_format"], "otlp_trace_json.v1");
    let resource = &export["resourceSpans"][0]["resource"]["attributes"];
    assert!(
        resource
            .as_array()
            .expect("resource attrs")
            .iter()
            .any(|attr| {
                attr["key"] == "service.name" && attr["value"]["stringValue"] == "agent-runtime"
            })
    );
    assert!(
        resource
            .as_array()
            .expect("resource attrs")
            .iter()
            .any(|attr| {
                attr["key"] == "run.scope.type" && attr["value"]["stringValue"] == "tenant"
            })
    );
    let spans = export["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .expect("spans array");
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0]["name"], "agent.run");
    assert_eq!(spans[0]["kind"], "SPAN_KIND_INTERNAL");
    assert_eq!(spans[0]["status"]["code"], "STATUS_CODE_OK");
    assert!(
        spans[0]["traceId"]
            .as_str()
            .is_some_and(|value| value.len() == 32)
    );
    assert!(
        spans[0]["spanId"]
            .as_str()
            .is_some_and(|value| value.len() == 16)
    );
    assert_eq!(spans[1]["name"], "llm.openai");
    assert_eq!(spans[1]["parentSpanId"], spans[0]["spanId"]);
    assert!(
        spans[1]["attributes"]
            .as_array()
            .expect("attrs")
            .iter()
            .any(|attr| { attr["key"] == "total_tokens" && attr["value"]["intValue"] == "18" })
    );
}

#[test]
fn trace_export_otel_pushes_otlp_http_json() {
    let (port, request_handle) = spawn_otlp_trace_collector();
    let endpoint = format!("http://127.0.0.1:{port}/v1/traces");
    let output = agent_cmd()
        .args([
            "trace",
            "export-otel",
            "../../fixtures/contracts/trace.valid.json",
            "--endpoint",
            &endpoint,
            "--header",
            "x-otlp-test=true",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("push report is JSON");
    assert_eq!(report["protocol_version"], "agent.v1");
    assert_eq!(report["export_format"], "otlp_trace_json.v1");
    assert_eq!(report["endpoint"], endpoint);
    assert_eq!(report["status_code"], 200);
    assert_eq!(report["span_count"], 2);

    let request = request_handle.join().expect("request captured");
    assert!(request.starts_with("POST /v1/traces HTTP/1.1"));
    assert!(request.contains("content-type: application/json"));
    assert!(request.contains("x-otlp-test: true"));
    assert!(request.contains(r#""resourceSpans""#));
    assert!(request.contains(r#""name":"agent.run""#));
}
