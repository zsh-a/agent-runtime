#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]
//! This crate is browser-only: it bridges the runtime to JavaScript through
//! `wasm-bindgen`, so it compiles solely for `wasm32-unknown-unknown`. Native
//! hosts use the CLI or embed the runtime crates directly.

//! Browser entry point for the Agent Runtime.
//!
//! The runtime is driven by JavaScript here rather than by the CLI: the host
//! supplies the LLM endpoint and the tool implementations, because tools in a
//! browser run against OPFS, Workers and WebGPU, which only the page can reach.
//!
//! `tools` plus `tool_execution: "client"` is the seam — the Rust side owns the
//! turn loop, provider calls, context planning and tracing, and hands tool calls
//! back to JavaScript, which executes them and resumes with the results.

use std::sync::Arc;

use agent_chat::{ChatResumeRequest, ChatTurnEvent, ChatTurnRequest, ChatTurnRunner};
use agent_core::{
    AgentError, AgentStateStore, RunId, RunScope, ToolContext, ToolError, ToolRegistry, ToolSpec,
    UserContext,
};
use agent_llm::OpenAiCompatibleProvider;
use agent_runtime::BasicAgentServices;
use agent_store::InMemoryStateStore;
use futures::StreamExt;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

#[wasm_bindgen(typescript_custom_section)]
const TOOL_TYPES: &str = r#"
export interface JsTool {
  /** A ToolSpec from agent-core: name, description, input_schema, risk, replay_policy. */
  readonly spec: unknown;
  /** Executes the tool; receives input as JSON, resolves with output as JSON. */
  readonly call: (inputJson: string) => Promise<string>;
}
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "JsTool")]
    pub type JsTool;
}

/// Tools dispatched to JavaScript.
struct JsToolRegistry {
    tools: Vec<JsToolEntry>,
}

struct JsToolEntry {
    spec: ToolSpec,
    call: js_sys::Function,
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait::async_trait(?Send))]
#[cfg_attr(
    not(all(target_arch = "wasm32", target_os = "unknown")),
    async_trait::async_trait
)]
impl ToolRegistry for JsToolRegistry {
    async fn list_tools(&self) -> Result<Vec<ToolSpec>, ToolError> {
        Ok(self.tools.iter().map(|entry| entry.spec.clone()).collect())
    }

    async fn call(&self, name: &str, input: Value, _ctx: ToolContext) -> Result<Value, ToolError> {
        let entry = self
            .tools
            .iter()
            .find(|entry| entry.spec.name == name)
            .ok_or_else(|| tool_validation(format!("unknown tool: {name}")))?;
        let payload = serde_json::to_string(&input).map_err(|e| tool_validation(e.to_string()))?;
        let result = entry
            .call
            .call1(&JsValue::NULL, &JsValue::from_str(&payload))
            .map_err(|error| tool_internal(js_message(&error)))?;
        let resolved = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(result))
            .await
            .map_err(|error| tool_internal(js_message(&error)))?;
        let text = resolved
            .as_string()
            .ok_or_else(|| tool_internal("tool must resolve to a JSON string"))?;
        serde_json::from_str(&text).map_err(|error| tool_validation(error.to_string()))
    }
}

fn js_message(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(value, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "JavaScript tool call failed".to_owned())
}

/// A chat turn driver bound to one endpoint, model and tool set.
#[wasm_bindgen]
pub struct AgentRuntime {
    runner: ChatTurnRunner,
}

#[wasm_bindgen]
impl AgentRuntime {
    /// Build a runtime for an OpenAI-compatible endpoint.
    #[wasm_bindgen(constructor)]
    pub fn new(
        provider: String,
        base_url: String,
        api_key: String,
        tools: Vec<JsTool>,
    ) -> Result<AgentRuntime, JsValue> {
        console_error_panic_hook::set_once();

        let provider_impl = OpenAiCompatibleProvider::new(provider, base_url, api_key)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;

        let mut entries = Vec::with_capacity(tools.len());
        for tool in tools {
            entries.push(js_tool_entry(tool)?);
        }

        let services = BasicAgentServices::new(
            "browser",
            RunId::new_v7(),
            None::<UserContext>,
            RunScope::Global,
            Arc::new(JsToolRegistry { tools: entries }),
            InMemoryStateStore::shared(),
        );

        Ok(AgentRuntime {
            runner: ChatTurnRunner::new(Arc::new(provider_impl), Arc::new(services)),
        })
    }

    /// Run a turn from a `ChatTurnRequest` JSON string to its terminal event.
    ///
    /// Resolves with every `ChatTurnEvent` as a JSON array. Under client tool
    /// execution the turn ends with a `done` event whose metadata is
    /// `requires_tool_results`; the caller then executes the emitted tool calls
    /// and continues through [`AgentRuntime::resume`].
    pub fn turn(&self, request_json: String) -> js_sys::Promise {
        let request: ChatTurnRequest = match serde_json::from_str(&request_json) {
            Ok(request) => request,
            Err(error) => {
                return js_sys::Promise::reject(&JsValue::from_str(&format!(
                    "invalid ChatTurnRequest: {error}"
                )));
            }
        };
        let runner = self.runner.clone();
        future_to_promise(async move {
            collect(runner.stream(request))
                .await
                .map(|text| JsValue::from_str(&text))
        })
    }

    /// Resume a suspended turn with the results of the tools the host executed.
    pub fn resume(&self, request_json: String) -> js_sys::Promise {
        let request: ChatResumeRequest = match serde_json::from_str(&request_json) {
            Ok(request) => request,
            Err(error) => {
                return js_sys::Promise::reject(&JsValue::from_str(&format!(
                    "invalid ChatResumeRequest: {error}"
                )));
            }
        };
        let runner = self.runner.clone();
        future_to_promise(async move {
            collect(runner.resume(request))
                .await
                .map(|text| JsValue::from_str(&text))
        })
    }
}

/// Drain an event stream into a JSON array, turning the first error into a
/// rejected promise so JavaScript sees the failure where it called in.
async fn collect(stream: agent_chat::ChatEventStream) -> Result<String, JsValue> {
    let mut events: Vec<ChatTurnEvent> = Vec::new();
    let mut stream = std::pin::pin!(stream);
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => events.push(event),
            Err(error) => return Err(JsValue::from_str(&error.to_string())),
        }
    }
    serde_json::to_string(&events).map_err(|error| JsValue::from_str(&error.to_string()))
}

fn js_tool_entry(tool: JsTool) -> Result<JsToolEntry, JsValue> {
    let value: JsValue = tool.into();
    let spec_value = js_sys::Reflect::get(&value, &JsValue::from_str("spec"))
        .map_err(|_| JsValue::from_str("tool is missing `spec`"))?;
    let call_value = js_sys::Reflect::get(&value, &JsValue::from_str("call"))
        .map_err(|_| JsValue::from_str("tool is missing `call`"))?;
    let json = js_sys::JSON::stringify(&spec_value)
        .map_err(|_| JsValue::from_str("tool spec is not JSON-serialisable"))?;
    let text = json
        .as_string()
        .ok_or_else(|| JsValue::from_str("tool spec did not serialise to a string"))?;
    let spec: ToolSpec = serde_json::from_str(&text)
        .map_err(|error| JsValue::from_str(&format!("invalid ToolSpec: {error}")))?;
    let call = call_value
        .dyn_into::<js_sys::Function>()
        .map_err(|_| JsValue::from_str("tool `call` must be a function"))?;
    Ok(JsToolEntry { spec, call })
}

/// The protocol version this build speaks, so hosts can refuse a mismatch.
#[wasm_bindgen]
pub fn protocol_version() -> String {
    agent_core::protocol_version()
}

/// `ToolError` is built from an `AgentError`, which carries the error kinds.
fn tool_validation(message: impl Into<String>) -> ToolError {
    ToolError::from_agent_error(AgentError::validation(message))
}

fn tool_internal(message: impl Into<String>) -> ToolError {
    ToolError::from_agent_error(AgentError::internal(message))
}

#[allow(dead_code)]
fn _assert_state_store_object_safe(_: Arc<dyn AgentStateStore>) {}
