use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{HookEventName, HookKind, protocol_version};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookEffect {
    Observe,
    Policy,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HookSpec {
    #[serde(default = "protocol_version")]
    pub protocol_version: String,
    pub name: String,
    pub event: HookEventName,
    pub kind: HookKind,
    #[serde(default = "default_hook_effect")]
    pub effect: HookEffect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecisionKind {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolicyDecision {
    pub decision: PolicyDecisionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

impl PolicyDecision {
    pub fn allow() -> Self {
        Self {
            decision: PolicyDecisionKind::Allow,
            reason: None,
            metadata: Value::Object(Default::default()),
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            decision: PolicyDecisionKind::Deny,
            reason: Some(reason.into()),
            metadata: Value::Object(Default::default()),
        }
    }

    pub fn is_denied(&self) -> bool {
        matches!(self.decision, PolicyDecisionKind::Deny)
    }
}

impl Default for PolicyDecision {
    fn default() -> Self {
        Self::allow()
    }
}

fn default_hook_effect() -> HookEffect {
    HookEffect::Observe
}

fn default_enabled() -> bool {
    true
}
