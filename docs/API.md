# API: MCP, dashboard, форматы файлов

**Инвариант.** Всё, что отдаётся наружу, либо read-only, либо пишет только
`runtime/`. Ни один интерфейс в этом документе не может инициировать сделку.

## 1. MCP-сервер (`cargo run -p mcp-server`)

Транспорт: JSON-RPC 2.0 через stdin/stdout. Протокол `2024-11-05`.
Имя сервера: `trading-hl-mcp-server`.

| Инструмент | Аргументы | Что возвращает |
|---|---|---|
| `list_instruments` | – | список торгуемых инструментов HL |
| `get_quote` | `coin` | bid / ask / mid / funding |
| `rank_funding` | `limit` (10), `min_oi` (0) | монеты, отсортированные по \|funding\| |
| `get_portfolio` | – | equity, cash, позиции |
| `get_kill_switch_status` | – | блокирован ли live и почему (`wf_latest.json`) |
| `get_shell_status` | `tail` (20) | последние `heartbeat` / `oos_status` / `risk_block` |

Инструмента для ордеров нет намеренно. Пример вызова:

```bash
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_kill_switch_status","arguments":{}}}' \
  | cargo run -p mcp-server --quiet
```

## 2. Dashboard (`cargo run -p dashboard`, порт 8080)

| Метод | Путь | Ответ |
|---|---|---|
| GET | `/api/health` | `{"status":"ok"}` |
| GET | `/api/ping` | то же |
| GET | `/api/status` | полный снимок состояния |
| GET | `/` | статический HTML |

Ключи `/api/status`:

| Ключ | Что внутри |
|---|---|
| `status`, `ts`, `root` | живость и корень проекта |
| `policy` | что включено и что заморожено, включая текстовое объяснение |
| `block` | `reasons[]`, `oos_blocking`, возраст отчёта, DSR/PBO |
| `shell` | последний `heartbeat`: equity, positions, dry |
| `s1` | часы S1 и цели недели |
| `journal` | 40 последних событий |
| `risk_blocks`, `kill_switches`, `oos_journal` | выборки по `kind` |
| `signals` | 30 последних строк `signals.jsonl` |
| `backtest` | сводка `wf_latest.json` |
| `equity_series` | точки equity из journal |

## 3. `runtime/signals.jsonl`

Одна JSON-строка на сигнал. Shell читает только строки, дописанные после старта.

| Поле | Тип | Обязательно | Смысл |
|---|---|---|---|
| `coin` | string | да | `ETH`, `SOL`, ... |
| `side` | string | да | `buy` \| `sell` \| `flat` \| `close` |
| `size` | number | нет | размер в монете; по умолчанию лимит риска |
| `note` | string | нет | человекочитаемая причина |
| `source` | string | нет | `external`, `l1:0x..`, `hunt`, `trend_plan` |
| `sl_pct`, `tp_pct` | number | нет | стоп/тейк в долях (0.02 = 2%) |
| `sl_price`, `tp_price` | number | нет | абсолютные цены, приоритетнее процентов |

```bash
echo '{"coin":"ETH","side":"buy","size":0.01,"sl_pct":0.02,"tp_pct":0.05,"note":"manual","source":"external"}' \
  >> runtime/signals.jsonl
```

Сигнал – это **намерение**, а не ордер: дальше его проверяют whitelist,
protections, `risk_gate` и OOS-гейт.

## 4. Journal (`runtime/journal/journal_YYYYMMDD.jsonl`)

Каждая строка несёт `version` (сейчас `1.0`); major mismatch отбрасывается при
чтении, minor принимается.

| `kind` | Поля | Когда |
|---|---|---|
| `heartbeat` | equity, available, daily_pnl, n_positions, mode | каждый цикл |
| `risk_block` | reason, detail | ордер отклонён |
| `kill_switch` | daily_pnl, equity, limit | дневной лимит убытка |
| `oos_status` | avg_sharpe, avg_pnl, n_folds, profitable_folds, blocking | старт и смена состояния |
| `external_signal` | source, coin, side, size, note | прочитан сигнал |
| `order_intent` | coin, side, size, reduce_only, reasoning, dry_run | перед отправкой |
| `order_result` | status, filled, avg_price, ok, error | после отправки |
| `bracket` | coin, action, level, mid | `register` / `trigger_sl` / `trigger_tp` / `removed` |
| `anomaly` | coin, name, value, context | структурный логгер |
| `note` | text | свободная запись, включая переходы `stop.flag` |

```bash
jq -c 'select(.kind=="risk_block")' runtime/journal/journal_$(date +%Y%m%d).jsonl
```

## 5. `wf_latest.json` (schema 1.2)

| Поле | Смысл |
|---|---|
| `schema_version` | `1.2` |
| `alpha`, `strategy`, `generated_at`, `exec`, `window` | метаданные прогона |
| `folds[]` | по фолду: `train`, `test`, `best_param`, `best_label`, `n_trials`, `deflated_sharpe`, `pbo` |
| `test_avg_sharpe`, `test_total_pnl`, `profitable_test_folds` | агрегаты OOS |
| `deflated_sharpe_min`, `deflated_sharpe_avg`, `pbo_avg`, `n_trials` | поправка на отбор |
| `prereg_hash`, `prereg` | отпечаток гипотезы и данных |
| `holdout` | `total_pnl`, `avg_sharpe`, `profitable_coins`, `coins[]` |
| `realism_notes[]` | что именно считалось реалистичным |

Схема аддитивна: читатели без нового поля работают по-старому, kill-switch без
`deflated_sharpe_min` не блокирует по DSR.

## 6. Файлы состояния

| Файл | Формат | Кто пишет |
|---|---|---|
| `runtime/paper_state.json` | `version`, `cash`, `daily_pnl`, `day`, `positions[]` | `PaperExchange` |
| `runtime/brackets.json` | массив брекетов | `BracketStore` |
| `runtime/protections.json` | `version`, `peak_equity`, `stops[]`, `closes[]` | `ProtectionStore` (только live) |
| `runtime/stop.flag` | одна строка причины | человек / бот / cron |
| `runtime/fx_cbr.json` | `FxRates` на дату | `CbrClient` |
| `runtime/state.json` | состояние shell | shell |

Битые файлы не приводят к панике: книга paper уходит в `*.json.bad`, protections
и брекеты стартуют пустыми с предупреждением в лог.
