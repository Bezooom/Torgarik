# 0002. OOS-гейт блокирует live в коде

**Status:** Accepted

## Context

Инструкция «не включай live, пока OOS красный» не работает: человек устаёт,
спешит и включает. Правило, которое живёт только в документе, – это пожелание.

## Decision

Вердикт kill-switch вычисляется из `data/reports/wf_latest.json` и применяется в
двух местах:

1. `Settings::validate()` не даёт стартовать live при красном OOS;
2. `force_dry_run(mode, execute, oos_blocking)` форсирует dry даже если live уже
   запущен и `--execute` передан.

Красный OOS = Sharpe < 0 **или** PnL < 0 **или** holdout red **или** non-finite
метрика. NaN блокирует: повреждённый отчёт не должен выглядеть зелёным.

## Consequences

- Оператор не может «случайно» включить автоторговлю.
- Любая ошибка в отчёте приводит к блокировке, а не к торговле. Это выбранный
  fail-closed.
- Обновление отчёта – отдельная операция (`scripts/wf_refresh.sh`), а не побочный
  эффект запуска.

## Verification

- `e2e_red_oos_blocks_live_allows_shell_paper`
- `dry_run_policy_oos_locks_live_execute`
- `deflated_sharpe_gate_blocks_a_lucky_winner` (NaN fail-closed)
