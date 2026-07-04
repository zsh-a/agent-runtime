use agent_core::ToolSpec;
use miette::Result;

use crate::runtime_config::{RuntimeSourceOptions, compose_runtime_sources, tool_source_label};

use super::{data::TuiOptions, policy::TuiToolPolicy, policy::TuiToolRisk};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiToolInventoryItem {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) source: String,
    pub(super) risk: TuiToolRisk,
    pub(super) allowed: bool,
}

impl TuiToolInventoryItem {
    pub(super) fn status_label(&self) -> &'static str {
        if !self.allowed {
            "blocked"
        } else if self.risk == TuiToolRisk::High {
            "approval"
        } else {
            "allowed"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TuiToolInventory {
    pub(super) items: Vec<TuiToolInventoryItem>,
}

impl TuiToolInventory {
    pub(super) fn total_count(&self) -> usize {
        self.items.len()
    }

    pub(super) fn high_risk_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.risk == TuiToolRisk::High)
            .count()
    }

    pub(super) fn blocked_count(&self) -> usize {
        self.items.iter().filter(|item| !item.allowed).count()
    }
}

pub(super) async fn load_tui_tool_inventory(options: &TuiOptions) -> Result<TuiToolInventory> {
    let composition = compose_runtime_sources(RuntimeSourceOptions {
        sources: options.runtime_sources.clone(),
        tool_overrides: options.tool_overrides.clone(),
    })
    .await?;
    Ok(tool_inventory_from_specs(
        &composition.tool_specs,
        TuiToolPolicy::new(options.allow_high_risk_tools),
    ))
}

fn tool_inventory_from_specs(tools: &[ToolSpec], policy: TuiToolPolicy) -> TuiToolInventory {
    let mut items = tools
        .iter()
        .map(|tool| {
            let decision = policy.evaluate(Some(tool));
            TuiToolInventoryItem {
                name: tool.name.clone(),
                description: tool.description.clone(),
                source: tool_source_label(tool),
                risk: decision.risk,
                allowed: decision.allowed,
            }
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.name.cmp(&right.name));
    TuiToolInventory { items }
}
