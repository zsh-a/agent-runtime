#![cfg(feature = "testkit")]

use agent_store::{
    InMemoryProposalStore, InMemoryRunStore, InMemorySessionStore, InMemoryStateStore,
    testkit::{
        assert_proposal_store_conformance, assert_run_store_conformance,
        assert_session_store_conformance, assert_state_store_conformance,
    },
};

#[tokio::test]
async fn public_testkit_helpers_are_available_to_downstream_crates() {
    let run_store = InMemoryRunStore::default();
    assert_run_store_conformance(&run_store).await;

    let proposal_store = InMemoryProposalStore::default();
    assert_proposal_store_conformance(&proposal_store).await;

    let session_store = InMemorySessionStore::default();
    assert_session_store_conformance(&session_store).await;

    let state_store = InMemoryStateStore::default();
    assert_state_store_conformance(&state_store).await;
}
