use super::*;

impl RuntimeServer {
    pub(crate) async fn create_proposal(
        &self,
        params: HttpProposalCreateParams,
    ) -> Result<ProposalEnvelope> {
        info!(
            run_id = %params.run_id,
            agent_id = %params.agent_id,
            proposal_kind = %params.kind,
            "server create_proposal requested",
        );
        let kind_spec = proposal_kind_spec(&self.catalog, &params.kind)?;
        let mut proposal = ProposalEnvelope::new(
            RunId(params.run_id),
            params.agent_id,
            params.kind,
            params.summary,
            params.payload,
        )
        .with_kind_policy(kind_spec);
        proposal.diffs = params.diffs;
        proposal.warnings = params.warnings;
        authorize_proposal_create_policy(&self.hooks, self.trace_store.as_ref(), &proposal).await?;
        self.proposal_store
            .create_proposal(proposal.clone())
            .await
            .into_diagnostic()?;
        append_proposal_created_trace_event(self.trace_store.as_ref(), &proposal).await?;
        Ok(proposal)
    }

    pub(crate) async fn list_proposals(
        &self,
        run_id: Option<String>,
    ) -> Result<Vec<ProposalEnvelope>> {
        let run_id = run_id.map(RunId);
        self.proposal_store
            .list_proposals(run_id.as_ref())
            .await
            .into_diagnostic()
    }

    pub(crate) async fn get_proposal(&self, proposal_id: ProposalId) -> Result<ProposalEnvelope> {
        self.proposal_store
            .get_proposal(&proposal_id)
            .await
            .into_diagnostic()?
            .ok_or_else(|| miette!("proposal '{}' was not found", proposal_id.0))
    }

    pub(crate) async fn decide_proposal(
        &self,
        proposal_id: ProposalId,
        params: HttpProposalDecisionParams,
    ) -> Result<ProposalDecisionResponse> {
        info!(
            proposal_id = %proposal_id.0,
            decision = %params.decision,
            "server decide_proposal requested",
        );
        let mut proposal = self.get_proposal(proposal_id.clone()).await?;
        let decision = parse_approval_decision(&params.decision)?;
        let response = decide_proposal_with_store(
            self.proposal_store.as_ref(),
            &mut proposal,
            ProposalDecisionInput {
                decision,
                approval_level: params.approval_level,
                decided_by: params.decided_by,
                comment: params.comment,
            },
        )
        .await?;
        append_proposal_decision_trace_event(self.trace_store.as_ref(), &response).await?;
        Ok(response)
    }

    pub(crate) async fn apply_proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<ProposalActionResponse> {
        self.execute_proposal_action(proposal_id, ProposalAction::Apply)
            .await
    }

    pub(crate) async fn undo_proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<ProposalActionResponse> {
        self.execute_proposal_action(proposal_id, ProposalAction::Undo)
            .await
    }

    async fn execute_proposal_action(
        &self,
        proposal_id: ProposalId,
        action: ProposalAction,
    ) -> Result<ProposalActionResponse> {
        info!(
            proposal_id = %proposal_id.0,
            action = ?action,
            "server proposal action requested",
        );
        let mut proposal = self.get_proposal(proposal_id).await?;
        let tool = proposal_action_tool(&self.catalog, &proposal.kind)?;
        authorize_proposal_apply_policy(
            &self.hooks,
            self.trace_store.as_ref(),
            &proposal,
            &tool,
            action,
        )
        .await?;
        let response = execute_proposal_action_with_store(
            self.proposal_store.as_ref(),
            self.services.as_ref(),
            &mut proposal,
            tool,
            action,
        )
        .await?;
        append_proposal_action_trace_event(self.trace_store.as_ref(), &response).await?;
        Ok(response)
    }
}
