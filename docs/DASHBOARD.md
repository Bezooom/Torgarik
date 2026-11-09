# Dashboard — понятная ops-панель

**Что это:** read-only веб-экран для **новичка и оператора**: «можно ли live?», «почему нет?», «что сделать сейчас?».  
**Не это:** торговый терминал, размещение ордеров, «процент побед».

## Запуск

```bash
# Рекомендуется: shell + dashboard одним скриптом
bash scripts/start.sh
# опции: --open  --port 8080  --no-build  --with-mcp
# stop / status / logs:  bash scripts/start.sh stop|status|logs

# Только UI
cargo run -p dashboard
export DASHBOARD_PORT=8080
export TRADING_ROOT=/path/to/Trading   # если cwd не корень workspace
```

Откройте: **http://127.0.0.1:8080** (только localhost).

## Что видит новичок

1. **Большой блок сверху** — вопрос «можно ли торговать реальными деньгами?» и ответ **НЕТ / условно**.
2. **Причины** — простым языком + severity (важно / внимание / инфо).
3. **Четыре плитки** — капитал, часы S1, Sharpe, PnL; у каждой кнопка **?** с подсказкой.
4. **Что сделать сейчас** — нумерованные шаги + команды с **Копировать**.
5. **Словарь** — Live/dry-run, shell, OOS, S1 (раскрывающиеся строки).
6. **Уведомления (toasts)** — shell выключен, OOS red, kill-switch, stale report.
7. **Онбординг** — модалка «как читать экран» (можно скрыть; «Сбросить подсказки» внизу).
8. **Журнал** — свёрнут по умолчанию (для любопытных).

UI опрашивает `/api/status` каждые **5 с**.

## API

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | SPA |
| `GET` | `/api/health` | `{ ok: true }` |
| `GET` | `/api/status` | Полный снимок + beginner-поля |

### Полезные поля `block.*` (для UI и MCP)

| Field | Meaning |
|-------|---------|
| `money_question_ru` / `money_answer_ru` | Вопрос/ответ «про деньги» |
| `verdict_plain_ru` | Одна фраза без жаргона |
| `next_actions_ru[]` | `{ title, detail, cmd, priority }` |
| `alerts[]` | `{ id, level, title, body, sticky }` → toasts |
| `live_auto_alpha_blocked` | OOS red / missing |
| `live_shell_execute_blocked` | live `--execute` → dry |
| `reasons[]` | Технические/операторские причины |
| `paper_shell_allowed` / `s1_manual_allowed` | всегда true по политике |

Остальное: `shell`, `s1`, `backtest`, `journal`, `risk_blocks`, `equity_series` — как раньше.

## Связанные команды

```bash
cargo run -p ml-trader -- --mode shell
bash scripts/ready.sh
python3 scripts/s1_status.py
```

Docs: [QUICK_START.md](QUICK_START.md) · [TIPS.md](TIPS.md) · [RISK_SHELL.md](RISK_SHELL.md) · [LIVE_UNLOCK.md](LIVE_UNLOCK.md) · [GUIDE.md](GUIDE.md)
