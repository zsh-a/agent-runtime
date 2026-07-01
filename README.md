# Agent Runtime

Schema-first Rust agent runtime for standalone maintenance, debugging, replay,
and CLI/server development.

## What Is In This Repo

- `crates/agent-core`: DTOs, traits, IDs, errors, trace/session/proposal contracts.
- `crates/agent-runtime`: runner, scheduler, retry/timeout, lease locking, trace capture.
- `crates/agent-store`: in-memory and file-backed run/proposal/session stores.
- `crates/agent-llm`: provider-neutral LLM DTOs and mock/OpenAI/Anthropic/Ollama providers.
- `crates/agent-chat`: shared ChatTurn request/event contract and tool-round loop.
- `crates/agent-cli`: local CLI, HTTP/stdio server, replay, eval, debug bundle, TUI.
- `schemas`: JSON Schema wire contracts.
- `fixtures/contracts`: valid and invalid contract fixtures.
- `fixtures/docs`: documentation and UI event examples.
- `examples`: local example registry and input fixtures.
- `evals`: deterministic eval examples and golden traces.
- `openapi/agent-runtime-api.yaml`: minimal HTTP API contract.

Host application adapters and business-domain code are intentionally not part of
this standalone repo. Host applications consume the JSON contracts, CLI/server
surfaces, or Rust crates through their own adapters.

Integration guidance for host applications lives under `docs/integration/`,
including the business agent integration guide and the NaviWealth-style Flutter
FRB native bridge guide.

## Common Commands

```bash
cargo test --workspace
cargo run -p agent-cli -- list
cargo run -p agent-cli -- run echo_agent --input examples/fixtures/echo-input.json
cargo run -p agent-cli -- validate schemas/run-request.schema.json fixtures/contracts/run-request.valid.json
cargo run -p agent-cli -- eval evals --store /private/tmp/agent-runtime-eval-store
cargo run -p agent-cli -- serve --catalog fixtures/contracts/catalog.valid.json --store /private/tmp/agent-runtime-http-store
```

Current architecture notes live under `docs/architecture/`. Historical
integration notes live under `docs/legacy/`.
