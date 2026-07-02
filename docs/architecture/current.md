# Agent Runtime Current Architecture

This is the current implementation map for agents working on the Rust agent
runtime. Use this document for code navigation and maintenance decisions.
`design.md` is the long-term design direction; `roadmap.md` turns known gaps
into phased work; this document describes what exists now.

## Scope

The runtime is a standalone Rust workspace. It provides:

- reusable Rust contracts and runner crates
- dependency-light TypeScript bindings under `bindings/ts`
- schema / fixture / OpenAPI wire contracts
- local CLI, HTTP, stdio, eval, replay, debug bundle, and TUI surfaces

The runtime must remain independent of host app frameworks and business domain
models. Host-specific policy, repositories, and device integrations live in
adapters outside this repository unless a migration explicitly moves them into
a Rust agent crate.

## Code Map

```text
crates/
  agent-core/       Stable DTOs, traits, IDs, errors, trace/session/proposal contracts
  agent-runtime/    AgentRunner, scheduler, retry/timeout, lease locking, trace capture
  agent-store/      In-memory and file-backed run/state/proposal/session stores
  agent-llm/        Provider-neutral LLM DTOs, mock/OpenAI/Anthropic/Ollama providers
  agent-chat/       Shared ChatTurn request/event contract and provider/tool loop
  agent-cli/        Local developer surfaces and host adapters

crates/agent-core/src/
  lib.rs            Public exports only
  ids.rs            Agent/run/proposal/session/thread/step identifiers
  errors.rs         Agent, store, and tool error records
  catalog.rs        Agent specs, schedules, catalog, prompt manifest
  run.rs            Run request/result/status and runtime records
  trace.rs          Trace and hook event wire records
  session.rs        Session/thread/step records
  proposal.rs       Proposal envelope and approval decisions
  services.rs       Host service traits
  stores.rs         Run/state/session/proposal/lock store traits

crates/agent-runtime/src/
  lib.rs            Public exports only
  runner.rs         AgentRunner lifecycle, retry/timeout, idempotency keys
  policy.rs         ExecutionPolicy
  lock.rs           AgentLockStore helpers and in-memory lease store
  recovery.rs       Stale run recovery
  scheduler.rs      Manual/interval schedule decisions
  services.rs       Basic and traced AgentServices wrappers
  trace.rs          In-memory trace sink
  registry.rs       In-memory AgentRegistry
  tests.rs          Runner lifecycle tests

crates/agent-store/src/
  lib.rs            Public exports only
  memory.rs         In-memory run/state/proposal/session stores
  file.rs           File-backed run/proposal/session stores
  util.rs           Shared scope and run sorting helpers
  tests.rs          Store backend tests

crates/agent-llm/src/
  lib.rs            Public exports only
  types.rs          Provider-neutral LLM DTOs, stream events, errors, provider trait
  mock.rs           Mock provider for local tests and deterministic runners
  usage.rs          Rough token usage estimator for mock/local responses
  sse.rs            Shared server-sent event frame parsing helpers
  providers/
    mod.rs          Provider exports and shared mapping helpers
    openai.rs       OpenAI-compatible chat/completions request, response, stream mapping
    anthropic.rs    Anthropic messages request, response, stream mapping
    ollama.rs       Ollama chat request, response, synthetic stream mapping
  tests.rs          Provider request mapping and SSE stream tests

crates/agent-chat/src/
  lib.rs            Public exports and tests
  types.rs          ChatTurn request/state/event/tool-result wire DTOs
  state.rs          Pure chat turn state transitions
  runner.rs         LLM/tool continuation loop and stream orchestration
  events.rs         Event sender helpers
  error.rs          ChatTurn error mapping

crates/agent-cli/src/
  main.rs           clap wiring and top-level command dispatch only
  chat.rs           CLI/TUI LLM provider construction for ChatTurnRunner
  commands/         Thin command handlers for run/catalog/tool/proposal/session/llm/cmd
  catalog.rs        Catalog loading, prompt manifest, catalog dry-run registry
  registry.rs       YAML registry and local example agent implementations
  tools.rs          Tool facade and CLI AgentServices
  tools/            Tool source manifests, process/MCP/HTTP adapters, shared errors
  runtime_server.rs HTTP/stdio runtime orchestration over AgentRunner
  server.rs         HTTP and stdio server transport handlers
  replay.rs         Trace replay modes and output comparison
  eval.rs           Eval case execution, golden trace checks, scoring hooks
  proposal.rs       Proposal lifecycle helpers and trace appenders
  session.rs        Session/thread/step reports and recording helpers
  debug_bundle.rs   Local reproduction bundle export and redaction helpers
  tui.rs, tui/      Interactive TUI shell, state, command handling, rendering

schemas/
  JSON Schema contracts for runtime wire types, including ChatTurn request,
  state, event, and tool-result resume payloads.

fixtures/contracts/
  Valid and invalid schema fixtures used by contract tests and CLI examples.
  ChatTurn fixtures are also consumed by `agent-chat` runtime tests so schema
  drift breaks locally.

openapi/agent-runtime-api.yaml
  Minimal HTTP API contract for server-first clients.

bindings/ts/
  Dependency-light TypeScript package for stable `agent.v1` wire types, a small
  HTTP client including `streamChatTurn()`, and `generateObject<T>()` helpers
  over JSON Schema structured LLM requests. It intentionally does not depend on
  Zod or AI SDK; host apps convert their schema system into JSON Schema before
  calling it.

docs/integration/
  Host-application integration guidance. `business-agent-integration.md`
  describes how other business domains build agents, catalogs, ToolHosts,
  proposals, and trace adapters on top of the runtime. `flutter-frb-native-bridge.md`
  documents the NaviWealth-style Flutter FRB/native bridge pattern without
  making Flutter or product code part of this standalone runtime repository.

docs/architecture/roadmap.md
  Phased roadmap for turning the current runtime kernel into production-ready
  business integration surfaces.
```

## Runtime Layers

1. `agent-core` defines protocol shapes and host-facing traits.
2. `agent-llm` owns provider-neutral request/response/event DTOs and provider
   clients.
3. `agent-chat` owns provider-neutral interactive ChatTurn request/event
   contracts and the LLM/tool continuation loop over `AgentServices`.
4. `agent-runtime` executes scheduled or explicit `Agent` instances through
   `AgentRunner`.
5. `agent-store` persists run/state/proposal/session records.
6. Host adapters implement `AgentServices` and tool/proposal/state behavior.
7. CLI, HTTP, stdio, and TUI are surfaces over these contracts.

`AgentRunner` returns `RunOutcome`, not only `AgentRunResult`, because callers
need both the final result and the captured `AgentTrace`.

## Interaction Entrypoints

Use these entrypoints when debugging:

```bash
rtk cargo run -p agent-cli -- list
rtk cargo run -p agent-cli -- run echo_agent --input examples/fixtures/echo-input.json
rtk cargo run -p agent-cli -- tui
rtk cargo run -p agent-cli -- serve --catalog fixtures/contracts/catalog.valid.json
curl -N -X POST http://127.0.0.1:8765/chat/turn -H 'content-type: application/json' -d '{"provider":"mock","model":"mock-model","messages":[{"role":"user","content":"ping"}]}'
rtk cargo run -p agent-cli -- validate schemas/run-request.schema.json fixtures/contracts/run-request.valid.json
```

TUI uses persistent natural input by default. Plain text runs the shared
`agent-chat` ChatTurn path; slash commands perform explicit runtime debugging:

```text
/run <agent_id> [json|text]
/tool <name> [json]
/replay <trace_path>
/inspect <run_id>
/refresh
/clear
/help
```

## Change Routing

| Change | Start Here | Required Checks |
|---|---|---|
| Add or change a wire field | `schemas/`, `crates/agent-core/`, fixtures | schema validation tests, `rtk cargo test -p agent-cli` |
| Change runner lifecycle | `crates/agent-runtime/src/lib.rs` | `rtk cargo test -p agent-runtime`, `rtk cargo test -p agent-cli` |
| Change LLM provider behavior | `crates/agent-llm/src/providers/` | `rtk cargo test -p agent-llm` |
| Change ChatTurn behavior | `crates/agent-chat/`, `schemas/chat-turn-*.schema.json`, ChatTurn fixtures | `rtk cargo test -p agent-chat`, `rtk cargo test -p agent-cli --test contracts` |
| Change HTTP ChatTurn streaming | `crates/agent-cli/src/server.rs`, `crates/agent-cli/src/runtime_server.rs`, `openapi/agent-runtime-api.yaml`, `bindings/ts/` | `rtk cargo test -p agent-cli --test catalog_cli http_server_streams_chat_turn_events`, TS binding tests |
| Change CLI command behavior | `crates/agent-cli/src/commands/` plus supporting module | focused command test, `rtk cargo test -p agent-cli` |
| Change tool host behavior | `crates/agent-cli/src/tools.rs`, `crates/agent-cli/src/tools/` | CLI tool tests |
| Change proposal apply behavior | `crates/agent-cli/src/proposal.rs` | proposal CLI tests |

## Invariants

- All top-level runtime wire messages carry `protocol_version: "agent.v1"`.
- Cross-language contracts are JSON-first. Keep envelopes stable and put
  business-specific data in `input`, `output`, `payload`, or `metadata`.
- Committed JSON Schemas under `schemas/` are the authoritative wire contract.
  Runtime DTOs must deserialize every committed valid fixture that maps to a
  Rust type, and contract changes must update schemas, fixtures, and contract
  tests in the same change.
- Runtime crates must not import host app framework or business feature code.
- TUI natural input must execute through `ChatTurnRunner`. Runtime debugging
  slash commands may execute through `AgentRunner`, but must still use shared
  JSON contracts rather than ad-hoc behavior.

## Current Design Drift

These are intentional or pending differences from the long-term design:

- No standalone `agent-tools` crate exists yet. Tool traits live in
  `agent-core`; concrete process/MCP/HTTP adapters are isolated under
  `crates/agent-cli/src/tools/` pending a future crate extraction.
- There is no standalone `bindings/dart` SDK package.
- Flutter FRB/native bridge guidance exists as documentation only; this
  standalone repository does not contain a generated Flutter package or FRB
  bindings.
- Trace is event-first (`events`) and does not currently expose a separate
  `spans` array.
- `ProposalEnvelope` does not carry a `risk` field; risk is currently expressed
  through tool/proposal metadata and host-side confirmation.
- `ScheduleSpec` supports `manual` and `interval`; cron/plugin/HTTP registries
  remain future work.
- External cancel/pause/resume is not fully implemented. TUI natural input uses
  the shared `agent-chat` stream, but terminal redraw/cancel is still not a
  non-blocking live run loop.

When code and older handoff notes disagree, use this document and current tests
as authority.
