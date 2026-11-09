# Config Guide — Торгарик

Объяснение параметров конфигурации, примеры dryrun/live, OOS-пороги.

---

## 1. Файлы конфигурации

| Файл | Назначение |
|------|-----------|
| `config/dryrun.toml` | Параметры тестового режима (paper) |
| `config/live.toml` | Параметры реального режима (live) |
| `config/wallets.example.json` | Шаблон списка кошельков (скопируйте в `config/wallets.json`, он в `.gitignore`) |

---

## 2. Ключевые параметры `dryrun.toml`

### `[exchange]` — подключение к бирже

| Параметр | Значение | Описание |
|----------|----------|----------|
| `primary_venue` | `hyperliquid` | Основная биржа |
| `hyperliquid_base_url` | `https://api.hyperliquid.xyz` | REST API URL |
| `hyperliquid_ws_url` | `wss://api.hyperliquid.xyz/ws` | WebSocket URL |
| `symbol_whitelist` | `["ETH", "SOL"]` | Монеты для торговли |
| `allow_synthetic` | `false` | Разрешить синтетические пары |
| `max_gas_price_gwei` | `50` | Макс. газ на L2 |
| `max_slippage` | `0.01` | Макс. проскальзывание 1% |

### `[risk]` — управление рисками

| Параметр | Значение | Описание |
|----------|----------|----------|
| `max_portfolio_exposure` | `0.20` | Макс. доля портфеля в позиции (20%) |
| `max_position_per_instrument` | `0.10` | Макс. доля одной монеты (10%) |
| `max_daily_loss` | `0.02` | Макс. дневной убыток (2%) |
| `stop_loss` | `0.06` | Stop-loss на позицию (6%) |
| `starting_bankroll` | `1000.0` | Стартовый капитал (USD) |

### `[strategy]` — стратегия majors_alpha

| Параметр | Значение | Описание |
|----------|----------|----------|
| `name` | `majors_alpha` | Имя стратегии |
| `funding_enter` | `0.00003` | Порог входа по funding |
| `funding_exit` | `0.000010` | Порог выхода по funding |
| `kelly_fraction` | `0.2` | Доля Келли для размера позиции |
| `loop_interval_secs` | `12` | Интервал цикла (сек) |

### `[strategy]` — вселенная (universe)

| Параметр | Значение | Описание |
|----------|----------|----------|
| `universe_mode` | `allowlist` | Режим вселенной |
| `blacklist` | `["ACE", "NOT", ...]` | Исключённые монеты |
| `min_oi_notional_usd` | `5 000 000` | Мин. open interest ($5M) |
| `min_mid` | `0.05` | Мин. средняя цена |
| `max_abs_funding` | `0.0005` | Макс. абсолютное funding |
| `max_positions` | `2` | Макс. открытых позиций |
| `top_n` | `2` | Топ-N монет |

### `[strategy]` — EMA + regime

| Параметр | Значение | Описание |
|----------|----------|----------|
| `ema_fast` | `5` | Быстрая EMA |
| `ema_slow` | `34` | Медленная EMA |
| `funding_veto` | `0.00015` | Veto по funding |
| `min_trend_strength` | `0.0008` | Мин. сила тренда |
| `vol_lookback` | `24` | Окно волатильности (часы) |
| `min_vol_ratio` | `0.0025` | Мин. отношение волатильности |

### `[ml]` — ML-модуль (опционально)

| Параметр | Значение | Описание |
|----------|----------|----------|
| `enabled` | `false` | Включён ли ML |
| `model_path` | `models/market_predictor.onnx` | Путь к модели |
| `consensus_threshold` | `0.65` | Порог консенсуса |
| `llm_endpoint` | `https://openrouter.ai/api/v1` | LLM endpoint |
| `sglang_endpoint` | `http://localhost:30000/v1` | SGLang endpoint |

---

## 3. Dryrun vs Live

### Dryrun (paper) — без реальных денег

```toml
# config/dryrun.toml
mode = "dry_run"
log_level = "info"
metrics_port = 9090
```

**Когда использовать:**
- Новый бот, первый запуск
- Тестирование стратегии
- OOS красный (не готов к live)
- Разработка и отладка

### Live — реальные деньги

```toml
# config/live.toml
mode = "live"
```

**Требования для live:**
1. `RUN_MODE=live`
2. `CONFIRM_LIVE=1`
3. `WALLET_PRIVATE_KEY` и `WALLET_ADDRESS`
4. OOS Sharpe > 0
5. Holdout PnL > 0
6. `allow_synthetic = false`

---

## 4. OOS-пороги и kill-switch

### Секция `[oos]` в TOML

Явно в `config/dryrun.toml` и `config/live.toml` (значения = defaults):

```toml
[oos]
sharpe_floor = 0.0
avg_pnl_floor = 0.0
min_profitable_fold_ratio = 0.0
```

### Что блокирует live

`OosStats::is_blocking()` / `is_blocking_with_settings` → `true`, если:

| Условие | Порог | Примечание |
|---------|-------|------------|
| `avg_sharpe` | `< sharpe_floor` | или **NaN/Inf** |
| `avg_pnl` | `< avg_pnl_floor` | или **NaN/Inf** |
| holdout | `total_pnl` / `avg_sharpe` red | или **NaN/Inf** |
| fold ratio | `< min_profitable_fold_ratio` | если ratio > 0 |

Fail-closed: corrupt metrics block live.

### Как интерпретировать Sharpe

| Значение | Интерпретация |
|----------|--------------|
| `< 0` | Стратегия проигрывает |
| `0 — 0.5` | Слабый edge |
| `0.5 — 1.0` | Приемлемый |
| `1.0 — 2.0` | Хороший |
| `> 2.0` | Отличный |

### Как интерпретировать PnL

| Значение | Интерпретация |
|----------|--------------|
| `< 0` | Убыток |
| `0 — 50` | Небольшой плюс |
| `50 — 200` | Хороший |
| `> 200` | Отличный |

### Holdout — самый честный тест

Holdout — это данные, которые модель **никогда не видела** при обучении.

- `holdout.total_pnl > 0` — стратегия赚钱 на новых данных
- `holdout.avg_sharpe > 0` — Sharpe на новых данных положительный
- `holdout.profitable_coins == n_coins` — все монеты赚钱

---

## 5. Примеры конфигураций

### Минимальная live-конфигурация

```bash
export RUN_MODE=live
export CONFIRM_LIVE=1
export WALLET_PRIVATE_KEY=0xabc...
export WALLET_ADDRESS=0xdef...
cargo run -p ml-trader -- --mode shell --execute
```

### Live с прокси (для РФ)

```bash
export RUN_MODE=live
export CONFIRM_LIVE=1
export WALLET_PRIVATE_KEY=0xabc...
export WALLET_ADDRESS=0xdef...
export HTTPS_PROXY=socks5://127.0.0.1:1080
cargo run -p ml-trader -- --mode shell --execute
```

### Dryrun с расширенным логированием

```toml
# config/dryrun.toml
mode = "dry_run"
log_level = "debug"
metrics_port = 9090
```

---

## 6. Переменные окружения

| Переменная | Обязательна | Описание |
|-----------|-------------|----------|
| `RUN_MODE` | нет | `dryrun`, `paper`, или `live` |
| `CONFIRM_LIVE` | для live | `1` — подтверждение |
| `WALLET_PRIVATE_KEY` | для live | Приватный ключ кошелька |
| `WALLET_ADDRESS` | для live | Адрес кошелька |
| `HTTPS_PROXY` | опционально | Прокси для подключения |
| `SGLANG_ENDPOINT` | для ML | Endpoint SGLang |
| `LLM_API_KEY` | для ML | Ключ OpenRouter |

---

## 7. Как проверить текущую конфигурацию

```bash
# Статус через dashboard
curl http://127.0.0.1:8080/api/status

# OOS-статус через MCP
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_kill_switch_status","arguments":{}}}' \
  | cargo run -p mcp-server --quiet

# Быстрая проверка через скрипт
bash scripts/ready.sh
```
