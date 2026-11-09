# 0007. Один генератор сигналов для бэктеста и shell

**Status:** Accepted

## Context

Генерация сигналов жила в бэктестере, а shell получал сигналы извне. Формально
это разные задачи, фактически – тихая ловушка: стратегия, прошедшая walk-forward,
и стратегия, которая пишет сигналы, могли быть разным кодом. Тогда весь
research-контур ничего не доказывает про эксплуатацию.

## Decision

`crates/strategies/src/signals.rs` – единственная реализация:

- `trend_plan(mids, funding, params) -> SignalPlan` (EMA + funding-veto + vol regime);
- `funding_plan(funding, params) -> Vec<f64>`.

Функции чистые: срезы чисел на входе, векторы на выходе, без I/O и часов.
Бэктестер делегирует им (`BacktestEngine::trend_plan`), `ml-trader --mode signals`
вызывает их же и пишет строки в `runtime/signals.jsonl`.

## Consequences

- «Прошло WF» и «торгует» – один и тот же код.
- Shell по-прежнему не торгует сам: сигнал проходит whitelist, protections,
  `risk_gate`, OOS-гейт.
- Каузальность проверяется один раз для одной реализации, а не для двух.

## Verification

- `plan_is_causal_by_construction`, `funding_plan_shorts_rich_funding_and_exits`
- `audit_passes_for_causal_trend_and_funding_plans` (после делегирования)
- `ml-trader --mode signals` → строки с `source: trend_plan`
