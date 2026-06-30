use std::collections::VecDeque;

use agent_core::AgentRunRecord;
use agent_llm::LlmMessage;
use camino::{Utf8Path, Utf8PathBuf};
use miette::{IntoDiagnostic, Result, miette};
use serde_json::Value;

use crate::{
    catalog::{CatalogSummary, read_catalog},
    chat::ChatLlmOptions,
    tools::ToolOverrides,
};

const MAX_EVENT_LINES: usize = 160;
const MAX_HISTORY_ITEMS: usize = 80;
const SCROLL_LINES: u16 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TranscriptRole {
    User,
    Assistant,
    System,
    Tool,
}

impl TranscriptRole {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Assistant => "Assistant",
            Self::System => "System",
            Self::Tool => "Tool",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct TranscriptItem {
    pub(super) role: TranscriptRole,
    pub(super) title: Option<String>,
    pub(super) content: String,
    pub(super) streaming: bool,
}

#[derive(Debug)]
pub(super) enum TuiUpdate {
    Event(String),
    AssistantDelta(String),
    AssistantReplace(String),
    AssistantFinish,
    ToolMessage {
        title: Option<String>,
        content: String,
    },
    ChatMessages(Vec<LlmMessage>),
    Busy(bool),
    Error(String),
}

#[derive(Debug, Clone)]
pub(crate) struct TuiOptions {
    pub(crate) catalog_path: Option<Utf8PathBuf>,
    pub(crate) trace_path: Option<Utf8PathBuf>,
    pub(crate) store_path: Utf8PathBuf,
    pub(crate) registry_path: Utf8PathBuf,
    pub(crate) tool_overrides: ToolOverrides,
    pub(crate) chat: ChatLlmOptions,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_retries: u32,
    pub(crate) retry_backoff_ms: u64,
    pub(crate) once: bool,
}

pub(super) struct TuiState {
    pub(super) options: TuiOptions,
    pub(super) catalog_summary: Option<CatalogSummary>,
    pub(super) trace: Option<agent_core::AgentTrace>,
    pub(super) trace_label: Option<String>,
    pub(super) recent_runs: Vec<AgentRunRecord>,
    pub(super) status: String,
    pub(super) input_mode: bool,
    pub(super) command_input: String,
    pub(super) transcript: Vec<TranscriptItem>,
    pub(super) events: VecDeque<String>,
    pub(super) chat_messages: Vec<LlmMessage>,
    pub(super) chat_scroll: u16,
    pub(super) event_scroll: u16,
    pub(super) input_history: VecDeque<String>,
    pub(super) history_cursor: Option<usize>,
    pub(super) busy: bool,
}

impl TuiState {
    pub(super) async fn load(options: TuiOptions) -> Result<Self> {
        let catalog_summary = load_catalog_summary(options.catalog_path.as_ref()).await?;
        let trace = load_trace(options.trace_path.as_ref()).await?;
        let trace_label = options.trace_path.as_ref().map(ToString::to_string);
        let recent_runs = read_recent_runs(&options.store_path).await?;
        let status = status_line(&catalog_summary, &trace, recent_runs.len());
        let mut state = Self {
            options,
            catalog_summary,
            trace,
            trace_label,
            recent_runs,
            status,
            input_mode: true,
            command_input: String::new(),
            transcript: Vec::new(),
            events: VecDeque::new(),
            chat_messages: Vec::new(),
            chat_scroll: 0,
            event_scroll: 0,
            input_history: VecDeque::new(),
            history_cursor: None,
            busy: false,
        };
        state.push_system_message("Ready. Type a message and press Enter. Use /help for commands.");
        state.push_event("ready");
        Ok(state)
    }

    pub(super) async fn refresh(&mut self) -> Result<()> {
        self.catalog_summary = load_catalog_summary(self.options.catalog_path.as_ref()).await?;
        if let Some(path) = &self.options.trace_path {
            self.trace = Some(read_trace(path.clone()).await?);
            self.trace_label = Some(path.to_string());
        }
        self.refresh_runs().await?;
        Ok(())
    }

    pub(super) async fn refresh_runs(&mut self) -> Result<()> {
        self.recent_runs = read_recent_runs(&self.options.store_path).await?;
        self.status = status_line(&self.catalog_summary, &self.trace, self.recent_runs.len());
        Ok(())
    }

    pub(super) fn set_trace(&mut self, label: impl Into<String>, trace: agent_core::AgentTrace) {
        self.trace = Some(trace);
        self.trace_label = Some(label.into());
        self.status = status_line(&self.catalog_summary, &self.trace, self.recent_runs.len());
    }

    pub(super) fn enter_command(&mut self, prefix: &str) {
        self.input_mode = true;
        self.command_input.clear();
        self.command_input.push_str(prefix);
        self.history_cursor = None;
    }

    pub(super) fn push_event(&mut self, line: impl Into<String>) {
        let line = line.into();
        for part in line.lines() {
            self.events.push_back(part.to_owned());
        }
        while self.events.len() > MAX_EVENT_LINES {
            self.events.pop_front();
        }
    }

    pub(super) fn clear_output(&mut self) {
        self.transcript.clear();
        self.events.clear();
        self.chat_scroll = 0;
        self.event_scroll = 0;
    }

    pub(super) fn push_user_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptRole::User, None, content.into(), false);
    }

    pub(super) fn push_assistant_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptRole::Assistant, None, content.into(), false);
    }

    pub(super) fn push_system_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptRole::System, None, content.into(), false);
    }

    pub(super) fn push_tool_message(
        &mut self,
        title: impl Into<Option<String>>,
        content: impl Into<String>,
    ) {
        self.push_transcript(TranscriptRole::Tool, title.into(), content.into(), false);
    }

    pub(super) fn start_assistant_stream(&mut self) {
        self.push_transcript(TranscriptRole::Assistant, None, String::new(), true);
    }

    pub(super) fn append_assistant_delta(&mut self, content: &str) {
        if !matches!(
            self.transcript.last(),
            Some(item) if item.role == TranscriptRole::Assistant && item.streaming
        ) {
            self.start_assistant_stream();
        }
        if let Some(item) = self.transcript.last_mut() {
            item.content.push_str(content);
        }
    }

    pub(super) fn replace_streaming_assistant(&mut self, content: impl Into<String>) {
        let content = content.into();
        if let Some(item) = self
            .transcript
            .iter_mut()
            .rev()
            .find(|item| item.role == TranscriptRole::Assistant && item.streaming)
        {
            item.content = content;
            item.streaming = false;
        } else if !content.is_empty() {
            self.push_assistant_message(content);
        }
    }

    pub(super) fn finish_assistant_stream(&mut self) {
        if let Some(item) = self
            .transcript
            .iter_mut()
            .rev()
            .find(|item| item.role == TranscriptRole::Assistant && item.streaming)
        {
            item.streaming = false;
        }
    }

    pub(super) fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    pub(super) fn apply_update(&mut self, update: TuiUpdate) {
        match update {
            TuiUpdate::Event(line) => self.push_event(line),
            TuiUpdate::AssistantDelta(content) => self.append_assistant_delta(&content),
            TuiUpdate::AssistantReplace(content) => self.replace_streaming_assistant(content),
            TuiUpdate::AssistantFinish => self.finish_assistant_stream(),
            TuiUpdate::ToolMessage { title, content } => self.push_tool_message(title, content),
            TuiUpdate::ChatMessages(messages) => {
                self.chat_messages = messages;
            }
            TuiUpdate::Busy(busy) => self.set_busy(busy),
            TuiUpdate::Error(message) => {
                self.replace_streaming_assistant(format!("Error: {message}"));
                self.push_event(format!("command failed: {message}"));
            }
        }
    }

    pub(super) fn remember_input(&mut self, input: impl Into<String>) {
        let input = input.into();
        if input.trim().is_empty() {
            return;
        }
        if self.input_history.back() == Some(&input) {
            self.history_cursor = None;
            return;
        }
        self.input_history.push_back(input);
        while self.input_history.len() > MAX_HISTORY_ITEMS {
            self.input_history.pop_front();
        }
        self.history_cursor = None;
    }

    pub(super) fn history_previous(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let next = match self.history_cursor {
            Some(index) if index > 0 => index - 1,
            Some(index) => index,
            None => self.input_history.len() - 1,
        };
        self.history_cursor = Some(next);
        if let Some(value) = self.input_history.get(next) {
            self.command_input = value.clone();
        }
    }

    pub(super) fn history_next(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 >= self.input_history.len() {
            self.history_cursor = None;
            self.command_input.clear();
        } else {
            let next = index + 1;
            self.history_cursor = Some(next);
            if let Some(value) = self.input_history.get(next) {
                self.command_input = value.clone();
            }
        }
    }

    pub(super) fn scroll_chat_up(&mut self) {
        self.chat_scroll = self.chat_scroll.saturating_sub(SCROLL_LINES);
    }

    pub(super) fn scroll_chat_down(&mut self) {
        self.chat_scroll = self.chat_scroll.saturating_add(SCROLL_LINES);
    }

    pub(super) fn scroll_activity_up(&mut self) {
        self.event_scroll = self.event_scroll.saturating_sub(SCROLL_LINES);
    }

    pub(super) fn scroll_activity_down(&mut self) {
        self.event_scroll = self.event_scroll.saturating_add(SCROLL_LINES);
    }

    pub(super) fn scroll_chat_top(&mut self) {
        self.chat_scroll = 0;
    }

    pub(super) fn scroll_chat_bottom(&mut self) {
        self.chat_scroll = u16::MAX / 2;
    }

    fn push_transcript(
        &mut self,
        role: TranscriptRole,
        title: Option<String>,
        content: String,
        streaming: bool,
    ) {
        self.transcript.push(TranscriptItem {
            role,
            title,
            content,
            streaming,
        });
        self.chat_scroll = 0;
    }
}

async fn load_catalog_summary(path: Option<&Utf8PathBuf>) -> Result<Option<CatalogSummary>> {
    match path {
        Some(path) => Ok(Some(CatalogSummary::from_catalog(
            &read_catalog(path.clone()).await?,
        ))),
        None => Ok(None),
    }
}

async fn load_trace(path: Option<&Utf8PathBuf>) -> Result<Option<agent_core::AgentTrace>> {
    match path {
        Some(path) => Ok(Some(read_trace(path.clone()).await?)),
        None => Ok(None),
    }
}

pub(super) async fn read_recent_runs(store_path: &Utf8Path) -> Result<Vec<AgentRunRecord>> {
    let runs_dir = store_path.join("runs");
    if !runs_dir.exists() {
        return Ok(vec![]);
    }
    let mut entries = fs_err::tokio::read_dir(&runs_dir)
        .await
        .map_err(|e| miette!("failed to read runs at {runs_dir}: {e}"))?;
    let mut records = Vec::new();
    while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|path| miette!("non-UTF-8 run path: {}", path.display()))?;
        if path.extension() != Some("json") {
            continue;
        }
        let record = serde_json::from_value::<AgentRunRecord>(read_json(path).await?)
            .map_err(|e| miette!("failed to parse run record: {e}"))?;
        records.push(record);
    }
    records.sort_by_key(|record| record.started_at);
    records.reverse();
    records.truncate(8);
    Ok(records)
}

pub(super) async fn read_trace(path: Utf8PathBuf) -> Result<agent_core::AgentTrace> {
    let value = read_json(path.clone()).await?;
    serde_json::from_value(value).map_err(|e| miette!("failed to parse trace at {path}: {e}"))
}

async fn read_json(path: Utf8PathBuf) -> Result<Value> {
    let bytes = fs_err::tokio::read(&path)
        .await
        .map_err(|e| miette!("failed to read JSON at {path}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| miette!("failed to parse JSON at {path}: {e}"))
}

fn status_line(
    catalog_summary: &Option<CatalogSummary>,
    trace: &Option<agent_core::AgentTrace>,
    run_count: usize,
) -> String {
    format!(
        "catalog: {} | trace: {} | runs: {}",
        catalog_summary
            .as_ref()
            .map(|summary| summary.agent_count.to_string())
            .unwrap_or_else(|| "not loaded".to_owned()),
        trace
            .as_ref()
            .map(|trace| trace.run_id.0.clone())
            .unwrap_or_else(|| "not loaded".to_owned()),
        run_count
    )
}
