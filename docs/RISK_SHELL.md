# Risk / journal shell

**Назначение:** не «ловить alpha», а **не слить** и **вести журнал**.  
Авто-стратегия **выключена**. Default: `cargo run -p ml-trader -- --mode shell`.

**Phase 1–4:** versioned journal, whitelist + `check_order`, OOS/holdout force-dry on live `--execute`, signal offset = EOF at start.

## Запуск

```bash
# только мониторинг + journal + dry signals
cargo run -p ml-trader -- --mode shell

# live shell (ордера только из signals.jsonl + CONFIRM_LIVE + keys)
export CONFIRM_LIVE=1
export WALLET_PRIVATE_KEY=0x...
export WALLET_ADDRESS=0x...
export RUN_MODE=live
cargo run -p ml-trader -- --mode shell --execute
```

## Внешний сигнал

Файл: `runtime/signals.jsonl` (одна JSON-строка = один intent)

```json
{"coin":"ETH","side":"buy","size":0.05,"note":"manual long","source":"me"}
{"coin":"ETH","side":"flat","note":"close"}
{"coin":"SOL","side":"sell","note":"discretionary short","source":"telegram"}
```

| side | действие |
|------|----------|
| `buy` / `long` | long (size optional → risk %) |
| `sell` / `short` | short |
| `flat` / `close` | закрыть позицию |

Shell:

1. при старте offset = EOF (старые строки **не** переигрываются)  
2. читает только **новые** строки  
3. whitelist + **protections** (stop-loss guard / cooldown / drawdown)  
4. `check_order` (position / daily loss / exposure)  
5. dry-run log или place (live+execute блокируется при красном OOS)  
6. пишет в `runtime/journal/journal_YYYYMMDD.jsonl`

## Journal events

Каждая строка – JSONL с полем **`version`** (сейчас `1.0`).  
Major mismatch (например `2.0`) отбрасывается при чтении; minor drift (`1.x`) принимается.

| kind | смысл |
|------|--------|
| `heartbeat` | equity / positions snapshot |
| `risk_block` | ордер/signal отклонён (whitelist, risk_gate, OOS, …) |
| `kill_switch` | дневной DD ≥ limit |
| `oos_status` | снимок OOS при старте shell |
| `external_signal` | строка из `signals.jsonl` |
| `order_intent` / `order_result` | intent и результат (в т.ч. `dry_run`) |
| `anomaly` | structural logger |
| `note` | произвольная заметка |
| `bracket` | локальный SL/TP: `register` / `trigger_sl` / `trigger_tp` / `removed` |

Разбор:

```bash
tail -f runtime/journal/journal_$(date +%Y%m%d).jsonl | jq .
```

## Protections (щит поверх risk_gate)

`risk_gate` отвечает «не велик ли ордер», protections – «не доказала ли книга, что торговать нельзя».
Проверяются **до** `check_order`, только на вход (закрытия не блокируются никогда).

| Guard | Условие блока | Настройка (`[risk.protections]`) |
|---|---|---|
| `StoplossGuard` | N стопов по монете за окно | `stoploss_guard_max_stops`, `stoploss_guard_window_hours` |
| `Cooldown` | по монете закрывались недавно | `cooldown_hours` |
| `MaxDrawdown` | equity ниже пика на X | `max_drawdown_frac` (по умолчанию 0.10) |

Любой guard отключается нулём. Причина попадает в journal как `risk_block` с
`reason = protection_stoploss_guard | protection_cooldown | protection_max_drawdown`.

Состояние: `runtime/protections.json` (версия схемы, битый файл → пустая история, без паники).
Пишется **только** при live+execute; dry и paper держат историю в памяти, чтобы репетиция не портила live-guard.

```bash
jq -c 'select(.kind=="risk_block")' runtime/journal/journal_$(date +%Y%m%d).jsonl
```

## Dry-книга (репетиция стопа)

В dry-режиме ничего не филлится на бирже, поэтому бумажная книга пуста. Чтобы SL/TP
можно было репетировать, shell ведёт собственную **dry-книгу**: dry-филл создаёт позицию,
bracket её мониторит, срабатывание закрывает и пишет `bracket` + `order_result`.

Проверка вживую (dry):

```bash
# 1. shell запущен, ETH mid ~2267
# 2. вход с заведомо срабатывающим SL (long: SL выше mid)
echo '{"coin":"ETH","side":"buy","size":0.01,"sl_price":2317.35,"note":"stop1"}' >> runtime/signals.jsonl
# 3. через цикл: bracket trigger_sl + protection record_stop
# 4. повторный вход той же монеты:
echo '{"coin":"ETH","side":"buy","size":0.02,"note":"second"}' >> runtime/signals.jsonl
# → risk_block protection_stoploss_guard (при stoploss_guard_max_stops = 1)
```

## Paper-книга переживает рестарт

`runtime/paper_state.json` (write-then-rename). Битый файл уходит в `*.json.bad`,
книга стартует с bankroll – «тихий ноль позиций» исключён. `daily_pnl` переносится
только внутри одного дня UTC. При старте bracket'ы сверяются с восстановленными
позициями (`bracket removed` для осиротевших).

## Stop-флаг и Telegram-контроль

`runtime/stop.flag` – файл-рубильник. Есть файл → новые входы блокируются
(`risk_block reason=kill_switch`), закрытия и брекеты работают. Нет файла → входы разрешены.
Shell перечитывает флаг каждый цикл и пишет `note` на каждом переходе.

```bash
echo "manual: too much chop" > runtime/stop.flag   # стоп
rm runtime/stop.flag                               # снять
grep -E "stop_flag" runtime/journal/journal_$(date +%Y%m%d).jsonl
```

Telegram-бот умеет только читать статус и трогать флаг. Ордеров он не ставит никогда.

```bash
export TELEGRAM_BOT_TOKEN=123:ABC
export TELEGRAM_CHAT_ID=123456789      # единственный чат, которому можно
cargo run -p ml-trader -- --mode tg-bot
```

| Команда | Действие |
|---|---|
| `/status` | состояние, вердикт OOS, DSR/PBO, возраст отчёта, последние события |
| `/block` | причины, по которым live заблокирован |
| `/stop` | создать флаг (входы стоп) |
| `/resume` | удалить флаг |
| `/help` | список команд |

Чат, не совпадающий с `TELEGRAM_CHAT_ID`, игнорируется с предупреждением в лог.

### OOS vs live

- **Paper / dryrun shell** – работает всегда (advisory).
- **Live + `--execute`** – если OOS red (`wf_latest.json` или holdout red), shell **форсирует dry-run** и пишет `risk_block` / `oos_status`.
- Авто-alpha live блокируется `Settings::validate()` / `validate_with_oos`.
- Полный чеклист разморозки: [LIVE_UNLOCK.md](LIVE_UNLOCK.md).

## Зачем это «про деньги»

- Ты (или внешний источник) = **alpha**  
- Бот = **risk + audit trail**  
- Без OOS-edge автоторговли депозит не жжётся алгоритмом  

Связка с anomaly logger: смотришь аномалии → **сам** решаешь → пишешь signal line.

## Dashboard

```bash
cargo run -p dashboard
# http://127.0.0.1:8080 – journal + state + S1, без ордеров
```

→ [DASHBOARD.md](DASHBOARD.md)
