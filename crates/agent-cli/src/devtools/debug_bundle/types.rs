use super::*;

#[derive(Debug, Clone)]
pub(crate) struct DebugBundleOptions {
    pub(crate) run_id: String,
    pub(crate) store_path: Utf8PathBuf,
    pub(crate) store_backend: RuntimeStoreBackend,
    pub(crate) out: Utf8PathBuf,
    pub(crate) catalog_path: Option<Utf8PathBuf>,
    pub(crate) trace_path: Option<Utf8PathBuf>,
    pub(crate) timeout_seconds: u64,
    pub(crate) materialize_artifacts: bool,
    pub(crate) artifact_resolver_path: Option<Utf8PathBuf>,
}

pub(super) struct DebugReplayAssets {
    pub(super) prompt_manifest: bool,
    pub(super) artifacts: bool,
    pub(super) artifact_materializations: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct DebugBundleManifest {
    pub(super) bundle_version: String,
    pub(super) protocol_version: String,
    pub(super) runtime_version: String,
    pub(super) run_id: String,
    pub(super) agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) agent_version: Option<String>,
    pub(super) created_at: String,
    pub(super) files: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct RedactionReport {
    pub(super) policy: String,
    pub(super) replacement: String,
    pub(super) redacted_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct DebugStateSnapshot {
    pub(super) protocol_version: String,
    pub(super) runtime_version: String,
    pub(super) captured_at: String,
    pub(super) store_root: String,
    pub(super) run_id: String,
    pub(super) agent_id: String,
    pub(super) run_status: agent_core::AgentRunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) session: Option<SessionRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thread: Option<ThreadRecord>,
    #[serde(default)]
    pub(super) steps: Vec<StepRecord>,
    #[serde(default)]
    pub(super) proposals: Vec<ProposalEnvelope>,
}

#[derive(Debug, Serialize)]
pub(super) struct DebugReplayConfig {
    pub(super) protocol_version: String,
    pub(super) runtime_version: String,
    pub(super) run_id: String,
    pub(super) agent_id: String,
    pub(super) replay_mode: String,
    pub(super) source_store: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source_trace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) catalog: Option<String>,
    pub(super) timeout_seconds: u64,
    pub(super) assets: BTreeMap<String, String>,
    pub(super) replay_command: Vec<String>,
    pub(super) run_request: RunRequest,
}

#[derive(Debug, Serialize)]
pub(super) struct ArtifactMaterializationManifest {
    pub(super) protocol_version: String,
    pub(super) runtime_version: String,
    pub(super) materialized_at: String,
    pub(super) mode: String,
    pub(super) records: Vec<ArtifactMaterializationRecord>,
}

#[derive(Debug, Serialize)]
pub(super) struct ArtifactMaterializationRecord {
    pub(super) artifact_id: String,
    pub(super) status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) bundled_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) blake3: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DebugArtifactResolverManifest {
    pub(super) protocol_version: String,
    #[serde(default)]
    pub(super) resolvers: Vec<ArtifactStoreResolver>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ArtifactStoreResolver {
    pub(super) provider: String,
    pub(super) root: Utf8PathBuf,
}

impl DebugBundleManifest {
    pub(super) fn new(
        record: &AgentRunRecord,
        agent_version: Option<String>,
        files: BTreeMap<String, String>,
    ) -> Self {
        Self {
            bundle_version: "debug_bundle.v1".to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            runtime_version: RUNTIME_VERSION.to_owned(),
            run_id: record.run_id.0.clone(),
            agent_id: record.agent_id.clone(),
            agent_version,
            created_at: time::OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc().to_string()),
            files,
        }
    }
}
