use super::*;

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
