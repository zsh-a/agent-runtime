#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT_DIR"

CONFIG="${AGENT_RUNTIME_CONFIG:-examples/tui/agent-runtime.toml}"
EXTRA_ARGS=()

if [[ -n "${AGENT_RUNTIME_STORE:-}" ]]; then
  EXTRA_ARGS+=(--store "$AGENT_RUNTIME_STORE")
fi

if [[ -n "${AGENT_RUNTIME_CATALOG:-}" ]]; then
  EXTRA_ARGS+=(--catalog "$AGENT_RUNTIME_CATALOG")
fi

if [[ -n "${MODEL:-}" ]]; then
  EXTRA_ARGS+=(--model "$MODEL")
fi

if [[ -n "${MODELSCOPE_BASE_URL:-}" ]]; then
  BASE_URL="${MODELSCOPE_BASE_URL%/}"
  if [[ "$BASE_URL" == "https://api-inference.modelscope.cn" ]]; then
    BASE_URL="$BASE_URL/v1"
  fi
  EXTRA_ARGS+=(--api-base-url "$BASE_URL")
fi

if [[ -n "${MAX_OUTPUT_TOKENS:-}" ]]; then
  EXTRA_ARGS+=(--max-output-tokens "$MAX_OUTPUT_TOKENS")
fi

if [[ -n "${MAX_TOOL_ROUNDS:-}" ]]; then
  EXTRA_ARGS+=(--max-tool-rounds "$MAX_TOOL_ROUNDS")
fi

if [[ -z "${MODELSCOPE_API_KEY:-}" && -n "${MODELSCOPE_TOKEN:-}" ]]; then
  export MODELSCOPE_API_KEY="$MODELSCOPE_TOKEN"
fi

if [[ -z "${MODELSCOPE_API_KEY:-}" ]]; then
  echo "MODELSCOPE_API_KEY or MODELSCOPE_TOKEN is required." >&2
  echo "Example: MODELSCOPE_API_KEY=ms examples/tui/modelscope/run.sh" >&2
  exit 1
fi

if ((${#EXTRA_ARGS[@]})); then
  exec cargo run -p agent-cli -- --config "$CONFIG" tui \
    "${EXTRA_ARGS[@]}" \
    "$@"
fi

exec cargo run -p agent-cli -- --config "$CONFIG" tui "$@"
