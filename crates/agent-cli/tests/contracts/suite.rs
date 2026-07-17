use std::path::{Path, PathBuf};

use agent_chat::{
    ChatResumeRequest, ChatToolResult, ChatTurnEvent, ChatTurnRequest, ChatTurnSnapshot,
    ChatTurnState,
};
use agent_core::{
    AgentRunResult, AgentRuntimeCatalog, AgentSpec, AgentTrace, ApprovalDecision, HookEvent,
    HookSpec, PromptManifest, ProposalEnvelope, RunRequest, SessionRecord, StepRecord,
    ThreadRecord, WorkflowRunRequest, WorkflowRunResult,
};
use agent_llm::{LlmRequest, LlmResponse};
use agent_runtime::RecoveryReport;
use serde_json::Value;

#[test]
fn committed_fixtures_match_json_schemas() {
    assert_valid(
        "schemas/run-request.schema.json",
        "fixtures/contracts/run-request.valid.json",
    );
    assert_valid(
        "schemas/run-request.schema.json",
        "fixtures/contracts/run-request.webhook.valid.json",
    );
    assert_valid(
        "schemas/workflow-run-request.schema.json",
        "fixtures/contracts/workflow-run-request.valid.json",
    );
    assert_valid(
        "schemas/workflow-run-result.schema.json",
        "fixtures/contracts/workflow-run-result.valid.json",
    );
    assert_valid(
        "schemas/http-agent-run-request.schema.json",
        "fixtures/contracts/http-agent-run-request.valid.json",
    );
    assert_valid(
        "schemas/http-agent-run-request.schema.json",
        "fixtures/contracts/http-agent-run-request.queue.valid.json",
    );
    assert_invalid(
        "schemas/run-request.schema.json",
        "fixtures/contracts/run-request.invalid.missing-protocol-version.json",
    );
    assert_valid(
        "schemas/run-result.schema.json",
        "fixtures/contracts/run-result.completed.valid.json",
    );
    assert_valid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.valid.json",
    );
    assert_valid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.valid.closed-early-step.json",
    );
    assert_invalid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.invalid.missing-step-run-state.json",
    );
    assert_invalid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.invalid.missing-step-terminal-reason.json",
    );
    assert_invalid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.invalid.unknown-step-terminal-reason.json",
    );
    assert_invalid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.invalid.unknown-step-status.json",
    );
    assert_invalid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.invalid.non-string-step-tool-name.json",
    );
    assert_invalid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.invalid.empty-step-tool-name.json",
    );
    assert_invalid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.invalid.mismatched-step-run-state-status.json",
    );
    assert_invalid(
        "schemas/trace.schema.json",
        "fixtures/contracts/trace.invalid.mismatched-step-terminal-status.json",
    );
    assert_valid(
        "schemas/catalog.schema.json",
        "fixtures/contracts/catalog.valid.json",
    );
    assert_valid(
        "schemas/agent-spec.schema.json",
        "fixtures/contracts/agent-spec.cron.valid.json",
    );
    assert_valid(
        "schemas/debug-bundle-manifest.schema.json",
        "fixtures/contracts/debug-bundle-manifest.valid.json",
    );
    assert_valid(
        "schemas/debug-state-snapshot.schema.json",
        "fixtures/contracts/debug-state-snapshot.valid.json",
    );
    assert_valid(
        "schemas/debug-replay-config.schema.json",
        "fixtures/contracts/debug-replay-config.valid.json",
    );
    assert_valid(
        "schemas/debug-artifact-materializations.schema.json",
        "fixtures/contracts/debug-artifact-materializations.valid.json",
    );
    assert_valid(
        "schemas/debug-artifact-resolvers.schema.json",
        "fixtures/contracts/debug-artifact-resolvers.valid.json",
    );
    assert_valid(
        "schemas/otel-trace-export.schema.json",
        "fixtures/contracts/otel-trace-export.valid.json",
    );
    assert_valid(
        "schemas/recovery-report.schema.json",
        "fixtures/contracts/recovery-report.valid.json",
    );
    assert_valid(
        "schemas/prompt-manifest.schema.json",
        "fixtures/contracts/prompt-manifest.valid.json",
    );
    assert_valid(
        "schemas/tool-call-record.schema.json",
        "fixtures/contracts/tool-call-record.valid.json",
    );
    assert_valid(
        "schemas/tool-source-manifest.schema.json",
        "fixtures/contracts/tool-source.example.json",
    );
    assert_valid(
        "schemas/tool-source-manifest.schema.json",
        "fixtures/contracts/mcp-tool-source.example.json",
    );
    assert_valid(
        "schemas/tool-source-manifest.schema.json",
        "fixtures/contracts/http-tool-source.example.json",
    );
    assert_valid(
        "schemas/tool-source-manifest.schema.json",
        "fixtures/contracts/http-tool-source.secret-header.example.json",
    );
    assert_valid(
        "schemas/tool-source-manifest.schema.json",
        "fixtures/contracts/shell-tool-source.example.json",
    );
    assert_valid(
        "schemas/hook-event.schema.json",
        "fixtures/contracts/hook-event.valid.json",
    );
    assert_valid(
        "schemas/hook-spec.schema.json",
        "fixtures/contracts/hook-spec.valid.json",
    );
    assert_valid(
        "schemas/proposal-envelope.schema.json",
        "fixtures/contracts/proposal-envelope.valid.json",
    );
    assert_valid(
        "schemas/approval-decision.schema.json",
        "fixtures/contracts/approval-decision.valid.json",
    );
    assert_valid(
        "schemas/llm-request.schema.json",
        "fixtures/contracts/llm-request.valid.json",
    );
    assert_valid(
        "schemas/llm-request.schema.json",
        "fixtures/contracts/llm-request.structured.valid.json",
    );
    assert_valid(
        "schemas/llm-response.schema.json",
        "fixtures/contracts/llm-response.valid.json",
    );
    assert_valid(
        "schemas/llm-response.schema.json",
        "fixtures/contracts/llm-response.structured.valid.json",
    );
    assert_valid(
        "schemas/chat-turn-request.schema.json",
        "fixtures/contracts/chat-turn-request.valid.json",
    );
    assert_valid(
        "schemas/chat-resume-request.schema.json",
        "fixtures/contracts/chat-resume-request.valid.json",
    );
    assert_invalid(
        "schemas/chat-turn-request.schema.json",
        "fixtures/contracts/chat-turn-request.invalid.missing-messages.json",
    );
    assert_valid(
        "schemas/chat-turn-state.schema.json",
        "fixtures/contracts/chat-turn-state.requires-tool-results.valid.json",
    );
    assert_valid(
        "schemas/chat-tool-result.schema.json",
        "fixtures/contracts/chat-tool-result.valid.json",
    );
    assert_valid(
        "schemas/chat-turn-snapshot.schema.json",
        "fixtures/contracts/chat-turn-snapshot.requires-tool-results.valid.json",
    );
    assert_valid(
        "schemas/chat-turn-event.schema.json",
        "fixtures/contracts/chat-turn-event.round-finished.requires-tool-results.valid.json",
    );
    assert_valid(
        "schemas/chat-turn-event.schema.json",
        "fixtures/contracts/chat-turn-event.context-snapshot.valid.json",
    );
    assert_valid(
        "schemas/session-record.schema.json",
        "fixtures/contracts/session-record.valid.json",
    );
    assert_valid(
        "schemas/thread-record.schema.json",
        "fixtures/contracts/thread-record.valid.json",
    );
    assert_valid(
        "schemas/step-record.schema.json",
        "fixtures/contracts/step-record.valid.json",
    );
}

#[test]
fn committed_valid_fixtures_deserialize_to_runtime_types() {
    assert_deserializes::<RunRequest>("fixtures/contracts/run-request.valid.json");
    assert_deserializes::<RunRequest>("fixtures/contracts/run-request.webhook.valid.json");
    assert_deserializes::<WorkflowRunRequest>("fixtures/contracts/workflow-run-request.valid.json");
    assert_deserializes::<WorkflowRunResult>("fixtures/contracts/workflow-run-result.valid.json");
    assert_deserializes::<AgentRunResult>("fixtures/contracts/run-result.completed.valid.json");
    assert_deserializes::<AgentTrace>("fixtures/contracts/trace.valid.json");
    assert_deserializes::<AgentTrace>("fixtures/contracts/trace.valid.closed-early-step.json");
    assert_deserializes::<AgentRuntimeCatalog>("fixtures/contracts/catalog.valid.json");
    assert_deserializes::<AgentSpec>("fixtures/contracts/agent-spec.cron.valid.json");
    assert_deserializes::<RecoveryReport>("fixtures/contracts/recovery-report.valid.json");
    assert_deserializes::<PromptManifest>("fixtures/contracts/prompt-manifest.valid.json");
    assert_deserializes::<HookEvent>("fixtures/contracts/hook-event.valid.json");
    assert_deserializes::<HookSpec>("fixtures/contracts/hook-spec.valid.json");
    assert_deserializes::<ProposalEnvelope>("fixtures/contracts/proposal-envelope.valid.json");
    assert_deserializes::<ApprovalDecision>("fixtures/contracts/approval-decision.valid.json");
    assert_deserializes::<LlmRequest>("fixtures/contracts/llm-request.valid.json");
    assert_deserializes::<LlmRequest>("fixtures/contracts/llm-request.structured.valid.json");
    assert_deserializes::<LlmResponse>("fixtures/contracts/llm-response.valid.json");
    assert_deserializes::<LlmResponse>("fixtures/contracts/llm-response.structured.valid.json");
    assert_deserializes::<ChatTurnRequest>("fixtures/contracts/chat-turn-request.valid.json");
    assert_deserializes::<ChatResumeRequest>("fixtures/contracts/chat-resume-request.valid.json");
    assert_deserializes::<ChatTurnState>(
        "fixtures/contracts/chat-turn-state.requires-tool-results.valid.json",
    );
    assert_deserializes::<ChatToolResult>("fixtures/contracts/chat-tool-result.valid.json");
    assert_deserializes::<ChatTurnSnapshot>(
        "fixtures/contracts/chat-turn-snapshot.requires-tool-results.valid.json",
    );
    assert_deserializes::<ChatTurnEvent>(
        "fixtures/contracts/chat-turn-event.round-finished.requires-tool-results.valid.json",
    );
    assert_deserializes::<ChatTurnEvent>(
        "fixtures/contracts/chat-turn-event.context-snapshot.valid.json",
    );
    assert_deserializes::<SessionRecord>("fixtures/contracts/session-record.valid.json");
    assert_deserializes::<ThreadRecord>("fixtures/contracts/thread-record.valid.json");
    assert_deserializes::<StepRecord>("fixtures/contracts/step-record.valid.json");
}

#[test]
fn example_registry_agents_match_agent_spec_schema() {
    let schema = read_json("schemas/agent-spec.schema.json");
    let validator = jsonschema::validator_for(&schema).expect("agent schema compiles");
    let registry: Value =
        serde_yaml::from_str(&read_text("examples/agents.yaml")).expect("example registry parses");
    let agents = registry
        .get("agents")
        .and_then(Value::as_array)
        .expect("registry contains agents");
    for agent in agents {
        let mut spec = agent.clone();
        spec.as_object_mut()
            .expect("agent is object")
            .remove("runner");
        assert_json_valid(&validator, &spec);
    }
}

#[test]
fn committed_eval_cases_match_schema() {
    let schema = read_json("schemas/eval-case.schema.json");
    let validator = jsonschema::validator_for(&schema).expect("eval case schema compiles");
    for path in [
        "evals/catalog_dry_run.yaml",
        "evals/tool_call_sequence.yaml",
        "evals/proposal_expectation.yaml",
    ] {
        let instance: Value =
            serde_yaml::from_str(&read_text(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_json_valid(&validator, &instance);
    }
}

#[test]
fn openapi_contract_documents_http_server_routes() {
    let openapi: Value = serde_yaml::from_str(&read_text("openapi/agent-runtime-api.yaml"))
        .expect("OpenAPI contract parses");
    assert_eq!(openapi["openapi"], "3.1.0");
    assert!(openapi["paths"]["/healthz"]["get"].is_object());
    assert!(openapi["paths"]["/catalog/summary"]["get"].is_object());
    assert!(openapi["paths"]["/metrics/summary"]["get"].is_object());
    assert!(openapi["paths"]["/chat/turn"]["post"].is_object());
    assert!(openapi["paths"]["/chat/resume"]["post"].is_object());
    assert!(openapi["paths"]["/workflows/run"]["post"].is_object());
    assert!(openapi["paths"]["/tools"]["get"].is_object());
    assert!(openapi["paths"]["/runs"]["get"].is_object());
    assert!(openapi["paths"]["/runs/{run_id}"]["get"].is_object());
    assert!(openapi["paths"]["/runs/{run_id}/trace"]["get"].is_object());
    assert!(openapi["paths"]["/runs/{run_id}/events"]["get"].is_object());
    assert!(openapi["paths"]["/runs/{run_id}/cancel"]["post"].is_object());
    assert!(openapi["paths"]["/runs/{run_id}/replay"]["post"].is_object());
    assert!(openapi["paths"]["/tools/{tool_name}/call"]["post"].is_object());
    assert!(openapi["paths"]["/proposals"]["get"].is_object());
    assert!(openapi["paths"]["/proposals"]["post"].is_object());
    assert!(openapi["paths"]["/proposals/{proposal_id}"]["get"].is_object());
    assert!(openapi["paths"]["/proposals/{proposal_id}/decision"]["post"].is_object());
    assert!(openapi["paths"]["/proposals/{proposal_id}/apply"]["post"].is_object());
    assert!(openapi["paths"]["/proposals/{proposal_id}/undo"]["post"].is_object());
    assert!(openapi["paths"]["/sessions"]["get"].is_object());
    assert!(openapi["paths"]["/sessions"]["post"].is_object());
    assert!(openapi["paths"]["/sessions/{session_id}"]["get"].is_object());
    assert!(openapi["paths"]["/sessions/{session_id}/fork"]["post"].is_object());
    assert!(openapi["paths"]["/agents/{agent_id}/run"]["post"].is_object());
    assert_eq!(
        openapi["paths"]["/agents/{agent_id}/run"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/AgentRunResponse"
    );
    assert_eq!(
        openapi["paths"]["/agents/{agent_id}/run"]["post"]["requestBody"]["content"]["application/json"]
            ["schema"]["$ref"],
        "../schemas/http-agent-run-request.schema.json"
    );
    assert_eq!(
        openapi["paths"]["/workflows/run"]["post"]["requestBody"]["content"]["application/json"]["schema"]
            ["$ref"],
        "../schemas/workflow-run-request.schema.json"
    );
    assert_eq!(
        openapi["paths"]["/workflows/run"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "../schemas/workflow-run-result.schema.json"
    );
    assert_eq!(
        openapi["paths"]["/runs/{run_id}/cancel"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/CancelRunResponse"
    );
    assert!(
        openapi["paths"]["/runs/{run_id}/cancel"]["post"]["responses"]["200"]["description"]
            .as_str()
            .unwrap_or_default()
            .contains("persist")
    );
    assert!(
        openapi["components"]["schemas"]["CancelRunResponse"]["properties"]
            ["cancellation_requested"]["description"]
            .as_str()
            .unwrap_or_default()
            .contains("persisted running record")
    );
    assert_eq!(
        openapi["paths"]["/tools/{tool_name}/call"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ToolCallResponse"
    );
    assert_eq!(
        openapi["paths"]["/metrics/summary"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/RuntimeMetricsSummary"
    );
    assert_eq!(
        openapi["components"]["schemas"]["RuntimeMetricsSummary"]["properties"]["tool_call_count"]
            ["minimum"],
        0
    );
    assert_eq!(
        openapi["components"]["schemas"]["RuntimeMetricsSummary"]["properties"]["artifact_ref_count"]
            ["minimum"],
        0
    );
    assert_eq!(
        openapi["components"]["schemas"]["RuntimeMetricsSummary"]["properties"]["runs_by_agent"]["additionalProperties"]
            ["$ref"],
        "#/components/schemas/RuntimeAgentMetrics"
    );
    assert_eq!(
        openapi["components"]["schemas"]["RuntimeMetricsSummary"]["properties"]["tool_calls_by_tool"]
            ["additionalProperties"]["$ref"],
        "#/components/schemas/RuntimeToolMetrics"
    );
    assert_eq!(
        openapi["components"]["schemas"]["RuntimeMetricsSummary"]["properties"]["llm_usage_by_provider"]
            ["additionalProperties"]["$ref"],
        "#/components/schemas/RuntimeLlmProviderMetrics"
    );
    assert_eq!(
        openapi["components"]["schemas"]["HookEvent"]["$ref"],
        "../schemas/hook-event.schema.json"
    );
    assert_eq!(
        openapi["components"]["schemas"]["PromptManifest"]["$ref"],
        "../schemas/prompt-manifest.schema.json"
    );
    assert_eq!(
        openapi["paths"]["/runs/{run_id}"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/AgentRunRecord"
    );
    assert_eq!(
        openapi["paths"]["/runs"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["items"]["$ref"],
        "#/components/schemas/AgentRunRecord"
    );
    assert_eq!(
        openapi["components"]["schemas"]["AgentRunRecord"]["properties"]["idempotency_key"]["pattern"],
        "^idem_[a-f0-9]{64}$"
    );
    assert_eq!(
        openapi["paths"]["/runs/{run_id}/trace"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "../schemas/trace.schema.json"
    );
    assert_eq!(
        openapi["paths"]["/runs/{run_id}/events"]["get"]["responses"]["200"]["content"]["text/event-stream"]
            ["schema"]["description"],
        "Each SSE data frame is a TraceEvent JSON object from trace.schema.json. SSE id fields are monotonically increasing run event cursors."
    );
    assert!(
        openapi["paths"]["/runs/{run_id}/events"]["get"]["parameters"]
            .as_array()
            .expect("run events parameters")
            .iter()
            .any(|param| param["name"] == "after" && param["in"] == "query")
    );
    assert!(
        openapi["paths"]["/runs/{run_id}/events"]["get"]["parameters"]
            .as_array()
            .expect("run events parameters")
            .iter()
            .any(|param| param["name"] == "follow" && param["in"] == "query")
    );
    assert!(
        openapi["paths"]["/runs/{run_id}/events"]["get"]["parameters"]
            .as_array()
            .expect("run events parameters")
            .iter()
            .any(|param| param["name"] == "Last-Event-ID" && param["in"] == "header")
    );
    assert_eq!(
        openapi["paths"]["/chat/turn"]["post"]["responses"]["200"]["content"]["text/event-stream"]
            ["schema"]["description"],
        "Each SSE data frame is a ChatTurnEvent JSON object from chat-turn-event.schema.json."
    );
    assert_eq!(
        openapi["paths"]["/chat/resume"]["post"]["requestBody"]["content"]["application/json"]["schema"]
            ["$ref"],
        "../schemas/chat-resume-request.schema.json"
    );
    assert_eq!(
        openapi["paths"]["/chat/resume"]["post"]["responses"]["200"]["content"]["text/event-stream"]
            ["schema"]["description"],
        "Each SSE data frame is a ChatTurnEvent JSON object from chat-turn-event.schema.json."
    );
    assert_eq!(
        openapi["paths"]["/runs/{run_id}/replay"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ReplayExecutionResponse"
    );
    assert_eq!(
        openapi["components"]["schemas"]["ReplayExecutionResponse"]["properties"]["mode"]["enum"],
        serde_json::json!(["view", "deterministic", "live"])
    );
    assert_eq!(
        openapi["paths"]["/proposals/{proposal_id}/decision"]["post"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["$ref"],
        "#/components/schemas/ProposalDecisionResponse"
    );
    assert_eq!(
        openapi["paths"]["/proposals/{proposal_id}/apply"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ProposalActionResponse"
    );
    assert_eq!(
        openapi["paths"]["/proposals"]["post"]["responses"]["403"]["content"]["application/json"]["schema"]
            ["$ref"],
        "#/components/schemas/ErrorResponse"
    );
    assert_eq!(
        openapi["paths"]["/proposals/{proposal_id}/apply"]["post"]["responses"]["403"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ErrorResponse"
    );
    assert_eq!(
        openapi["components"]["schemas"]["ErrorResponse"]["properties"]["details"]["type"],
        "object"
    );
    assert_eq!(
        openapi["paths"]["/proposals/{proposal_id}/undo"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ProposalActionResponse"
    );
    assert_eq!(
        openapi["paths"]["/sessions"]["post"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["$ref"],
        "#/components/schemas/SessionCreateResponse"
    );
    assert_eq!(
        openapi["paths"]["/sessions/{session_id}"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/SessionShowResponse"
    );
    assert_eq!(
        openapi["paths"]["/sessions/{session_id}/fork"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ThreadForkResponse"
    );
}

fn assert_valid(schema_path: &str, instance_path: &str) {
    let schema = read_json(schema_path);
    let instance = read_json(instance_path);
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    assert_json_valid(&validator, &instance);
}

fn assert_invalid(schema_path: &str, instance_path: &str) {
    let schema = read_json(schema_path);
    let instance = read_json(instance_path);
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    assert!(
        !validator.is_valid(&instance),
        "{instance_path} unexpectedly matched {schema_path}"
    );
}

fn assert_deserializes<T>(instance_path: &str)
where
    T: serde::de::DeserializeOwned,
{
    let _: T = serde_json::from_str(&read_text(instance_path))
        .unwrap_or_else(|e| panic!("{instance_path}: {e}"));
}

fn assert_json_valid(validator: &jsonschema::Validator, instance: &Value) {
    if validator.is_valid(instance) {
        return;
    }
    let errors = validator
        .iter_errors(instance)
        .map(|error| error.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    panic!("JSON schema validation failed:\n{errors}");
}

fn read_json(path: &str) -> Value {
    serde_json::from_str(&read_text(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn read_text(path: &str) -> String {
    std::fs::read_to_string(repo_path(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn repo_path(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}
