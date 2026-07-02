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
- CLI, HTTP, stdio, TUI, replay, eval, and debug-bundle development workflows
- initial dependency-light TypeScript bindings for HTTP runtime calls, chat
  streaming, and structured LLM object generation
- file-backed local stores and in-memory test stores

It is not yet a full production agent platform. The main gaps are generated or
broader SDK coverage, chat resume and cancellation, external run control,
production stores, distributed scheduling, stronger risk policy, artifact
handling, and richer observability.

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
  - catalog schema validation
  - tool-source manifest validation
  - dry-run tool call
  - proposal envelope creation
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
  - resume from `ChatTurnState` plus `ChatToolResult`
  - persist or correlate chat turn traces where host apps need replay
  - expose client disconnect/cancel semantics consistently
- Add run control protocol:
  - cancel active run
  - mark cancellation in run record and trace
  - expose terminal cancellation status consistently
- Extend HTTP and stdio contracts for:
  - live run event streaming
  - structured runtime errors with stable codes
  - request metadata for user/session/thread correlation
- Add server-side request validation against committed JSON Schemas.
- Extend the TypeScript client reference implementation or add generated OpenAPI
  client configuration.

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
- Add migration/versioning strategy for persisted runtime records.
- Extend scheduling beyond manual/interval:
  - cron schedule spec
  - timezone handling
  - webhook trigger envelope
  - queue/worker adapter contract
- Add stale-run recovery tests for DB-backed semantics.
- Add tenant/user scope guidance for locks, stores, metrics, and traces.

### Acceptance Criteria

- Runtime can run safely in more than one worker process against a shared store.
- Scheduled jobs can be expressed without host-specific ad hoc metadata.
- Stale running records can be recovered deterministically after restart.
- Store implementations pass a shared conformance test suite.

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
  - proposal risk
  - required approval level
  - policy id and policy version
  - expiry and revocation behavior
- Add proposal diff/warning metadata conventions.
- Add approval-chain support:
  - single-user approval
  - multi-approver approval
  - policy-denied terminal state
- Add host-side policy hook contracts:
  - pre-tool-call policy check
  - pre-proposal-create policy check
  - pre-apply policy check
- Add audit-oriented trace events for policy decisions and proposal actions.

### Acceptance Criteria

- High-risk tools can be blocked before execution by policy.
- Proposal records carry enough structured risk data for a host UI to render
  warnings without parsing free-form text.
- Approval decisions are auditable and replay-visible.
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
- Extract reusable tool adapters from `agent-cli/src/tools/` into an
  `agent-tools` crate when the API is stable enough.
- Provide a host compatibility test harness:
  - validate catalog
  - call each tool fixture
  - validate proposal fixtures
  - verify trace redaction

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
  - child runs
  - dependency edges
  - fan-out/fan-in
  - compensation and rollback metadata
- Add artifact reference protocol:
  - blob/document references
  - content hashes
  - redaction classification
  - host-owned artifact store hooks
- Add richer observability:
  - optional spans alongside event-first traces
  - OpenTelemetry export guidance
  - cost and token usage aggregation
  - latency metrics by provider/tool/agent
- Add replay support for multi-run workflows and artifact references.

### Acceptance Criteria

- A workflow can express parent/child run relationships without embedding
  business-specific metadata.
- Large files can be referenced without putting raw blobs in JSON traces.
- Production traces can be exported to common observability systems.
- Replay remains deterministic for workflows that use recorded tool outputs.

## Explicit Non-Goals

- Runtime should not own business repositories, product data models, or UI.
- Runtime should not become the production auth gateway for host applications.
- Runtime should not persist provider secrets in traces, catalogs, or tool
  outputs.
- Runtime should not execute high-risk side effects directly unless the host app
  explicitly exposes a direct tool and accepts that risk.
- Runtime should not require one database, queue, frontend framework, or mobile
  bridge technology.

## Suggested Near-Term Order

1. Add smoke tests for `examples/business-integration/`.
2. Add cancellation protocol for runs and chat turns.
3. Add ChatTurn resume support over HTTP/SSE.
4. Add DB-backed store conformance tests before implementing a concrete backend.
5. Promote proposal risk metadata into stable schema fields.
6. Extend generated TypeScript or Dart bindings after the above protocol
   surfaces settle.
