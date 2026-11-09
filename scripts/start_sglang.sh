#!/usr/bin/env bash
set -e

MODEL_PATH="${SGLANG_MODEL_PATH:?set SGLANG_MODEL_PATH to a local model directory}"
HOST="127.0.0.1"
PORT="30000"

echo "🚀 Launching SGLang Server with model: $MODEL_PATH..."
echo "📍 API Endpoint: http://$HOST:$PORT/v1"

/usr/bin/python3 -m sglang.launch_server \
    --model-path "$MODEL_PATH" \
    --host "$HOST" \
    --port "$PORT" \
    --mem-fraction-static 0.8
