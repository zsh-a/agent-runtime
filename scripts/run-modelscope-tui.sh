#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

MODEL="${MODEL:-stepfun-ai/Step-3.7-Flash}"
BASE_URL="${MODELSCOPE_BASE_URL:-https://api-inference.modelscope.cn/v1}"
STORE="${AGENT_RUNTIME_STORE:-.agent-runtime/modelscope-tui-store}"
CATALOG="${AGENT_RUNTIME_CATALOG:-fixtures/contracts/catalog.valid.json}"
MAX_OUTPUT_TOKENS="${MAX_OUTPUT_TOKENS:-1024}"
MAX_TOOL_ROUNDS="${MAX_TOOL_ROUNDS:-4}"

BASE_URL="${BASE_URL%/}"
if [[ "$BASE_URL" == "https://api-inference.modelscope.cn" ]]; then
  BASE_URL="$BASE_URL/v1"
fi

if [[ -z "${MODELSCOPE_API_KEY:-}" && -n "${MODELSCOPE_TOKEN:-}" ]]; then
  export MODELSCOPE_API_KEY="$MODELSCOPE_TOKEN"
fi

if [[ -z "${MODELSCOPE_API_KEY:-}" ]]; then
  echo "MODELSCOPE_API_KEY or MODELSCOPE_TOKEN is required." >&2
  echo "Example: MODELSCOPE_API_KEY=ms scripts/run-modelscope-tui.sh" >&2
  exit 1
fi

exec cargo run -p agent-cli -- tui \
  --catalog "$CATALOG" \
  --store "$STORE" \
  --provider anthropic \
  --model "$MODEL" \
  --api-base-url "$BASE_URL" \
  --api-key-env MODELSCOPE_API_KEY \
  --max-output-tokens "$MAX_OUTPUT_TOKENS" \
  --max-tool-rounds "$MAX_TOOL_ROUNDS" \
  "$@"
