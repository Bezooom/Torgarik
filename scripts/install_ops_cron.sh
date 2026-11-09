#!/usr/bin/env bash
# Install daily WF refresh cron (user crontab).
# Ежедневно в 06:15: пересборка + walk-forward + Telegram-алерт о вердикте.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$ROOT/logs"

CRON_LINE="15 6 * * * cd \"$ROOT\" && /usr/bin/env bash scripts/wf_refresh.sh >> \"$ROOT/logs/wf_refresh.log\" 2>&1"

# Remove old lines for this script, append new
TMP=$(mktemp)
crontab -l 2>/dev/null | grep -v 'scripts/wf_refresh.sh' >"$TMP" || true
echo "$CRON_LINE" >>"$TMP"
crontab "$TMP"
rm -f "$TMP"

echo "Installed cron:"
echo "  $CRON_LINE"
echo "Logs: $ROOT/logs/wf_refresh.log"
echo "Report: $ROOT/data/reports/wf_latest.json (+ архив в data/reports/history/)"
echo
echo "Telegram-алерты: создай $ROOT/.env.tg (см. scripts/tg_notify.sh --help)"
echo "One-shot now:"
echo "  bash $ROOT/scripts/wf_refresh.sh"
