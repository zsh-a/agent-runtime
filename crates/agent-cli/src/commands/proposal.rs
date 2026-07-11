use agent_core::{ProposalEnvelope, ProposalId, RunId};
use agent_runtime::HookManager;
use camino::Utf8PathBuf;
use clap::Subcommand;
use miette::{IntoDiagnostic, Result, miette};

use crate::{
    catalog::read_catalog,
    cli_input::read_command_input,
    config::{RuntimeStoreBackend, configured_path},
    print_json,
    proposal::{
        ProposalAction, ProposalActionResponse, ProposalDecisionInput,
        append_proposal_action_trace_event, append_proposal_created_trace_event,
        append_proposal_decision_trace_event, authorize_proposal_apply_policy,
        authorize_proposal_create_policy, decide_proposal_with_store,
        execute_proposal_action_with_store, parse_approval_decision, parse_approval_level,
        proposal_action_tool,
    },
    runtime_stores::RuntimeStores,
    tools::{CliServices, ToolOverrides, tool_overrides},
};

const DEFAULT_STORE: &str = ".agent-runtime/store";

#[derive(Debug, Subcommand)]
pub(crate) enum ProposalCommand {
    Create {
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        agent_id: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        summary: String,
        #[arg(long)]
        payload: Option<Utf8PathBuf>,
        #[arg(long)]
        payload_json: Option<String>,
        #[arg(long)]
        diffs_json: Option<String>,
        #[arg(long)]
        warnings_json: Option<String>,
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
    },
    List {
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
        #[arg(long)]
        run_id: Option<String>,
    },
    Inspect {
        proposal_id: String,
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
    },
    Decide {
        proposal_id: String,
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
        #[arg(long)]
        decision: String,
        #[arg(long)]
        approval_level: Option<String>,
        #[arg(long)]
        decided_by: Option<String>,
        #[arg(long)]
        comment: Option<String>,
    },
    Apply {
        proposal_id: String,
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
        #[arg(long)]
        catalog: Utf8PathBuf,
        #[arg(
            long = "tool-host",
            num_args = 1..,
            value_name = "COMMAND"
        )]
        tool_host: Vec<String>,
        #[arg(long = "mock-tool", value_name = "NAME=JSON_OR_@PATH")]
        mock_tool: Vec<String>,
        #[arg(long = "tool-source", value_name = "PATH")]
        tool_source: Vec<Utf8PathBuf>,
    },
    Undo {
        proposal_id: String,
        #[arg(long, default_value = ".agent-runtime/store")]
        store: Utf8PathBuf,
        #[arg(long)]
        catalog: Utf8PathBuf,
        #[arg(
            long = "tool-host",
            num_args = 1..,
            value_name = "COMMAND"
        )]
        tool_host: Vec<String>,
        #[arg(long = "mock-tool", value_name = "NAME=JSON_OR_@PATH")]
        mock_tool: Vec<String>,
        #[arg(long = "tool-source", value_name = "PATH")]
        tool_source: Vec<Utf8PathBuf>,
    },
}

pub(crate) async fn run_proposal_command(
    command: ProposalCommand,
    hooks: HookManager,
    store_backend: RuntimeStoreBackend,
    configured_store: Option<Utf8PathBuf>,
) -> Result<()> {
    match command {
        ProposalCommand::Create {
            run_id,
            agent_id,
            kind,
            summary,
            payload,
            payload_json,
            diffs_json,
            warnings_json,
            store,
        } => {
            let payload = read_command_input(payload, payload_json).await?;
            let mut proposal =
                ProposalEnvelope::new(RunId(run_id), agent_id, kind, summary, payload);
            proposal.diffs = parse_json_vec("diffs-json", diffs_json)?;
            proposal.warnings = parse_json_vec("warnings-json", warnings_json)?;
            let store_path = proposal_store_path(store, configured_store.as_ref());
            let stores = RuntimeStores::open(store_backend, store_path).await?;
            authorize_proposal_create_policy(&hooks, stores.trace_store.as_ref(), &proposal)
                .await?;
            stores
                .proposal_store
                .create_proposal(proposal.clone())
                .await
                .into_diagnostic()?;
            append_proposal_created_trace_event(stores.trace_store.as_ref(), &proposal).await?;
            print_json(&proposal)
        }
        ProposalCommand::List { store, run_id } => {
            let store_path = proposal_store_path(store, configured_store.as_ref());
            let stores = RuntimeStores::open(store_backend, store_path).await?;
            let run_id = run_id.map(RunId);
            let proposals = stores
                .proposal_store
                .list_proposals(run_id.as_ref())
                .await
                .into_diagnostic()?;
            print_json(&proposals)
        }
        ProposalCommand::Inspect { proposal_id, store } => {
            let store_path = proposal_store_path(store, configured_store.as_ref());
            let stores = RuntimeStores::open(store_backend, store_path).await?;
            let proposal = stores
                .proposal_store
                .get_proposal(&ProposalId(proposal_id.clone()))
                .await
                .into_diagnostic()?
                .ok_or_else(|| miette!("proposal '{proposal_id}' was not found"))?;
            print_json(&proposal)
        }
        ProposalCommand::Decide {
            proposal_id,
            store,
            decision,
            approval_level,
            decided_by,
            comment,
        } => {
            let store_path = proposal_store_path(store, configured_store.as_ref());
            let stores = RuntimeStores::open(store_backend, store_path).await?;
            let proposal_id = ProposalId(proposal_id);
            let mut proposal = stores
                .proposal_store
                .get_proposal(&proposal_id)
                .await
                .into_diagnostic()?
                .ok_or_else(|| miette!("proposal '{}' was not found", proposal_id.0))?;
            let decision = parse_approval_decision(&decision)?;
            let approval_level = approval_level
                .as_deref()
                .map(parse_approval_level)
                .transpose()?;
            let response = decide_proposal_with_store(
                stores.proposal_store.as_ref(),
                &mut proposal,
                ProposalDecisionInput {
                    decision,
                    approval_level,
                    decided_by,
                    comment,
                },
            )
            .await?;
            append_proposal_decision_trace_event(stores.trace_store.as_ref(), &response).await?;
            print_json(&response)
        }
        ProposalCommand::Apply {
            proposal_id,
            store,
            catalog,
            tool_host,
            mock_tool,
            tool_source,
        } => {
            let response = execute_proposal_action(
                ProposalId(proposal_id),
                proposal_store_path(store, configured_store.as_ref()),
                store_backend,
                catalog,
                tool_overrides(tool_host, mock_tool, tool_source).await?,
                hooks,
                ProposalAction::Apply,
            )
            .await?;
            print_json(&response)
        }
        ProposalCommand::Undo {
            proposal_id,
            store,
            catalog,
            tool_host,
            mock_tool,
            tool_source,
        } => {
            let response = execute_proposal_action(
                ProposalId(proposal_id),
                proposal_store_path(store, configured_store.as_ref()),
                store_backend,
                catalog,
                tool_overrides(tool_host, mock_tool, tool_source).await?,
                hooks,
                ProposalAction::Undo,
            )
            .await?;
            print_json(&response)
        }
    }
}

fn proposal_store_path(store: Utf8PathBuf, configured_store: Option<&Utf8PathBuf>) -> Utf8PathBuf {
    configured_path(store, DEFAULT_STORE, configured_store)
}

fn parse_json_vec<T>(name: &str, value: Option<String>) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    match value {
        Some(value) => serde_json::from_str(&value)
            .map_err(|e| miette!("failed to parse --{name} as JSON array: {e}")),
        None => Ok(Vec::new()),
    }
}

async fn execute_proposal_action(
    proposal_id: ProposalId,
    store_path: Utf8PathBuf,
    store_backend: RuntimeStoreBackend,
    catalog_path: Utf8PathBuf,
    mut tool_overrides: ToolOverrides,
    hooks: HookManager,
    action: ProposalAction,
) -> Result<ProposalActionResponse> {
    let catalog = read_catalog(catalog_path).await?;
    let stores = RuntimeStores::open(store_backend, store_path).await?;
    tool_overrides.extend_tool_specs(catalog.tools.clone());
    let services = CliServices::with_stores(
        tool_overrides,
        stores.state_store.clone(),
        stores.proposal_store.clone(),
    );
    let mut proposal = stores
        .proposal_store
        .get_proposal(&proposal_id)
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("proposal '{}' was not found", proposal_id.0))?;
    let tool = proposal_action_tool(&catalog, &proposal.kind)?;
    authorize_proposal_apply_policy(
        &hooks,
        stores.trace_store.as_ref(),
        &proposal,
        &tool,
        action,
    )
    .await?;
    let response = execute_proposal_action_with_store(
        stores.proposal_store.as_ref(),
        &services,
        &mut proposal,
        tool,
        action,
    )
    .await?;
    append_proposal_action_trace_event(stores.trace_store.as_ref(), &response).await?;
    Ok(response)
}
