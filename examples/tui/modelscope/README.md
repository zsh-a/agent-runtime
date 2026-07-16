# ModelScope TUI example

Run the TUI against ModelScope's OpenAI-compatible endpoint:

```bash
MODELSCOPE_API_KEY=... examples/tui/modelscope/run.sh
```

Optional environment variables include `MODEL`, `MODELSCOPE_BASE_URL`,
`AGENT_RUNTIME_CONFIG`, `AGENT_RUNTIME_STORE`, `AGENT_RUNTIME_CATALOG`,
`MAX_OUTPUT_TOKENS`, and `MAX_TOOL_ROUNDS`. Additional arguments are forwarded
to `agent tui`.
