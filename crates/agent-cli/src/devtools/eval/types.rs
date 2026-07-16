use super::*;

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct EvalCase {
    pub(super) id: String,
    pub(super) agent_id: String,
    pub(super) catalog: Utf8PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) golden_trace: Option<Utf8PathBuf>,
    #[serde(default)]
    pub(super) input: Value,
    pub(super) expect: EvalExpect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) scoring_hook: Option<EvalScoringHook>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct EvalExpect {
    pub(super) status: agent_core::AgentRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) trace_events: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_calls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) proposals: Option<EvalProposalExpect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) prompt_manifest: Option<EvalPromptManifestExpect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) output_mode: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct EvalProposalExpect {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) min_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) statuses: Vec<ProposalStatus>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct EvalPromptManifestExpect {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) agent_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) model_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) tool_schema_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) block_hashes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct EvalScoringHook {
    pub(super) command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) min_score: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct EvalScoringResult {
    pub(super) passed: bool,
    pub(super) score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) comment: Option<String>,
}

#[derive(Debug)]
pub(super) struct EvalScoringHookOutcome {
    pub(super) result: EvalScoringResult,
    pub(super) hook_event: HookEvent,
}

#[derive(Debug, Serialize)]
pub(super) struct EvalReport {
    pub(super) id: String,
    pub(super) passed: bool,
    pub(super) run_id: String,
    pub(super) agent_id: String,
    pub(super) status: agent_core::AgentRunStatus,
    pub(super) checked: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) scoring_comment: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) hooks: Vec<HookEvent>,
}

#[derive(Debug, Serialize)]
pub(super) struct EvalSuiteReport {
    pub(super) passed: bool,
    pub(super) total: usize,
    pub(super) passed_count: usize,
    pub(super) failed_count: usize,
    pub(super) reports: Vec<EvalReport>,
}

#[derive(Debug, Serialize)]
pub(super) struct EvalCreateReport {
    pub(super) id: String,
    pub(super) run_id: String,
    pub(super) agent_id: String,
    pub(super) eval_file: String,
    pub(super) golden_trace: String,
}
