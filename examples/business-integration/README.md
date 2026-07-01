# Business Integration Example

This example shows how a business domain can connect to the runtime without putting business code into runtime crates.

It models a simple customer-success workflow:

- `customer_summary_agent` is described in a catalog.
- `customer_tool_host.py` owns business data and tool behavior.
- `tool-source.json` exposes the tool host to the CLI/server.
- `run-customer-summary.json` exercises a read-only tool call through the catalog dry-run agent.
- `run-followup-proposal.json` creates a proposal envelope for a user-confirmed follow-up email.

## Validate contracts

```bash
cargo run -p agent-cli -- validate \
  schemas/catalog.schema.json \
  examples/business-integration/catalog.json

cargo run -p agent-cli -- validate \
  schemas/tool-source-manifest.schema.json \
  examples/business-integration/tool-source.json
```

## Run a tool-backed workflow

```bash
cargo run -p agent-cli -- run customer_summary_agent \
  --catalog examples/business-integration/catalog.json \
  --tool-source examples/business-integration/tool-source.json \
  --input examples/business-integration/run-customer-summary.json \
  --store /private/tmp/agent-runtime-business-example-store
```

## Create a proposal

```bash
cargo run -p agent-cli -- run customer_summary_agent \
  --catalog examples/business-integration/catalog.json \
  --tool-source examples/business-integration/tool-source.json \
  --input examples/business-integration/run-followup-proposal.json \
  --store /private/tmp/agent-runtime-business-example-store
```

## Start HTTP server

```bash
cargo run -p agent-cli -- serve \
  --catalog examples/business-integration/catalog.json \
  --tool-source examples/business-integration/tool-source.json \
  --store /private/tmp/agent-runtime-business-example-store
```

Then call `GET /catalog/summary`, `GET /tools`, or `POST /agents/customer_summary_agent/run`.

The current catalog path uses the runtime's dry-run agent. Production integrations should replace this with a host-owned adapter or a code-backed agent while preserving the same JSON contracts.
