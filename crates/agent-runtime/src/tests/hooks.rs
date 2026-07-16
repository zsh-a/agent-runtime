use super::*;

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
