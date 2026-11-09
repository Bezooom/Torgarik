#!/usr/bin/env bash
# wf_refresh.sh — авто-refresh OOS-отчёта (walk-forward + holdout, realistic exec)
# и Telegram-алерт с вердиктом kill-switch.
#
# Usage:
#   bash scripts/wf_refresh.sh                          # trend/majors, сборка если нужно
#   bash scripts/wf_refresh.sh --alpha funding --preset core --no-build
#   bash scripts/wf_refresh.sh --no-notify              # без Telegram
#
# Флаги:
#   --alpha trend|funding|majors_alpha   (default trend)
#   --preset majors|core|ethsol|meme     (default majors)
#   --capital N                          (default 10000)
#   --holdout-frac F                     (default 0.15)
#   --no-build                           не пересобирать backtester
#   --no-notify                          не слать Telegram
#
# Cron: bash scripts/install_ops_cron.sh
# Гейты вердикта: docs/LIVE_UNLOCK.md §A1–A3 (Sharpe>0 и PnL>0 на OOS и holdout).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

ALPHA="trend"
PRESET="majors"
CAPITAL="10000"
HOLDOUT_FRAC="0.15"
BUILD="yes"
NOTIFY="yes"

while [ $# -gt 0 ]; do
    case "$1" in
        --alpha) ALPHA="$2"; shift 2 ;;
        --preset) PRESET="$2"; shift 2 ;;
        --capital) CAPITAL="$2"; shift 2 ;;
        --holdout-frac) HOLDOUT_FRAC="$2"; shift 2 ;;
        --no-build) BUILD="no"; shift ;;
        --no-notify) NOTIFY="no"; shift ;;
        *) echo "Unknown flag: $1" >&2; exit 2 ;;
    esac
done

REPORT="data/reports/wf_latest.json"
BIN="target/release/backtester"
LOG="logs/wf_refresh.log"
mkdir -p logs data/reports/history

log() { echo "[$(date '+%F %T')] $*"; }

notify() {
    if [ "$NOTIFY" = "yes" ]; then
        printf '%s' "$1" | bash "$SCRIPT_DIR/tg_notify.sh" || log "[wf] TG notify failed (non-fatal)"
    fi
}

if [ "$BUILD" = "yes" ]; then
    log "[wf] cargo build --release -p backtester ..."
    if ! cargo build --release -p backtester >>"$LOG" 2>&1; then
        log "[wf] BUILD FAILED — см. $LOG"
        notify "🔴 Torgarik WF refresh FAILED
step: cargo build -p backtester
см. logs/wf_refresh.log"
        exit 3
    fi
fi

if [ ! -x "$BIN" ]; then
    log "[wf] Нет бинаря $BIN — запусти без --no-build или собери вручную."
    exit 2
fi

log "[wf] walk-forward alpha=$ALPHA preset=$PRESET capital=$CAPITAL holdout=$HOLDOUT_FRAC taker=3.5 slip=2 impact=1"

# shellcheck disable=SC2086
if ! "$BIN" --walk-forward --alpha "$ALPHA" --preset "$PRESET" \
        --capital "$CAPITAL" --holdout-frac "$HOLDOUT_FRAC" \
        --taker-bps 3.5 --slip-bps 2 --impact-bps 1 \
        --report "$REPORT" >>"$LOG" 2>&1; then
    log "[wf] BACKTEST FAILED — см. $LOG"
    notify "🔴 Torgarik WF refresh FAILED
alpha=$ALPHA preset=$PRESET
см. logs/wf_refresh.log"
    exit 4
fi

# Архивная копия — история вердиктов по датам.
ARCHIVE="data/reports/history/wf_${ALPHA}_${PRESET}_$(date +%F_%H%M).json"
cp "$REPORT" "$ARCHIVE"

# Вердикт по гейтам A1–A3: NaN/Inf трактуется как red (fail-closed).
readarray -t RESULT < <(python3 - "$REPORT" "$ALPHA" "$PRESET" <<'PY'
import json, math, sys

r = json.load(open(sys.argv[1]))
alpha, preset = sys.argv[2], sys.argv[3]
oos_sharpe = r.get("test_avg_sharpe")
oos_pnl = r.get("test_total_pnl")
h = r.get("holdout") or {}
h_pnl = h.get("total_pnl")
h_sharpe = h.get("avg_sharpe")
green_folds = r.get("profitable_test_folds")
folds = len(r.get("folds") or [])
coins = r.get("n_coins")


def bad(x):
    if not isinstance(x, (int, float)):
        return True
    return not math.isfinite(x) or x <= 0


reasons = []
if bad(oos_sharpe):
    reasons.append(f"OOS Sharpe {oos_sharpe} <= 0")
if bad(oos_pnl):
    reasons.append(f"OOS PnL ${oos_pnl} <= 0")
if bad(h_pnl):
    reasons.append(f"Holdout PnL ${h_pnl} <= 0")
if bad(h_sharpe):
    reasons.append(f"Holdout Sharpe {h_sharpe} <= 0")


def fmt(x):
    if isinstance(x, (int, float)) and math.isfinite(x):
        return f"{x:.2f}"
    return str(x)


verdict = "GREEN" if not reasons else "RED"
emoji = "🟢" if verdict == "GREEN" else "🔒"
lines = [
    f"{emoji} Torgarik WF {alpha}/{preset}",
    f"OOS Sharpe {fmt(oos_sharpe)} | PnL ${fmt(oos_pnl)}",
    f"Holdout Sharpe {fmt(h_sharpe)} | PnL ${fmt(h_pnl)}",
]
if folds:
    lines.append(f"Folds green {green_folds}/{folds}" + (f" | coins {coins}" if coins else ""))
if verdict == "GREEN":
    lines.append("Verdict: gates A1–A3 GREEN → можно смотреть LIVE_UNLOCK B/C")
else:
    lines.append("Verdict: NOT READY (live остаётся frozen)")
    lines.extend(f"• {x}" for x in reasons)

print(verdict)
print("\n".join(lines))
PY
)

VERDICT="${RESULT[0]:-RED}"
MESSAGE="$(printf '%s\n' "${RESULT[@]:1}")"

log "[wf] verdict=$VERDICT archive=$ARCHIVE"

notify "$MESSAGE"

if [ "$VERDICT" != "GREEN" ]; then
    exit 1
fi
