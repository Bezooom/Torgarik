#!/usr/bin/env bash
# Venue reachability smoke-test (read-only).
# Usage: bash scripts/smoke_venue.sh
# Optional: HTTPS_PROXY / ALL_PROXY for proxied check.

set -euo pipefail

HL_URL="${HL_URL:-https://api.hyperliquid.xyz/info}"

echo "=== Venue smoke (read-only) ==="
echo "Time: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
if [[ -n "${HTTPS_PROXY:-}${ALL_PROXY:-}${HTTP_PROXY:-}" ]]; then
  echo "Proxy: HTTPS_PROXY=${HTTPS_PROXY:-} ALL_PROXY=${ALL_PROXY:-} HTTP_PROXY=${HTTP_PROXY:-}"
else
  echo "Proxy: (none — direct)"
fi
echo

# --- Hyperliquid ---
echo "--- Hyperliquid POST ${HL_URL} type=allMids ---"
hl_code=$(curl -sS -m 20 -o /tmp/hl_allmids.json -w "%{http_code}" \
  -X POST "$HL_URL" \
  -H 'Content-Type: application/json' \
  -d '{"type":"allMids"}' || echo "000")
echo "HTTP $hl_code"
if [[ "$hl_code" == "200" ]]; then
  btc=$(python3 -c "import json;d=json.load(open('/tmp/hl_allmids.json'));print(d.get('BTC','?'))" 2>/dev/null || echo "?")
  eth=$(python3 -c "import json;d=json.load(open('/tmp/hl_allmids.json'));print(d.get('ETH','?'))" 2>/dev/null || echo "?")
  echo "  BTC mid≈ $btc  ETH mid≈ $eth"
  echo "  READ: OK"
else
  echo "  READ: FAIL"
fi

echo
echo "--- Hyperliquid POST type=metaAndAssetCtxs (funding/mark) ---"
hl_meta_code=$(curl -sS -m 20 -o /tmp/hl_meta.json -w "%{http_code}" \
  -X POST "$HL_URL" \
  -H 'Content-Type: application/json' \
  -d '{"type":"metaAndAssetCtxs"}' || echo "000")
echo "HTTP $hl_meta_code"
if [[ "$hl_meta_code" == "200" ]]; then
  echo "  meta+ctxs: OK (inspect /tmp/hl_meta.json for funding)"
else
  echo "  meta+ctxs: FAIL"
fi

echo
echo "=== Summary ==="
echo "Place/cancel NOT tested by this script — do manual UI smoke (see docs/VENUE_SMOKE.md)."
echo "Fill the matrix in docs/VENUE_SMOKE.md with your results."
