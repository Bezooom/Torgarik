# Troubleshooting — Торгарик

Типовые проблемы, причины и решения.

---

## 1. OOS Red — «Не готов к live»

### Симптомы

```
🔒 Verdict: Not ready to trade live
   Reason: OOS Sharpe < 0 (-4.72)
```

### Причины

| Причина | Как проверить | Решение |
|---------|--------------|---------|
| **Sharpe < 0** | `data/reports/wf_latest.json` → `test_avg_sharpe` | Запустить backtest с другими параметрами |
| **PnL < 0** | `test_total_pnl` | Проверить данные, увеличить embargo |
| **Holdout красный** | `holdout.total_pnl` | Увеличить `holdout-frac` или изменить стратегию |
| **Мало данных** | `train_bars` / `test_bars` | Увеличить `--days` при сборе данных |
| **Нестабильные данные** | `realism_notes` | Проверить `taker_bps`, `slip_bps`, `impact_bps` |

### Как исправить

```bash
# Запустить backtest с реалистичными параметрами
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --folds 3 --embargo 2 --slip-bps 2 --impact-bps 1 --taker-bps 3.5 \
  --holdout-frac 0.15 --report data/reports/wf_latest.json

# Проверить статус
bash scripts/ready.sh
```

### Что делать, пока OOS красный

- Торговать в **dry-run** (paper)
- Собирать **S1-часы**
- Исследовать **аномалии**
- Следить за **L1 leaderboard**

---

## 2. API Errors — проблемы с Hyperliquid

### Симптомы

```
Error: failed to connect to Hyperliquid
Error: 429 Too Many Requests
Error: WebSocket connection closed
```

### Причины и решения

| Ошибка | Причина | Решение |
|--------|---------|---------|
| `failed to connect` | Нет сети / wrong URL | Проверить `hyperliquid_base_url` |
| `429 Too Many Requests` | Rate limit | Увеличить `loop_interval_secs` |
| `WebSocket closed` | Нестабильная сеть | Перезапустить shell |
| `Invalid API key` | Неверный ключ | Проверить `WALLET_PRIVATE_KEY` |
| `Timeout` | Медленная сеть (RF) | Настроить `HTTPS_PROXY` |

### Проверка подключения

```bash
# Smoke-тест
bash scripts/smoke_venue.sh

# Проверка WebSocket
cargo run -p ml-trader -- --mode shell
# Должен печатать ценовые квоты каждые 12 сек
```

### Прокси для РФ

```bash
# В config/dryrun.toml
[exchange]
# proxy_url = "socks5://127.0.0.1:1080"

# Или через env
export HTTPS_PROXY=socks5://127.0.0.1:1080
```

---

## 3. Backtest Failures — проблемы с backtest

### Симптомы

```
Error: holdout.profitable_coins == 0
Error: test_avg_sharpe < 0
Error: test_total_pnl < 0
```

### Типовые проблемы

| Проблема | Причина | Решение |
|----------|---------|---------|
| **Слишком маленький holdout** | `holdout-frac` слишком мал | Увеличить до `0.2` |
| **Overfitting** | Модель слишком сложная | Уменьшить `n_estimators`, упростить стратегию |
| **Нестандартные данные** | Плохие OHLCV | Пересобрать данные |
| **Неверные fees** | `taker_bps` не соответствует HL | Установить `--taker-bps 3.5` |
| **Мало фолдов** | Мало данных для теста | Увеличить `--folds 5` |

### Как запустить backtest правильно

```bash
# Базовый
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --holdout-frac 0.15 --report data/reports/wf_latest.json

# С расширенным эмбарго
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --folds 5 --embargo 4 --slip-bps 2 --impact-bps 1 --taker-bps 3.5 \
  --holdout-frac 0.2 --report data/reports/wf_latest.json

#查看 результат
cat data/reports/wf_latest.json | python3 -m json.tool
```

---

## 4. Dashboard — проблемы с UI

### Симптомы

- Dashboard не открывается
- API `/api/status` возвращает ошибку
- Ошибки в консоли при запуске

### Диагностика

```bash
# Проверить, запущен ли dashboard
curl http://127.0.0.1:8080/api/status

# Проверить порт
ss -tlnp | grep 8080

# Перезапустить
cargo run -p dashboard
```

### Типовые ошибки

| Ошибка | Решение |
|--------|---------|
| `Port 8080 already in use` | Завершить другой процесс или изменить порт |
| `Connection refused` | Shell не запущен — запустить `cargo run -p ml-trader` |
| `OSError: [Errno 24] Too many open files` | Увеличить `ulimit -n` |

---

## 5. S1 — проблемы с skill-путём

### Симптомы

- `s1_status.py` показывает 0 часов
- Journal пустой
- Непонятно, сколько часов торговать

### Проверка

```bash
# Статус S1
python3 scripts/s1_status.py

# Ежедневная запись
python3 scripts/s1_daily_log.py --hours 3 --track skill --shell --note "..."

# Проверить journal
cat runtime/journal/journal_$(date +%Y%m%d).jsonl
```

### Рекомендации

- Минимум **3 часа** в день
- Записывать **каждый день**
- Использовать **--note** для контекста
- Следить за **S1-часами** в dashboard

---

## 6. «Am I ready?» — ответы

### Сценарий 1: OOS Sharpe < 0, но PnL > 0

**Ответ:** ⚠️ **Almost ready**

Sharpe < 0 говорит о нестабильности. Рекомендуется:
- Увеличить количество фолдов
- Проверить embargo
- Торговать в dry-run ещё неделю

### Сценарий 2: OOS Sharpe > 0, но Holdout < 0

**Ответ:** ⚠️ **Holdout не прошёл**

Holdout красный говорит о переобучении. Рекомендуется:
- Увеличить `holdout-frac` до `0.2`
- Упростить стратегию
- Проверить данные на выбросы

### Сценарий 3: Все зелёные, но Sharpe < 1

**Ответ:** ✅ **Ready, но осторожно**

Можно переходить в live с минимальным размером позиции. Рекомендуется:
- Начать с `max_portfolio_exposure = 0.10`
- Мониторить первые 2 недели
- Иметь план abort

### Сценарий 4: Все зелёные, Sharpe > 1

**Ответ:** ✅ **Ready to trade live!**

Можно переходить в live с полным размером. Рекомендуется:
- Установить `RUN_MODE=live`
- Установить `CONFIRM_LIVE=1`
- Запустить `cargo run -p ml-trader -- --mode shell --execute`

---

## 7. Чеклист перед live

```bash
# 1. Проверить OOS
bash scripts/ready.sh
# Ожидаемо: "Ready to trade live"

# 2. Проверить API
bash scripts/smoke_venue.sh
# Ожидаемо: "Venue OK"

# 3. Проверить journal
cargo run -p ml-trader -- --mode shell
# Ожидаемо: печатает квоты

# 4. Запустить live
export RUN_MODE=live
export CONFIRM_LIVE=1
export WALLET_PRIVATE_KEY=0x...
export WALLET_ADDRESS=0x...
cargo run -p ml-trader -- --mode shell --execute
```

---

## 8. Полезные ссылки

| Ресурс | Описание |
|--------|----------|
| [QUICK_START.md](QUICK_START.md) | Пошаговый guide |
| [CONFIG_GUIDE.md](CONFIG_GUIDE.md) | Параметры конфигурации |
| [LIVE_UNLOCK.md](LIVE_UNLOCK.md) | Чеклист для live |
| [BACKTEST.md](BACKTEST.md) | Backtest детали |
| [RISK_SHELL.md](RISK_SHELL.md) | Risk shell детали |
| [DASHBOARD.md](DASHBOARD.md) | Dashboard API |
