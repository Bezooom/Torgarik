#!/usr/bin/env bash
# tg_notify.sh — отправка уведомлений в Telegram.
#
# Usage:
#   echo "текст" | bash scripts/tg_notify.sh
#   bash scripts/tg_notify.sh "короткий текст"
#   bash scripts/tg_notify.sh --silent "тихое уведомление (без звука)"
#
# Настройка (один раз):
#   1) Создай бота у @BotFather → получишь TELEGRAM_BOT_TOKEN
#   2) Напиши боту любое сообщение, затем:
#      curl "https://api.telegram.org/bot$TOKEN/getUpdates"  → возьми chat.id
#   3) Создай файл .env.tg в корне проекта:
#        TELEGRAM_BOT_TOKEN=123456:ABC...
#        TELEGRAM_CHAT_ID=123456789
#
# Без .env.tg / env-переменных скрипт молча пропускает отправку (exit 0),
# чтобы cron-задачи не падали на машинах без Telegram.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

SILENT=""
MSG_ARGS=()
for arg in "$@"; do
    case "$arg" in
        -s|--silent) SILENT="true" ;;
        -h|--help)
            sed -n '2,20p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) MSG_ARGS+=("$arg") ;;
    esac
done

# Файл с токеном: KEY=VALUE построчно; не перетирает уже выставленный env.
if [ -f "$ROOT/.env.tg" ]; then
    # shellcheck disable=SC1091
    set -a; . "$ROOT/.env.tg"; set +a
fi

if [ -z "${TELEGRAM_BOT_TOKEN:-}" ] || [ -z "${TELEGRAM_CHAT_ID:-}" ]; then
    echo "[tg] Telegram не настроен (.env.tg или env TELEGRAM_BOT_TOKEN/TELEGRAM_CHAT_ID) — пропуск." >&2
    exit 0
fi

if [ ${#MSG_ARGS[@]} -gt 0 ]; then
    MESSAGE="${MSG_ARGS[*]}"
else
    MESSAGE="$(cat)"
fi

if [ -z "$MESSAGE" ]; then
    echo "[tg] Пустое сообщение — пропуск." >&2
    exit 0
fi

RESPONSE=$(curl -sS --max-time 15 \
    -X POST "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
    --data-urlencode "chat_id=${TELEGRAM_CHAT_ID}" \
    --data-urlencode "text=${MESSAGE}" \
    ${SILENT:+--data-urlencode disable_notification=true} 2>&1) || {
        echo "[tg] Ошибка сети: $RESPONSE" >&2
        exit 1
    }

if ! grep -q '"ok":true' <<<"$RESPONSE"; then
    echo "[tg] Telegram ответил ошибкой: $RESPONSE" >&2
    exit 1
fi
