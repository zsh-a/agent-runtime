use agent_core::{AgentRuntimeCatalog, ToolSpec};
use agent_runtime::{AGENT_RUN_TOOL_NAME, ensure_agent_run_tool};
use miette::Result;

use crate::{catalog::read_catalog, tools::builtin_tools};

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
    let catalog = match &options.catalog_path {
        Some(path) => Some(read_catalog(path.clone()).await?),
        None => None,
    };
    let tools = chat_tools_from_catalog(catalog.as_ref(), options);
    Ok(tool_inventory_from_specs(
        &tools,
        TuiToolPolicy::new(options.allow_high_risk_tools),
    ))
}

pub(super) fn chat_tools_from_catalog(
    catalog: Option<&AgentRuntimeCatalog>,
    options: &TuiOptions,
) -> Vec<ToolSpec> {
    let mut tools = catalog
        .map(|catalog| catalog.tools.clone())
        .unwrap_or_default();
    tools.extend(options.tool_overrides.source_specs.clone());
    ensure_agent_run_tool(&mut tools);
    for tool in builtin_tools() {
        if !tools.iter().any(|existing| existing.name == tool.name) {
            tools.push(tool);
        }
    }
    tools
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

fn tool_source_label(tool: &ToolSpec) -> String {
    if let Some(source) = tool
        .metadata
        .get("source")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        return source.to_owned();
    }
    match tool.name.as_str() {
        AGENT_RUN_TOOL_NAME => "agent_runtime_builtin".to_owned(),
        "echo" => "agent_cli_builtin".to_owned(),
        _ => "catalog".to_owned(),
    }
}
