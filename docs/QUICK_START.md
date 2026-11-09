# Quick Start — Торгарик

Пошаговый guide для новых пользователей. От нуля до live-трейдинга.

> **Если термины пугают:** полная «инструкция для людей» → **[ДЛЯ_НОВИЧКОВ.md](ДЛЯ_НОВИЧКОВ.md)**  
> (ежедневный порядок, шпаргалка команд).

---

## 1. Первые шаги (5 минут)

```bash
# Убедитесь, что Rust установлен
rustc --version
cargo --version

cd /path/to/Trading

# Комплексный запуск: shell + dashboard (dry-run)
bash scripts/start.sh
# Откройте http://127.0.0.1:8080
```

Альтернатива — два терминала вручную:

```bash
cargo run -p ml-trader -- --mode shell   # терминал A
cargo run -p dashboard                   # терминал B → :8080
```

---

## 2. Проверка готовности к live

```bash
# Самый простой способ
bash scripts/ready.sh
```

Ожидаемый вывод:

```
📊 Trading System Status
━━━━━━━━━━━━━━━━━━━━━━
OOS Sharpe:    -4.72 (need > 0) ❌
OOS PnL:      -$70.38 (need > 0) ❌
Holdout PnL:  -$70.38 (need > 0) ❌
Live Status:  FROZEN

🔒 Verdict: Not ready to trade live
   Reason: OOS Sharpe < 0 (-4.72)
```

---

## 3. Типовые сценарии

### Сценарий A: «Хочу просто посмотреть, что происходит»

```bash
# Shell — основной терминал
cargo run -p ml-trader -- --mode shell

# Dashboard — браузер
cargo run -p dashboard
```

**Что вы увидите:** ценовые квоты, OOS-статус, journal, S1-часы.

### Сценарий B: «Хочу проверить, готов ли бот к live»

```bash
# Быстрая проверка
bash scripts/ready.sh

# Или детальный
cargo run -p ml-trader -- --mode shell
```

### Сценарий B: «Хочу запустить backtest»

```bash
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --holdout-frac 0.15 --report data/reports/wf_latest.json
```

Результат: `data/reports/wf_latest.json` — отчёт с Sharpe, PnL, holdout.

### Сценарий C: «Хочу перейти в live»

1. Проверьте OOS: `bash scripts/ready.sh` (должен быть зелёным)
2. Настройте ключи:

```bash
export RUN_MODE=live
export CONFIRM_LIVE=1
export WALLET_PRIVATE_KEY=0x...
export WALLET_ADDRESS=0x...
```

3. Запустите:

```bash
cargo run -p ml-trader -- --mode shell --execute
```

Подробно: [LIVE_UNLOCK.md](LIVE_UNLOCK.md)

### Сценарий D: «Хочу исследовать аномалии»

```bash
# Сбор данных
python3 scripts/collect_hl_history.py --coins BTC,ETH,SOL --days 60

# Анализ
python3 scripts/analyze_hypotheses.py --coins ETH,SOL,BTC

# Аномалии
cargo run -p ml-trader -- --mode anomaly
```

### Сценарий E: «Хочу просто вести дневник (S1)»

```bash
# Ежедневная запись
python3 scripts/s1_daily_log.py --hours 3 --track skill --shell --note "..."

# Статус
python3 scripts/s1_status.py
```

---

## 4. Troubleshooting

### «cargo run — нет такого файла или каталога»

```bash
# Проверьте, что вы в корне проекта
pwd
# Ожидаемо: путь к папке Torgarik (там лежит Cargo.toml)

# Проверьте Rust
rustc --version
```

### «Dashboard не открывается»

```bash
# Проверьте, запущен ли dashboard
curl http://127.0.0.1:8080/api/status
```

### «OOS красный, не могу перейти в live»

Это нормально. OOS Sharpe -4.72, Holdout PnL -$70.38. Live заблокирован kill-switch.

**Что делать:**
1. Продолжайте торговать в dry-run / paper
2. Собирайте S1-часы
3. Запустите backtest с другими параметрами
4. Проверьте [TROUBLESHOOTING.md](TROUBLESHOOTING.md)

### «API error при подключении к Hyperliquid»

```bash
# Проверьте подключение
bash scripts/smoke_venue.sh
```

---

## 5. Полезные команды-шпаргалки

| Задача | Команда |
|--------|---------|
| Запустить shell | `cargo run -p ml-trader -- --mode shell` |
| Запустить dashboard | `cargo run -p dashboard` |
| Проверить готовность | `bash scripts/ready.sh` |
| Запустить backtest | `cargo run -p backtester -- --walk-forward` |
| Проверить S1 | `python3 scripts/s1_status.py` |
| Проверить API | `bash scripts/smoke_venue.sh` |
| Запустить MCP | `cargo run -p mcp-server` |
| Запустить anomaly | `cargo run -p ml-trader -- --mode anomaly` |
| Запустить alpha | `cargo run -p ml-trader -- --mode alpha` |

---

## 6. Следующие шаги

1. **Прочитайте** [TROUBLESHOOTING.md](TROUBLESHOOTING.md) — ответы на типовые проблемы
2. **Изучите** [CONFIG_GUIDE.md](CONFIG_GUIDE.md) — как настраивать параметры
3. **Следите** за [ROADMAP.md](ROADMAP.md) — что будет дальше
4. **Читайте** [TIPS.md](TIPS.md) — полезные подсказки
