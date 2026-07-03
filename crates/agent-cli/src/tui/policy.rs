use agent_core::{ToolRisk, ToolSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TuiToolRisk {
    ReadOnly,
    Low,
    Medium,
    High,
}

impl TuiToolRisk {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

impl From<&ToolRisk> for TuiToolRisk {
    fn from(value: &ToolRisk) -> Self {
        match value {
            ToolRisk::ReadOnly => Self::ReadOnly,
            ToolRisk::Low => Self::Low,
            ToolRisk::Medium => Self::Medium,
            ToolRisk::High => Self::High,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TuiToolPolicyDecision {
    pub(super) risk: TuiToolRisk,
    pub(super) allowed: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TuiToolPolicy {
    allow_high_risk: bool,
}

impl TuiToolPolicy {
    pub(super) fn new(allow_high_risk: bool) -> Self {
        Self { allow_high_risk }
    }

    pub(super) fn evaluate(self, spec: Option<&ToolSpec>) -> TuiToolPolicyDecision {
        let risk = spec
            .map(|spec| TuiToolRisk::from(&spec.risk))
            .unwrap_or(TuiToolRisk::High);
        TuiToolPolicyDecision {
            risk,
            allowed: risk != TuiToolRisk::High || self.allow_high_risk,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_spec_is_treated_as_high_risk() {
        let denied = TuiToolPolicy::new(false).evaluate(None);
        assert_eq!(denied.risk, TuiToolRisk::High);
        assert!(!denied.allowed);

        let allowed = TuiToolPolicy::new(true).evaluate(None);
        assert_eq!(allowed.risk, TuiToolRisk::High);
        assert!(allowed.allowed);
    }
}
