use super::*;

pub(super) async fn http_proposal_create(
    State(server): State<RuntimeServer>,
    Json(params): Json<HttpProposalCreateParams>,
) -> Response {
    match server.create_proposal(params).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_report_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_create_failed",
            err,
        ),
    }
}

pub(super) async fn http_proposals(
    State(server): State<RuntimeServer>,
    Query(params): Query<HttpProposalListParams>,
) -> Response {
    match server.list_proposals(params.run_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_list_failed",
            err,
        ),
    }
}

pub(super) async fn http_proposal_inspect(
    State(server): State<RuntimeServer>,
    Path(proposal_id): Path<String>,
) -> Response {
    match server.get_proposal(ProposalId(proposal_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_error(StatusCode::NOT_FOUND, "proposal_not_found", err),
    }
}

pub(super) async fn http_proposal_decide(
    State(server): State<RuntimeServer>,
    Path(proposal_id): Path<String>,
    Json(params): Json<HttpProposalDecisionParams>,
) -> Response {
    match server
        .decide_proposal(ProposalId(proposal_id), params)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_report_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_decide_failed",
            err,
        ),
    }
}

pub(super) async fn http_proposal_apply(
    State(server): State<RuntimeServer>,
    Path(proposal_id): Path<String>,
) -> Response {
    match server.apply_proposal(ProposalId(proposal_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_report_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_apply_failed",
            err,
        ),
    }
}

pub(super) async fn http_proposal_undo(
    State(server): State<RuntimeServer>,
    Path(proposal_id): Path<String>,
) -> Response {
    match server.undo_proposal(ProposalId(proposal_id)).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => http_report_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proposal_undo_failed",
            err,
        ),
    }
}
