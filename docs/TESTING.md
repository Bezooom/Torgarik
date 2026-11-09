# Тесты: что чем покрыто

**Инвариант.** Любое утверждение в документации либо подтверждено тестом, либо
помечено как непроверяемое здесь. Сетевые тесты помечены `#[ignore]` и запускаются
явно: обычный прогон не зависит от интернета.

## 1. Один прогон

```bash
cargo test --workspace          # 193 теста, ~10 с
cargo check --workspace --all-targets
```

Сетевые (не входят в обычный прогон):

```bash
cargo test -p trading-exchange --lib moex_live -- --ignored --nocapture
cargo test -p trading-exchange --lib cbr_live  -- --ignored --nocapture
```

## 2. Карта по крейтам

| Крейт | Тестов | Что покрыто |
|---|---|---|
| `trading-common` | 45 | конфиг и `Settings`, `OosStats` (Sharpe/PnL/holdout/DSR/PBO), journal-версии, `tax` (FIFO, НДФЛ, ИИС-3), `fx` |
| `trading-exchange` | 23 | paper-книга и её персистентность, `MarketData`/`Exchange`, MOEX ISS (котировки, стакан, свечи, дивиденды), ЦБ РФ |
| `trading-strategies` | 42 | funding, majors_alpha, Kelly, risk_gate, `protections` (три гарда), `signals` (каузальность, funding-veto) |
| `backtester` | 27 | движок и издержки, walk-forward и holdout, look-ahead audit, `stats` (PSR/DSR/PBO), `prereg`, `as_of` |
| `ml-trader` | 44 | risk shell (политика dry, protections, dry-книга), брекеты и сверка, journal, `kill_flag`, `tg_control`, `signal_gen` |
| `dashboard`, `mcp-server` | 0 | покрытие косвенное: компиляция и ручной smoke |

## 3. Тесты, которые ловят конкретные классы ошибок

| Класс ошибки | Тест |
|---|---|
| «зелёный» OOS на удаче | `deflated_sharpe_rejects_a_lucky_winner`, `pbo_gate_blocks_above_half_and_can_be_disabled` |
| подглядывание в будущее | `audit_detects_a_signal_that_peeks_at_the_next_bar` |
| некаузальный индикатор | `plan_is_causal_by_construction`, `audit_passes_for_causal_trend_and_funding_plans` |
| тихая потеря paper-позиций | `persistent_paper_book_survives_restart`, `corrupt_paper_state_is_quarantined_not_silently_zeroed` |
| осиротевший брекет | `reconcile_keeps_only_matching_positions` |
| торговля после серии стопов | `stoploss_guard_blocks_after_threshold_and_expires_with_window` |
| просадка без тормоза | `drawdown_guard_blocks_all_entries_below_peak` |
| «нет доступа» выглядит как «нет данных» | `dividends_block_absent_is_an_error_not_an_empty_list`, `quote_without_trades_is_insufficient_data_not_zero` |
| NaN проходит как «зелёный» | `deflated_sharpe_gate_blocks_a_lucky_winner` (NaN fail-closed) |
| чужой чат управляет ботом | `parses_commands_including_group_suffix_and_aliases` + фильтр по `chat_id` в `run_bot` |
| курс валют подменён нулём | `rejects_zero_or_non_finite_rates`, `zero_or_negative_value_is_rejected` |

## 4. Что тестами не покрыто и почему

| Область | Причина |
|---|---|
| Реальные ордера HL | нет ключей в CI; проверяется вручную по [VENUE_SMOKE.md](VENUE_SMOKE.md) |
| Telegram API | нужен бот-токен; парсинг и рендеринг покрыты, сетевой цикл – нет |
| Стакан и дивиденды MOEX | ISS требует авторизацию; тесты фиксируют честную ошибку доступа |
| UI dashboard | статический HTML + `/api/status`; проверяется глазами |
| РФ-брокер (Tinkoff/Alor) | адаптера нет: без боевых ключей его нечем проверить |

## 5. Правила для новых тестов

1. Тест проверяет наблюдаемое поведение, а не реализацию.
2. Тест, который проходит на пустой заглушке, удаляется.
3. Один тест – одна причина падения.
4. Сеть и время не должны влиять на результат обычного прогона.
5. Тест, закрепляющий случайную формулировку или побочный эффект, удаляется при
   первом же рефакторинге.

## 6. Перед мержем

```bash
cargo check --workspace --all-targets
cargo test --workspace
```

Красный любой из двух – работа не закончена. Сетевые тесты прогоняются отдельно и
не блокируют мерж, но при изменении `moex.rs` или `cbr.rs` их надо запустить.
