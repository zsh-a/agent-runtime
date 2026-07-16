use super::*;

#[tokio::test]
async fn workflow_command_runs_dag_and_shows_node_summary() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workflow_path = dir.path().join("workflow.json");
    fs_err::write(
        &workflow_path,
        r#"{
  "protocol_version": "agent.v1",
  "workflow_id": "tui_workflow_test",
  "nodes": [
    {
      "node_id": "first",
      "agent_id": "echo_agent",
      "input": {"message": "root"}
    },
    {
      "node_id": "second",
      "agent_id": "echo_agent",
      "depends_on": ["first"],
      "input": {"message": "child"},
      "input_mappings": [
        {
          "from_node": "first",
          "from_path": "/message",
          "to_path": "/source/message"
        }
      ]
    }
  ],
  "metadata": {"source": "tui_test"}
}"#,
    )
    .expect("workflow writes");
    let workflow_path = Utf8PathBuf::from_path_buf(workflow_path).expect("workflow path is utf8");
    let mut state = test_state(&dir, "mock response").await;

    execute_command(&mut state, &format!("/workflow {workflow_path}"))
        .await
        .expect("workflow command succeeds");

    assert!(state.trace.is_some());
    assert_eq!(state.recent_runs.len(), 2);
    let workflow = state.latest_workflow.as_ref().expect("workflow summary");
    assert_eq!(workflow.workflow_id, "tui_workflow_test");
    assert_eq!(workflow.status, "completed");
    assert_eq!(workflow.node_count, 2);
    assert_eq!(workflow.completed_count, 2);
    assert_eq!(workflow.failed_count, 0);
    assert_eq!(workflow.skipped_count, 0);
    assert_eq!(workflow.nodes[1].depends_on, vec!["first"]);
    assert!(state.activity.iter().any(|activity| {
        activity.kind == TuiActivityKind::Run
            && activity.title == "workflow tui_workflow_test"
            && activity.detail.as_deref() == Some("2 nodes completed")
    }));
    assert!(state.transcript.iter().any(|item| {
        item.content
            .contains("Workflow tui_workflow_test: completed")
            && item.content.contains("- first -> echo_agent [completed]")
            && item.content.contains("- second -> echo_agent [completed]")
            && item.content.contains("deps=first")
    }));
    let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
    assert!(rendered.contains("workflow"));
    assert!(rendered.contains("tui_workflow_test [completed]"));
    assert!(rendered.contains("nodes 2 ok 2 fail 0 skip 0"));
}

#[tokio::test]
async fn workflow_command_validates_schema_before_running() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workflow_path = dir.path().join("invalid-workflow.json");
    fs_err::write(
        &workflow_path,
        r#"{"protocol_version":"agent.v1","workflow_id":"missing_nodes"}"#,
    )
    .expect("workflow writes");
    let workflow_path = Utf8PathBuf::from_path_buf(workflow_path).expect("workflow path is utf8");
    let mut state = test_state(&dir, "mock response").await;

    let error = execute_command(&mut state, &format!("/workflow {workflow_path}"))
        .await
        .expect_err("invalid workflow is rejected");

    assert!(
        error
            .to_string()
            .contains("workflow request failed schema validation")
    );
    assert!(state.recent_runs.is_empty());
}
