# Agent Runtime

Schema-first Rust agent runtime extracted from NaviWealth for standalone
maintenance, debugging, replay, and CLI/server development.

## What Is In This Repo

- `crates/agent-core`: DTOs, traits, IDs, errors, trace/session/proposal contracts.
- `crates/agent-runtime`: runner, scheduler, retry/timeout, lease locking, trace capture.
- `crates/agent-store`: in-memory and file-backed run/proposal/session stores.
- `crates/agent-llm`: provider-neutral LLM DTOs and mock/OpenAI/Anthropic/Ollama providers.
- `crates/agent-chat`: shared ChatTurn request/event contract and tool-round loop.
- `crates/agent-cli`: local CLI, HTTP/stdio server, replay, eval, debug bundle, TUI.
- `schemas/agent-runtime`: JSON Schema wire contracts.
- `fixtures/agent-runtime`: valid and invalid contract fixtures.
- `examples/agent-runtime`: local example registry and input fixtures.
- `evals/agent-runtime`: deterministic eval examples and golden traces.
- `openapi/agent-runtime-api.yaml`: minimal HTTP API contract.

Flutter, Riverpod, Drift, and NaviWealth business adapters are intentionally not
part of this standalone repo. Host applications consume the JSON contracts,
CLI/server surfaces, or Rust crates through their own adapters.

## Common Commands

```bash
cargo test --workspace
cargo run -p agent-cli -- list
cargo run -p agent-cli -- run echo_agent --input examples/agent-runtime/fixtures/echo-input.json
cargo run -p agent-cli -- validate schemas/agent-runtime/run-request.schema.json fixtures/agent-runtime/run-request.valid.json
cargo run -p agent-cli -- eval evals/agent-runtime --store /private/tmp/agent-runtime-eval-store
cargo run -p agent-cli -- serve --catalog fixtures/agent-runtime/catalog.valid.json --store /private/tmp/agent-runtime-http-store
```

The historical architecture notes copied from NaviWealth are under
`docs/architecture/`. Treat host-specific Flutter/FRB references there as
integration notes, not standalone crate dependencies.
