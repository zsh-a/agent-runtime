# Flutter FRB Native Bridge Integration

This guide describes how a NaviWealth-style Flutter application should embed
the standalone agent runtime through a Flutter Rust Bridge (FRB) native bridge.
It is integration guidance for host applications; the runtime repository must
remain independent of Flutter, Riverpod, mobile platform APIs, and product
domain models.

## Goal

Use the Rust runtime as the shared agent protocol and execution layer while the
Flutter app keeps ownership of product state, user permissions, persistence,
device integrations, and UI.

```text
Flutter UI
  -> Feature-owned Dart agent adapter
  -> App-level AgentRuntimeBridge
  -> FRB native Rust bridge
  -> agent-chat / agent-runtime / agent-llm
  -> Dart ToolHost
  -> Riverpod providers / repositories / platform APIs
```

The important boundary is JSON-first. Rust owns protocol validation, provider
mapping, chat turn state, tool-round budgeting, run traces, and proposal
envelopes. Dart owns host services and side effects.

## When To Use This Path

Use the FRB/native bridge when the Flutter app needs:

- local mobile or desktop execution without a long-running HTTP sidecar
- direct use of device-local profile settings and credentials
- device or app database tools implemented in Dart
- native streaming events for chat UI
- a clear "no app server required" direct-provider path

Use the HTTP server path instead when the app is web-first, server-first, or
needs a remote runtime process shared by multiple clients.

## Ownership Boundaries

Runtime crates may depend on:

- `agent-core` wire DTOs, traits, errors, trace/session/proposal contracts
- `agent-chat` ChatTurn request/event/state contracts
- `agent-llm` provider-neutral LLM request/response/event contracts
- `agent-runtime` explicit or scheduled agent execution
- `agent-store` storage traits or local development stores

Host apps own:

- Flutter UI state and navigation
- Riverpod providers and feature repositories
- active domain selection
- user and account context
- permission prompts and risk confirmations
- local app database writes
- platform APIs such as files, health data, calendar, contacts, keychain, and
  secure storage
- production trace persistence if the app already has an AI trace model

Do not import Flutter or product feature code into runtime crates. Add host
behavior through bridge functions, `AgentServices`, tool dispatch, or proposal
application.

## App-Side Package Layout

A NaviWealth-style app should keep the runtime adapter near the app composition
root, not inside individual feature packages.

```text
lib/agent_runtime/
  agent_runtime_bridge.dart        // app-facing abstract interface
  frb_agent_runtime_bridge.dart    // FRB implementation
  http_agent_runtime_bridge.dart   // optional development fallback
  agent_runtime_contracts.dart     // JSON DTO helpers and validators
  agent_runtime_catalog.dart       // active-domain catalog composition
  agent_runtime_tool_host.dart     // tool name -> Dart service dispatch
  agent_runtime_chat_runner.dart   // ChatTurnEvent -> app chat events
  agent_runtime_proposals.dart     // proposal confirmation and apply hooks
  agent_runtime_trace_recorder.dart
```

Feature code should depend on feature-owned seams, for example
`BriefingAgent`, `InboxTriageAgent`, or `AiChatRunner`, not directly on the
low-level bridge. This keeps runtime routing replaceable.

## Native Bridge Shape

The first FRB surface should stay JSON-string based. That avoids tight coupling
between generated Dart classes and Rust DTO internals while still enforcing the
committed JSON Schemas.

Minimum bridge functions:

```rust
pub fn agent_runtime_validate_catalog(catalog_json: String) -> Result<String>;
pub fn agent_runtime_catalog_summary(catalog_json: String) -> Result<String>;

pub fn agent_runtime_validate_llm_request(request_json: String) -> Result<String>;
pub fn agent_runtime_complete_llm(request_json: String) -> Result<String>;

pub fn agent_runtime_validate_chat_turn_request(request_json: String) -> Result<String>;
pub fn agent_runtime_stream_chat_turn(request_json: String) -> Stream<String>;

pub fn agent_runtime_run_once(request_json: String) -> Result<String>;
pub fn agent_runtime_validate_proposal(proposal_json: String) -> Result<String>;
```

Each function should return either a canonical JSON payload or a structured
bridge error encoded as JSON. Avoid returning partially typed native structs to
Dart until the contract is stable enough to generate bindings from `schemas/`.

For device tools that must execute in Dart, expose the typed embedded
start/continue state machine rather than reimplementing step validation in the
bridge:

```rust
let snapshot = EffectStepLoop::start_snapshot(
    &catalog,
    request,
    agent_id,
    EmbeddedRunLimits::default(),
)?;
// Serialize `snapshot` across FRB. The host executes only the requested effect.
let next = EffectStepLoop::continue_snapshot(
    &catalog,
    snapshot,
    effect_response,
    agent_id,
)?;
```

`EmbeddedRunSnapshot` is a versioned serializable checkpoint defined by
`schemas/embedded-run-snapshot.schema.json`; its current step is defined by
`schemas/embedded-run-step.schema.json`. Flutter may persist the snapshot JSON
in an app-owned store and return it on resume, but it must not mutate
continuation, progress, run-state, or trace fields. Rust validates those fields
before advancing and owns effect-budget/subagent-depth termination. Use
`start_requested_subagent` and `resume_parent_from_subagent` so nested runs
share the same effect budget.

## Catalog Composition

The Flutter app should compose a catalog from active domains and product
capabilities, then pass it to Rust as JSON.

Recommended rules:

- Keep `protocol_version` as `agent.v1`.
- Keep business-specific identifiers in `id`, `metadata`, and tool names.
- Include only tools that the current user, account, device, and feature flags
  can actually use.
- Generate prompt blocks from product-owned policy text, not from runtime code.
- Validate catalog JSON before any run or chat turn starts.

Runtime catalog data should describe capabilities, not hold live application
state. Live state should be fetched by tools on demand.

## Chat Integration

Interactive chat should use the shared `ChatTurnRequest` and
`ChatTurnEvent` contract.

Flutter builds a request:

```dart
final request = {
  'protocol_version': 'agent.v1',
  'surface': 'mobile',
  'mode': 'natural_language',
  'session_id': sessionId,
  'thread_id': threadId,
  'agent_id': agentId,
  'provider': profile.provider,
  'model': profile.model,
  'messages': messages,
  'temperature': profile.temperature,
  'max_output_tokens': profile.maxOutputTokens,
  'tools': activeToolSpecs,
  'metadata': {
    'source': 'flutter',
    'routing_reason': 'frb_chat',
  },
  'max_tool_rounds': 4,
};
```

Rust streams JSON `ChatTurnEvent` frames. Flutter maps them to app UI events:

| ChatTurnEvent kind | Flutter behavior |
|---|---|
| `started` | mark turn active and attach provider/model metadata |
| `llm_started` | show round activity if needed |
| `delta` | append to the active assistant message |
| `thinking_delta` | record or display reasoning activity only where allowed |
| `tool_call_start` / `tool_call_delta` / `tool_call_end` | show tool activity, usually collapsed |
| `tool_result` | append a tool trace item; special-case `ask_user` |
| `usage` | record token usage and cost metadata |
| `round_finished` | persist the round response and updated state metadata |
| `done` | close the active assistant stream |
| `error` | close the stream and surface a retryable app error |

For mobile UX, do not render every token as a separate chat row. Accumulate
`delta` events into one assistant bubble and keep tool/debug events in a
secondary activity model.

## Tool Host

Dart should expose app capabilities through a single `AgentRuntimeToolHost`.
The tool host receives JSON-RPC-like calls from Rust and dispatches them to
feature services.

```dart
abstract interface class AgentRuntimeToolHost {
  Future<Map<String, Object?>> callTool(
    String name,
    Map<String, Object?> input,
    AgentRuntimeToolContext context,
  );
}
```

Recommended tool design:

- Use stable snake_case tool names, such as `get_portfolio_snapshot`.
- Keep tool inputs and outputs JSON-object shaped.
- Put user-visible side effects behind proposal confirmation.
- Mark read-only tools as `read_only`.
- Mark writes, trades, health edits, notifications, and destructive actions as
  `medium` or `high`.
- Enforce app permissions in Dart even if Rust already validated the tool spec.

The runtime should never call feature repositories directly. It should only see
tool names, JSON inputs, JSON outputs, risk metadata, and errors.

## Proposals And Confirmation

For user-visible side effects, prefer proposal flow over direct tool writes.

```text
Rust produces ProposalEnvelope
  -> Dart parses and renders summary / warnings / diff
  -> user explicitly confirms
  -> Dart ProposalApplier performs the product-specific write
  -> result is recorded as trace/proposal action
```

Host confirmation is mandatory for actions such as portfolio changes, order
placement, account edits, irreversible deletes, health data writes, and
notification scheduling. The runtime can suggest the action; the Flutter app
must own final authorization.

## Runs Versus Chat Turns

Use `agent-runtime` `RunRequest` for explicit product workflows:

- scheduled summaries
- triage or classification
- deterministic tool-plan execution
- proposal generation
- background jobs

Use `agent-chat` `ChatTurnRequest` for user-facing conversation:

- streaming assistant responses
- multiple tool rounds
- `ask_user` pauses
- provider-neutral chat events

A Flutter chat screen can start with `ChatTurnRequest`, while a background
feature such as a morning briefing can use `RunRequest` or a profile-backed
run wrapper.

## Provider Profiles

The Flutter app should own editable provider profiles and credentials. Rust
should receive only the resolved provider configuration needed for a call:

- provider kind, such as `openai-compatible`, `anthropic`, `ollama`, or `mock`
- model id
- base URL when required
- request tuning such as temperature and max output tokens
- API key through the native secure path selected by the app

Do not persist provider secrets in trace payloads, catalog metadata, or tool
outputs. Trace redaction should treat headers, API keys, bearer tokens, account
ids, and raw documents as sensitive unless a product policy says otherwise.

## Trace Persistence

Rust should emit runtime-shaped traces and chat events. Flutter should adapt
them into the app's production trace store when one exists.

Recommended trace fields:

- runtime protocol version
- route or surface, such as `frb_chat` or `frb_profile_turn`
- session id and thread id
- agent id and prompt/catalog version
- provider and model
- tool calls, inputs after redaction, outputs after redaction
- proposal ids and confirmation result
- token usage
- terminal status and error code

Keep raw provider responses and full tool outputs out of user-visible logs
unless the app has an explicit debug/export flow.

## Cancellation And Resilience

Flutter should be able to cancel an active chat turn or run when the user
leaves the screen. The FRB bridge should propagate cancellation to Rust where
the provider supports it and mark the app-side turn as cancelled.

Error mapping should preserve:

- stable error code
- user-safe message
- retryable flag
- provider or tool origin
- redacted diagnostic details

The UI should distinguish provider failures, validation failures, permission
failures, tool failures, and user cancellation.

## Testing Contract

Minimum tests for a Flutter integration:

- catalog JSON generated by the app validates against `schemas/catalog.schema.json`
- each app tool spec has valid input and output schema
- every production tool has a golden request/response fixture
- ChatTurn request fixtures validate before FRB dispatch
- ChatTurn stream fixtures map to the app chat event vocabulary
- proposal fixtures require explicit confirmation before apply
- trace recorder fixtures preserve run ids, tool ids, status, and redaction
- provider profile tests reject missing keys, invalid base URLs, and secret
  leakage in traces

Runtime contract changes must update `schemas/`, `fixtures/contracts/`, and the
host app compatibility fixtures together.

## Migration Plan

1. Add the app-level `AgentRuntimeBridge` abstraction with a mock
   implementation.
2. Generate active-domain catalog JSON and validate it through the native
   bridge.
3. Add `AgentRuntimeToolHost` and start with read-only tools.
4. Route one low-risk workflow through `RunRequest`.
5. Route chat through `ChatTurnRequest` and map streaming events into the
   existing chat UI model.
6. Add proposal confirmation for write tools.
7. Record FRB runtime traces into the app's existing trace store.
8. Gradually migrate feature agents behind feature-owned seams, not direct
   bridge calls.

## Implementation Checklist

- The runtime repo stays standalone and does not import Flutter app code.
- Dart owns product state, permissions, side effects, and user confirmation.
- Rust owns JSON contract validation, provider mapping, ChatTurn state, and
  run/proposal envelopes.
- All cross-boundary payloads carry `protocol_version: "agent.v1"`.
- Business-specific data lives in `input`, `output`, `payload`, or `metadata`.
- Tool names and proposal kinds are stable and fixture-tested.
- Streaming chat updates merge deltas into one assistant message.
- Sensitive values are redacted before trace persistence.
- Feature code depends on feature-owned adapters, not raw FRB functions.
