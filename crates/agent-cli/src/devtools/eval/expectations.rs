use super::*;

pub(super) fn eval_prompt_manifest_expectation(
    manifest: &PromptManifest,
) -> EvalPromptManifestExpect {
    EvalPromptManifestExpect {
        id: Some(manifest.id.clone()),
        version: Some(manifest.version.clone()),
        agent_version: Some(manifest.agent_version.clone()),
        model_family: Some(manifest.model_family.clone()),
        provider: Some(manifest.provider.clone()),
        model: Some(manifest.model.clone()),
        tool_schema_version: Some(manifest.tool_schema_version.clone()),
        block_hashes: manifest
            .blocks
            .iter()
            .map(|block| block.content_hash.clone())
            .collect(),
    }
}

pub(super) fn eval_proposal_expectation(
    proposals: &[ProposalEnvelope],
) -> Option<EvalProposalExpect> {
    if proposals.is_empty() {
        return None;
    }

    let mut kinds = Vec::new();
    let mut statuses = Vec::new();
    for proposal in proposals {
        if !kinds.contains(&proposal.kind) {
            kinds.push(proposal.kind.clone());
        }
        if !statuses.contains(&proposal.status) {
            statuses.push(proposal.status.clone());
        }
    }

    Some(EvalProposalExpect {
        min_count: Some(proposals.len()),
        kinds,
        statuses,
    })
}

pub(super) fn check_prompt_manifest_expectation(
    eval_id: &str,
    expected: &EvalPromptManifestExpect,
    manifest: &PromptManifest,
) -> Result<()> {
    check_expected_prompt_field(eval_id, "id", expected.id.as_deref(), &manifest.id)?;
    check_expected_prompt_field(
        eval_id,
        "version",
        expected.version.as_deref(),
        &manifest.version,
    )?;
    check_expected_prompt_field(
        eval_id,
        "agent_version",
        expected.agent_version.as_deref(),
        &manifest.agent_version,
    )?;
    check_expected_prompt_field(
        eval_id,
        "model_family",
        expected.model_family.as_deref(),
        &manifest.model_family,
    )?;
    check_expected_prompt_field(
        eval_id,
        "provider",
        expected.provider.as_deref(),
        &manifest.provider,
    )?;
    check_expected_prompt_field(eval_id, "model", expected.model.as_deref(), &manifest.model)?;
    check_expected_prompt_field(
        eval_id,
        "tool_schema_version",
        expected.tool_schema_version.as_deref(),
        &manifest.tool_schema_version,
    )?;

    if !expected.block_hashes.is_empty() {
        let actual = manifest
            .blocks
            .iter()
            .map(|block| block.content_hash.as_str())
            .collect::<Vec<_>>();
        let expected_hashes = expected
            .block_hashes
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if actual != expected_hashes {
            return Err(miette!(
                "eval {} expected prompt block hashes {:?}, got {:?}",
                eval_id,
                expected_hashes,
                actual
            ));
        }
    }
    Ok(())
}

pub(super) fn check_expected_prompt_field(
    eval_id: &str,
    field: &str,
    expected: Option<&str>,
    actual: &str,
) -> Result<()> {
    if let Some(expected) = expected
        && actual != expected
    {
        return Err(miette!(
            "eval {} expected prompt manifest {} {}, got {}",
            eval_id,
            field,
            expected,
            actual
        ));
    }
    Ok(())
}
