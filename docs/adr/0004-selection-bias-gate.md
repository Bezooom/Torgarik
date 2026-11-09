# 0004. DSR и PBO участвуют в kill-switch

**Status:** Accepted

## Context

Kill-switch верил сырому Sharpe лучшей конфигурации. Но лучшая из `N` попыток –
это максимум выборки: он завышен даже на чистом шуме. Гейт, который этого не
учитывает, пропускает ровно тот результат, который надо было остановить.

## Decision

Отчёт несёт `deflated_sharpe_min` и `pbo_avg`. `OosStats` блокирует live, если:

- `deflated_sharpe_min <= 0` (победитель неотличим от удачи), или
- `pbo_avg > 0.5` (in-sample победитель чаще не выживает OOS).

Оба поля опциональны: отчёты без них читаются по-старому, чтобы старый
`wf_latest.json` не ломал shell.

## Consequences

- Появляется реальная причина отказывать «зелёному» результату.
- Нужны `bar_returns` и матрица испытаний: отчёт остаётся компактным (агрегаты),
  память расходуется только во время прогона.
- Порог `pbo > 0.5` консервативен; `max_pbo = 1.0` отключает проверку для
  исследовательских прогонов.

## Verification

- `deflated_sharpe_rejects_a_lucky_winner`, `cscv_flags_a_noise_winner`
- `pbo_gate_blocks_above_half_and_can_be_disabled`
- `old_report_without_verdicts_still_parses`
