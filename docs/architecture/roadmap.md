# Agent Runtime Roadmap

This roadmap turns the current architecture gaps into phased engineering work.
It is scoped to the standalone runtime repository and the reusable integration
surfaces around it. Business-domain adapters, product policies, repositories,
and application UI remain owned by host applications.

## Current Baseline

The runtime currently works well as a schema-first agent execution kernel for:

- explicit `RunRequest` workflows
- low-risk background jobs
- JSON tool dispatch through host adapters
- proposal envelope creation and approval/application flows
- provider-neutral chat turn state and event contracts
- HTTP/SSE chat turn streaming over `agent-chat`
- HTTP/SSE chat resume, active-run event streaming, and active-run cancellation
- CLI, HTTP, stdio, TUI, replay, eval, and debug-bundle development workflows
- initial dependency-light TypeScript bindings for HTTP runtime calls, chat
  streaming, and structured LLM object generation
- reusable `agent-tools` adapters for JSONL process, MCP stdio, and HTTP JSON
  tool sources
- tool input/output JSON Schema validation for tools with declared `ToolSpec`
  schemas
- tool-source timeout and retry policy for retryable external tool failures
- tool-source serialized output-size limits for external tool calls
- file-backed local stores and in-memory test stores
- shared conformance tests for current file-backed and in-memory
  `AgentRunStore`, `AgentProposalStore`, and `AgentSessionStore`
- manual, interval, and five-field cron schedules for UTC, IANA timezones, or
  fixed-offset timezones
- first-class `RunRequest` trigger envelopes for webhook and queue deliveries,
  exposed through HTTP and stdio `agent.run`
- a `compat check` CLI harness for host integration smoke tests covering
  catalog/tool-source schema validation, dry-run fixtures, proposal fixtures,
  and debug-bundle redaction export

It is not yet a full production agent platform. The main gaps are generated or
broader SDK coverage, durable chat/run control across process restarts,
production stores, distributed scheduling, stronger risk policy, artifact
handling, richer observability, and fully stabilized reusable ToolHost APIs.

## Roadmap Principles

- Keep runtime crates independent of host app frameworks and business models.
- Keep cross-language contracts JSON-first and schema-backed.
- Prefer adding stable protocol surfaces before adding generated SDK wrappers.
- Preserve CLI/eval/replay coverage for every new runtime behavior.
- Treat user-visible side effects as proposal-first unless a host app explicitly
  marks a tool as safe for direct execution.
- Add production capabilities behind traits and transport contracts so host apps
  can choose their own database, auth, queue, and deployment model.

## P0: Stabilize Business Integration

Goal: make low-risk business agents easy to build and validate without changing
runtime internals.

### Business Scenarios

- Customer/account summaries
- Classification and triage
- Draft generation
- Read-only operational assistants
- Proposal generation for user-confirmed side effects

### Deliverables

- Keep `docs/integration/business-agent-integration.md` current with examples.
- Maintain `examples/business-integration/` as the canonical business adapter
  smoke test.
- Add contract tests for the business integration example:
  - catalog schema validation (covered by `agent compat check`)
  - tool-source manifest validation (covered by `agent compat check`)
  - dry-run tool call (covered by `agent compat check`)
  - proposal envelope creation (covered by `agent compat check`)
- Document the difference between catalog dry-run agents, configuration-backed
  agents, and code-backed agents.
- Add a compatibility checklist for host applications:
  - catalog generation
  - ToolHost behavior
  - proposal confirmation
  - trace redaction
  - provider profile validation

### Acceptance Criteria

- A new business team can copy the example and run a read-only agent locally in
  under one hour.
- Example commands in docs are covered by tests or a CI smoke script.
- Contract changes that break the example fail locally.

## P1: Production Server Surface

Goal: make server-first and web-first integrations viable without custom
transport glue for common workflows.

### Business Scenarios

- Web app agent chat
- Backend-owned agent execution
- Multi-client runtime process
- Live progress UI for long-running workflows
- User-initiated cancellation

### Deliverables

- Extend HTTP/SSE chat endpoints over `agent-chat`:
  - resume from `ChatTurnState` plus `ChatToolResult` (implemented for the
    current HTTP/server path)
  - persist or correlate chat turn traces where host apps need replay
  - expose client disconnect/cancel semantics consistently
- Add run control protocol:
  - cancel active run (implemented for in-process active runs)
  - persist cancellation intent for running records under
    `metadata.control.cancel_requested`
  - active runners poll persisted cancellation intent and convert it into their
    local cancellation token
  - mark cancellation in run record and trace (implemented for active runs that
    reach the shared `AgentRunner` cancellation path)
  - expose terminal cancellation status consistently across restarts and
    multi-instance deployments
- Extend HTTP and stdio contracts for:
  - live run event streaming
  - structured runtime errors with stable codes
  - request metadata for user/session/thread correlation
- Add server-side request validation against committed JSON Schemas.
- Extend the TypeScript client reference implementation or add generated
  OpenAPI client configuration. The hand-written reference client covers the
  current HTTP chat/run/proposal surfaces; generated broader coverage remains.

### Acceptance Criteria

- A web client can stream chat without using the TUI or custom Rust code.
- A client can cancel an active chat/run and observe a consistent terminal event.
- HTTP behavior is documented in `openapi/agent-runtime-api.yaml`.
- Contract tests cover chat stream event fixtures and cancellation records.

## P2: Production Persistence And Scheduling

Goal: support production deployment patterns without forcing file-backed local
stores or external-only scheduling.

### Business Scenarios

- Multi-instance backend deployment
- Tenant-scoped background jobs
- Scheduled daily/weekly business tasks
- Queue or webhook-triggered agents
- Reliable recovery after process restarts

### Deliverables

- Add production store guidance and optional reference implementations:
  - DB-backed `AgentRunStore`
  - DB-backed `AgentProposalStore`
  - DB-backed `AgentSessionStore`
  - distributed `AgentLockStore`
- Add migration/versioning strategy for persisted runtime records. The SQLite
  reference backend owns its current schema through a versioned migration list
  and rejects unsupported future schema versions; explicit multi-version data
  migrations and ownership policy remain future work.
- Extend scheduling beyond manual/interval:
  - cron schedule spec (implemented for five-field expressions)
  - timezone handling (implemented for UTC, IANA timezone names, and fixed
    offsets)
  - webhook trigger envelope (implemented as `trigger=webhook` plus
    `trigger_envelope` on `RunRequest` and HTTP/stdio `agent.run`)
  - queue/worker adapter contract (implemented at the protocol envelope level
    through `trigger=queue`; agent/workflow leases and renewal use the
    configured `AgentLockStore`; concrete queue backends remain future work)
- Add shared store conformance tests before implementing concrete DB backends
  (implemented for current file-backed and in-memory run/proposal/session
  stores; future DB stores should run the same behavior suite).
- Add stale-run recovery tests for DB-backed semantics. The SQLite reference
  backend is covered through CLI recovery; multi-instance recovery semantics
  remain future work.
- Add durable lock-store coordination tests for DB-backed semantics. The SQLite
  reference backend is covered for distinct runner handles sharing the same DB
  file; multi-process deployment validation remains future work.
- Add tenant/user scope guidance for locks, stores, metrics, and traces
  (implemented as first-class `RunRequest.scope` /
  `WorkflowRunRequest.scope`, persisted `AgentRunRecord.scope`, scope-aware
  lease keys and idempotency material, and inherited `agent.run` subagent
  scope).

### Acceptance Criteria

- Runtime can run safely in more than one worker process against a shared store.
- Scheduled jobs can be expressed without host-specific ad hoc metadata.
- Stale running records can be recovered deterministically after restart.
- Store implementations pass a shared conformance test suite. Current
  file-backed and in-memory run/proposal/session stores do; future DB stores
  should be added to the same suite.

## P3: Safety, Policy, And Approval Depth

Goal: support higher-risk domains such as finance, healthcare, enterprise admin,
and irreversible data mutation.

### Business Scenarios

- Trades and portfolio changes
- Payment/refund workflows
- Account edits
- Irreversible deletes
- Health data writes
- Notification scheduling
- Multi-step approvals

### Deliverables

- Promote risk policy to first-class protocol fields:
  - proposal risk (implemented)
  - required approval level (implemented)
  - policy id and policy version (implemented)
  - expiry (implemented through proposal kind relative expiry and envelope
    `expires_at`) and revocation behavior
- Add proposal diff/warning metadata conventions (implemented as
  `ProposalEnvelope.diffs` and `ProposalEnvelope.warnings`, accepted by CLI and
  HTTP proposal creation and passed through to proposal action tools)
- Add approval-chain support:
  - single-user approval (implemented as the default approval decision level)
  - multi-approver approval (implemented as accumulated distinct
    `ApprovalDecision` records on `ProposalEnvelope.approval_decisions`, with
    `required_approver_count` quorum semantics)
  - policy-denied terminal state
- Add host-side policy hook contracts:
  - pre-tool-call policy check
  - pre-proposal-create policy check (implemented as `BeforeProposalCreate`
    for agent-created proposals and manual CLI/HTTP proposal create)
  - pre-apply policy check (implemented as `BeforeProposalApply` for proposal
    apply, before status changes to `applying`)
- Add audit-oriented trace events for policy decisions and proposal actions.
  Proposal decision traces now include approval level and optional deciding
  actor, and policy hook invocations are trace-visible for proposal create and
  apply when the associated run trace exists; broader policy-decision audit
  trails remain.

### Acceptance Criteria

- High-risk tools can be blocked before execution by policy.
- Proposal records carry enough structured risk data, diffs, and warnings for a
  host UI to render review surfaces without parsing free-form text.
- Approval decisions are auditable and replay-visible for single-decision and
  multi-approver flows, including approval level, optional deciding actor, and
  the accumulated decision chain on the proposal envelope.
- Apply/undo behavior has stable terminal statuses and error semantics.

## P4: Reusable SDKs And Tooling

Goal: reduce duplicated adapter work across host applications.

### Business Scenarios

- Flutter mobile integration
- TypeScript web/backend integration
- Shared ToolHost implementations
- CI compatibility checks for host apps

### Deliverables

- Add `bindings/dart` or a generated Dart contract package.
- Extend `bindings/ts` or add a generated TypeScript contract package for
  broader protocol coverage.
- Provide client helpers for:
  - catalog validation
  - chat event mapping
  - proposal decision/apply
  - trace redaction hooks
- Continue stabilizing the extracted `agent-tools` crate into a reusable
  ToolHost API with richer per-tool policy, a host secret manager, and stronger
  sandbox guidance. Source-level timeouts/retries and HTTP header environment
  placeholders are implemented for the current manifest adapters, along with
  source-level serialized output-size limits and process/MCP stdio
  `cwd`/`env`/`inherit_env` controls.
- Provide a host compatibility test harness:
  - validate catalog (implemented for provided catalog path)
  - call tool-backed run fixture (implemented through catalog dry-run input)
  - validate proposal fixtures (implemented by checking proposal creation)
  - verify trace redaction (implemented by exporting a redacted debug bundle)

### Acceptance Criteria

- Flutter and TypeScript integrations can consume generated contract types
  rather than hand-written JSON maps for stable protocol surfaces.
- Tool adapter behavior is reusable outside `agent-cli`.
- Host apps can run compatibility tests in CI without shelling out to bespoke
  scripts.

## P5: Advanced Runtime Capabilities

Goal: handle complex agent products that need more than single-run execution.

### Business Scenarios

- Multi-agent workflow orchestration
- DAG-style tool and agent execution
- Long-running research tasks
- Artifact-heavy document workflows
- Cross-service observability and cost analytics

### Deliverables

- Add workflow/graph execution contracts:
  - child runs (implemented as `workflow.parent_run_id` / `root_run_id`)
  - dependency edges (implemented as `workflow.dependencies`)
  - fan-out/fan-in (implemented as workflow ids)
  - compensation and rollback metadata (implemented as `workflow.compensation`;
    workflow nodes can declare compensation agents and the local DAG executor
    runs them in reverse topological result order after downstream failure)
  - DAG scheduling/execution semantics (implemented as deterministic in-process
    ready-node scheduling through `AgentRunner::run_workflow`, exposed via
    `agent workflow run`, HTTP `POST /workflows/run`, and stdio `workflow.run`;
    nodes whose dependencies are complete can run concurrently, with
    same-agent nodes serialized against the local run lease)
  - workflow execution leases (implemented as scope-aware
    `workflow_id` / scope keys through the configured `AgentLockStore`, with
    periodic renewal while work is active; distributed work stealing/execution
    and durable saga recovery remain future work)
  - dataflow mapping between nodes (implemented as `input_mappings` from direct
    dependency `output` JSON Pointer paths into the target node input, with
    optional defaults and primitive `string` / `number` / `integer` /
    `boolean` / `json_string` transforms; expression/template transforms remain
    future work)
- Add artifact reference protocol:
  - blob/document references (implemented as typed `artifact_refs`)
  - content hashes (implemented as `sha256`)
  - redaction classification (implemented on `ArtifactRef`)
  - host-owned artifact store hooks (implemented as `publish_artifact`)
  - replay materialization from artifact stores (implemented for local files
    and explicit host store resolver manifests via debug bundle
    `--materialize-artifacts`; live remote fetchers remain future work)
- Add richer observability:
  - provider spans alongside the implemented run/tool/state spans
    (implemented for LLM trace events)
  - OpenTelemetry export guidance (implemented as `agent trace export-otel`
    OTLP JSON-style document export plus OTLP HTTP collector push)
  - cost and token usage aggregation (implemented as
    `AgentTrace.usage_summary`)
  - latency metrics by provider/tool/agent (implemented in
    `/metrics/summary` as `llm_usage_by_provider`, `tool_calls_by_tool`, and
    `runs_by_agent`)
- Add replay support for multi-run workflows and artifact references.
  - artifact reference manifests in debug bundles (implemented as
    `artifacts.json`)
  - artifact byte materialization from host stores (implemented for local
    `file://` / `metadata.local_path` artifacts and explicit
    `provider/bucket/key` resolver manifests with
    `artifact_materializations.json`; live remote fetcher traits remain)

### Acceptance Criteria

- A workflow can express parent/child run relationships without embedding
  business-specific metadata. The protocol fields and a local concurrent DAG
  executor exist, with CLI/HTTP/stdio entrypoints, scope-aware workflow leases,
  and persisted node traces for normal run inspection; distributed workflow
  work stealing/execution and durable compensation recovery remain future work.
- Large files can be referenced without putting raw blobs in JSON traces.
- Production traces can be exported to common observability systems.
- Replay remains deterministic for workflows that use recorded tool outputs.

## Explicit Non-Goals

- Runtime should not own business repositories, product data models, or UI.
- Runtime should not become the production auth gateway for host applications.
- Runtime should not persist provider secrets in traces, catalogs, or tool
  outputs. Tool-source manifests should use environment placeholders such as
  `${env:NAME}` for HTTP header secrets.
- Runtime should not execute high-risk side effects directly unless the host app
  explicitly exposes a direct tool and accepts that risk.
- Runtime should not require one database, queue, frontend framework, or mobile
  bridge technology.

## Suggested Near-Term Order

Implemented baseline: business integration smoke coverage, active-run
cancellation plus persisted cancellation intent, HTTP/SSE ChatTurn resume, and
proposal risk/approval metadata are now part of the current runtime surface. A
reusable `agent-store` conformance testkit now covers `AgentRunStore`,
`AgentProposalStore`, and `AgentSessionStore` implementations, and an optional
SQLite reference backend is available behind the `agent-store/sqlite` feature.
CLI/server paths can opt into that backend with `runtime.store_backend =
"sqlite"` while keeping trace/debug artifacts under `runtime.store`. The
SQLite backend owns its schema through an internal migration list and rejects
unsupported future schema versions, but does not yet include a full
cross-version data migration policy. Developer workflows
that still depend on file-backed runtime stores fail closed under the SQLite
backend instead of silently mixing persistence modes.

1. Define migration/versioning ownership for persisted runtime records,
   including multi-instance lock/state-store and recovery semantics.
2. Extend durable run event replay and subscription contracts across process
   restarts and multi-instance deployments.
3. Extend generated TypeScript or Dart SDK coverage from committed schemas and
   OpenAPI contracts once the next persistence/event-log surfaces settle.
