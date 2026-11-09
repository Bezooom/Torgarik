#!/usr/bin/env bash
# ready.sh — Проверка готовности trading-системы к live-трейдингу
# Usage: bash scripts/ready.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPORT="$PROJECT_DIR/data/reports/wf_latest.json"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if jq is available
if ! command -v jq &> /dev/null; then
    echo "⚠️  jq не установлен. Установите: brew install jq (macOS) или apt install jq (Linux)" >&2
    echo "   Без jq скрипт будет работать в режиме fallback." >&2
fi

# Parse JSON report
parse_json() {
    local json="$1"
    local key="$2"
    
    if command -v jq &> /dev/null; then
        jq -r "$key" <<< "$json"
    else
        # Fallback: simple grep/sed parsing
        echo "$json" | sed -n "s/.*\"$key\"\s*:\s*\([0-9.e-]*\).*/\1/p" | head -1
    fi
}

# Check if report exists
if [ ! -f "$REPORT" ]; then
    echo "❌ Отчёт не найден: $REPORT" >&2
    echo "   Запустите backtest: cargo run -p backtester -- --walk-forward --preset ethsol --report data/reports/wf_latest.json" >&2
    exit 1
fi

# Read report
REPORT_JSON=$(cat "$REPORT")

# Parse values
OOS_SHARPE=$(parse_json "$REPORT_JSON" '.test_avg_sharpe')
OOS_PNL=$(parse_json "$REPORT_JSON" '.test_total_pnl')
HOLDOUT_PNL=$(parse_json "$REPORT_JSON" '.holdout.total_pnl')
HOLDOUT_SHARPE=$(parse_json "$REPORT_JSON" '.holdout.avg_sharpe')
PROFITABLE_COINS=$(parse_json "$REPORT_JSON" '.holdout.profitable_coins')
N_COINS=$(parse_json "$REPORT_JSON" '.holdout.n_coins')
REPORT_AGE=$(parse_json "$REPORT_JSON" '.generated_at')
ALPHA=$(parse_json "$REPORT_JSON" '.alpha')
STRATEGY=$(parse_json "$REPORT_JSON" '.strategy')
N_FOLDS=$(parse_json "$REPORT_JSON" '.n_folds')
EMBARGO=$(parse_json "$REPORT_JSON" '.embargo_bars')
HOLDOUT_FRAC=$(parse_json "$REPORT_JSON" '.holdout_frac')

# Helper to compare floats
float_gt() {
    python3 -c "print('true' if $1 > $2 else 'false')"
}

# Determine status
SHARPE_GREEN=$(float_gt "$OOS_SHARPE" 0)
PNL_GREEN=$(float_gt "$OOS_PNL" 0)
HOLDOUT_GREEN=$(float_gt "$HOLDOUT_PNL" 0)
HOLDOUT_SHARPE_GREEN=$(float_gt "$HOLDOUT_SHARPE" 0)

# Count failures
FAILURES=0
REASONS=()

if [ "$SHARPE_GREEN" = "false" ]; then
    FAILURES=$((FAILURES + 1))
    REASONS+=("OOS Sharpe < 0 ($OOS_SHARPE)")
fi

if [ "$PNL_GREEN" = "false" ]; then
    FAILURES=$((FAILURES + 1))
    REASONS+=("OOS PnL < 0 ($OOS_PNL)")
fi

if [ "$HOLDOUT_GREEN" = "false" ]; then
    FAILURES=$((FAILURES + 1))
    REASONS+=("Holdout PnL < 0 ($HOLDOUT_PNL)")
fi

if [ "$HOLDOUT_SHARPE_GREEN" = "false" ]; then
    FAILURES=$((FAILURES + 1))
    REASONS+=("Holdout Sharpe < 0 ($HOLDOUT_SHARPE)")
fi

# Print report
echo ""
echo "📊 Trading System Status"
echo "━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Format values with units
format_value() {
    local val="$1"
    local unit="$2"
    if [ "$unit" = "usd" ]; then
        # Handle negative numbers by stripping minus, formatting, and re-adding
        if [[ "$val" == -* ]]; then
            local abs_val="${val#-}"
            local formatted
            formatted=$(printf "%'d" "$abs_val" 2>/dev/null) || formatted="$abs_val"
            echo "-$formatted"
        else
            printf "%'d" "$val" 2>/dev/null || echo "$val"
        fi
    else
        echo "$val"
    fi
}

# Sharpe
if [ "$SHARPE_GREEN" = "true" ]; then
    printf "${GREEN}OOS Sharpe:%8s  > 0 ✅%s${NC}\n" "$OOS_SHARPE"
else
    printf "${RED}OOS Sharpe:%8s  < 0 ❌%s${NC}\n" "$OOS_SHARPE"
fi

# OOS PnL
if [ "$PNL_GREEN" = "true" ]; then
    printf "${GREEN}OOS PnL:%10s  > 0 ✅%s${NC}\n" "$(format_value "$OOS_PNL" usd)"
else
    printf "${RED}OOS PnL:%10s  < 0 ❌%s${NC}\n" "$(format_value "$OOS_PNL" usd)"
fi

# Holdout PnL
if [ "$HOLDOUT_GREEN" = "true" ]; then
    printf "${GREEN}Holdout PnL:%9s  > 0 ✅%s${NC}\n" "$(format_value "$HOLDOUT_PNL" usd)"
else
    printf "${RED}Holdout PnL:%9s  < 0 ❌%s${NC}\n" "$(format_value "$HOLDOUT_PNL" usd)"
fi

# Holdout Sharpe
if [ "$HOLDOUT_SHARPE_GREEN" = "true" ]; then
    printf "${GREEN}Holdout Sharpe:%7s  > 0 ✅%s${NC}\n" "$HOLDOUT_SHARPE"
else
    printf "${RED}Holdout Sharpe:%7s  < 0 ❌%s${NC}\n" "$HOLDOUT_SHARPE"
fi

# Profitable coins
printf "Profitable coins:%4s / %s" "$PROFITABLE_COINS" "$N_COINS"
if [ "$PROFITABLE_COINS" -gt 0 ]; then
    echo " ✅"
else
    echo " ❌"
fi

echo ""
echo "📋 Configuration:"
echo "   Strategy: $ALPHA / $STRATEGY"
echo "   Folds: $N_FOLDS, Embargo: $EMBARGO bars"
echo "   Holdout fraction: $HOLDOUT_FRAC"
echo ""

# Verdict
echo "━━━━━━━━━━━━━━━━━━━━━━"
if [ "$FAILURES" -eq 0 ]; then
    echo -e "${GREEN}🟢 Verdict: Ready to trade live${NC}"
    echo ""
    echo "   Все проверки пройдены. Можно переходить в live!"
    echo "   Установка: export RUN_MODE=live && export CONFIRM_LIVE=1"
else
    echo -e "${RED}🔒 Verdict: Not ready to trade live${NC}"
    echo ""
    echo "   Причины:"
    for reason in "${REASONS[@]}"; do
        echo "   - $reason"
    done
    echo ""
    echo "   Рекомендации:"
    echo "   - Продолжать торговать в dry-run / paper"
    echo "   - Собирать S1-часы"
    echo "   - Запустить backtest с другими параметрами"
    echo "   - Подробнее: docs/TROUBLESHOOTING.md"
fi
echo ""

# Exit code
if [ "$FAILURES" -eq 0 ]; then
    exit 0
else
    exit 1
fi
