use std::sync::Arc;

use agent_chat::{ChatTurnEvent, ChatTurnEventKind, ChatTurnRequest, ChatTurnRunner};
use agent_core::{
    AgentRegistry, AgentRunStore, AgentServices, PROTOCOL_VERSION, RunId, RunRequest, ToolRisk,
    ToolSpec,
};
use agent_llm::{LlmMessage, LlmRole, user_message};
use agent_runtime::AgentRunner;
use agent_store::{FileProposalStore, FileRunStore};
use camino::Utf8PathBuf;
use futures::StreamExt;
use miette::{IntoDiagnostic, Result, miette};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::{
    catalog::{load_catalog_registry, read_catalog},
    chat::provider_from_options,
    config::execution_policy,
    registry::load_registry,
    tools::CliServices,
    trace_store::write_store_trace,
};

use super::data::{TuiState, TuiUpdate, read_trace};

pub(super) async fn execute_command(state: &mut TuiState, input: &str) -> Result<()> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }
    let Some(command) = input.strip_prefix('/') else {
        return run_natural_language_command(state, input).await;
    };
    execute_slash_command(state, command.trim()).await
}

pub(super) fn start_natural_language_task(
    state: &mut TuiState,
    text: String,
    sender: UnboundedSender<TuiUpdate>,
) -> JoinHandle<()> {
    let mut messages = state.chat_messages.clone();
    messages.push(user_message(text.clone()));
    let options = state.options.clone();
    state.push_user_message(text);
    state.start_assistant_stream();
    state.set_busy(true);
    tokio::spawn(async move {
        if let Err(error) = run_natural_language_stream(options, messages, sender.clone()).await {
            let _ = sender.send(TuiUpdate::Error(error.to_string()));
        }
        let _ = sender.send(TuiUpdate::Busy(false));
    })
}

async fn execute_slash_command(state: &mut TuiState, input: &str) -> Result<()> {
    let (verb, rest) = split_once(input);
    match verb {
        "help" | "?" => show_help(state),
        "clear" => state.clear_output(),
        "refresh" => {
            state.refresh().await?;
            state.push_event("refreshed catalog/trace/store");
        }
        "run" => run_agent_command(state, rest).await?,
        "tool" | "call" => tool_call_command(state, rest).await?,
        "trace" | "replay" => load_trace_command(state, rest).await?,
        "inspect" => inspect_run_command(state, rest).await?,
        other => state.push_system_message(format!(
            "unknown command '/{other}'. Try: /help, /run, /tool, /replay, /inspect, /refresh, /clear"
        )),
    }
    Ok(())
}

fn show_help(state: &mut TuiState) {
    state.push_system_message(
        "Type natural language and press Enter to chat with the default agent.\n\n\
        Slash commands:\n\
        /run <agent_id> [json|text]  run a specific runtime agent\n\
        /tool <name> [json]          call a tool through active CLI services\n\
        /replay <trace_path>         load a trace into the side panel\n\
        /inspect <run_id>            load a persisted run record summary\n\
        /refresh                     reload catalog, trace, and recent runs\n\
        /clear                       clear chat and activity\n\n\
        Keys:\n\
        Enter sends, Shift+Enter inserts a newline, Esc/Ctrl-C cancels a running task\n\
        Left/Right move the cursor, Ctrl/Alt+Left/Right move by word\n\
        Ctrl+A/E jump to start/end, Ctrl+U/K delete before/after cursor\n\
        Ctrl+W deletes the previous word, Up/Down browse input history\n\
        PageUp/PageDown scroll chat, Tab completes slash commands",
    );
}

async fn run_natural_language_command(state: &mut TuiState, text: &str) -> Result<()> {
    let agent_id = default_agent_id(state).await?;
    state.push_user_message(text.to_owned());
    run_chat_turn(state, &agent_id, text).await
}

async fn run_agent_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let (agent_id, json_input) = split_name_and_json(rest, "agent id")?;
    let input = parse_run_input(json_input)?;
    state.push_user_message(format!("/run {agent_id} {}", compact_json(&input)));
    run_agent_with_input(state, &agent_id, input, "slash_command").await
}

async fn run_agent_with_input(
    state: &mut TuiState,
    agent_id: &str,
    input: Value,
    input_mode: &str,
) -> Result<()> {
    let registry = load_active_registry(state).await?;
    let store_path = state.options.store_path.clone();
    let store = Arc::new(
        FileRunStore::new(store_path.clone())
            .await
            .into_diagnostic()?,
    );
    let proposal_store = Arc::new(
        FileProposalStore::new(store_path.clone())
            .await
            .into_diagnostic()?,
    );
    let services = Arc::new(CliServices::with_proposal_store(
        state.options.tool_overrides.clone(),
        proposal_store,
    ));
    let runner = AgentRunner::new(registry, store, services).with_policy(execution_policy(
        state.options.timeout_seconds,
        state.options.max_retries,
        state.options.retry_backoff_ms,
    ));
    let outcome = runner
        .run_once(
            agent_id,
            RunRequest {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: None,
                input,
                user: None,
                trigger: agent_core::TriggerKind::Manual,
                metadata: json!({
                    "source": "agent_tui",
                    "input_mode": input_mode,
                    "surface": "agent_tui"
                }),
            },
        )
        .await
        .into_diagnostic()?;
    write_store_trace(&store_path, &outcome.trace).await?;
    state.set_trace(
        format!("latest run {}", outcome.result.run_id.0),
        outcome.trace,
    );
    state.refresh_runs().await?;
    state.push_event(format!(
        "run {} {} {:?}",
        outcome.result.run_id.0, outcome.result.agent_id, outcome.result.status
    ));
    if let Some(summary) = outcome.result.summary {
        state.push_event(format!("summary: {summary}"));
    }
    push_agent_output(state, &outcome.result.output);
    Ok(())
}

async fn tool_call_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let (name, json_input) = split_name_and_json(rest, "tool name")?;
    let input = parse_json_or_default(json_input, "tool input")?;
    state.push_user_message(format!("/tool {name} {}", compact_json(&input)));
    let services = CliServices::new(state.options.tool_overrides.clone());
    let output = services
        .call_tool(&name, input)
        .await
        .map_err(|err| miette!(err.record.message))?;
    state.push_tool_message(Some(name), pretty_json(&output));
    Ok(())
}

async fn run_chat_turn(state: &mut TuiState, agent_id: &str, text: &str) -> Result<()> {
    let provider = provider_from_options(&state.options.chat)?;
    let proposal_store = Arc::new(
        FileProposalStore::new(state.options.store_path.clone())
            .await
            .into_diagnostic()?,
    );
    let services = Arc::new(CliServices::with_proposal_store(
        state.options.tool_overrides.clone(),
        proposal_store,
    ));
    let runner = ChatTurnRunner::new(provider, services);
    let user = user_message(text);
    state.chat_messages.push(user.clone());
    let request = ChatTurnRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        turn_id: None,
        surface: Some("agent_tui".to_owned()),
        mode: Some("natural_language".to_owned()),
        session_id: None,
        thread_id: None,
        agent_id: Some(agent_id.to_owned()),
        provider: state.options.chat.provider.clone(),
        model: state.options.chat.model.clone(),
        messages: chat_request_messages(&state.options, agent_id, state.chat_messages.clone())
            .await?,
        temperature: state.options.chat.temperature,
        max_output_tokens: state.options.chat.max_output_tokens,
        tools: chat_tools(state).await?,
        metadata: json!({
            "source": "agent_tui",
            "surface": "agent_tui",
            "mode": "natural_language",
        }),
        max_tool_rounds: state.options.chat.max_tool_rounds,
    };

    let mut stream = runner.stream(request);
    let mut assistant_text = String::new();
    let mut final_response = None;
    state.start_assistant_stream();
    while let Some(event) = stream.next().await {
        let event = event.map_err(|err| miette!(err.record.message))?;
        apply_chat_event_to_tui(state, &event, &mut assistant_text, &mut final_response);
    }
    let assistant_content = final_response
        .as_ref()
        .map(|response: &agent_llm::LlmResponse| response.content.clone())
        .filter(|content| !content.is_empty())
        .unwrap_or(assistant_text);
    if !assistant_content.is_empty() {
        state.replace_active_assistant(assistant_content.clone());
        state.chat_messages.push(LlmMessage {
            role: LlmRole::Assistant,
            content: Value::String(assistant_content),
            name: None,
            metadata: json!({}),
        });
    } else {
        state.finish_assistant_stream();
    }
    Ok(())
}

async fn run_natural_language_stream(
    options: super::data::TuiOptions,
    mut messages: Vec<LlmMessage>,
    sender: UnboundedSender<TuiUpdate>,
) -> Result<()> {
    let agent_id = default_agent_id_from_options(&options).await?;
    let provider = provider_from_options(&options.chat)?;
    let proposal_store = Arc::new(
        FileProposalStore::new(options.store_path.clone())
            .await
            .into_diagnostic()?,
    );
    let services = Arc::new(CliServices::with_proposal_store(
        options.tool_overrides.clone(),
        proposal_store,
    ));
    let runner = ChatTurnRunner::new(provider, services);
    let request = ChatTurnRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        turn_id: None,
        surface: Some("agent_tui".to_owned()),
        mode: Some("natural_language".to_owned()),
        session_id: None,
        thread_id: None,
        agent_id: Some(agent_id.clone()),
        provider: options.chat.provider.clone(),
        model: options.chat.model.clone(),
        messages: chat_request_messages(&options, &agent_id, messages.clone()).await?,
        temperature: options.chat.temperature,
        max_output_tokens: options.chat.max_output_tokens,
        tools: chat_tools_from_options(&options).await?,
        metadata: json!({
            "source": "agent_tui",
            "surface": "agent_tui",
            "mode": "natural_language",
        }),
        max_tool_rounds: options.chat.max_tool_rounds,
    };

    let mut stream = runner.stream(request);
    let mut assistant_text = String::new();
    let mut final_response = None;
    while let Some(event) = stream.next().await {
        let event = event.map_err(|err| miette!(err.record.message))?;
        for update in updates_from_chat_event(&event, &mut assistant_text, &mut final_response) {
            let _ = sender.send(update);
        }
    }
    let assistant_content = final_response
        .as_ref()
        .map(|response: &agent_llm::LlmResponse| response.content.clone())
        .filter(|content| !content.is_empty())
        .unwrap_or(assistant_text);
    if !assistant_content.is_empty() {
        let _ = sender.send(TuiUpdate::AssistantReplace(assistant_content.clone()));
        messages.push(LlmMessage {
            role: LlmRole::Assistant,
            content: Value::String(assistant_content),
            name: None,
            metadata: json!({}),
        });
        let _ = sender.send(TuiUpdate::ChatMessages(messages));
    } else {
        let _ = sender.send(TuiUpdate::AssistantFinish);
    }
    Ok(())
}

fn apply_chat_event_to_tui(
    state: &mut TuiState,
    event: &ChatTurnEvent,
    assistant_text: &mut String,
    final_response: &mut Option<agent_llm::LlmResponse>,
) {
    for update in updates_from_chat_event(event, assistant_text, final_response) {
        state.apply_update(update);
    }
}

fn updates_from_chat_event(
    event: &ChatTurnEvent,
    assistant_text: &mut String,
    final_response: &mut Option<agent_llm::LlmResponse>,
) -> Vec<TuiUpdate> {
    let mut updates = Vec::new();
    match event.kind {
        ChatTurnEventKind::Started => {
            let provider = event
                .metadata
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let model = event
                .metadata
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            updates.push(TuiUpdate::Event(format!(
                "chat started: {provider}/{model}"
            )));
        }
        ChatTurnEventKind::LlmStarted => {
            updates.push(TuiUpdate::Event(format!("round {} started", event.round)));
        }
        ChatTurnEventKind::Delta => {
            if let Some(content) = &event.content {
                assistant_text.push_str(content);
                updates.push(TuiUpdate::AssistantDelta(content.clone()));
            }
        }
        ChatTurnEventKind::ThinkingDelta => {
            if let Some(content) = &event.content {
                updates.push(TuiUpdate::Event(format!("thinking: {content}")));
            }
        }
        ChatTurnEventKind::ThinkingSignatureDelta => {}
        ChatTurnEventKind::ToolCallStart => {
            updates.push(TuiUpdate::Event(format!(
                "tool start: {} {}",
                event.tool_call_id.as_deref().unwrap_or(""),
                event.tool_name.as_deref().unwrap_or("")
            )));
        }
        ChatTurnEventKind::ToolCallDelta => {
            if let Some(partial) = &event.partial_input_json {
                updates.push(TuiUpdate::Event(format!(
                    "tool args {}: {partial}",
                    event.tool_call_id.as_deref().unwrap_or("")
                )));
            }
        }
        ChatTurnEventKind::ToolCallEnd => {
            updates.push(TuiUpdate::Event(format!(
                "tool ready: {} {}",
                event.tool_call_id.as_deref().unwrap_or(""),
                event.tool_name.as_deref().unwrap_or("")
            )));
        }
        ChatTurnEventKind::ToolResult => {
            let tool_name = event.tool_name.as_deref().unwrap_or("");
            let output = event.tool_output.as_ref().unwrap_or(&Value::Null);
            if tool_name == "ask_user" {
                if let Some(lines) = decision_request_lines(output) {
                    updates.push(TuiUpdate::ToolMessage {
                        title: Some(tool_name.to_owned()),
                        content: lines.join("\n"),
                    });
                } else {
                    updates.push(TuiUpdate::ToolMessage {
                        title: Some(tool_name.to_owned()),
                        content: format!("tool result: {}", compact_json(output)),
                    });
                }
            } else {
                updates.push(TuiUpdate::ToolMessage {
                    title: Some(tool_name.to_owned()),
                    content: format!("tool result: {}", compact_json(output)),
                });
            }
        }
        ChatTurnEventKind::Usage => {
            if let Some(usage) = &event.usage {
                updates.push(TuiUpdate::Event(format!(
                    "usage: input={} output={} total={}",
                    usage.input_tokens, usage.output_tokens, usage.total_tokens
                )));
            }
        }
        ChatTurnEventKind::RoundFinished => {
            if let Some(response) = &event.response {
                *final_response = Some(response.clone());
                updates.push(TuiUpdate::Event(format!(
                    "round {} finished: {:?}",
                    event.round, response.finish_reason
                )));
            }
        }
        ChatTurnEventKind::Error => {
            let message = event.content.as_deref().unwrap_or("unknown error");
            updates.push(TuiUpdate::AssistantReplace(format!("Error: {message}")));
            updates.push(TuiUpdate::Event(format!("chat error: {message}")));
        }
        ChatTurnEventKind::Done => {
            let reason = event
                .metadata
                .get("stop_reason")
                .and_then(Value::as_str)
                .unwrap_or("done");
            updates.push(TuiUpdate::AssistantFinish);
            updates.push(TuiUpdate::Event(format!(
                "done: {reason} in {} round(s)",
                event.round
            )));
        }
    }
    updates
}

async fn chat_tools(state: &TuiState) -> Result<Vec<ToolSpec>> {
    chat_tools_from_options(&state.options).await
}

async fn chat_request_messages(
    options: &super::data::TuiOptions,
    agent_id: &str,
    messages: Vec<LlmMessage>,
) -> Result<Vec<LlmMessage>> {
    let Some(system_prompt) = catalog_system_prompt(options, agent_id).await? else {
        return Ok(messages);
    };
    let mut request_messages = Vec::with_capacity(messages.len() + 1);
    request_messages.push(LlmMessage {
        role: LlmRole::System,
        content: Value::String(system_prompt),
        name: None,
        metadata: json!({"source": "agent_catalog"}),
    });
    request_messages.extend(messages);
    Ok(request_messages)
}

async fn catalog_system_prompt(
    options: &super::data::TuiOptions,
    agent_id: &str,
) -> Result<Option<String>> {
    let Some(path) = &options.catalog_path else {
        return Ok(None);
    };
    let catalog = read_catalog(path.clone()).await?;
    let Some(agent) = catalog.agents.iter().find(|agent| agent.id == agent_id) else {
        return Ok(None);
    };

    let mut sections = vec![format!("You are {}.", agent.name)];
    if let Some(description) = agent
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        sections.push(description.to_owned());
    }
    let mut prompt_blocks = catalog.prompt_blocks;
    prompt_blocks.sort_by_key(|block| block.index);
    for block in prompt_blocks {
        let text = block.text.trim();
        if !text.is_empty() {
            sections.push(text.to_owned());
        }
    }
    if !catalog.tools.is_empty() {
        sections.push(
            "Use the provided tools when they are necessary. Keep normal replies concise and direct."
                .to_owned(),
        );
    }
    Ok(Some(sections.join("\n\n")))
}

async fn chat_tools_from_options(options: &super::data::TuiOptions) -> Result<Vec<ToolSpec>> {
    let mut tools = match &options.catalog_path {
        Some(path) => read_catalog(path.clone()).await?.tools,
        None => Vec::new(),
    };
    tools.extend(options.tool_overrides.source_specs.clone());
    if !tools.iter().any(|tool| tool.name == "echo") {
        tools.push(ToolSpec {
            name: "echo".to_owned(),
            description: "Echo the provided JSON input.".to_owned(),
            input_schema: json!({"type": "object"}),
            output_schema: Some(json!({"type": "object"})),
            risk: ToolRisk::ReadOnly,
            metadata: json!({"source": "agent_cli_builtin"}),
        });
    }
    Ok(tools)
}

async fn load_trace_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let path = rest.trim();
    if path.is_empty() {
        return Err(miette!("trace path is required"));
    }
    let path = Utf8PathBuf::from(path);
    let trace = read_trace(path.clone()).await?;
    state.set_trace(path.to_string(), trace);
    state.push_event(format!("loaded trace {path}"));
    Ok(())
}

async fn inspect_run_command(state: &mut TuiState, rest: &str) -> Result<()> {
    let run_id = rest.trim();
    if run_id.is_empty() {
        return Err(miette!("run id is required"));
    }
    let store = FileRunStore::new(state.options.store_path.clone())
        .await
        .into_diagnostic()?;
    let record = store
        .get_run(&RunId(run_id.to_owned()))
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette!("run '{run_id}' was not found"))?;
    state.push_event(format!(
        "run {} {} {:?}",
        run_id, record.agent_id, record.status
    ));
    state.push_system_message(pretty_json(&record));
    Ok(())
}

fn split_once(input: &str) -> (&str, &str) {
    input
        .trim()
        .split_once(char::is_whitespace)
        .map(|(head, tail)| (head, tail.trim()))
        .unwrap_or((input.trim(), ""))
}

fn split_name_and_json<'a>(input: &'a str, label: &str) -> Result<(String, &'a str)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(miette!("{label} is required"));
    }
    let (name, rest) = split_once(input);
    if name.trim().is_empty() {
        return Err(miette!("{label} is required"));
    }
    Ok((name.to_owned(), rest))
}

fn parse_json_or_default(input: &str, label: &str) -> Result<Value> {
    if input.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(input).map_err(|e| miette!("failed to parse {label} as JSON: {e}"))
}

fn parse_run_input(input: &str) -> Result<Value> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_str(input) {
        Ok(value) => Ok(value),
        Err(_) => Ok(json!({"message": input})),
    }
}

async fn default_agent_id(state: &TuiState) -> Result<String> {
    default_agent_id_from_options(&state.options).await
}

async fn default_agent_id_from_options(options: &super::data::TuiOptions) -> Result<String> {
    let agents = match &options.catalog_path {
        Some(path) => read_catalog(path.clone()).await?.agents,
        None => load_registry(options.registry_path.clone())
            .await?
            .list_specs(),
    };
    agents
        .into_iter()
        .next()
        .map(|agent| agent.id)
        .ok_or_else(|| miette!("no default agent is available"))
}

async fn load_active_registry(state: &TuiState) -> Result<Arc<dyn AgentRegistry>> {
    match &state.options.catalog_path {
        Some(path) => {
            let registry: Arc<dyn AgentRegistry> = load_catalog_registry(path.clone()).await?;
            Ok(registry)
        }
        None => {
            let registry: Arc<dyn AgentRegistry> =
                load_registry(state.options.registry_path.clone())
                    .await?
                    .into_agent_registry();
            Ok(registry)
        }
    }
}

fn push_agent_output(state: &mut TuiState, output: &Value) {
    if let Some(message) = output.get("message").and_then(Value::as_str) {
        state.push_assistant_message(message.to_owned());
    } else if let Some(content) = output.get("content").and_then(Value::as_str) {
        state.push_assistant_message(content.to_owned());
    } else {
        state.push_assistant_message(pretty_json(output));
    }
}

fn decision_request_lines(output: &Value) -> Option<Vec<String>> {
    let object = output.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("decision_request") {
        return None;
    }
    let title = object.get("title").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }
    let options = object.get("options").and_then(Value::as_array)?;
    if options.len() < 2 {
        return None;
    }

    let mut lines = vec![format!("decision: {title}")];
    if let Some(context) = object
        .get("context")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|context| !context.is_empty())
    {
        lines.push(format!("context: {context}"));
    }
    for (index, option) in options.iter().enumerate() {
        let option = option.as_object()?;
        let label = option.get("label").and_then(Value::as_str)?.trim();
        if label.is_empty() {
            return None;
        }
        let description = option
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|description| !description.is_empty());
        let recommended = option
            .get("recommended")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let marker = if recommended { " [recommended]" } else { "" };
        match description {
            Some(description) => {
                lines.push(format!("  {}. {label} - {description}{marker}", index + 1))
            }
            None => lines.push(format!("  {}. {label}{marker}", index + 1)),
        }
    }
    lines.push("reply with your choice to continue".to_owned());
    Some(lines)
}

fn pretty_json(value: &impl Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "<unprintable json>".to_owned())
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chat::ChatLlmOptions,
        tools::ToolOverrides,
        tui::data::{TranscriptRole, TuiOptions},
    };

    fn temp_store_path(dir: &tempfile::TempDir) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(dir.path().join("store")).expect("temp path should be utf8")
    }

    fn test_chat_options(response: &str) -> ChatLlmOptions {
        ChatLlmOptions {
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            mock_response: response.to_owned(),
            api_base_url: None,
            api_key_env: "OPENAI_API_KEY".to_owned(),
            anthropic_version: "2023-06-01".to_owned(),
            temperature: None,
            max_output_tokens: None,
            max_tool_rounds: 4,
        }
    }

    #[tokio::test]
    async fn run_command_executes_agent_and_loads_trace() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = TuiState::load(TuiOptions {
            catalog_path: None,
            trace_path: None,
            store_path: temp_store_path(&dir),
            registry_path: Utf8PathBuf::from("../../examples/agents.yaml"),
            tool_overrides: ToolOverrides::default(),
            chat: test_chat_options("mock response"),
            timeout_seconds: 60,
            max_retries: 0,
            retry_backoff_ms: 0,
            once: false,
        })
        .await
        .expect("state loads");

        execute_command(
            &mut state,
            r#"/run echo_agent {"message":"from interactive tui"}"#,
        )
        .await
        .expect("run command succeeds");

        assert!(state.trace.is_some());
        assert_eq!(state.recent_runs.len(), 1);
        assert_eq!(state.recent_runs[0].agent_id, "echo_agent");
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("from interactive tui"))
        );
    }

    #[tokio::test]
    async fn natural_language_input_runs_default_agent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = TuiState::load(TuiOptions {
            catalog_path: None,
            trace_path: None,
            store_path: temp_store_path(&dir),
            registry_path: Utf8PathBuf::from("../../examples/agents.yaml"),
            tool_overrides: ToolOverrides::default(),
            chat: test_chat_options("chat answer"),
            timeout_seconds: 60,
            max_retries: 0,
            retry_backoff_ms: 0,
            once: false,
        })
        .await
        .expect("state loads");

        execute_command(&mut state, "Summarize my day")
            .await
            .expect("natural input runs");

        assert!(state.trace.is_none());
        assert!(state.recent_runs.is_empty());
        assert_eq!(state.chat_messages.len(), 2);
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("Summarize my day"))
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("chat answer"))
        );
        let assistant_items = state
            .transcript
            .iter()
            .filter(|item| item.role == TranscriptRole::Assistant)
            .collect::<Vec<_>>();
        assert_eq!(assistant_items.len(), 1);
        assert_eq!(assistant_items[0].content, "chat answer");
    }

    #[tokio::test]
    async fn natural_language_task_streams_updates_to_tui_state() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = TuiState::load(TuiOptions {
            catalog_path: None,
            trace_path: None,
            store_path: temp_store_path(&dir),
            registry_path: Utf8PathBuf::from("../../examples/agents.yaml"),
            tool_overrides: ToolOverrides::default(),
            chat: test_chat_options("background answer"),
            timeout_seconds: 60,
            max_retries: 0,
            retry_backoff_ms: 0,
            once: false,
        })
        .await
        .expect("state loads");
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();

        let task = start_natural_language_task(&mut state, "Hello async".to_owned(), sender);
        assert!(state.busy);
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("Hello async"))
        );

        loop {
            let update = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
                .await
                .expect("update arrives")
                .expect("update exists");
            state.apply_update(update);
            if !state.busy {
                break;
            }
        }
        task.await.expect("task joins");

        assert_eq!(state.chat_messages.len(), 2);
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("background answer"))
        );
        let assistant_items = state
            .transcript
            .iter()
            .filter(|item| item.role == TranscriptRole::Assistant)
            .collect::<Vec<_>>();
        assert_eq!(assistant_items.len(), 1);
        assert_eq!(assistant_items[0].content, "background answer");
        assert!(
            crate::tui::render::render_tui_once(&state)
                .expect("tui renders")
                .contains("background answer")
        );
    }

    #[tokio::test]
    async fn tui_applies_shared_agent_chat_turn_event_fixture() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = TuiState::load(TuiOptions {
            catalog_path: None,
            trace_path: None,
            store_path: temp_store_path(&dir),
            registry_path: Utf8PathBuf::from("../../examples/agents.yaml"),
            tool_overrides: ToolOverrides::default(),
            chat: test_chat_options("unused"),
            timeout_seconds: 60,
            max_retries: 0,
            retry_backoff_ms: 0,
            once: false,
        })
        .await
        .expect("state loads");
        state.clear_output();

        let events: Vec<ChatTurnEvent> = serde_json::from_str(include_str!(
            "../../../../fixtures/docs/agent_chat_turn_events.json"
        ))
        .expect("shared chat turn events fixture");
        let mut assistant_text = String::new();
        let mut final_response = None;
        for event in &events {
            apply_chat_event_to_tui(&mut state, event, &mut assistant_text, &mut final_response);
        }

        assert_eq!(assistant_text, "Checking ");
        assert!(final_response.is_some());
        assert!(
            state
                .events
                .iter()
                .any(|line| line.contains("tool start: call_1 get_holdings"))
        );
        assert!(
            state
                .events
                .iter()
                .any(|line| line.contains("usage: input=11 output=7 total=18"))
        );
        assert!(
            state
                .events
                .iter()
                .any(|line| line.contains("round 1 finished"))
        );
    }

    #[tokio::test]
    async fn tui_renders_shared_ask_user_turn_fixture_as_decision_options() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = TuiState::load(TuiOptions {
            catalog_path: None,
            trace_path: None,
            store_path: temp_store_path(&dir),
            registry_path: Utf8PathBuf::from("../../examples/agents.yaml"),
            tool_overrides: ToolOverrides::default(),
            chat: test_chat_options("unused"),
            timeout_seconds: 60,
            max_retries: 0,
            retry_backoff_ms: 0,
            once: false,
        })
        .await
        .expect("state loads");
        state.clear_output();

        let events: Vec<ChatTurnEvent> = serde_json::from_str(include_str!(
            "../../../../fixtures/docs/agent_chat_ask_user_turn_events.json"
        ))
        .expect("shared ask_user turn events fixture");
        let mut assistant_text = String::new();
        let mut final_response = None;
        for event in &events {
            apply_chat_event_to_tui(&mut state, event, &mut assistant_text, &mut final_response);
        }

        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("decision: Implementation path"))
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("1. Context transcript"))
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("[recommended]"))
        );
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("reply with your choice to continue"))
        );
    }

    #[tokio::test]
    async fn run_command_accepts_text_input() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = TuiState::load(TuiOptions {
            catalog_path: None,
            trace_path: None,
            store_path: temp_store_path(&dir),
            registry_path: Utf8PathBuf::from("../../examples/agents.yaml"),
            tool_overrides: ToolOverrides::default(),
            chat: test_chat_options("mock response"),
            timeout_seconds: 60,
            max_retries: 0,
            retry_backoff_ms: 0,
            once: false,
        })
        .await
        .expect("state loads");

        execute_command(&mut state, "/run echo_agent hello tui")
            .await
            .expect("text run command succeeds");

        assert!(state.trace.is_some());
        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains("hello tui"))
        );
    }

    #[tokio::test]
    async fn tool_command_calls_active_services() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut state = TuiState::load(TuiOptions {
            catalog_path: None,
            trace_path: None,
            store_path: temp_store_path(&dir),
            registry_path: Utf8PathBuf::from("../../examples/agents.yaml"),
            tool_overrides: ToolOverrides::default(),
            chat: test_chat_options("mock response"),
            timeout_seconds: 60,
            max_retries: 0,
            retry_backoff_ms: 0,
            once: false,
        })
        .await
        .expect("state loads");

        execute_command(&mut state, r#"/tool echo {"value":42}"#)
            .await
            .expect("tool command succeeds");

        assert!(
            state
                .transcript
                .iter()
                .any(|item| item.content.contains(r#""value": 42"#))
        );
    }
}
