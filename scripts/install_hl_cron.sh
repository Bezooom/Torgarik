#!/usr/bin/env bash
# Install hourly HL history collection cron (user crontab).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOG_DIR="$ROOT/logs"
mkdir -p "$LOG_DIR" "$ROOT/data/history" "$ROOT/data/snapshots"

# Majors-only by default (B filters); no meme auto-top
CRON_LINE="5 * * * * cd \"$ROOT\" && /usr/bin/python3 scripts/collect_hl_history.py --cron --majors --days 14 >> \"$LOG_DIR/hl_collect.log\" 2>&1"

# Remove old lines for this script, append new
TMP=$(mktemp)
crontab -l 2>/dev/null | grep -v 'collect_hl_history.py' >"$TMP" || true
echo "$CRON_LINE" >>"$TMP"
crontab "$TMP"
rm -f "$TMP"

echo "Installed cron:"
echo "  $CRON_LINE"
echo "Logs: $LOG_DIR/hl_collect.log"
echo "Data: $ROOT/data/history  $ROOT/data/snapshots/universe_hourly.jsonl"
echo
echo "One-shot now:"
echo "  python3 $ROOT/scripts/collect_hl_history.py --days 14 --auto-top 10"
