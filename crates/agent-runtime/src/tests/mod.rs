use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use agent_core::{
    Agent, AgentContext, AgentError, AgentErrorKind, AgentErrorRecord, AgentEvent,
    AgentEventEmitter, AgentLockStore, AgentRunRecord, AgentRunResult, AgentRunStatus,
    AgentRunStore, AgentSpec, AgentStateAccess, ArtifactKind, ArtifactPublishRequest,
    ArtifactPublisher, ArtifactRef, ArtifactStoreRef, HookEffect, HookEventName, HookKind,
    HookSpec, PROTOCOL_VERSION, PolicyDecision, ProposalCreator, RedactionClassification, RunId,
    RunLease, RunRequest, RunScope, ScheduleSpec, StoreError, SubagentRequest, SubagentRunner,
    ToolCaller, ToolError, WorkflowInputMapping, WorkflowInputTransform, WorkflowRunNode,
    WorkflowRunNodeCompensation, WorkflowRunRequest,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::{Notify, broadcast};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

use super::*;

struct EchoAgent;

#[async_trait]
impl Agent for EchoAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "echo".to_owned(),
            name: "Echo".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.echo".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "echo",
            ctx.now,
            ctx.input,
            Some("echoed input".to_owned()),
        ))
    }
}

struct ParentAgent;

#[async_trait]
impl Agent for ParentAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "parent".to_owned(),
            name: "Parent".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.subagent".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let output = ctx
            .services
            .run_subagent(SubagentRequest {
                agent_id: "echo".to_owned(),
                input: json!({"from": "parent"}),
                run_id: None,
                scope: None,
                workflow: None,
                metadata: json!({}),
            })
            .await
            .map_err(|error| AgentError {
                record: error.record,
            })?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "parent",
            ctx.now,
            output,
            Some("parent delegated".to_owned()),
        ))
    }
}

struct ToolAgent;

#[async_trait]
impl Agent for ToolAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "tool_user".to_owned(),
            name: "Tool User".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.tool".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let output = ctx
            .services
            .call_tool("lookup", ctx.input.clone())
            .await
            .map_err(|error| AgentError {
                record: error.record,
            })?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "tool_user",
            ctx.now,
            json!({"tool_output": output}),
            Some("tool call completed".to_owned()),
        ))
    }
}

struct ArtifactAgent;

#[async_trait]
impl Agent for ArtifactAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "artifact_agent".to_owned(),
            name: "Artifact Agent".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.artifact".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let artifact = ctx
            .services
            .publish_artifact(ArtifactPublishRequest {
                artifact_id: Some("artifact_test_pdf".to_owned()),
                kind: Some(ArtifactKind::Document),
                uri: "artifact://test/report.pdf".to_owned(),
                media_type: Some("application/pdf".to_owned()),
                size_bytes: Some(1024),
                sha256: Some(
                    "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd".to_owned(),
                ),
                redaction_classification: Some(RedactionClassification::Confidential),
                metadata: json!({"title": "Report"}),
            })
            .await?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "artifact_agent",
            ctx.now,
            json!({"artifact_id": artifact.artifact_id}),
            Some("artifact published".to_owned()),
        ))
    }
}

struct UsageAgent;

#[async_trait]
impl Agent for UsageAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "usage_agent".to_owned(),
            name: "Usage Agent".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.usage".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        ctx.services
            .emit_event(AgentEvent {
                kind: "llm_response".to_owned(),
                occurred_at: ctx.now,
                payload: json!({
                    "provider": "openai",
                    "model": "gpt-test",
                    "duration_ms": 42,
                    "usage": {
                        "input_tokens": 11,
                        "output_tokens": 7,
                        "total_tokens": 18,
                        "cost_micros": 123,
                        "cost_currency": "USD"
                    }
                }),
            })
            .await?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "usage_agent",
            ctx.now,
            json!({"ok": true}),
            Some("usage emitted".to_owned()),
        ))
    }
}

#[tokio::test]
async fn runner_executes_agent_and_records_trace() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store.clone(), services);

    let outcome = runner
        .run_once(
            "echo",
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input: json!({"hello": "world"}),
                user: None,
                scope: None,
                trigger: agent_core::TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata: json!({}),
            },
        )
        .await
        .expect("run succeeds");

    assert!(matches!(outcome.result.status, AgentRunStatus::Completed));
    assert_eq!(outcome.result.output, json!({"hello": "world"}));
    assert_eq!(outcome.trace.events.len(), 2);
    assert_eq!(outcome.trace.spans.len(), 1);
    let span = &outcome.trace.spans[0];
    assert_eq!(span.name, "agent.run");
    assert_eq!(span.status, "completed");
    assert_eq!(
        span.attributes["run_id"],
        json!(outcome.result.run_id.0.clone())
    );
    assert_eq!(span.attributes["agent_id"], json!("echo"));
    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert!(
        stored
            .idempotency_key
            .as_deref()
            .is_some_and(|key| key.starts_with("idem_"))
    );
}

#[tokio::test]
async fn runner_uses_explicit_tenant_scope_for_runs() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store.clone(), services);

    let mut request = run_request();
    request.scope = Some(RunScope::Tenant("tenant_acme".to_owned()));
    request.user = Some(agent_core::UserContext {
        user_id: "user_123".to_owned(),
        metadata: json!({}),
    });

    let outcome = runner
        .run_once("echo", request)
        .await
        .expect("tenant scoped run succeeds");

    let record = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert_eq!(record.scope, RunScope::Tenant("tenant_acme".to_owned()));
}

#[tokio::test]
async fn runner_captures_agent_events_and_usage_summary() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(UsageAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let outcome = runner
        .run_once("usage_agent", run_request())
        .await
        .expect("usage run succeeds");

    let llm_event = outcome
        .trace
        .events
        .iter()
        .find(|event| event.kind == "llm_response")
        .expect("llm event is traced");
    assert_eq!(llm_event.payload["agent_id"], json!("usage_agent"));
    assert_eq!(
        llm_event.payload["run_id"],
        json!(outcome.result.run_id.0.clone())
    );

    let usage = outcome.trace.usage_summary.as_ref().expect("usage summary");
    assert_eq!(usage.llm_request_count, 1);
    assert_eq!(usage.input_tokens, 11);
    assert_eq!(usage.output_tokens, 7);
    assert_eq!(usage.total_tokens, 18);
    assert_eq!(usage.cost_micros_by_currency["USD"], 123);
    assert_eq!(usage.by_provider[0].provider, "openai");
    assert_eq!(usage.by_provider[0].model.as_deref(), Some("gpt-test"));

    let llm_span = outcome
        .trace
        .spans
        .iter()
        .find(|span| span.name == "llm.openai")
        .expect("llm span");
    assert_eq!(llm_span.status, "completed");
    assert_eq!(llm_span.duration_ms, 42);
    assert_eq!(llm_span.attributes["provider"], json!("openai"));
    assert_eq!(llm_span.attributes["model"], json!("gpt-test"));
    assert_eq!(llm_span.attributes["total_tokens"], json!(18));
}

#[tokio::test]
async fn runner_executes_workflow_dag_and_populates_dependency_edges() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store.clone(), services);

    let result = runner
        .run_workflow(WorkflowRunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            workflow_id: "workflow_test".to_owned(),
            root_run_id: None,
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            nodes: vec![
                WorkflowRunNode {
                    node_id: "root".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({"step": "root"}),
                    input_mappings: vec![],
                    depends_on: vec![],
                    compensation: None,
                    metadata: json!({"phase": 1}),
                },
                WorkflowRunNode {
                    node_id: "child".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({"step": "child"}),
                    input_mappings: vec![],
                    depends_on: vec!["root".to_owned()],
                    compensation: None,
                    metadata: json!({"phase": 2}),
                },
            ],
            metadata: json!({"case": "dag_success"}),
        })
        .await
        .expect("workflow succeeds");

    assert_eq!(result.status, AgentRunStatus::Completed);
    assert_eq!(result.nodes.len(), 2);
    let root_run_id = result.nodes[0].run_id.as_ref().expect("root run id");
    assert_eq!(result.root_run_id.as_ref(), Some(root_run_id));
    assert_eq!(result.nodes[0].output, json!({"step": "root"}));
    assert_eq!(result.nodes[1].output, json!({"step": "child"}));
    let root_trace = result.nodes[0].trace.as_ref().expect("root node trace");
    assert_eq!(root_trace.run_id, root_run_id.clone());
    assert_eq!(
        root_trace
            .workflow
            .as_ref()
            .and_then(|workflow| workflow.workflow_id.as_deref()),
        Some("workflow_test")
    );

    let child_run_id = result.nodes[1].run_id.as_ref().expect("child run id");
    let child_record = run_store
        .get_run(child_run_id)
        .await
        .expect("run store reads")
        .expect("child run record exists");
    let workflow = child_record.workflow.expect("child workflow");
    assert_eq!(workflow.workflow_id.as_deref(), Some("workflow_test"));
    assert_eq!(workflow.root_run_id.as_ref(), Some(root_run_id));
    assert_eq!(workflow.parent_run_id.as_ref(), Some(root_run_id));
    assert_eq!(workflow.parent_agent_id.as_deref(), Some("echo"));
    assert_eq!(workflow.dependencies.len(), 1);
    assert_eq!(workflow.dependencies[0].run_id, root_run_id.clone());
    assert_eq!(workflow.dependencies[0].edge.as_deref(), Some("depends_on"));
    assert_eq!(
        workflow.dependencies[0].metadata["workflow_node_id"],
        "root"
    );
    assert_eq!(workflow.metadata["workflow_node_id"], "child");
    assert_eq!(
        workflow.metadata["workflow_metadata"]["case"],
        "dag_success"
    );
    assert_eq!(workflow.metadata["node_metadata"]["phase"], 2);
}

#[tokio::test]
async fn runner_runs_ready_workflow_nodes_in_parallel() {
    let counters = Arc::new(ConcurrencyCounters::default());
    let registry = InMemoryAgentRegistry::shared(vec![
        Arc::new(SlowAgent::new("slow_a", counters.clone())),
        Arc::new(SlowAgent::new("slow_b", counters.clone())),
    ]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services).with_policy(ExecutionPolicy {
        timeout: Duration::from_secs(5),
        max_retries: 0,
        retry_backoff: Duration::ZERO,
        max_concurrent_runs: 4,
    });

    let result = runner
        .run_workflow(WorkflowRunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            workflow_id: "workflow_parallel".to_owned(),
            root_run_id: None,
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            nodes: vec![
                WorkflowRunNode {
                    node_id: "left".to_owned(),
                    agent_id: "slow_a".to_owned(),
                    run_id: None,
                    input: json!({"branch": "left"}),
                    input_mappings: vec![],
                    depends_on: vec![],
                    compensation: None,
                    metadata: json!({}),
                },
                WorkflowRunNode {
                    node_id: "right".to_owned(),
                    agent_id: "slow_b".to_owned(),
                    run_id: None,
                    input: json!({"branch": "right"}),
                    input_mappings: vec![],
                    depends_on: vec![],
                    compensation: None,
                    metadata: json!({}),
                },
                WorkflowRunNode {
                    node_id: "left_again".to_owned(),
                    agent_id: "slow_a".to_owned(),
                    run_id: None,
                    input: json!({"branch": "left_again"}),
                    input_mappings: vec![],
                    depends_on: vec![],
                    compensation: None,
                    metadata: json!({}),
                },
            ],
            metadata: json!({"case": "parallel_ready_nodes"}),
        })
        .await
        .expect("workflow succeeds");

    assert_eq!(result.status, AgentRunStatus::Completed);
    assert_eq!(
        result
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        vec!["left", "right", "left_again"]
    );
    assert!(
        result
            .nodes
            .iter()
            .all(|node| node.status == AgentRunStatus::Completed)
    );
    assert!(counters.max_seen.load(Ordering::SeqCst) >= 2);
    assert_eq!(counters.completed.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn runner_maps_workflow_dependency_outputs_into_node_input() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store.clone(), services);

    let result = runner
        .run_workflow(WorkflowRunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            workflow_id: "workflow_dataflow".to_owned(),
            root_run_id: None,
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            nodes: vec![
                WorkflowRunNode {
                    node_id: "collect".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({
                        "customer": {
                            "id": "cust_001",
                            "plan": "enterprise"
                        }
                    }),
                    input_mappings: vec![],
                    depends_on: vec![],
                    compensation: None,
                    metadata: json!({}),
                },
                WorkflowRunNode {
                    node_id: "summarize".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({"format": "brief"}),
                    input_mappings: vec![
                        WorkflowInputMapping {
                            from_node: "collect".to_owned(),
                            from_path: "/customer/id".to_owned(),
                            to_path: "/customer_id".to_owned(),
                            transform: WorkflowInputTransform::None,
                            default: None,
                        },
                        WorkflowInputMapping {
                            from_node: "collect".to_owned(),
                            from_path: "/missing/region".to_owned(),
                            to_path: "/region".to_owned(),
                            transform: WorkflowInputTransform::None,
                            default: Some(json!("us")),
                        },
                    ],
                    depends_on: vec!["collect".to_owned()],
                    compensation: None,
                    metadata: json!({}),
                },
            ],
            metadata: json!({}),
        })
        .await
        .expect("workflow succeeds");

    assert_eq!(result.status, AgentRunStatus::Completed);
    assert_eq!(
        result.nodes[1].output,
        json!({
            "format": "brief",
            "customer_id": "cust_001",
            "region": "us"
        })
    );

    let summarize_run_id = result.nodes[1].run_id.as_ref().expect("summarize run id");
    let summarize_record = run_store
        .get_run(summarize_run_id)
        .await
        .expect("run store reads")
        .expect("summarize run record exists");
    assert_eq!(summarize_record.input["customer_id"], "cust_001");
    assert_eq!(summarize_record.input["region"], "us");
}

#[tokio::test]
async fn runner_applies_workflow_input_mapping_transforms() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let result = runner
        .run_workflow(WorkflowRunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            workflow_id: "workflow_transform".to_owned(),
            root_run_id: None,
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            nodes: vec![
                WorkflowRunNode {
                    node_id: "collect".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({
                        "metrics": {
                            "count": "42",
                            "ratio": "3.5",
                            "enabled": "true",
                            "status": 7,
                            "payload": {"region": "us", "tier": "gold"}
                        }
                    }),
                    input_mappings: vec![],
                    depends_on: vec![],
                    compensation: None,
                    metadata: json!({}),
                },
                WorkflowRunNode {
                    node_id: "normalize".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({}),
                    input_mappings: vec![
                        WorkflowInputMapping {
                            from_node: "collect".to_owned(),
                            from_path: "/metrics/count".to_owned(),
                            to_path: "/count".to_owned(),
                            transform: WorkflowInputTransform::Integer,
                            default: None,
                        },
                        WorkflowInputMapping {
                            from_node: "collect".to_owned(),
                            from_path: "/metrics/ratio".to_owned(),
                            to_path: "/ratio".to_owned(),
                            transform: WorkflowInputTransform::Number,
                            default: None,
                        },
                        WorkflowInputMapping {
                            from_node: "collect".to_owned(),
                            from_path: "/metrics/enabled".to_owned(),
                            to_path: "/enabled".to_owned(),
                            transform: WorkflowInputTransform::Boolean,
                            default: None,
                        },
                        WorkflowInputMapping {
                            from_node: "collect".to_owned(),
                            from_path: "/metrics/status".to_owned(),
                            to_path: "/status".to_owned(),
                            transform: WorkflowInputTransform::String,
                            default: None,
                        },
                        WorkflowInputMapping {
                            from_node: "collect".to_owned(),
                            from_path: "/metrics/payload".to_owned(),
                            to_path: "/payload_json".to_owned(),
                            transform: WorkflowInputTransform::JsonString,
                            default: None,
                        },
                    ],
                    depends_on: vec!["collect".to_owned()],
                    compensation: None,
                    metadata: json!({}),
                },
            ],
            metadata: json!({}),
        })
        .await
        .expect("workflow succeeds");

    assert_eq!(result.status, AgentRunStatus::Completed);
    assert_eq!(result.nodes[1].output["count"], json!(42));
    assert_eq!(result.nodes[1].output["ratio"], json!(3.5));
    assert_eq!(result.nodes[1].output["enabled"], json!(true));
    assert_eq!(result.nodes[1].output["status"], json!("7"));
    assert_eq!(
        result.nodes[1].output["payload_json"],
        json!("{\"region\":\"us\",\"tier\":\"gold\"}")
    );
}

#[tokio::test]
async fn runner_fails_workflow_node_when_input_transform_fails() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let result = runner
        .run_workflow(WorkflowRunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            workflow_id: "workflow_transform_failure".to_owned(),
            root_run_id: None,
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            nodes: vec![
                WorkflowRunNode {
                    node_id: "collect".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({"payload": {"nested": true}}),
                    input_mappings: vec![],
                    depends_on: vec![],
                    compensation: None,
                    metadata: json!({}),
                },
                WorkflowRunNode {
                    node_id: "normalize".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({}),
                    input_mappings: vec![WorkflowInputMapping {
                        from_node: "collect".to_owned(),
                        from_path: "/payload".to_owned(),
                        to_path: "/payload".to_owned(),
                        transform: WorkflowInputTransform::Boolean,
                        default: None,
                    }],
                    depends_on: vec!["collect".to_owned()],
                    compensation: None,
                    metadata: json!({}),
                },
            ],
            metadata: json!({}),
        })
        .await
        .expect("workflow returns failed result");

    assert_eq!(result.status, AgentRunStatus::Failed);
    assert_eq!(result.nodes[1].status, AgentRunStatus::Failed);
    assert_eq!(result.nodes[1].metadata["reason"], "input_mapping_failed");
    assert!(
        result.nodes[1]
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("cannot be converted to boolean"))
    );
}

#[tokio::test]
async fn runner_skips_workflow_nodes_with_failed_dependencies() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let result = runner
        .run_workflow(WorkflowRunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            workflow_id: "workflow_failure".to_owned(),
            root_run_id: None,
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            nodes: vec![
                WorkflowRunNode {
                    node_id: "missing".to_owned(),
                    agent_id: "missing_agent".to_owned(),
                    run_id: None,
                    input: json!({}),
                    input_mappings: vec![],
                    depends_on: vec![],
                    compensation: None,
                    metadata: json!({}),
                },
                WorkflowRunNode {
                    node_id: "after_missing".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({"step": "after"}),
                    input_mappings: vec![],
                    depends_on: vec!["missing".to_owned()],
                    compensation: None,
                    metadata: json!({}),
                },
            ],
            metadata: json!({}),
        })
        .await
        .expect("workflow returns failed result");

    assert_eq!(result.status, AgentRunStatus::Failed);
    assert_eq!(result.nodes[0].status, AgentRunStatus::Failed);
    assert!(
        result.nodes[0]
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("unknown agent"))
    );
    assert_eq!(result.nodes[1].status, AgentRunStatus::Skipped);
    assert!(result.nodes[1].run_id.is_none());
    assert_eq!(
        result.nodes[1].metadata["blocked_dependencies"][0],
        "missing"
    );
}

#[tokio::test]
async fn runner_compensates_completed_workflow_nodes_after_failure() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store.clone(), services);

    let result = runner
        .run_workflow(WorkflowRunRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            workflow_id: "workflow_compensate".to_owned(),
            root_run_id: None,
            user: None,
            scope: None,
            trigger: agent_core::TriggerKind::Manual,
            trigger_envelope: None,
            nodes: vec![
                WorkflowRunNode {
                    node_id: "reserve".to_owned(),
                    agent_id: "echo".to_owned(),
                    run_id: None,
                    input: json!({"reservation_id": "res_1"}),
                    input_mappings: vec![],
                    depends_on: vec![],
                    compensation: Some(WorkflowRunNodeCompensation {
                        agent_id: "echo".to_owned(),
                        run_id: None,
                        strategy: Some("release_reservation".to_owned()),
                        input: json!({"release": "res_1"}),
                        metadata: json!({"reason": "downstream_failure"}),
                    }),
                    metadata: json!({"phase": "reserve"}),
                },
                WorkflowRunNode {
                    node_id: "charge".to_owned(),
                    agent_id: "missing_agent".to_owned(),
                    run_id: None,
                    input: json!({"reservation_id": "res_1"}),
                    input_mappings: vec![],
                    depends_on: vec!["reserve".to_owned()],
                    compensation: None,
                    metadata: json!({"phase": "charge"}),
                },
            ],
            metadata: json!({"case": "compensation"}),
        })
        .await
        .expect("workflow returns failed result");

    assert_eq!(result.status, AgentRunStatus::Failed);
    assert_eq!(result.nodes[0].status, AgentRunStatus::Completed);
    assert_eq!(result.nodes[1].status, AgentRunStatus::Failed);
    let reserve_run_id = result.nodes[0].run_id.as_ref().expect("reserve run id");
    let compensation = result.nodes[0]
        .compensation
        .as_ref()
        .expect("compensation result");
    assert_eq!(compensation.agent_id, "echo");
    assert_eq!(compensation.status, AgentRunStatus::Completed);
    assert_eq!(compensation.output, json!({"release": "res_1"}));
    let compensation_run_id = compensation.run_id.as_ref().expect("compensation run id");
    assert_ne!(compensation_run_id, reserve_run_id);
    assert_eq!(
        compensation
            .trace
            .as_ref()
            .expect("compensation trace")
            .workflow
            .as_ref()
            .and_then(|workflow| workflow.compensation.as_ref())
            .map(|compensation| compensation.compensates_run_id.clone()),
        Some(reserve_run_id.clone())
    );

    let compensation_record = run_store
        .get_run(compensation_run_id)
        .await
        .expect("run store reads")
        .expect("compensation run record exists");
    let workflow = compensation_record
        .workflow
        .expect("compensation workflow metadata");
    assert_eq!(workflow.workflow_id.as_deref(), Some("workflow_compensate"));
    assert_eq!(workflow.root_run_id.as_ref(), result.root_run_id.as_ref());
    assert_eq!(workflow.parent_run_id.as_ref(), Some(reserve_run_id));
    assert_eq!(workflow.parent_agent_id.as_deref(), Some("echo"));
    let run_compensation = workflow.compensation.expect("run compensation");
    assert_eq!(run_compensation.compensates_run_id, reserve_run_id.clone());
    assert_eq!(
        run_compensation.strategy.as_deref(),
        Some("release_reservation")
    );
    assert_eq!(
        run_compensation.metadata["compensation_metadata"]["reason"],
        "downstream_failure"
    );
    assert_eq!(workflow.metadata["workflow_compensation"], true);
}

#[tokio::test]
async fn runner_collects_published_artifact_refs() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(ArtifactAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let outcome = runner
        .run_once("artifact_agent", run_request())
        .await
        .expect("artifact run succeeds");

    assert_eq!(outcome.trace.artifact_refs.len(), 1);
    let artifact = &outcome.trace.artifact_refs[0];
    assert_eq!(artifact.artifact_id, "artifact_test_pdf");
    assert_eq!(artifact.kind, ArtifactKind::Document);
    assert_eq!(
        artifact.redaction_classification,
        RedactionClassification::Confidential
    );
    assert_eq!(
        artifact.store.as_ref().expect("store ref").provider,
        "test_artifact_store"
    );
    assert!(
        outcome
            .trace
            .events
            .iter()
            .any(|event| event.kind == "artifact_published")
    );
}

#[tokio::test]
async fn runner_derives_tool_spans_from_tool_events() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(ToolAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let outcome = runner
        .run_once(
            "tool_user",
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input: json!({"query": "hello"}),
                user: None,
                scope: None,
                trigger: agent_core::TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata: json!({}),
            },
        )
        .await
        .expect("tool run succeeds");

    assert!(
        outcome
            .trace
            .events
            .iter()
            .any(|event| event.kind == "tool_call")
    );
    let run_span_id = outcome.trace.spans[0].span_id.clone();
    let tool_span = outcome
        .trace
        .spans
        .iter()
        .find(|span| span.name == "tool.lookup")
        .expect("tool span exists");
    assert_eq!(
        tool_span.parent_span_id.as_deref(),
        Some(run_span_id.as_str())
    );
    assert_eq!(tool_span.status, "completed");
    assert_eq!(tool_span.attributes["tool_name"], json!("lookup"));
    assert!(
        tool_span.attributes["input_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
}

#[tokio::test]
async fn runner_traces_state_reads_and_writes() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(StateAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services);

    let outcome = runner
        .run_once(
            "stateful",
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input: json!({"counter": 7}),
                user: None,
                scope: None,
                trigger: agent_core::TriggerKind::Manual,
                trigger_envelope: None,
                workflow: None,
                metadata: json!({}),
            },
        )
        .await
        .expect("stateful run succeeds");

    let write = outcome
        .trace
        .events
        .iter()
        .find(|event| event.kind == "state_write")
        .expect("state write event exists");
    assert_eq!(write.payload["agent_id"], "stateful");
    assert_eq!(write.payload["key"], "last_input");
    assert_eq!(write.payload["status"], "completed");
    assert!(write.payload.get("value").is_none());
    assert!(
        write.payload["value_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );

    let read = outcome
        .trace
        .events
        .iter()
        .find(|event| event.kind == "state_read")
        .expect("state read event exists");
    assert_eq!(read.payload["agent_id"], "stateful");
    assert_eq!(read.payload["key"], "last_input");
    assert_eq!(read.payload["found"], true);
    assert!(read.payload.get("value").is_none());
    assert_eq!(outcome.result.output["loaded"]["counter"], 7);

    let run_span_id = outcome.trace.spans[0].span_id.clone();
    let state_span_names = outcome
        .trace
        .spans
        .iter()
        .filter(|span| span.parent_span_id.as_deref() == Some(run_span_id.as_str()))
        .map(|span| span.name.as_str())
        .collect::<Vec<_>>();
    assert!(state_span_names.contains(&"state.write"));
    assert!(state_span_names.contains(&"state.read"));
}

#[tokio::test]
async fn runner_observe_hooks_record_invocations() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let hooks = HookManager::new(vec![HookRegistration::native(
        hook_spec(
            "record_run_start",
            HookEventName::RunStart,
            HookEffect::Observe,
        ),
        Arc::new(AllowHook),
    )]);
    let runner = AgentRunner::new(registry, run_store, services).with_hooks(hooks);

    let outcome = runner
        .run_once("echo", run_request())
        .await
        .expect("run succeeds");

    let hook = outcome
        .trace
        .events
        .iter()
        .find(|event| event.kind == "hook_invocation")
        .expect("hook invocation traced");
    assert_eq!(hook.payload["hook_name"], "record_run_start");
    assert_eq!(hook.payload["status"], "completed");
    assert_eq!(hook.payload["hook_event"], "RunStart");
}

#[tokio::test]
async fn runner_observes_agent_step_hooks() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let hooks = HookManager::new(vec![
        HookRegistration::native(
            hook_spec(
                "before_agent_step",
                HookEventName::BeforeAgentStep,
                HookEffect::Observe,
            ),
            Arc::new(AllowHook),
        ),
        HookRegistration::native(
            hook_spec(
                "after_agent_step",
                HookEventName::AfterAgentStep,
                HookEffect::Observe,
            ),
            Arc::new(AllowHook),
        ),
    ]);
    let runner = AgentRunner::new(registry, run_store, services).with_hooks(hooks);

    let outcome = runner
        .run_once("echo", run_request())
        .await
        .expect("run succeeds");

    let before = outcome
        .trace
        .events
        .iter()
        .find(|event| {
            event.kind == "hook_invocation" && event.payload["hook_name"] == "before_agent_step"
        })
        .expect("before step hook invocation traced");
    assert_eq!(before.payload["hook_event"], "BeforeAgentStep");
    assert_eq!(before.payload["output"]["input"]["agent_id"], "echo");
    assert_eq!(before.payload["output"]["input"]["attempt"], 1);

    let after = outcome
        .trace
        .events
        .iter()
        .find(|event| {
            event.kind == "hook_invocation" && event.payload["hook_name"] == "after_agent_step"
        })
        .expect("after step hook invocation traced");
    assert_eq!(after.payload["hook_event"], "AfterAgentStep");
    assert_eq!(after.payload["output"]["input"]["status"], "completed");
    assert_eq!(after.payload["output"]["input"]["attempt"], 1);
}

#[tokio::test]
async fn policy_hook_can_deny_agent_step() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let hooks = HookManager::new(vec![HookRegistration::native(
        hook_spec(
            "deny_agent_step",
            HookEventName::BeforeAgentStep,
            HookEffect::Policy,
        ),
        Arc::new(DenyHook),
    )]);
    let runner = AgentRunner::new(registry, run_store, services).with_hooks(hooks);

    let outcome = runner
        .run_once("echo", run_request())
        .await
        .expect("denied run returns outcome");

    assert_eq!(outcome.result.status, AgentRunStatus::Failed);
    assert_eq!(
        outcome.result.error.as_ref().expect("run error").code,
        "policy_denied"
    );
    assert!(
        outcome
            .trace
            .events
            .iter()
            .any(|event| event.kind == "hook_invocation"
                && event.payload["hook_name"] == "deny_agent_step"
                && event.payload["hook_event"] == "BeforeAgentStep")
    );
    assert!(!outcome.trace.events.iter().any(|event| {
        event.kind == "hook_invocation" && event.payload["hook_event"] == "AfterAgentStep"
    }));
}

#[tokio::test]
async fn policy_hook_can_deny_state_save() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(StateAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let hooks = HookManager::new(vec![HookRegistration::native(
        hook_spec(
            "deny_state_save",
            HookEventName::BeforeStateSave,
            HookEffect::Policy,
        ),
        Arc::new(DenyHook),
    )]);
    let runner = AgentRunner::new(registry, run_store, services).with_hooks(hooks);

    let outcome = runner
        .run_once("stateful", run_request())
        .await
        .expect("denied run returns outcome");

    assert_eq!(outcome.result.status, AgentRunStatus::Failed);
    assert_eq!(
        outcome.result.error.as_ref().expect("run error").code,
        "policy_denied"
    );
    assert!(
        outcome
            .trace
            .events
            .iter()
            .any(|event| event.kind == "hook_invocation"
                && event.payload["hook_name"] == "deny_state_save")
    );
    assert!(
        !outcome
            .trace
            .events
            .iter()
            .any(|event| event.kind == "state_write")
    );
}

#[tokio::test]
async fn native_subagent_service_executes_subagent() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(ParentAgent), Arc::new(EchoAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store.clone(), services);
    let mut request = run_request();
    request.scope = Some(RunScope::Tenant("tenant_acme".to_owned()));

    let outcome = runner
        .run_once("parent", request)
        .await
        .expect("parent run succeeds");

    assert_eq!(outcome.result.status, AgentRunStatus::Completed);
    assert_eq!(outcome.result.output["result"]["agent_id"], "echo");
    assert_eq!(outcome.result.output["result"]["output"]["from"], "parent");
    let event_kinds = outcome
        .trace
        .events
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"subagent_started"));
    assert!(event_kinds.contains(&"subagent_finished"));
    let child_run_id = outcome.result.output["result"]["run_id"]
        .as_str()
        .expect("child run id");
    let child = run_store
        .get_run(&RunId(child_run_id.to_owned()))
        .await
        .expect("run store reads")
        .expect("child run exists");
    assert_eq!(child.agent_id, "echo");
    assert_eq!(child.scope, RunScope::Tenant("tenant_acme".to_owned()));
    let child_workflow = child.workflow.expect("child workflow exists");
    assert_eq!(
        child_workflow
            .parent_run_id
            .as_ref()
            .map(|run_id| run_id.0.as_str()),
        Some(outcome.result.run_id.0.as_str())
    );
    assert_eq!(
        child_workflow
            .root_run_id
            .as_ref()
            .map(|run_id| run_id.0.as_str()),
        Some(outcome.result.run_id.0.as_str())
    );
    assert_eq!(child_workflow.parent_agent_id.as_deref(), Some("parent"));
    assert_eq!(
        outcome.result.output["result"]["workflow"]["parent_run_id"],
        outcome.result.run_id.0
    );
    assert_eq!(
        outcome.result.output["trace"]["workflow"]["parent_run_id"],
        outcome.result.run_id.0
    );
}

#[test]
fn run_idempotency_key_is_stable_for_retry_material() {
    let scope = RunScope::Global;
    let request = RunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: None,
        input: json!({"message": "ignored"}),
        user: None,
        scope: None,
        trigger: agent_core::TriggerKind::Scheduled,
        trigger_envelope: None,
        workflow: None,
        metadata: json!({"scheduled_for": "2026-06-28T09:00:00Z"}),
    };
    let same_retry = RunRequest {
        input: json!({"message": "different input does not affect retry identity"}),
        ..request.clone()
    };
    let different_schedule = RunRequest {
        metadata: json!({"scheduled_for": "2026-06-28T10:00:00Z"}),
        ..request.clone()
    };

    let run_id = RunId("run_test".to_owned());
    let first = run_idempotency_key("echo", &scope, &request, &run_id);
    let second = run_idempotency_key("echo", &scope, &same_retry, &run_id);
    let third = run_idempotency_key("echo", &scope, &different_schedule, &run_id);

    assert_eq!(first, second);
    assert_ne!(first, third);
    assert_eq!(first.len(), "idem_".len() + 64);
}

#[test]
fn run_idempotency_key_uses_external_trigger_identity() {
    let scope = RunScope::Global;
    let request = RunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: None,
        input: json!({"message": "ignored"}),
        user: None,
        scope: None,
        trigger: agent_core::TriggerKind::Webhook,
        trigger_envelope: Some(agent_core::TriggerEnvelope {
            source: "github.webhook".to_owned(),
            id: Some("evt_1".to_owned()),
            received_at: None,
            payload: json!({"action": "opened"}),
            metadata: json!({}),
        }),
        workflow: None,
        metadata: json!({}),
    };
    let same_retry = RunRequest {
        input: json!({"message": "different input"}),
        ..request.clone()
    };
    let different_event = RunRequest {
        trigger_envelope: Some(agent_core::TriggerEnvelope {
            id: Some("evt_2".to_owned()),
            ..request
                .trigger_envelope
                .clone()
                .expect("request has trigger envelope")
        }),
        ..request.clone()
    };
    let payload_without_id = RunRequest {
        trigger_envelope: Some(agent_core::TriggerEnvelope {
            id: None,
            payload: json!({"action": "closed"}),
            ..request
                .trigger_envelope
                .clone()
                .expect("request has trigger envelope")
        }),
        ..request.clone()
    };

    let run_id = RunId("run_test".to_owned());
    let first = run_idempotency_key("echo", &scope, &request, &run_id);
    let second = run_idempotency_key("echo", &scope, &same_retry, &run_id);
    let third = run_idempotency_key("echo", &scope, &different_event, &run_id);
    let fourth = run_idempotency_key("echo", &scope, &payload_without_id, &run_id);

    assert_eq!(first, second);
    assert_ne!(first, third);
    assert_ne!(first, fourth);
}

#[tokio::test]
async fn runner_respects_max_concurrent_runs_policy() {
    let counters = Arc::new(ConcurrencyCounters::default());
    let registry =
        InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::new("slow", counters.clone()))]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = Arc::new(AgentRunner::new(registry, run_store, services).with_policy(
        ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        },
    ));

    let first = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run_once("slow", run_request()).await })
    };
    let second = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run_once("slow", run_request()).await })
    };

    first
        .await
        .expect("first task joins")
        .expect("first run succeeds");
    second
        .await
        .expect("second task joins")
        .expect("second run succeeds");

    assert_eq!(counters.max_seen.load(Ordering::SeqCst), 1);
    assert_eq!(counters.completed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn runner_skips_duplicate_agent_scope_when_lease_is_active() {
    let counters = Arc::new(ConcurrencyCounters::default());
    let registry =
        InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::new("slow", counters.clone()))]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = Arc::new(AgentRunner::new(registry, run_store, services).with_policy(
        ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 2,
        },
    ));

    let first = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run_once("slow", run_request()).await })
    };
    sleep(Duration::from_millis(10)).await;
    let second = runner
        .run_once("slow", run_request())
        .await
        .expect("second run returns skipped outcome");
    let first = first
        .await
        .expect("first task joins")
        .expect("first run succeeds");

    let statuses = [first.result.status, second.result.status];
    assert!(statuses.contains(&AgentRunStatus::Completed));
    assert!(statuses.contains(&AgentRunStatus::Skipped));
    assert_eq!(counters.max_seen.load(Ordering::SeqCst), 1);
    assert_eq!(counters.completed.load(Ordering::SeqCst), 1);
    assert_eq!(second.trace.events[0].kind, "run_skipped");
}

#[tokio::test]
async fn sqlite_shared_store_coordinates_duplicate_agent_scope_across_runner_handles() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp
        .path()
        .join("runtime.sqlite")
        .to_str()
        .expect("utf8 temp path")
        .to_owned();
    let counters = Arc::new(ConcurrencyCounters::default());
    let first_started = Arc::new(Notify::new());

    let first_store = Arc::new(
        agent_store::SqliteStore::open(db_path.as_str())
            .await
            .expect("first sqlite handle opens"),
    );
    let second_store = Arc::new(
        agent_store::SqliteStore::open(db_path.as_str())
            .await
            .expect("second sqlite handle opens"),
    );

    let first_runner = Arc::new(
        AgentRunner::new(
            InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::with_started_notify(
                "slow",
                counters.clone(),
                first_started.clone(),
            ))]),
            first_store.clone(),
            Arc::new(NoopServices {
                state_store: agent_store::InMemoryStateStore::shared(),
            }),
        )
        .with_lock_store(first_store.clone())
        .with_policy(ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 2,
        }),
    );
    let second_runner = AgentRunner::new(
        InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::new("slow", counters.clone()))]),
        second_store.clone(),
        Arc::new(NoopServices {
            state_store: agent_store::InMemoryStateStore::shared(),
        }),
    )
    .with_lock_store(second_store.clone())
    .with_policy(ExecutionPolicy {
        timeout: Duration::from_secs(5),
        max_retries: 0,
        retry_backoff: Duration::ZERO,
        max_concurrent_runs: 2,
    });

    let first = {
        let first_runner = first_runner.clone();
        tokio::spawn(async move { first_runner.run_once("slow", run_request()).await })
    };
    timeout(Duration::from_secs(1), first_started.notified())
        .await
        .expect("first sqlite-backed run enters the slow agent before duplicate run starts");
    let second = second_runner
        .run_once("slow", run_request())
        .await
        .expect("second runner returns outcome");
    let first = first
        .await
        .expect("first task joins")
        .expect("first runner returns outcome");

    let statuses = [first.result.status, second.result.status];
    assert!(statuses.contains(&AgentRunStatus::Completed));
    assert!(statuses.contains(&AgentRunStatus::Skipped));
    assert_eq!(counters.max_seen.load(Ordering::SeqCst), 1);
    assert_eq!(counters.completed.load(Ordering::SeqCst), 1);

    let persisted = agent_store::SqliteStore::open(db_path.as_str())
        .await
        .expect("verification sqlite handle opens")
        .list_runs(Some("slow"), None)
        .await
        .expect("runs list from sqlite");
    assert_eq!(persisted.len(), 2);
    assert_eq!(
        persisted
            .iter()
            .filter(|run| run.status == AgentRunStatus::Completed)
            .count(),
        1
    );
    assert_eq!(
        persisted
            .iter()
            .filter(|run| run.status == AgentRunStatus::Skipped)
            .count(),
        1
    );
}

#[tokio::test]
async fn runner_skips_duplicate_workflow_scope_when_lease_is_active() {
    let counters = Arc::new(ConcurrencyCounters::default());
    let registry =
        InMemoryAgentRegistry::shared(vec![Arc::new(SlowAgent::new("slow", counters.clone()))]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = Arc::new(AgentRunner::new(registry, run_store, services).with_policy(
        ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 2,
        },
    ));

    let first = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run_workflow(slow_workflow_request()).await })
    };
    sleep(Duration::from_millis(10)).await;
    let second = runner
        .run_workflow(slow_workflow_request())
        .await
        .expect("second workflow returns skipped result");
    let first = first
        .await
        .expect("first workflow task joins")
        .expect("first workflow succeeds");
    let third = runner
        .run_workflow(slow_workflow_request())
        .await
        .expect("workflow lease is released after first run");

    assert_eq!(first.status, AgentRunStatus::Completed);
    assert_eq!(second.status, AgentRunStatus::Skipped);
    assert_eq!(third.status, AgentRunStatus::Completed);
    assert_eq!(second.nodes.len(), 1);
    assert_eq!(second.nodes[0].status, AgentRunStatus::Skipped);
    assert_eq!(second.nodes[0].metadata["reason"], "workflow_lease_active");
    assert_eq!(
        second.nodes[0].metadata["workflow"]["scope"]["type"],
        "tenant"
    );
    assert_eq!(
        second.nodes[0].metadata["workflow"]["scope"]["id"],
        "tenant_slow"
    );
    assert_eq!(counters.completed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn runner_renews_run_lease_while_active() {
    let lock_store = Arc::new(CountingLockStore::default());
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(LeaseProbeAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services)
        .with_lock_store(lock_store.clone())
        .with_policy(ExecutionPolicy {
            timeout: Duration::from_millis(500),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });

    let outcome = runner
        .run_once("lease_probe", run_request())
        .await
        .expect("lease probe run succeeds");

    assert_eq!(outcome.result.status, AgentRunStatus::Completed);
    assert_eq!(lock_store.release_count.load(Ordering::SeqCst), 1);
    assert!(
        lock_store
            .renewed_keys()
            .iter()
            .any(|key| key == "agent:lease_probe:scope:global")
    );
}

#[tokio::test]
async fn runner_renews_workflow_lease_while_active() {
    let lock_store = Arc::new(CountingLockStore::default());
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(LeaseProbeAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = AgentRunner::new(registry, run_store, services)
        .with_lock_store(lock_store.clone())
        .with_policy(ExecutionPolicy {
            timeout: Duration::from_millis(500),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });

    let result = runner
        .run_workflow(lease_probe_workflow_request())
        .await
        .expect("lease probe workflow succeeds");

    assert_eq!(result.status, AgentRunStatus::Completed);
    let renewed_keys = lock_store.renewed_keys();
    assert!(
        renewed_keys
            .iter()
            .any(|key| { key == "workflow:workflow_lease_probe:scope:tenant:tenant_lease" })
    );
    assert!(
        renewed_keys
            .iter()
            .any(|key| key == "agent:lease_probe:scope:tenant:tenant_lease")
    );
}

#[test]
fn scheduler_fires_cron_once_per_matching_minute() {
    let scheduler = AgentScheduler;
    let spec = scheduled_spec(ScheduleSpec::Cron {
        expression: "30 9 * * MON-FRI".to_owned(),
        timezone: "UTC".to_owned(),
    });
    let now = parse_rfc3339("2026-07-03T09:30:45Z");

    assert!(scheduler.should_fire(&spec, now, None));
    assert!(!scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T09:31:00Z"), None));
    assert!(!scheduler.should_fire(
        &spec,
        now,
        Some(&run_record_started_at(parse_rfc3339(
            "2026-07-03T09:30:05Z"
        ))),
    ));
    assert!(scheduler.should_fire(
        &spec,
        now,
        Some(&run_record_started_at(parse_rfc3339(
            "2026-07-03T09:29:59Z"
        ))),
    ));
}

#[test]
fn scheduler_applies_fixed_offset_timezone_for_cron() {
    let scheduler = AgentScheduler;
    let spec = scheduled_spec(ScheduleSpec::Cron {
        expression: "0 9 * * *".to_owned(),
        timezone: "+08:00".to_owned(),
    });

    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T01:00:00Z"), None));
    assert!(!scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T09:00:00Z"), None));
}

#[test]
fn scheduler_applies_named_timezone_database_for_cron() {
    let scheduler = AgentScheduler;
    let spec = scheduled_spec(ScheduleSpec::Cron {
        expression: "0 9 * * *".to_owned(),
        timezone: "America/New_York".to_owned(),
    });

    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T13:00:00Z"), None));
    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-01-05T14:00:00Z"), None));
    assert!(!scheduler.should_fire(&spec, parse_rfc3339("2026-07-03T14:00:00Z"), None));
}

#[test]
fn scheduler_uses_standard_cron_or_for_restricted_day_fields() {
    let scheduler = AgentScheduler;
    let spec = scheduled_spec(ScheduleSpec::Cron {
        expression: "30 9 1 * MON".to_owned(),
        timezone: "UTC".to_owned(),
    });

    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-07-01T09:30:00Z"), None));
    assert!(scheduler.should_fire(&spec, parse_rfc3339("2026-07-06T09:30:00Z"), None));
    assert!(!scheduler.should_fire(&spec, parse_rfc3339("2026-07-02T09:30:00Z"), None));
}

#[tokio::test]
async fn runner_releases_lease_when_final_run_update_fails() {
    let lock_store = Arc::new(CountingLockStore::default());
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]);
    let run_store = Arc::new(FailingUpdateRunStore);
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner =
        AgentRunner::new(registry, run_store, services).with_lock_store(lock_store.clone());

    let error = match runner.run_once("echo", run_request()).await {
        Ok(_) => panic!("final run update should fail"),
        Err(error) => error,
    };

    assert_eq!(error.record.code, "internal_error");
    assert_eq!(lock_store.release_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runner_retries_retryable_agent_errors() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(FlakyAgent {
        attempts: attempts.clone(),
    })]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner =
        AgentRunner::new(registry, run_store.clone(), services).with_policy(ExecutionPolicy {
            timeout: Duration::from_secs(5),
            max_retries: 1,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });

    let outcome = runner
        .run_once("flaky", run_request())
        .await
        .expect("retryable run eventually succeeds");

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(outcome.result.status, AgentRunStatus::Completed);
    assert_eq!(outcome.result.output["attempt"], 2);
    let event_kinds = outcome
        .trace
        .events
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        event_kinds
            .iter()
            .filter(|kind| **kind == "run_attempt_started")
            .count(),
        2
    );
    assert!(event_kinds.contains(&"run_retry_scheduled"));

    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert_eq!(stored.status, AgentRunStatus::Completed);
    assert_eq!(stored.output["attempt"], 2);
}

#[tokio::test]
async fn runner_can_cancel_active_run_and_broadcast_events() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(BlockingAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner =
        AgentRunner::new(registry, run_store.clone(), services).with_policy(ExecutionPolicy {
            timeout: Duration::from_secs(30),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });
    let cancellation = CancellationToken::new();
    let (events, mut receiver) = broadcast::channel(16);
    let event_buffer = Arc::new(TraceEventBuffer::default());
    let request = RunRequest {
        run_id: Some(RunId("run_cancel_test".to_owned())),
        ..run_request()
    };
    let run = tokio::spawn({
        let cancellation = cancellation.clone();
        let event_buffer = event_buffer.clone();
        async move {
            let control = RunControl {
                cancellation,
                trace_events: Some(events),
                trace_event_buffer: Some(event_buffer.clone()),
            };
            runner
                .run_once_with_control("blocking", request, control)
                .await
        }
    });

    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("run_started event arrives")
            .expect("event channel stays open");
        if event.kind == "run_started" {
            break;
        }
    }
    assert!(
        event_buffer
            .events()
            .await
            .iter()
            .any(|event| event.kind == "run_started"),
        "trace event buffer should observe events before they are broadcast"
    );
    cancellation.cancel();

    let outcome = run
        .await
        .expect("run task joins")
        .expect("cancelled run returns outcome");

    assert_eq!(outcome.result.status, AgentRunStatus::Cancelled);
    assert_eq!(
        outcome.result.error.as_ref().expect("cancel error").code,
        "cancelled"
    );
    let event_kinds = outcome
        .trace
        .events
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"run_started"));
    assert!(event_kinds.contains(&"run_cancel_requested"));
    assert!(event_kinds.contains(&"run_cancelled"));
    assert!(event_kinds.contains(&"run_finished"));
    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert_eq!(stored.status, AgentRunStatus::Cancelled);
}

#[tokio::test]
async fn runner_cancels_run_when_lease_ownership_is_lost() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(BlockingAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let services = Arc::new(NoopServices {
        state_store: agent_store::InMemoryStateStore::shared(),
    });
    let runner = AgentRunner::new(registry, run_store.clone(), services)
        .with_lock_store(Arc::new(LosingLockStore))
        .with_policy(ExecutionPolicy {
            timeout: Duration::from_millis(300),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        });

    let outcome = timeout(
        Duration::from_secs(2),
        runner.run_once("blocking", run_request()),
    )
    .await
    .expect("lease loss cancels promptly")
    .expect("cancelled run returns an outcome");

    assert_eq!(outcome.result.status, AgentRunStatus::Cancelled);
    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run reads")
        .expect("run exists");
    assert_eq!(stored.status, AgentRunStatus::Cancelled);
}

#[tokio::test]
async fn runner_deduplicates_external_trigger_identity() {
    let executions = Arc::new(AtomicUsize::new(0));
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(CountingAgent {
        executions: executions.clone(),
    })]);
    let runner = AgentRunner::new(
        registry,
        agent_store::InMemoryRunStore::shared(),
        Arc::new(NoopServices {
            state_store: agent_store::InMemoryStateStore::shared(),
        }),
    );
    let request = RunRequest {
        trigger: agent_core::TriggerKind::Queue,
        trigger_envelope: Some(agent_core::TriggerEnvelope {
            source: "orders".to_owned(),
            id: Some("message-1".to_owned()),
            received_at: None,
            payload: json!({}),
            metadata: json!({}),
        }),
        ..run_request()
    };

    let first = runner
        .run_once("counting", request.clone())
        .await
        .expect("first delivery runs");
    let second = runner
        .run_once("counting", request)
        .await
        .expect("duplicate delivery resolves");

    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(first.result.run_id, second.result.run_id);
    assert!(first.should_persist_trace());
    assert!(!second.should_persist_trace());
    assert_eq!(second.trace.events[0].kind, "run_deduplicated");
}

#[tokio::test]
async fn runner_finalizes_running_record_when_policy_hook_errors() {
    let run_id = RunId("run_hook_failure".to_owned());
    let run_store = agent_store::InMemoryRunStore::shared();
    let runner = AgentRunner::new(
        InMemoryAgentRegistry::shared(vec![Arc::new(EchoAgent)]),
        run_store.clone(),
        Arc::new(NoopServices {
            state_store: agent_store::InMemoryStateStore::shared(),
        }),
    )
    .with_hooks(HookManager::new(vec![HookRegistration::native(
        hook_spec(
            "failing_policy",
            HookEventName::BeforeAgentStep,
            HookEffect::Policy,
        ),
        Arc::new(FailingHook),
    )]));

    let result = runner
        .run_once(
            "echo",
            RunRequest {
                run_id: Some(run_id.clone()),
                ..run_request()
            },
        )
        .await;
    assert!(result.is_err(), "policy infrastructure failure propagates");

    let stored = run_store
        .get_run(&run_id)
        .await
        .expect("run reads")
        .expect("run exists");
    assert_eq!(stored.status, AgentRunStatus::Failed);
    assert!(stored.finished_at.is_some());
}

#[tokio::test]
async fn runner_observes_persisted_cancellation_request() {
    let registry = InMemoryAgentRegistry::shared(vec![Arc::new(BlockingAgent)]);
    let run_store = agent_store::InMemoryRunStore::shared();
    let state_store = agent_store::InMemoryStateStore::shared();
    let services = Arc::new(NoopServices { state_store });
    let runner = Arc::new(
        AgentRunner::new(registry, run_store.clone(), services).with_policy(ExecutionPolicy {
            timeout: Duration::from_secs(30),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        }),
    );
    let (events, mut receiver) = broadcast::channel(16);
    let run_id = RunId("run_store_cancel_test".to_owned());
    let request = RunRequest {
        run_id: Some(run_id.clone()),
        ..run_request()
    };
    let run = tokio::spawn({
        let runner = runner.clone();
        async move {
            let control = RunControl {
                trace_events: Some(events),
                ..RunControl::default()
            };
            runner
                .run_once_with_control("blocking", request, control)
                .await
        }
    });

    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("run_started event arrives")
            .expect("event channel stays open");
        if event.kind == "run_started" {
            break;
        }
    }
    let mut stored = run_store
        .get_run(&run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    stored.request_cancellation(OffsetDateTime::now_utc(), Some("test".to_owned()));
    let expected_version = stored.version;
    stored.version += 1;
    run_store
        .update_run(stored, expected_version)
        .await
        .expect("run cancellation intent persists")
        .then_some(())
        .expect("run cancellation update wins");

    let outcome = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("persisted cancellation is observed")
        .expect("run task joins")
        .expect("cancelled run returns outcome");

    assert_eq!(outcome.result.status, AgentRunStatus::Cancelled);
    assert_eq!(
        outcome.result.error.as_ref().expect("cancel error").code,
        "cancelled"
    );
    let event_kinds = outcome
        .trace
        .events
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"run_cancel_requested"));
    assert!(event_kinds.contains(&"run_cancelled"));
    let stored = run_store
        .get_run(&outcome.result.run_id)
        .await
        .expect("run store reads")
        .expect("run record exists");
    assert_eq!(stored.status, AgentRunStatus::Cancelled);
    assert_eq!(stored.metadata["control"]["cancel_requested"], true);
    assert_eq!(stored.metadata["control"]["cancel_requested_by"], "test");
}

#[tokio::test]
async fn recovery_abandons_only_stale_running_runs() {
    let store = agent_store::InMemoryRunStore::shared();
    let now = OffsetDateTime::now_utc();
    store
        .create_run(AgentRunRecord {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            version: 1,
            run_id: RunId("run_stale".to_owned()),
            idempotency_key: Some("idem_stale".to_owned()),
            agent_id: "echo".to_owned(),
            status: AgentRunStatus::Running,
            scope: RunScope::Global,
            started_at: now - time::Duration::seconds(120),
            finished_at: None,
            input: json!({"message": "old"}),
            output: json!({}),
            error: None,
            workflow: None,
            metadata: json!({}),
        })
        .await
        .expect("stale run saved");
    store
        .create_run(AgentRunRecord {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            version: 1,
            run_id: RunId("run_fresh".to_owned()),
            idempotency_key: Some("idem_fresh".to_owned()),
            agent_id: "echo".to_owned(),
            status: AgentRunStatus::Running,
            scope: RunScope::Global,
            started_at: now,
            finished_at: None,
            input: json!({"message": "fresh"}),
            output: json!({}),
            error: None,
            workflow: None,
            metadata: json!({}),
        })
        .await
        .expect("fresh run saved");
    store
        .create_run(AgentRunRecord {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            version: 1,
            run_id: RunId("run_completed_old".to_owned()),
            idempotency_key: Some("idem_completed_old".to_owned()),
            agent_id: "echo".to_owned(),
            status: AgentRunStatus::Completed,
            scope: RunScope::Global,
            started_at: now - time::Duration::seconds(120),
            finished_at: Some(now - time::Duration::seconds(119)),
            input: json!({"message": "already done"}),
            output: json!({}),
            error: None,
            workflow: None,
            metadata: json!({}),
        })
        .await
        .expect("completed run saved");

    let report = recover_stale_runs(
        store.as_ref(),
        &ExecutionPolicy {
            timeout: Duration::from_secs(60),
            max_retries: 0,
            retry_backoff: Duration::ZERO,
            max_concurrent_runs: 1,
        },
    )
    .await
    .expect("recovery succeeds");

    // Recovery asks the store for running candidates instead of scanning every
    // historical run record.
    assert_eq!(report.scanned_runs, 2);
    assert_eq!(report.abandoned_count, 1);
    assert_eq!(report.recovered_runs[0].run_id.0, "run_stale");
    let stale = store
        .get_run(&RunId("run_stale".to_owned()))
        .await
        .expect("stale run reads")
        .expect("stale run exists");
    assert_eq!(stale.status, AgentRunStatus::Abandoned);
    assert_eq!(
        stale.error.expect("stale run has error").code,
        "stale_running_run_abandoned"
    );
    let fresh = store
        .get_run(&RunId("run_fresh".to_owned()))
        .await
        .expect("fresh run reads")
        .expect("fresh run exists");
    assert_eq!(fresh.status, AgentRunStatus::Running);
    assert!(fresh.finished_at.is_none());
}

struct StateAgent;

#[async_trait]
impl Agent for StateAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "stateful".to_owned(),
            name: "Stateful".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.state".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        ctx.services
            .save_state("last_input", ctx.input.clone())
            .await?;
        let loaded = ctx.services.load_state("last_input").await?;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "stateful",
            ctx.now,
            json!({"loaded": loaded}),
            Some("stateful run completed".to_owned()),
        ))
    }
}

struct FlakyAgent {
    attempts: Arc<AtomicUsize>,
}

#[async_trait]
impl Agent for FlakyAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "flaky".to_owned(),
            name: "Flaky".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.flaky".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt == 1 {
            return Err(AgentError {
                record: Box::new(AgentErrorRecord {
                    kind: AgentErrorKind::TransientExternalError,
                    code: "transient_test_error".to_owned(),
                    message: "transient failure".to_owned(),
                    retryable: true,
                    details: json!({"attempt": attempt}),
                }),
            });
        }
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "flaky",
            ctx.now,
            json!({"attempt": attempt}),
            Some("flaky run completed".to_owned()),
        ))
    }
}

struct BlockingAgent;

#[async_trait]
impl Agent for BlockingAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "blocking".to_owned(),
            name: "Blocking".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.blocking".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        ctx.trace
            .emit(agent_core::TraceEvent::new("blocking.started", json!({})))
            .await?;
        sleep(Duration::from_secs(60)).await;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "blocking",
            ctx.now,
            json!({}),
            Some("blocking completed".to_owned()),
        ))
    }
}

struct CountingAgent {
    executions: Arc<AtomicUsize>,
}

#[async_trait]
impl Agent for CountingAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "counting".to_owned(),
            name: "Counting".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: Vec::new(),
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "counting",
            ctx.now,
            json!({}),
            None,
        ))
    }
}

#[derive(Default)]
struct ConcurrencyCounters {
    current: AtomicUsize,
    max_seen: AtomicUsize,
    completed: AtomicUsize,
}

struct SlowAgent {
    id: String,
    counters: Arc<ConcurrencyCounters>,
    started: Option<Arc<Notify>>,
}

impl SlowAgent {
    fn new(id: impl Into<String>, counters: Arc<ConcurrencyCounters>) -> Self {
        Self {
            id: id.into(),
            counters,
            started: None,
        }
    }

    fn with_started_notify(
        id: impl Into<String>,
        counters: Arc<ConcurrencyCounters>,
        started: Arc<Notify>,
    ) -> Self {
        Self {
            id: id.into(),
            counters,
            started: Some(started),
        }
    }
}

#[async_trait]
impl Agent for SlowAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: self.id.clone(),
            name: "Slow".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.slow".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        let current = self.counters.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.counters.max_seen.fetch_max(current, Ordering::SeqCst);
        if let Some(started) = &self.started {
            started.notify_one();
        }
        sleep(Duration::from_millis(100)).await;
        self.counters.current.fetch_sub(1, Ordering::SeqCst);
        self.counters.completed.fetch_add(1, Ordering::SeqCst);
        Ok(AgentRunResult::completed(
            ctx.run_id,
            self.id.clone(),
            ctx.now,
            ctx.input,
            Some("slow run completed".to_owned()),
        ))
    }
}

struct LeaseProbeAgent;

#[async_trait]
impl Agent for LeaseProbeAgent {
    fn spec(&self) -> AgentSpec {
        AgentSpec {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id: "lease_probe".to_owned(),
            name: "Lease Probe".to_owned(),
            description: None,
            version: "0.1.0".to_owned(),
            schedule: ScheduleSpec::Manual,
            capabilities: vec!["debug.lease".to_owned()],
            metadata: json!({}),
        }
    }

    async fn run(&self, ctx: AgentContext) -> Result<AgentRunResult, AgentError> {
        sleep(Duration::from_millis(250)).await;
        Ok(AgentRunResult::completed(
            ctx.run_id,
            "lease_probe",
            ctx.now,
            ctx.input,
            Some("lease probe completed".to_owned()),
        ))
    }
}

fn run_request() -> RunRequest {
    RunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: None,
        input: json!({}),
        user: None,
        scope: None,
        trigger: agent_core::TriggerKind::Manual,
        trigger_envelope: None,
        workflow: None,
        metadata: json!({}),
    }
}

fn lease_probe_workflow_request() -> WorkflowRunRequest {
    WorkflowRunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        workflow_id: "workflow_lease_probe".to_owned(),
        root_run_id: None,
        user: None,
        scope: Some(RunScope::Tenant("tenant_lease".to_owned())),
        trigger: agent_core::TriggerKind::Manual,
        trigger_envelope: None,
        nodes: vec![WorkflowRunNode {
            node_id: "lease_probe_node".to_owned(),
            agent_id: "lease_probe".to_owned(),
            run_id: None,
            input: json!({"workflow": "lease_probe"}),
            input_mappings: vec![],
            depends_on: vec![],
            compensation: None,
            metadata: json!({}),
        }],
        metadata: json!({}),
    }
}

fn slow_workflow_request() -> WorkflowRunRequest {
    WorkflowRunRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        workflow_id: "workflow_slow".to_owned(),
        root_run_id: None,
        user: None,
        scope: Some(RunScope::Tenant("tenant_slow".to_owned())),
        trigger: agent_core::TriggerKind::Manual,
        trigger_envelope: None,
        nodes: vec![WorkflowRunNode {
            node_id: "slow_node".to_owned(),
            agent_id: "slow".to_owned(),
            run_id: None,
            input: json!({"workflow": "slow"}),
            input_mappings: vec![],
            depends_on: vec![],
            compensation: None,
            metadata: json!({}),
        }],
        metadata: json!({}),
    }
}

fn scheduled_spec(schedule: ScheduleSpec) -> AgentSpec {
    AgentSpec {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        id: "scheduled".to_owned(),
        name: "Scheduled".to_owned(),
        description: None,
        version: "0.1.0".to_owned(),
        schedule,
        capabilities: vec!["debug.schedule".to_owned()],
        metadata: json!({}),
    }
}

fn parse_rfc3339(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).expect("valid RFC3339 test timestamp")
}

fn run_record_started_at(started_at: OffsetDateTime) -> AgentRunRecord {
    AgentRunRecord {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        version: 1,
        run_id: RunId("run_schedule_test".to_owned()),
        idempotency_key: Some("idem_schedule_test".to_owned()),
        agent_id: "scheduled".to_owned(),
        status: AgentRunStatus::Completed,
        scope: RunScope::Global,
        started_at,
        finished_at: Some(started_at + time::Duration::seconds(1)),
        input: json!({}),
        output: json!({}),
        error: None,
        workflow: None,
        metadata: json!({}),
    }
}

fn hook_spec(name: &str, event: HookEventName, effect: HookEffect) -> HookSpec {
    HookSpec {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        name: name.to_owned(),
        event,
        kind: HookKind::NativeRust,
        effect,
        command: None,
        timeout_ms: None,
        enabled: true,
        metadata: json!({}),
    }
}

#[derive(Default)]
struct CountingLockStore {
    release_count: AtomicUsize,
    renew_count: AtomicUsize,
    renewed_keys: Mutex<Vec<String>>,
}

struct LosingLockStore;

#[async_trait]
impl AgentLockStore for LosingLockStore {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError> {
        let now = OffsetDateTime::now_utc();
        Ok(Some(RunLease {
            key: key.to_owned(),
            owner: owner.to_owned(),
            acquired_at: now,
            expires_at: now + time::Duration::try_from(ttl).unwrap_or(time::Duration::MAX),
        }))
    }

    async fn renew(&self, _lease: &RunLease, _ttl: Duration) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn release(&self, _lease: RunLease) -> Result<(), StoreError> {
        Ok(())
    }
}

impl CountingLockStore {
    fn renewed_keys(&self) -> Vec<String> {
        self.renewed_keys
            .lock()
            .expect("renewed keys lock is not poisoned")
            .clone()
    }
}

#[async_trait]
impl AgentLockStore for CountingLockStore {
    async fn acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RunLease>, StoreError> {
        let now = OffsetDateTime::now_utc();
        Ok(Some(RunLease {
            key: key.to_owned(),
            owner: owner.to_owned(),
            acquired_at: now,
            expires_at: now + time::Duration::try_from(ttl).unwrap_or(time::Duration::MAX),
        }))
    }

    async fn renew(&self, lease: &RunLease, _ttl: Duration) -> Result<bool, StoreError> {
        self.renew_count.fetch_add(1, Ordering::SeqCst);
        self.renewed_keys
            .lock()
            .expect("renewed keys lock is not poisoned")
            .push(lease.key.clone());
        Ok(true)
    }

    async fn release(&self, _lease: RunLease) -> Result<(), StoreError> {
        self.release_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct FailingUpdateRunStore;

#[async_trait]
impl AgentRunStore for FailingUpdateRunStore {
    async fn create_run(&self, _run: AgentRunRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn update_run(
        &self,
        _run: AgentRunRecord,
        _expected_version: u64,
    ) -> Result<bool, StoreError> {
        Err(StoreError::new("forced update failure"))
    }

    async fn get_run(&self, _run_id: &RunId) -> Result<Option<AgentRunRecord>, StoreError> {
        Ok(None)
    }

    async fn find_run_by_idempotency_key(
        &self,
        _agent_id: &str,
        _scope: &RunScope,
        _idempotency_key: &str,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        Ok(None)
    }

    async fn list_runs(
        &self,
        _agent_id: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Vec<AgentRunRecord>, StoreError> {
        Ok(Vec::new())
    }

    async fn last_run(
        &self,
        _agent_id: &str,
        _scope: &RunScope,
    ) -> Result<Option<AgentRunRecord>, StoreError> {
        Ok(None)
    }
}

struct AllowHook;

struct FailingHook;

#[async_trait]
impl crate::hooks::HookHandler for FailingHook {
    async fn handle(&self, _invocation: crate::hooks::HookInvocation) -> Result<Value, AgentError> {
        Err(AgentError::internal("policy backend unavailable"))
    }
}

#[async_trait]
impl crate::hooks::HookHandler for AllowHook {
    async fn handle(&self, invocation: crate::hooks::HookInvocation) -> Result<Value, AgentError> {
        Ok(json!({
            "event": invocation.event,
            "input": invocation.input,
        }))
    }
}

struct DenyHook;

#[async_trait]
impl crate::hooks::HookHandler for DenyHook {
    async fn handle(&self, _invocation: crate::hooks::HookInvocation) -> Result<Value, AgentError> {
        serde_json::to_value(PolicyDecision::deny("state writes disabled for test"))
            .map_err(|error| AgentError::internal(error.to_string()))
    }
}

struct NoopServices {
    state_store: Arc<dyn agent_core::AgentStateStore>,
}

#[async_trait]
impl ToolCaller for NoopServices {
    async fn call_tool(&self, _name: &str, _input: Value) -> Result<Value, ToolError> {
        Ok(json!({}))
    }
}

#[async_trait]
impl AgentEventEmitter for NoopServices {
    async fn emit_event(&self, _event: AgentEvent) -> Result<(), AgentError> {
        Ok(())
    }
}

#[async_trait]
impl AgentStateAccess for NoopServices {
    async fn load_state(&self, key: &str) -> Result<Option<Value>, AgentError> {
        self.state_store
            .load("echo", &RunScope::Global, key)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }

    async fn save_state(&self, key: &str, value: Value) -> Result<(), AgentError> {
        self.state_store
            .save("echo", &RunScope::Global, key, value)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))
    }
}

#[async_trait]
impl ProposalCreator for NoopServices {}

#[async_trait]
impl SubagentRunner for NoopServices {}

#[async_trait]
impl ArtifactPublisher for NoopServices {
    async fn publish_artifact(
        &self,
        request: ArtifactPublishRequest,
    ) -> Result<ArtifactRef, AgentError> {
        Ok(ArtifactRef {
            artifact_id: request
                .artifact_id
                .unwrap_or_else(|| "artifact_test_generated".to_owned()),
            kind: request.kind.unwrap_or(ArtifactKind::Blob),
            uri: request.uri,
            media_type: request.media_type,
            size_bytes: request.size_bytes,
            sha256: request.sha256,
            redaction_classification: request
                .redaction_classification
                .unwrap_or(RedactionClassification::Internal),
            store: Some(ArtifactStoreRef {
                provider: "test_artifact_store".to_owned(),
                bucket: Some("test-bucket".to_owned()),
                key: Some("report.pdf".to_owned()),
                version: Some("v1".to_owned()),
                metadata: json!({}),
            }),
            metadata: request.metadata,
        })
    }
}
