use std::{collections::BTreeMap, time::Duration};

use agent_core::{ContextPolicy, HookSpec};
use agent_runtime::{ExecutionPolicy, HookManager};
use camino::Utf8PathBuf;
use miette::{Result, miette};
use serde::{Deserialize, Serialize};

use crate::runtime_config::RuntimeSources;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AgentConfigFile {
    #[serde(default)]
    runtime: RuntimeProfile,
    #[serde(default)]
    profiles: BTreeMap<String, RuntimeProfile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RuntimeProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) store: Option<Utf8PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) eval_store: Option<Utf8PathBuf>,
    #[serde(default, skip_serializing_if = "RuntimeSources::is_empty")]
    pub(crate) sources: RuntimeSources,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) default_agent: Option<String>,
    #[serde(default, skip_serializing_if = "RuntimeTools::is_empty")]
    pub(crate) tools: RuntimeTools,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stdio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retry_backoff_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) hooks: Option<Vec<HookSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context: Option<RuntimeContextPolicy>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RuntimeContextPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reserve_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preserve_recent_messages: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) compact_when_over_budget: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RuntimeTools {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sources: Option<Vec<Utf8PathBuf>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mocks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host: Option<Vec<String>>,
}

impl RuntimeTools {
    pub(crate) fn is_empty(&self) -> bool {
        self.sources.is_none() && self.mocks.is_none() && self.host.is_none()
    }

    fn merge(&mut self, overlay: RuntimeTools) {
        if overlay.sources.is_some() {
            self.sources = overlay.sources;
        }
        if overlay.mocks.is_some() {
            self.mocks = overlay.mocks;
        }
        if overlay.host.is_some() {
            self.host = overlay.host;
        }
    }
}

impl RuntimeContextPolicy {
    fn merge(&mut self, overlay: RuntimeContextPolicy) {
        if overlay.max_input_tokens.is_some() {
            self.max_input_tokens = overlay.max_input_tokens;
        }
        if overlay.reserve_output_tokens.is_some() {
            self.reserve_output_tokens = overlay.reserve_output_tokens;
        }
        if overlay.preserve_recent_messages.is_some() {
            self.preserve_recent_messages = overlay.preserve_recent_messages;
        }
        if overlay.compact_when_over_budget.is_some() {
            self.compact_when_over_budget = overlay.compact_when_over_budget;
        }
    }

    fn apply_to(&self, mut policy: ContextPolicy) -> ContextPolicy {
        if let Some(value) = self.max_input_tokens {
            policy.max_input_tokens = value;
        }
        if let Some(value) = self.reserve_output_tokens {
            policy.reserve_output_tokens = value;
        }
        if let Some(value) = self.preserve_recent_messages {
            policy.preserve_recent_messages = value;
        }
        if let Some(value) = self.compact_when_over_budget {
            policy.compact_when_over_budget = value;
        }
        policy
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EffectiveAgentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) profile: Option<String>,
    pub(crate) runtime: RuntimeProfile,
}

impl RuntimeProfile {
    fn merge(&mut self, overlay: RuntimeProfile) {
        if overlay.profile.is_some() {
            self.profile = overlay.profile;
        }
        if overlay.store.is_some() {
            self.store = overlay.store;
        }
        if overlay.eval_store.is_some() {
            self.eval_store = overlay.eval_store;
        }
        self.sources.merge(overlay.sources);
        if overlay.default_agent.is_some() {
            self.default_agent = overlay.default_agent;
        }
        self.tools.merge(overlay.tools);
        if overlay.host.is_some() {
            self.host = overlay.host;
        }
        if overlay.port.is_some() {
            self.port = overlay.port;
        }
        if overlay.stdio.is_some() {
            self.stdio = overlay.stdio;
        }
        if overlay.timeout_seconds.is_some() {
            self.timeout_seconds = overlay.timeout_seconds;
        }
        if overlay.max_retries.is_some() {
            self.max_retries = overlay.max_retries;
        }
        if overlay.retry_backoff_ms.is_some() {
            self.retry_backoff_ms = overlay.retry_backoff_ms;
        }
        if overlay.hooks.is_some() {
            self.hooks = overlay.hooks;
        }
        if let Some(context) = overlay.context {
            match &mut self.context {
                Some(base) => base.merge(context),
                None => self.context = Some(context),
            }
        }
    }
}

pub(crate) async fn load_agent_config(
    config_path: Option<Utf8PathBuf>,
    requested_profile: Option<&str>,
) -> Result<EffectiveAgentConfig> {
    let config_path = match config_path {
        Some(path) => Some(path),
        None => std::env::var("AGENT_RUNTIME_CONFIG")
            .ok()
            .map(Utf8PathBuf::from)
            .or_else(|| {
                let default = Utf8PathBuf::from("agent-runtime.toml");
                default.exists().then_some(default)
            }),
    };
    let Some(path) = config_path else {
        return Ok(EffectiveAgentConfig {
            source: None,
            profile: requested_profile.map(str::to_owned),
            runtime: RuntimeProfile::default(),
        });
    };

    let text = fs_err::tokio::read_to_string(&path)
        .await
        .map_err(|e| miette!("failed to read config at {path}: {e}"))?;
    let file: AgentConfigFile =
        toml::from_str(&text).map_err(|e| miette!("failed to parse TOML config at {path}: {e}"))?;
    let profile_name = requested_profile
        .map(str::to_owned)
        .or_else(|| file.runtime.profile.clone());
    let mut runtime = file.runtime;
    if let Some(profile_name) = &profile_name {
        let overlay =
            file.profiles.get(profile_name).cloned().ok_or_else(|| {
                miette!("profile '{profile_name}' was not found in config at {path}")
            })?;
        runtime.merge(overlay);
    }
    Ok(EffectiveAgentConfig {
        source: Some(path.to_string()),
        profile: profile_name,
        runtime,
    })
}

pub(crate) fn configured_path(
    value: Utf8PathBuf,
    default: &str,
    configured: Option<&Utf8PathBuf>,
) -> Utf8PathBuf {
    if value == Utf8PathBuf::from(default) {
        configured.cloned().unwrap_or(value)
    } else {
        value
    }
}

pub(crate) fn configured_u64(value: u64, default: u64, configured: Option<u64>) -> u64 {
    if value == default {
        configured.unwrap_or(value)
    } else {
        value
    }
}

pub(crate) fn configured_u32(value: u32, default: u32, configured: Option<u32>) -> u32 {
    if value == default {
        configured.unwrap_or(value)
    } else {
        value
    }
}

pub(crate) fn configured_u16(value: u16, default: u16, configured: Option<u16>) -> u16 {
    if value == default {
        configured.unwrap_or(value)
    } else {
        value
    }
}

pub(crate) fn configured_string(
    value: String,
    default: &str,
    configured: Option<&String>,
) -> String {
    if value == default {
        configured.cloned().unwrap_or(value)
    } else {
        value
    }
}

pub(crate) fn configured_hooks(configured: Option<&Vec<HookSpec>>) -> Vec<HookSpec> {
    configured.cloned().unwrap_or_default()
}

pub(crate) fn hook_manager(specs: Vec<HookSpec>) -> Result<HookManager> {
    HookManager::from_specs(specs).map_err(|error| miette!(error.record.message))
}

pub(crate) fn context_policy(configured: Option<&RuntimeContextPolicy>) -> ContextPolicy {
    configured
        .map(|context| context.apply_to(ContextPolicy::default()))
        .unwrap_or_default()
}

pub(crate) fn execution_policy(
    timeout_seconds: u64,
    max_retries: u32,
    retry_backoff_ms: u64,
) -> ExecutionPolicy {
    ExecutionPolicy {
        timeout: Duration::from_secs(timeout_seconds),
        max_retries,
        retry_backoff: Duration::from_millis(retry_backoff_ms),
        max_concurrent_runs: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_agent_config_merges_profile_hooks() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("agent-runtime.toml");
        fs_err::write(
            &path,
            r#"
[runtime]
timeout_seconds = 10
default_agent = "echo_agent"
hooks = [
  { name = "audit_run", event = "RunStart", kind = "process", effect = "observe", command = ["audit-hook"], timeout_ms = 1000 },
]

[runtime.sources]
registry = "../../examples/agents.yaml"

[runtime.tools]
sources = ["tools/base.json"]
mocks = ['echo={"ok":true}']
host = ["agent", "dev-tool-host"]

[runtime.context]
max_input_tokens = 1000
reserve_output_tokens = 100
preserve_recent_messages = 8

[profiles.strict]
timeout_seconds = 20
hooks = [
  { name = "guard_tool", event = "BeforeToolCall", kind = "process", effect = "policy", command = ["guard-hook"] },
]

[profiles.strict.tools]
sources = ["tools/strict.json"]

[profiles.strict.context]
reserve_output_tokens = 200
compact_when_over_budget = false
"#,
        )
        .expect("write config");

        let config = load_agent_config(
            Some(Utf8PathBuf::from_path_buf(path).unwrap()),
            Some("strict"),
        )
        .await
        .expect("config loads");
        let hooks = config.runtime.hooks.expect("profile hooks");

        assert_eq!(config.runtime.timeout_seconds, Some(20));
        assert_eq!(config.runtime.default_agent.as_deref(), Some("echo_agent"));
        assert_eq!(
            config.runtime.tools.sources.as_deref(),
            Some(&[Utf8PathBuf::from("tools/strict.json")][..])
        );
        assert_eq!(
            config.runtime.tools.mocks.as_deref(),
            Some(&["echo={\"ok\":true}".to_owned()][..])
        );
        assert_eq!(
            config.runtime.tools.host.as_deref(),
            Some(&["agent".to_owned(), "dev-tool-host".to_owned()][..])
        );
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "guard_tool");
        assert_eq!(
            hooks[0].command.as_deref(),
            Some(&["guard-hook".to_owned()][..])
        );
        let policy = context_policy(config.runtime.context.as_ref());
        assert_eq!(policy.max_input_tokens, 1000);
        assert_eq!(policy.reserve_output_tokens, 200);
        assert_eq!(policy.preserve_recent_messages, 8);
        assert!(!policy.compact_when_over_budget);
    }

    #[test]
    fn hook_manager_accepts_empty_config() {
        let manager = hook_manager(configured_hooks(None)).expect("empty hooks are valid");
        let _ = manager;
    }
}
