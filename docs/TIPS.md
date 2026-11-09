# Tips — Торгарик

Подсказки для разных уровней пользователей.

---

## 1. Quick Tips для CLI

### Первые запуски

```bash
# Всегда начинайте с dry-run
cargo run -p ml-trader -- --mode shell

# Не забудьте запустить dashboard
cargo run -p dashboard
```

### Проверка статуса

```bash
# Самый быстрый способ проверить готовность
bash scripts/ready.sh

# Более детально
curl http://127.0.0.1:8080/api/status
```

### Backtest

```bash
# Быстрый backtest
cargo run -p backtester -- --walk-forward --preset ethsol \
  --report data/reports/wf_latest.json

# С расширенным анализом
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --folds 5 --embargo 3 --holdout-frac 0.2 \
  --slip-bps 2 --impact-bps 1 --taker-bps 3.5 \
  --report data/reports/wf_latest.json
```

### S1-дисциплина

```bash
# Ежедневная запись (минимум 3 часа)
python3 scripts/s1_daily_log.py --hours 3 --track skill --shell --note "..."

# Проверка статуса
python3 scripts/s1_status.py
```

---

## 2. Tips для конфигурации

### Переключение режимов

```bash
# Dry-run (по умолчанию)
export RUN_MODE=dryrun

# Paper (то же самое, другой префикс)
export RUN_MODE=paper

# Live
export RUN_MODE=live
export CONFIRM_LIVE=1
export WALLET_PRIVATE_KEY=0x...
export WALLET_ADDRESS=0x...
```

### Настройка прокси (для РФ)

```bash
# Вариант 1: через env
export HTTPS_PROXY=socks5://127.0.0.1:1080

# Вариант 2: через config/dryrun.toml
# [exchange]
# proxy_url = "socks5://127.0.0.1:1080"
```

### Расширенное логирование

```bash
# Debug-режим
RUST_LOG=debug cargo run -p ml-trader -- --mode shell

# Только info (по умолчанию)
cargo run -p ml-trader -- --mode shell
```

### Мониторинг метрик

```bash
# Метрики на порту 9090
curl http://127.0.0.1:9090/metrics

# В браузере
open http://127.0.0.1:9090/metrics
```

---

## 3. Tips для мониторинга

### Регулярные проверки

```bash
# Утром: проверить готовность
bash scripts/ready.sh

# Вечером: проверить S1
python3 scripts/s1_status.py

# Раз в неделю: backtest
cargo run -p backtester -- --walk-forward --preset ethsol \
  --report data/reports/wf_latest.json
```

### Ключевые метрики для отслеживания

| Метрика | Где смотреть | Норма |
|---------|-------------|-------|
| OOS Sharpe | `scripts/ready.sh` | `> 0` |
| Holdout PnL | `scripts/ready.sh` | `> 0` |
| Live-статус | `scripts/ready.sh` | `Ready` |
| S1-часы | `s1_status.py` | `≥ 3/день` |
| Journal | `runtime/journal/` | `Ежедневный` |
| API-подключение | `smoke_venue.sh` | `OK` |

### Когда беспокоиться

| Сигнал | Действие |
|--------|----------|
| OOS Sharpe < -2 | Перестать торговать live |
| Holdout PnL < -$100 | Увеличить embargo |
| API ошибки > 5/день | Проверить прокси |
| S1 < 10 часов/неделю | Увеличить дисциплину |
| Dashboard не отвечает | Перезапустить |

---

## 4. Tips для live-трейдинга

### Перед первым live

```bash
# 1. Проверить OOS
bash scripts/ready.sh
# Должно быть: "Ready to trade live"

# 2. Проверить API
bash scripts/smoke_venue.sh
# Должно быть: "Venue OK"

# 3. Запустить shell
cargo run -p ml-trader -- --mode shell --execute
```

### Во время live

```bash
# Мониторить каждые 30 минут
curl http://127.0.0.1:8080/api/status

# Проверить journal
tail -20 runtime/journal/journal_$(date +%Y%m%d).jsonl
```

### При проблемах

```bash
# Если OOS стал красным
# 1. Остановить --execute
# 2. Вернуть RUN_MODE=dryrun
# 3. Запустить backtest
# 4. Дождаться зелёного OOS
```

---

## 5. Tips для разработки

### Запуск в режиме разработчика

```bash
# С подробным выводом
cargo run -p ml-trader -- --mode shell --verbose

# С логированием
RUST_LOG=debug cargo run -p ml-trader -- --mode shell
```

### Тестирование

```bash
# Все тесты
cargo test --workspace

# Проверка типов
cargo check --workspace --all-targets
```

### MCP-сервер

```bash
# Запуск MCP
cargo run -p mcp-server

# Проверка статуса
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_kill_switch_status","arguments":{}}}' \
  | cargo run -p mcp-server --quiet
```

---

## 6. Tips для исследования

### Сбор данных

```bash
# История Hyperliquid
python3 scripts/collect_hl_history.py --coins BTC,ETH,SOL --days 60

# Funding snapshot
python3 scripts/fetch_hl_funding_snapshot.py
```

### Анализ

```bash
# Гипотезы
python3 scripts/analyze_hypotheses.py --coins ETH,SOL,BTC

# L1 leaderboard
python3 scripts/l1_leaderboard.py --top 30
```


---

## 7. Tips для не-технических пользователей

### «Что мне делать?»

1. **Утром:** `bash scripts/ready.sh` — посмотреть, готов ли бот
2. **В течение дня:** `cargo run -p ml-trader -- --mode shell` — запустить shell
3. **Вечером:** `python3 scripts/s1_status.py` — проверить S1
4. **Раз в неделю:** `bash scripts/ready.sh` — проверить OOS

### «Что означает "OOS красный"?»

OOS красный = бот ещё не доказал, что赚钱 на новых данных.

**Что делать:**
- Не переходить в live
- Продолжать торговать в paper
- Собирать S1-часы
- Ждать зелёного OOS

### «Когда можно переходить в live?»

Когда `bash scripts/ready.sh` показывает:

```
🟢 Ready to trade live
```

---

## 8. Полезные ссылки

| Ресурс | Описание |
|--------|----------|
| [QUICK_START.md](QUICK_START.md) | Пошаговый guide |
| [CONFIG_GUIDE.md](CONFIG_GUIDE.md) | Параметры конфигурации |
| [TROUBLESHOOTING.md](TROUBLESHOOTING.md) | Решение проблем |
| [LIVE_UNLOCK.md](LIVE_UNLOCK.md) | Чеклист для live |
| [GUIDE.md](GUIDE.md) | Полная инструкция |
