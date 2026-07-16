use super::*;

#[tokio::test]
async fn proposals_command_lists_store_items_and_updates_side_panel() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let first = create_test_proposal(
        &state.options.store_path,
        "run_prop_1",
        "echo_agent",
        "edit_file",
        "Update a file",
        ProposalStatus::PendingApproval,
    )
    .await;
    create_test_proposal(
        &state.options.store_path,
        "run_prop_2",
        "echo_agent",
        "send_email",
        "Send a customer email",
        ProposalStatus::Approved,
    )
    .await;

    execute_command(&mut state, "/proposals")
        .await
        .expect("proposals command succeeds");

    let proposals = state.latest_proposals.as_ref().expect("proposal summary");
    assert_eq!(proposals.total_count, 2);
    assert_eq!(proposals.pending_count, 1);
    assert_eq!(proposals.approved_count, 1);
    assert!(state.transcript.iter().any(|item| {
        item.content.contains("Proposals: 2 total, 1 pending")
            && item.content.contains(&first.proposal_id.0)
            && item.content.contains("[pending_approval] edit_file")
    }));
    let rendered = crate::tui::render::render_tui_once(&state).expect("tui renders");
    assert!(rendered.contains("proposals"));
    assert!(rendered.contains("total 2 pend 1 ok 1 deny 0"));
}

#[tokio::test]
async fn proposal_command_defaults_to_single_loaded_proposal() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let proposal = create_test_proposal(
        &state.options.store_path,
        "run_prop_default",
        "echo_agent",
        "edit_file",
        "Update one file",
        ProposalStatus::PendingApproval,
    )
    .await;

    execute_command(&mut state, "/proposals run_prop_default")
        .await
        .expect("proposals command succeeds");
    execute_command(&mut state, "/proposal")
        .await
        .expect("proposal command succeeds");

    assert!(state.transcript.iter().any(|item| {
        item.role == TranscriptRole::User
            && item.content == format!("/proposal {}", proposal.proposal_id.0)
    }));
    assert!(state.transcript.iter().any(|item| {
        item.content.contains(&proposal.proposal_id.0)
            && item.content.contains("\"summary\": \"Update one file\"")
    }));
    assert_eq!(
        state
            .latest_proposals
            .as_ref()
            .expect("proposal summary")
            .total_count,
        1
    );
}

#[tokio::test]
async fn proposal_command_requires_id_when_multiple_proposals_are_loaded() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    create_test_proposal(
        &state.options.store_path,
        "run_prop_many_1",
        "echo_agent",
        "edit_file",
        "Update one file",
        ProposalStatus::PendingApproval,
    )
    .await;
    create_test_proposal(
        &state.options.store_path,
        "run_prop_many_2",
        "echo_agent",
        "send_email",
        "Send one email",
        ProposalStatus::PendingApproval,
    )
    .await;
    execute_command(&mut state, "/proposals")
        .await
        .expect("proposals command succeeds");

    let error = execute_command(&mut state, "/proposal")
        .await
        .expect_err("multiple proposals require explicit id");

    assert!(
        error
            .to_string()
            .contains("2 proposals are currently shown")
    );
}

#[tokio::test]
async fn approve_proposal_command_updates_store_and_summary() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let proposal = create_test_proposal(
        &state.options.store_path,
        "run_prop_approve",
        "echo_agent",
        "edit_file",
        "Update a file",
        ProposalStatus::PendingApproval,
    )
    .await;

    execute_command(
        &mut state,
        &format!("/approve-proposal {} looks good", proposal.proposal_id.0),
    )
    .await
    .expect("approve proposal succeeds");

    let stored = load_proposal(
        state.options.store_backend,
        &state.options.store_path,
        &proposal.proposal_id,
    )
    .await
    .expect("proposal loads");
    assert_eq!(stored.status, ProposalStatus::Approved);
    assert_eq!(
        stored.approval_decisions[0].comment.as_deref(),
        Some("looks good")
    );
    assert_eq!(
        state
            .latest_proposals
            .as_ref()
            .expect("proposal summary")
            .proposals[0]
            .status,
        "approved"
    );
    assert!(state.transcript.iter().any(|item| {
        item.content
            .contains(&format!("Proposal {} approved", proposal.proposal_id.0))
    }));
}

#[tokio::test]
async fn deny_proposal_command_updates_store_and_summary() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = test_state(&dir, "mock response").await;
    let proposal = create_test_proposal(
        &state.options.store_path,
        "run_prop_deny",
        "echo_agent",
        "send_email",
        "Send a customer email",
        ProposalStatus::PendingApproval,
    )
    .await;

    execute_command(
        &mut state,
        &format!("/deny-proposal {} too risky", proposal.proposal_id.0),
    )
    .await
    .expect("deny proposal succeeds");

    let stored = load_proposal(
        state.options.store_backend,
        &state.options.store_path,
        &proposal.proposal_id,
    )
    .await
    .expect("proposal loads");
    assert_eq!(stored.status, ProposalStatus::Denied);
    assert_eq!(
        state
            .latest_proposals
            .as_ref()
            .expect("proposal summary")
            .proposals[0]
            .status,
        "denied"
    );
}
