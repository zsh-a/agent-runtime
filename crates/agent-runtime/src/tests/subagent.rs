use super::*;

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
