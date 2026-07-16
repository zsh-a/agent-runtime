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
  agent-tools/      Reusable process, MCP stdio, HTTP JSON, mock, and builtin tool adapters
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
  embedded.rs       Typed embedded run-step, host-effect, response, and checkpoint contracts
  services.rs       Host service traits
  stores.rs         Run/state/session/proposal/lock store traits

crates/agent-runtime/src/
  lib.rs            Public exports only
  loop_core.rs      Embedded host-effect start/continue state machine and typed API
  runner/
    mod.rs          AgentRunner types, construction, scope, and shared helpers
    execution.rs    Single-run persistence, locking, cancellation, and finalization
    attempts.rs     Retry loop, policy hooks, timeout, and agent-step execution
    scheduling.rs   Scheduled-agent tick evaluation
    control.rs      Lease renewal helpers
    idempotency.rs  Idempotency key derivation and duplicate-run projection
    workflow/
      execution.rs  Workflow lease, concurrent DAG execution, and compensation
  execution_support.rs  Result, cancellation, and CAS finalization helpers
  observability.rs  Trace spans, artifacts, and usage summaries
  workflow.rs       Workflow planning, inputs, status, and compensation helpers
  policy.rs         ExecutionPolicy
  lock.rs           AgentLockStore helpers and in-memory lease store
  recovery.rs       Stale run recovery
  scheduler.rs      Manual/interval schedule decisions
  services.rs       Basic and traced AgentServices wrappers
  trace.rs          In-memory trace sink
  registry.rs       In-memory AgentRegistry
  tests/            Shared runner fixtures plus focused lifecycle, workflow,
                    locking, hooks, observability, scheduler, and recovery tests

crates/agent-store/src/
  lib.rs            Public exports only
  memory.rs         In-memory run/state/proposal/session stores
  file.rs           File-backed run/proposal/session stores
  util.rs           Shared scope and run sorting helpers
  sqlite/
    mod.rs          SQLite connection lifecycle, migrations, shared helpers
    codec.rs        SQLite value, cursor, status, and error conversions
    run.rs          Run persistence and optimistic concurrency
    events.rs       Persistent run event log
    trace.rs        Trace persistence
    state.rs        Scoped agent state
    lock.rs         Lease acquisition, renewal, and release
    proposal.rs     Proposal persistence
    session.rs      Session, thread, and step persistence
  tests/            Store backend tests

crates/agent-tools/src/
  lib.rs            Public tool override facade, builtin debug tools, schema validation
  manifest.rs       Tool source manifest loading and protocol dispatch
  process.rs        JSONL and MCP stdio process tool hosts
  http.rs           HTTP JSON tool endpoint adapter
  mcp.rs            Minimal MCP stdio request/response mapping
  error.rs          Shared ToolError helpers

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
  tests.rs, tests/  Thin test facade plus provider-specific request/SSE tests

crates/agent-chat/src/
  lib.rs            Public exports only
  types.rs          ChatTurn request/state/event/tool-result wire DTOs
  state.rs          Pure chat turn state transitions
  context.rs        Context snapshot and deterministic recent-message compaction
  runner.rs         LLM/tool continuation loop, context_snapshot events, stream orchestration
  events.rs         Event sender helpers
  error.rs          ChatTurn error mapping
  tests.rs          ChatTurn unit and fixture tests

crates/agent-cli/src/
  main.rs           Thin binary entrypoint only
  lib.rs            Internal module map and reusable application entrypoint
  app/
    mod.rs          Application module facade
    entrypoint.rs   clap definitions, effective configuration, top-level dispatch
    infrastructure.rs Logging, JSON validation, and JSON output infrastructure
                    Remaining modules own runtime composition, catalog, tools,
                    proposals, and sessions
  interfaces/       HTTP and stdio transports plus server runtime orchestration
  devtools/         Eval, replay, metrics, debug bundles, OTLP, development hosts
  commands/         Thin command handlers for run/catalog/tool/proposal/session/llm/cmd
  tui.rs, tui/      Interactive TUI shell with narrow submodules:
                   commands/ separates dispatch, actions, argument parsing,
                   and presentation;
                   chat owns the ChatTurn client-tool loop;
                   chat_events adapts ChatTurnEvent into TUI updates;
                   approval/policy gate high-risk tools;
                   runtime is the TUI facade over catalog, store, runner, tools,
                   and tool inventory;
                   data/ separates view models, state behavior, and loading;
                   render/ separates panels, selection, input, transcript,
                   approval overlays, and styles;
                   terminal owns the event loop and input devices;
                   tests/ mirrors these boundaries with focused test modules.

crates/agent-cli/tests/
  cli.rs            CLI integration test target
  cli/suite.rs      Thin integration-test module facade and shared imports
  cli/support.rs    Shared process, SQLite, and HTTP test infrastructure
  cli/catalog.rs    Catalog summary, listing, and prompt-manifest CLI tests
  cli/llm.rs        Provider-specific `agent llm complete` tests
  cli/tools.rs      Tool catalog, mock, process, MCP, HTTP, and shell adapter CLI tests
  cli/validation.rs JSON Schema validation command tests
  cli/config_profiles.rs, cli/config_stores.rs
                    Runtime profile and persistence-backend configuration tests
  cli/run.rs        Catalog-backed run and tool-loop integration tests
  cli/workflow.rs   Workflow command validation and persistence tests
  cli/replay.rs, cli/telemetry.rs
                    Replay and trace-export integration tests
  cli/debug_bundle.rs, cli/recovery.rs, cli/metrics.rs
                    Debug export, stale-run recovery, and metrics tests
  cli/server_*.rs   HTTP/stdio core, run, chat, proposal, and session scenarios
  cli/eval.rs       Evaluation execution and fixture-generation tests
  cli/proposal_*.rs Proposal lifecycle, approval, and policy command tests
  cli/tui.rs        One-shot TUI integration tests
  cli/compat.rs     Business integration compatibility smoke test
  contracts/        JSON Schema, OpenAPI, and fixture conformance suite

schemas/
  JSON Schema contracts for runtime wire types, including ChatTurn request,
  state, event, and tool-result resume payloads.

fixtures/contracts/
  Valid and invalid schema fixtures used by contract tests and CLI examples.

fixtures/chat/
  ChatTurn event sequences consumed by `agent-chat` and TUI tests.

examples/tui/modelscope/
  Provider-specific ModelScope TUI launcher example. Provider examples do not
  live in the repository maintenance-script directory.

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
2. `agent-tools` owns reusable tool-source loading and process/MCP/HTTP tool
   adapters.
3. `agent-llm` owns provider-neutral request/response/event DTOs and provider
   clients.
4. `agent-chat` owns provider-neutral interactive ChatTurn request/event
   contracts and the LLM/tool continuation loop over `AgentServices`.
5. `agent-runtime` executes scheduled or explicit `Agent` instances through
   `AgentRunner`.
6. `agent-store` persists run/state/proposal/session records.
7. Host adapters implement `AgentServicesFactory`; each run binds an immutable
   `ExecutionContext` before exposing `AgentServices` to agent code.
8. CLI, HTTP, stdio, and TUI are surfaces over these contracts.

`AgentRunner` returns `RunOutcome`, not only `AgentRunResult`, because callers
need both the final result and the captured `AgentTrace`. Its disposition marks
idempotent duplicate deliveries so adapters do not overwrite the original trace.

## Interaction Entrypoints

Use these entrypoints when debugging:

```bash
rtk cargo run -p agent-cli -- list
rtk cargo run -p agent-cli -- run echo_agent --input examples/fixtures/echo-input.json
rtk cargo run -p agent-cli -- tui
rtk cargo run -p agent-cli -- serve --catalog fixtures/contracts/catalog.valid.json
curl -N -X POST http://127.0.0.1:8765/chat/turn -H 'content-type: application/json' -d '{"protocol_version":"agent.v1","provider":"mock","model":"mock-model","messages":[{"role":"user","content":"ping"}]}'
rtk cargo run -p agent-cli -- validate schemas/run-request.schema.json fixtures/contracts/run-request.valid.json
```

The embedded HTTP server only accepts loopback bind addresses. Remote exposure
must go through a host-owned authenticated gateway.

TUI uses persistent natural input by default. Plain text runs the shared
`agent-chat` ChatTurn path; slash commands perform explicit runtime debugging:

```text
/run <agent_id> [json|text]
/tools
/tool <name> [json]
/replay <trace_path>
/inspect <run_id>
/refresh
/clear
/help
```

`/tools` shows the TUI chat tool inventory: catalog tools, configured
tool-source manifest tools, the runtime built-in `agent.run`, and the local
debug `echo` tool, annotated with risk and policy status. HTTP `GET /tools`
returns the server runtime tool catalog: catalog tools, configured tool-source
manifest tools, and `agent.run`.

## Change Routing

| Change | Start Here | Required Checks |
|---|---|---|
| Add or change a wire field | `schemas/`, `crates/agent-core/`, fixtures | schema validation tests, `rtk cargo test -p agent-cli` |
| Change runner lifecycle | `crates/agent-runtime/src/lib.rs` | `rtk cargo test -p agent-runtime`, `rtk cargo test -p agent-cli` |
| Change LLM provider behavior | `crates/agent-llm/src/providers/` | `rtk cargo test -p agent-llm` |
| Change ChatTurn behavior | `crates/agent-chat/`, `schemas/chat-turn-*.schema.json`, ChatTurn fixtures | `rtk cargo test -p agent-chat`, `rtk cargo test -p agent-cli --test contracts` |
| Change HTTP ChatTurn streaming | `crates/agent-cli/src/interfaces/`, `openapi/agent-runtime-api.yaml`, `bindings/ts/` | `rtk cargo test -p agent-cli --test cli http_server_streams_chat_turn_events`, TS binding tests |
| Change CLI command behavior | `crates/agent-cli/src/commands/` plus supporting module | focused command test, `rtk cargo test -p agent-cli` |
| Change tool host behavior | `crates/agent-tools/`, `crates/agent-cli/src/app/tools.rs` | CLI tool tests and focused `agent-tools` tests |
| Change proposal apply behavior | `crates/agent-cli/src/app/proposal.rs` | proposal CLI tests |

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

- `agent-tools` exists and is used by `agent-cli`; it validates declared
  tool input/output schemas at the dispatch boundary and applies source-level
  timeout/retry/output-size policy for JSONL process, MCP stdio, and HTTP JSON
  sources. Process and MCP stdio sources can set `cwd`, explicit `env`, and
  `inherit_env`; HTTP source headers and process env values can reference
  environment variables with `${env:NAME}` so manifests do not need to store
  bearer tokens directly. Its public facade is still oriented around local
  CLI/server tool overrides rather than a fully stabilized reusable ToolHost API
  with a host secret manager and sandbox policy.
- `agent-store` has shared conformance coverage for file, in-memory, and SQLite
  run, proposal, session, state, lock, trace, and event stores. Run creation is
  insert-only, run and proposal updates use optimistic versions, and SQLite
  enforces a unique run idempotency identity. Legacy run records without a
  version deserialize as version 1.
- `RunRequest.scope` and `WorkflowRunRequest.scope` are first-class run-scope
  overrides with `global`, `user`, and `tenant` variants. The resolved scope is
  stored on every `AgentRunRecord`, participates in idempotency material and
  per-agent lease keys, is exposed to `AgentContext`, and is inherited by
  `agent.run` subagent calls unless the tool input supplies its own `scope`.
  State storage is keyed by agent and resolved scope. Agent and workflow leases
  are renewed while work is active; loss of lease ownership cancels active agent
  work instead of allowing an unfenced execution to continue.
  When `scope` is omitted, the runtime preserves the older behavior: `user`
  creates a user scope, and no user creates a global scope.
- There is no standalone `bindings/dart` SDK package.
- Flutter FRB/native bridge guidance exists as documentation only; this
  standalone repository does not contain a generated Flutter package or FRB
  bindings.
- Trace remains event-first (`events`) and also exposes optional `spans` for
  observability. The runtime emits a run-level `agent.run` span, records
  agent-emitted events through `AgentServices::emit_event`, derives child spans
  for traced tool calls, state reads/writes, and LLM provider events, and
  aggregates `AgentTrace.usage_summary` from LLM trace events with token totals,
  optional cost micros by currency, and provider/model breakdowns.
  `agent metrics summary` and `GET /metrics/summary` aggregate persisted runs
  and traces into global counts plus grouped `runs_by_agent`,
  `tool_calls_by_tool`, and `llm_usage_by_provider` summaries with latency,
  failure, token, and cost fields.
  `agent trace export-otel` converts committed `AgentTrace.spans` into a
  schema-backed OTLP JSON-style `resourceSpans` document for collector or
  offline pipeline ingestion, and can POST the same payload to an OTLP HTTP
  traces endpoint with `--endpoint` or standard OTEL exporter environment
  variables.
- `ProposalEnvelope` carries an optimistic `version`, `risk`, `approval_policy`,
  `approval_required`,
  `required_approval_level`, `required_approver_count`, `approval_decisions`,
  structured `diffs`, structured `warnings`, `policy_id`, `policy_version`,
  and `expires_at`. `ApprovalDecision` records the deciding actor when supplied
  and the approval level used for the decision; approve decisions must satisfy
  the envelope's required level. Multi-approver proposals can accumulate
  distinct `single_user` approvals and remain `pending_approval` until the
  required approver count is reached; `admin` or explicit `multi_approver`
  decisions can satisfy the requirement directly.
  `BeforeProposalCreate` policy hooks can deny manual CLI/HTTP proposal
  creation before persistence, and `BeforeProposalApply` policy hooks can deny
  proposal apply before the proposal enters `applying`. HTTP proposal policy
  denials return `403` with the stable error code `policy_denied`. Policy
  revocation remains future work.
- `ScheduleSpec` supports `manual`, `interval`, and five-field `cron`
  expressions for UTC, IANA timezone names, or fixed-offset timezones.
- `RunRequest` supports `manual`, `scheduled`, `replay`, `webhook`, and
  `queue` trigger kinds. Webhook and queue deliveries can carry a
  `trigger_envelope` with `source`, optional event/message `id`, `received_at`,
  payload, and metadata through HTTP and stdio `agent.run`. Concrete plugin
  schedules, queue consumers, and HTTP/webhook trigger registries remain future
  work.
- `RunRequest`, `AgentRunResult`, `AgentRunRecord`, and `AgentTrace` carry an
  optional `workflow` object for parent/root run links, dependency edges,
  fan-out/fan-in ids, compensation metadata, and workflow metadata. The
  built-in `agent.run` subagent tool populates parent/root links for child
  runs. `WorkflowRunRequest` / `WorkflowRunResult` define a schema-backed DAG
  execution contract, and `AgentRunner::run_workflow` schedules ready nodes
  concurrently in-process while preserving deterministic topological result
  ordering, fills child run workflow metadata, records dependency edges, and
  skips nodes whose dependencies failed. Nodes for the same agent are not
  launched concurrently by the local scheduler, so workflow fan-out does not
  fight the agent/scope run lease. Workflow nodes can declare a compensation
  agent; when a later node fails, completed nodes with compensation
  declarations are compensated in reverse topological result order and the
  compensation runs carry `workflow.compensation` metadata pointing at the
  compensated run. The DAG executor acquires and renews a scope-aware workflow
  lease through the configured `AgentLockStore`, so another worker using the
  same distributed lock backend will skip a duplicate `workflow_id` / scope
  while the first execution is active. Nodes can also declare `input_mappings`
  that copy values from direct dependency outputs into the node input using
  JSON Pointer paths, with optional defaults for missing source paths and
  primitive transforms (`string`, `number`, `integer`, `boolean`, or
  `json_string`). `agent workflow run
  <workflow.json>`, HTTP `POST /workflows/run`, and stdio `workflow.run` expose
  the same local DAG executor. Workflow node and compensation results include
  optional traces, and the CLI/HTTP server write those traces into the normal
  store trace directory for `/runs/{run_id}/trace`, replay, and debug bundle
  workflows. Distributed work stealing/execution, expression/template
  transforms, and durable saga-style compensation recovery remain future work.
- `AgentTrace.artifact_refs` is a typed artifact reference protocol with
  `kind`, URI, media type, size, SHA-256, redaction classification, optional
  host store locator, and metadata. `AgentServices::publish_artifact` is the
  host-owned hook for registering external blob/document references without
  embedding raw file contents in runtime JSON. Debug bundles export a redacted
  `artifacts.json` asset and list it in replay config when a trace has artifact
  references. `agent debug-bundle export --materialize-artifacts` can copy
  local artifact bytes referenced by `file://` URIs or `metadata.local_path`,
  or by a host store locator whose provider is mapped through
  `--artifact-resolver <manifest.json>`, into an `artifacts/` bundle directory
  and records the result in `artifact_materializations.json`. Artifact store
  resolvers are explicit local root mappings for `provider/bucket/key`; the
  runtime does not fetch remote bytes or own blob-store credentials implicitly.
- `agent compat check` provides a host integration smoke harness. It validates
  catalog and tool-source schemas, executes catalog dry-run fixtures, verifies
  proposal fixture creation, and can export a redacted debug bundle to exercise
  the trace redaction path.
- HTTP run cancellation records durable intent on running records under
  `metadata.control.cancel_requested`; active runners poll the store and convert
  that intent into their local cancellation token. Run finalization and
  cancellation use optimistic concurrency so concurrent control updates are not
  silently overwritten. `GET /runs/{run_id}/events?follow=true` tails the
  durable event store when the run belongs to another runtime instance, and
  closes after the shared run record reaches a terminal status. Full
  pause/resume remains future work.

Use this document and the current test suite as the implementation authority.
