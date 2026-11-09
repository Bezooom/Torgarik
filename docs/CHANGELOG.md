# Changelog

Формат: по фазам, только то, что меняет поведение или контракт. Внутренние
рефакторинги без изменения поведения не перечисляются.

## Phase 6 – щит, point-in-time, РФ-контур (2026-11)

### Добавлено

- **Protections** (`crates/strategies/src/protections.rs`): `StoplossGuard`,
  `Cooldown`, `MaxDrawdown`. Гейт стоит до `check_order`, закрытия не блокируются.
  Состояние в `runtime/protections.json`, пишется только при live+execute.
- **Dry-книга** в shell: dry-филл создаёт позицию, поэтому SL/TP репетируются
  в dry-режиме, а не удаляются как осиротевшие брекеты.
- **`runtime/paper_state.json`**: paper-книга переживает рестарт, битый файл
  уходит в `*.json.bad`, `daily_pnl` переносится только внутри дня UTC.
- **Сверка брекетов** при старте: `BracketStore::reconcile` снимает брекеты без
  соответствующей позиции.
- **Look-ahead audit**: `BacktestEngine::audit_causality` перезапускает генератор
  на префиксах и требует бит-в-бит совпадения; провал = exit 3.
- **`--as-of`**: срез истории на дату, включительно, устойчив к неотсортированным
  барам.
- **`stats`** (`crates/backtester/src/stats.rs`): PSR, ожидаемый максимум Sharpe,
  deflated Sharpe, CSCV PBO, нормальные CDF/quantile без внешних крейтов.
- **Kill-switch читает DSR/PBO**: `deflated_sharpe_min <= 0` или `pbo_avg > 0.5`
  блокируют live; отчёты без полей читаются по-старому.
- **Pre-registration** (`crates/backtester/src/prereg.rs`): `--prereg-only`,
  `--prereg-expect`, exit 4 при расхождении.
- **Один генератор сигналов** (`crates/strategies/src/signals.rs`): бэктестер
  делегирует ему, `ml-trader --mode signals` пишет те же сигналы в
  `runtime/signals.jsonl`.
- **`MarketData` отделён от `Exchange`**: площадка без пути ордера реализует
  только чтение; существующие реализации не изменились (blanket impl).
- **MOEX ISS** (`crates/exchange/src/moex.rs`): инструменты, котировки, стакан,
  свечи, дивиденды. Read-only.
- **Курс ЦБ** (`crates/common/src/fx.rs`, `crates/exchange/src/cbr.rs`):
  USD/EUR/CNY → RUB, суточный кэш, ошибка вместо нулевого курса.
- **Налоги** (`crates/common/src/tax.rs`): FIFO-лоты, НДФЛ 13/15% с порогом
  2.4 млн ₽, ИИС-3 (5 лет), дивиденды, комиссии.
- **Remote stop** (`crates/ml-trader/src/kill_flag.rs`): файл `runtime/stop.flag`
  блокирует входы, переходы журналируются.
- **Telegram control** (`crates/ml-trader/src/tg_control.rs`): `/status`,
  `/block`, `/stop`, `/resume`, `/help`; чужой чат игнорируется.

### Изменено

- **`wf_latest.json` → schema 1.2**: добавлены `n_trials`, `deflated_sharpe_min`,
  `deflated_sharpe_avg`, `pbo_avg`, `prereg_hash`, `prereg`. Поля аддитивные.
- **`BacktestResult`**: добавлено `bar_returns` (только в памяти, `serde(skip)`).
- **`OosStats`**: добавлены `deflated_sharpe`, `pbo` и метод `block_reasons`.
- **`RiskShell`**: порядок гейтов теперь whitelist → protections → `check_order`;
  добавлены dry-книга, сверка брекетов и проверка stop-флага каждый цикл.
- **`ml-trader` режимы**: `shell`, `signals`, `tg-bot`, `anomaly`, `alpha`.
- **Dashboard** `block.reasons`: добавлены причины по DSR и PBO.

### Исправлено

- Брекет в dry-режиме удалялся сразу после регистрации (позиции нет), поэтому
  SL/TP нельзя было репетировать.
- MOEX-котировка могла отдать пересечённый стакан (`bid > ask`); теперь такой
  тач заменяется на `mid`.
- Отсутствующий блок `dividends` в ISS больше не выглядит как «дивидендов нет»:
  возвращается ошибка доступа.
- `--as-of` валидируется до любых веток запуска, а не только в walk-forward.

## Phase 5 – ops automation

- Cron-refresh OOS-отчёта, Telegram-уведомления (`scripts/tg_notify.sh`),
  `scripts/install_ops_cron.sh`.

## Phase 4 – integration gates

- E2E-политика: красный OOS блокирует live, paper и shell остаются доступны.
- Holdout участвует в `OosStats::is_blocking`.
- `validate_with_oos` инъектируется для тестов.
- Параллельный `refresh_market_data` (`tokio::join!`).

## Phase 3 – realism

- Комиссия HL taker 3.5 bps по умолчанию, база слиппеджа + impact
  `sqrt(participation)`, задержка очереди в барах.
- Report schema 1.1: `generated_at`, `exec`, `window`, `strategy`.
- Holdout-хвост вне grid-search.

## Phase 2 – observability

- Блок-панель dashboard, возраст OOS, S1-часы.
- MCP: `get_kill_switch_status`, `get_shell_status`.

## Phase 1 – risk shell

- Journal с `version`, `oos_status`, flush на запись.
- `RiskShell::check_order`, whitelist, лимиты размера.
- OOS форсирует dry в live+execute.

## Phase 0 – green build

- Доменные типы в `common`, HL-стек в `exchange`, PM-клиенты помечены
  `#[deprecated]`.
