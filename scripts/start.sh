#!/usr/bin/env bash
# start.sh — комплексный запуск Торгарика (S1 + Risk Shell + Dashboard)
#
#   bash scripts/start.sh              # старт shell + dashboard (dry-run)
#   bash scripts/start.sh start        # то же
#   bash scripts/start.sh stop
#   bash scripts/start.sh restart
#   bash scripts/start.sh status
#   bash scripts/start.sh logs [shell|dashboard|all]
#   bash scripts/start.sh ready        # только ready.sh
#
# Опции start/restart:
#   --with-mcp       поднять mcp-server (для агентов)
#   --no-build       не делать cargo build (быстрее, если уже собрано)
#   --skip-ready     не печатать ready.sh после старта
#   --skip-smoke     не проверять HL API
#   --open           открыть dashboard в браузере (xdg-open / open)
#   --port N         порт dashboard (default 8080 / DASHBOARD_PORT)
#
# Важно: этот скрипт НИКОГДА не запускает live --execute.
# Live только вручную после docs/LIVE_UNLOCK.md.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

RUNTIME_DIR="$ROOT/runtime"
LOG_DIR="$RUNTIME_DIR/logs"
PID_DIR="$RUNTIME_DIR/pids"
mkdir -p "$LOG_DIR" "$PID_DIR" "$RUNTIME_DIR/journal" "$RUNTIME_DIR/s1"

# ── colors ──────────────────────────────────────────────────────────
if [[ -t 1 ]]; then
  RED=$'\033[0;31m'; GREEN=$'\033[0;32m'; YELLOW=$'\033[1;33m'
  CYAN=$'\033[0;36m'; BOLD=$'\033[1m'; DIM=$'\033[2m'; NC=$'\033[0m'
else
  RED=; GREEN=; YELLOW=; CYAN=; BOLD=; DIM=; NC=
fi

info()  { echo "${CYAN}→${NC} $*"; }
ok()    { echo "${GREEN}✓${NC} $*"; }
warn()  { echo "${YELLOW}!${NC} $*"; }
err()   { echo "${RED}✗${NC} $*" >&2; }
header(){ echo; echo "${BOLD}$*${NC}"; echo "${DIM}$(printf '─%.0s' {1..48})${NC}"; }

# ── defaults / flags ────────────────────────────────────────────────
CMD="${1:-start}"
if [[ $# -gt 0 ]]; then shift; fi

WITH_MCP=0
DO_BUILD=1
SKIP_READY=0
SKIP_SMOKE=0
OPEN_BROWSER=0
PORT="${DASHBOARD_PORT:-8080}"
LOG_TAIL=40

while [[ $# -gt 0 ]]; do
  case "$1" in
    --with-mcp)   WITH_MCP=1; shift ;;
    --no-build)   DO_BUILD=0; shift ;;
    --skip-ready) SKIP_READY=1; shift ;;
    --skip-smoke) SKIP_SMOKE=1; shift ;;
    --open)       OPEN_BROWSER=1; shift ;;
    --port)
      PORT="${2:?--port needs value}"
      shift 2
      ;;
    --port=*)
      PORT="${1#--port=}"
      shift
      ;;
    shell|dashboard|mcp|all)
      # for logs subcommand
      LOG_TARGET="$1"
      shift
      ;;
    -h|--help)
      awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} /^set /{exit} /^$/{if(seen++)exit; print; next}' "$0"
      exit 0
      ;;
    *)
      err "Неизвестный аргумент: $1"
      echo "  bash scripts/start.sh --help"
      exit 2
      ;;
  esac
done

export DASHBOARD_PORT="$PORT"
export TRADING_ROOT="$ROOT"

SHELL_PID_FILE="$PID_DIR/shell.pid"
DASH_PID_FILE="$PID_DIR/dashboard.pid"
MCP_PID_FILE="$PID_DIR/mcp.pid"
SHELL_LOG="$LOG_DIR/shell.log"
DASH_LOG="$LOG_DIR/dashboard.log"
MCP_LOG="$LOG_DIR/mcp.log"

# ── helpers ─────────────────────────────────────────────────────────
pid_alive() {
  local pid="${1:-}"
  [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null
}

read_pid() {
  local f="$1"
  if [[ -f "$f" ]]; then
    tr -d '[:space:]' <"$f" || true
  fi
}

stop_one() {
  local name="$1" pf="$2"
  local pid
  pid="$(read_pid "$pf")"
  if pid_alive "$pid"; then
    info "Останавливаю $name (pid $pid)…"
    kill "$pid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      pid_alive "$pid" || break
      sleep 0.3
    done
    if pid_alive "$pid"; then
      warn "$name не остановился — SIGKILL"
      kill -9 "$pid" 2>/dev/null || true
    fi
    ok "$name остановлен"
  else
    info "$name уже не запущен"
  fi
  rm -f "$pf"
}

# Kill leftover by command pattern only if our pidfile is stale
is_our_process() {
  local pid="$1" pattern="$2"
  pid_alive "$pid" || return 1
  local cmd
  cmd="$(ps -p "$pid" -o args= 2>/dev/null || true)"
  [[ "$cmd" == *"$pattern"* ]]
}

wait_http() {
  local url="$1" tries="${2:-30}"
  local i
  for ((i=1; i<=tries; i++)); do
    if curl -sf -m 1 "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.4
  done
  return 1
}

preflight() {
  header "1. Проверки"
  local fail=0

  if ! command -v cargo >/dev/null 2>&1; then
    err "cargo не найден. Установите Rust: https://rustup.rs"
    fail=1
  else
    ok "cargo $(cargo --version | awk '{print $2}')"
  fi

  if ! command -v curl >/dev/null 2>&1; then
    warn "curl не найден — smoke/health checks ограничены"
  else
    ok "curl есть"
  fi

  if ! command -v python3 >/dev/null 2>&1; then
    warn "python3 не найден — S1-скрипты недоступны"
  else
    ok "python3 есть"
  fi

  if [[ ! -f "$ROOT/Cargo.toml" ]]; then
    err "Не вижу Cargo.toml в $ROOT"
    fail=1
  else
    ok "корень проекта: $ROOT"
  fi

  mkdir -p "$LOG_DIR" "$PID_DIR" "$RUNTIME_DIR/journal" "$RUNTIME_DIR/s1"
  ok "runtime/: journal, logs, pids, s1"

  if [[ -f "$ROOT/data/reports/wf_latest.json" ]]; then
    ok "OOS-отчёт: data/reports/wf_latest.json"
  else
    warn "Нет data/reports/wf_latest.json — live будет заблокирован (это нормально)"
  fi

  if [[ $fail -ne 0 ]]; then
    err "Preflight failed"
    exit 1
  fi
}

maybe_smoke() {
  if [[ $SKIP_SMOKE -eq 1 ]]; then
    info "smoke HL пропущен (--skip-smoke)"
    return 0
  fi
  if ! command -v curl >/dev/null 2>&1; then
    return 0
  fi
  header "2. Связь с Hyperliquid (read-only)"
  local code
  code=$(curl -sS -m 12 -o /tmp/torgarik_hl_mids.json -w "%{http_code}" \
    -X POST "${HL_URL:-https://api.hyperliquid.xyz/info}" \
    -H 'Content-Type: application/json' \
    -d '{"type":"allMids"}' 2>/dev/null || echo "000")
  if [[ "$code" == "200" ]]; then
    local btc eth
    btc=$(python3 -c "import json;d=json.load(open('/tmp/torgarik_hl_mids.json'));print(d.get('BTC','?'))" 2>/dev/null || echo "?")
    eth=$(python3 -c "import json;d=json.load(open('/tmp/torgarik_hl_mids.json'));print(d.get('ETH','?'))" 2>/dev/null || echo "?")
    ok "Hyperliquid OK (HTTP 200)  BTC≈$btc  ETH≈$eth"
  else
    warn "Hyperliquid HTTP $code — shell может работать со stale/ошибками сети"
    warn "Прокси: HTTPS_PROXY / ALL_PROXY. Подробнее: bash scripts/smoke_venue.sh"
  fi
}

build_bins() {
  if [[ $DO_BUILD -eq 0 ]]; then
    info "Сборка пропущена (--no-build)"
    return 0
  fi
  header "3. Сборка (shell + dashboard${WITH_MCP:+ + mcp})"
  local pkgs=(-p ml-trader -p dashboard)
  if [[ $WITH_MCP -eq 1 ]]; then
    pkgs+=(-p mcp-server)
  fi
  info "cargo build ${pkgs[*]} …"
  if cargo build "${pkgs[@]}" 2>&1 | tee "$LOG_DIR/build.log" | tail -n 5; then
    ok "сборка завершена (лог: runtime/logs/build.log)"
  else
    err "сборка упала — смотри runtime/logs/build.log"
    exit 1
  fi
}

start_shell() {
  local pid
  pid="$(read_pid "$SHELL_PID_FILE")"
  if is_our_process "$pid" "ml-trader"; then
    ok "risk shell уже запущен (pid $pid)"
    return 0
  fi
  rm -f "$SHELL_PID_FILE"

  info "Стартую risk shell (dry-run, без --execute)…"
  local bin=""
  if [[ -x "$ROOT/target/release/ml-trader" ]]; then
    bin="$ROOT/target/release/ml-trader"
  elif [[ -x "$ROOT/target/debug/ml-trader" ]]; then
    bin="$ROOT/target/debug/ml-trader"
  fi

  if [[ -n "$bin" ]]; then
    nohup env TRADING_ROOT="$ROOT" \
      "$bin" --mode shell \
      >>"$SHELL_LOG" 2>&1 &
  else
    warn "бинарник ml-trader не найден — cargo run (медленнее)"
    nohup env TRADING_ROOT="$ROOT" \
      cargo run -q -p ml-trader -- --mode shell \
      >>"$SHELL_LOG" 2>&1 &
  fi
  echo $! >"$SHELL_PID_FILE"
  sleep 0.8
  pid="$(read_pid "$SHELL_PID_FILE")"
  if pid_alive "$pid"; then
    ok "risk shell pid=$pid  лог: runtime/logs/shell.log"
  else
    err "shell не стартовал — tail runtime/logs/shell.log"
    tail -n 30 "$SHELL_LOG" 2>/dev/null || true
    exit 1
  fi
}

start_dashboard() {
  local pid
  pid="$(read_pid "$DASH_PID_FILE")"
  if is_our_process "$pid" "dashboard"; then
    # check if same port still answers
    if curl -sf -m 1 "http://127.0.0.1:${PORT}/api/health" >/dev/null 2>&1; then
      ok "dashboard уже запущен (pid $pid) → http://127.0.0.1:${PORT}"
      return 0
    fi
  fi
  rm -f "$DASH_PID_FILE"

  info "Стартую dashboard на :${PORT}…"
  local bin=""
  if [[ -x "$ROOT/target/release/dashboard" ]]; then
    bin="$ROOT/target/release/dashboard"
  elif [[ -x "$ROOT/target/debug/dashboard" ]]; then
    bin="$ROOT/target/debug/dashboard"
  fi

  if [[ -n "$bin" ]]; then
    nohup env TRADING_ROOT="$ROOT" DASHBOARD_PORT="$PORT" \
      "$bin" \
      >>"$DASH_LOG" 2>&1 &
  else
    warn "бинарник dashboard не найден — cargo run (медленнее)"
    nohup env TRADING_ROOT="$ROOT" DASHBOARD_PORT="$PORT" \
      cargo run -q -p dashboard \
      >>"$DASH_LOG" 2>&1 &
  fi
  echo $! >"$DASH_PID_FILE"

  if wait_http "http://127.0.0.1:${PORT}/api/health" 40; then
    pid="$(read_pid "$DASH_PID_FILE")"
    ok "dashboard pid=$pid  →  ${BOLD}http://127.0.0.1:${PORT}${NC}"
  else
    err "dashboard не ответил на /api/health — tail runtime/logs/dashboard.log"
    tail -n 40 "$DASH_LOG" 2>/dev/null || true
    exit 1
  fi
}

start_mcp() {
  if [[ $WITH_MCP -ne 1 ]]; then
    return 0
  fi
  local pid
  pid="$(read_pid "$MCP_PID_FILE")"
  if is_our_process "$pid" "mcp-server"; then
    ok "mcp-server уже запущен (pid $pid)"
    return 0
  fi
  rm -f "$MCP_PID_FILE"
  info "Стартую mcp-server…"
  local bin=""
  if [[ -x "$ROOT/target/release/mcp-server" ]]; then
    bin="$ROOT/target/release/mcp-server"
  elif [[ -x "$ROOT/target/debug/mcp-server" ]]; then
    bin="$ROOT/target/debug/mcp-server"
  fi
  if [[ -n "$bin" ]]; then
    nohup env TRADING_ROOT="$ROOT" "$bin" >>"$MCP_LOG" 2>&1 &
  else
    nohup env TRADING_ROOT="$ROOT" cargo run -q -p mcp-server >>"$MCP_LOG" 2>&1 &
  fi
  echo $! >"$MCP_PID_FILE"
  sleep 0.5
  pid="$(read_pid "$MCP_PID_FILE")"
  if pid_alive "$pid"; then
    ok "mcp-server pid=$pid  лог: runtime/logs/mcp.log"
  else
    warn "mcp-server не поднялся — см. runtime/logs/mcp.log"
  fi
}

print_status() {
  header "Статус процессов"
  local sp dp mp
  sp="$(read_pid "$SHELL_PID_FILE")"
  dp="$(read_pid "$DASH_PID_FILE")"
  mp="$(read_pid "$MCP_PID_FILE")"

  if is_our_process "$sp" "ml-trader"; then
    ok "risk shell   running  pid=$sp"
  else
    warn "risk shell   stopped"
  fi

  if is_our_process "$dp" "dashboard"; then
    if curl -sf -m 1 "http://127.0.0.1:${PORT}/api/health" >/dev/null 2>&1; then
      ok "dashboard    running  pid=$dp  http://127.0.0.1:${PORT}"
    else
      warn "dashboard    pid=$dp но /api/health не отвечает"
    fi
  else
    warn "dashboard    stopped"
  fi

  if [[ -f "$MCP_PID_FILE" ]]; then
    if is_our_process "$mp" "mcp-server"; then
      ok "mcp-server   running  pid=$mp"
    else
      warn "mcp-server   stopped (pidfile stale)"
    fi
  else
    info "mcp-server   (не запрашивался)"
  fi

  # Optional: short API snapshot for humans
  if curl -sf -m 2 "http://127.0.0.1:${PORT}/api/status" -o /tmp/torgarik_status.json 2>/dev/null; then
    header "Снимок с dashboard"
    python3 - <<'PY' 2>/dev/null || true
import json
from pathlib import Path
d = json.loads(Path("/tmp/torgarik_status.json").read_text())
b = d.get("block") or {}
s = d.get("shell") or {}
print(f"  Вопрос:  {b.get('money_question_ru') or '—'}")
print(f"  Ответ:   {b.get('money_answer_ru') or b.get('headline') or '—'}")
print(f"  По-русски: {b.get('verdict_plain_ru') or b.get('summary_ru') or '—'}")
print(f"  Shell:   status={s.get('status')} mode={s.get('mode')} equity={s.get('equity')}")
acts = b.get("next_actions_ru") or []
if acts:
    print("  Дальше:")
    for i, a in enumerate(sorted(acts, key=lambda x: x.get("priority", 99))[:3], 1):
        print(f"    {i}. {a.get('title','')}")
        if a.get("cmd"):
            print(f"       $ {a['cmd']}")
PY
  fi
}

print_ready() {
  if [[ $SKIP_READY -eq 1 ]]; then
    return 0
  fi
  header "Готовность к live (ready.sh)"
  if [[ -x "$ROOT/scripts/ready.sh" ]]; then
    bash "$ROOT/scripts/ready.sh" || true
  else
    warn "scripts/ready.sh не найден"
  fi
}

print_next() {
  header "Что дальше"
  echo "  ${BOLD}Откройте UI:${NC}     http://127.0.0.1:${PORT}"
  echo "  ${BOLD}Статус:${NC}          bash scripts/start.sh status"
  echo "  ${BOLD}Логи shell:${NC}      bash scripts/start.sh logs shell"
  echo "  ${BOLD}Логи UI:${NC}         bash scripts/start.sh logs dashboard"
  echo "  ${BOLD}Стоп всего:${NC}      bash scripts/start.sh stop"
  echo "  ${BOLD}S1 часы:${NC}         python3 scripts/s1_status.py"
  echo "  ${BOLD}Готовность:${NC}      bash scripts/ready.sh"
  echo
  echo "  ${DIM}Live auto-alpha не запускается этим скриптом (защита).${NC}"
  echo "  ${DIM}Чеклист: docs/LIVE_UNLOCK.md · Подсказки: docs/QUICK_START.md${NC}"
}

do_start() {
  echo
  echo "${BOLD}╔══════════════════════════════════════════════╗${NC}"
  echo "${BOLD}║  Торгарик — комплексный запуск (dry-run)    ║${NC}"
  echo "${BOLD}╚══════════════════════════════════════════════╝${NC}"
  echo "${DIM}корень: $ROOT${NC}"

  preflight
  maybe_smoke
  build_bins

  header "4. Запуск сервисов"
  start_shell
  start_dashboard
  start_mcp

  print_status
  print_ready
  print_next

  if [[ $OPEN_BROWSER -eq 1 ]]; then
    local url="http://127.0.0.1:${PORT}"
    if command -v xdg-open >/dev/null 2>&1; then
      xdg-open "$url" >/dev/null 2>&1 || true
    elif command -v open >/dev/null 2>&1; then
      open "$url" >/dev/null 2>&1 || true
    else
      warn "не удалось открыть браузер автоматически: $url"
    fi
  fi
}

do_stop() {
  header "Остановка стека"
  stop_one "mcp-server" "$MCP_PID_FILE"
  stop_one "dashboard" "$DASH_PID_FILE"
  stop_one "risk shell" "$SHELL_PID_FILE"
  ok "готово"
}

do_logs() {
  local target="${LOG_TARGET:-all}"
  case "$target" in
    shell)
      header "shell.log (последние ${LOG_TAIL})"
      tail -n "$LOG_TAIL" "$SHELL_LOG" 2>/dev/null || warn "нет $SHELL_LOG"
      ;;
    dashboard)
      header "dashboard.log (последние ${LOG_TAIL})"
      tail -n "$LOG_TAIL" "$DASH_LOG" 2>/dev/null || warn "нет $DASH_LOG"
      ;;
    mcp)
      header "mcp.log (последние ${LOG_TAIL})"
      tail -n "$LOG_TAIL" "$MCP_LOG" 2>/dev/null || warn "нет $MCP_LOG"
      ;;
    all|*)
      header "shell.log"
      tail -n 20 "$SHELL_LOG" 2>/dev/null || warn "нет shell.log"
      header "dashboard.log"
      tail -n 20 "$DASH_LOG" 2>/dev/null || warn "нет dashboard.log"
      if [[ -f "$MCP_LOG" ]]; then
        header "mcp.log"
        tail -n 15 "$MCP_LOG" 2>/dev/null || true
      fi
      ;;
  esac
}

# ── dispatch ────────────────────────────────────────────────────────
case "$CMD" in
  start)
    do_start
    ;;
  stop)
    do_stop
    ;;
  restart)
    do_stop
    sleep 0.5
    do_start
    ;;
  status)
    print_status
    ;;
  logs)
    do_logs
    ;;
  ready)
    bash "$ROOT/scripts/ready.sh"
    ;;
  help|-h|--help)
    awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} /^set /{exit} /^$/{if(seen++)exit; print; next}' "$0"
    ;;
  *)
    err "Неизвестная команда: $CMD"
    echo "  start | stop | restart | status | logs | ready | help"
    exit 2
    ;;
esac
