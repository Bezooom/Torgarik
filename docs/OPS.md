# Runbook: эксплуатация контура

**Инвариант.** Journal – источник истины. Dashboard, Telegram и MCP только читают
его (и файлы состояния) и никогда не являются единственным местом, где что-то
известно.

## 1. Штатный запуск

```bash
bash scripts/start.sh                 # shell + dashboard (dry-run)
bash scripts/start.sh --with-mcp      # + mcp-server для агентов
bash scripts/start.sh --port 8090     # другой порт dashboard
bash scripts/start.sh status          # что запущено
bash scripts/start.sh logs            # хвост логов
bash scripts/start.sh stop            # остановить
```

Проверка готовности:

```bash
bash scripts/ready.sh
curl -s http://127.0.0.1:8080/api/health
```

## 2. Что должно быть видно после старта

| Признак | Где |
|---|---|
| `RISK SHELL started dry=... execute=...` | лог shell |
| `paper state ... absent` или `restored` | лог shell |
| `equity=... daily_pnl=... pos=...` каждые `loop_interval_secs` | лог shell |
| `block.reasons` и `oos_blocking` | `/api/status` |
| строки `heartbeat` / `oos_status` | journal |

Если через два цикла нет `heartbeat` – смотреть [TROUBLESHOOTING.md](TROUBLESHOOTING.md).

## 3. Ежедневный цикл оператора

```bash
python3 scripts/s1_status.py          # S1: часы и цели
python3 scripts/advisor.py            # советник: факты + простой текст
bash scripts/wf_refresh.sh            # обновить OOS-отчёт
bash scripts/smoke_venue.sh           # проверить площадку
```

## 4. Cron

```bash
bash scripts/install_ops_cron.sh      # OOS refresh + отчёты
bash scripts/install_hl_cron.sh       # сбор истории HL
```

Оба скрипта идемпотентны: повторный запуск не дублирует записи в crontab.

## 5. Алерты

```bash
echo "текст" | bash scripts/tg_notify.sh
```

Требуется `.env.tg` (`TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`). Без файла скрипт
молча выходит с кодом 0, чтобы cron не падал.

Односторонние уведомления и двусторонний контроль – разные вещи:

| Канал | Что умеет |
|---|---|
| `tg_notify.sh` | отправка текста |
| `ml-trader --mode tg-bot` | `/status`, `/block`, `/stop`, `/resume` |

## 6. Остановка и аварийный стоп

Три уровня, от мягкого к жёсткому:

```bash
echo "manual: разбор ситуации" > runtime/stop.flag   # 1. блок новых входов
bash scripts/start.sh stop                            # 2. остановить процессы
# 3. на бирже: закрыть позиции вручную
```

`stop.flag` перечитывается каждый цикл; закрытия и брекеты продолжают работать.
Каждый переход пишется в journal как `note`.

```bash
rm runtime/stop.flag                                  # снять стоп
```

## 7. Telegram-контроль

```bash
export TELEGRAM_BOT_TOKEN=123:ABC
export TELEGRAM_CHAT_ID=123456789
cargo run -p ml-trader -- --mode tg-bot
```

| Команда | Действие |
|---|---|
| `/status` | состояние, вердикт OOS, DSR/PBO, возраст отчёта, последние события |
| `/block` | причины блокировки live |
| `/stop` | создать `runtime/stop.flag` |
| `/resume` | удалить флаг |
| `/help` | список команд |

Бот не умеет ставить, отменять и размерять ордера. Чужой чат игнорируется с
предупреждением в лог. Подробнее: [SECURITY.md](SECURITY.md).

## 8. Разморозка live

Только по [LIVE_UNLOCK.md](LIVE_UNLOCK.md), все пункты A, B, C. Порядок:

1. `bash scripts/wf_refresh.sh` – свежий отчёт.
2. Проверить `deflated_sharpe_min`, `pbo_avg`, `holdout`.
3. `--prereg-expect <hash>` – гипотеза та же.
4. Paper несколько дней, journal без сюрпризов.
5. `RUN_MODE=live CONFIRM_LIVE=1 ... --execute`.

Любой красный пункт – стоп. Это не рекомендация, а гейт в коде.

## 9. Инциденты

| Симптом | Первое действие |
|---|---|
| `KILL-SWITCH daily_pnl=...` | ничего не делать руками, смотреть `risk_block`; торговля уже остановлена |
| `protection_max_drawdown` | разобрать просадку, снять protections только осознанно |
| `STOP FLAG set` | кто-то нажал `/stop`; выяснить кто, потом `/resume` |
| `bracket close failed ... UNPROTECTED` | позиция без стопа: закрыть вручную или повторить цикл |
| `paper state ... quarantined` | книга была битой, старт с bankroll; проверить `*.json.bad` |
| `oos kill-switch ACTIVE` | нормальное состояние, live заблокирован; работать S1 |

## 10. Резервное копирование

| Что | Почему |
|---|---|
| `runtime/journal/` | аудит и вся история решений |
| `runtime/paper_state.json` | книга paper |
| `runtime/protections.json` | история стопов (только live) |
| `runtime/brackets.json` | активные SL/TP |
| `data/reports/wf_latest.json` | вердикт kill-switch |

Journal – append-only, поэтому копируется инкрементально без риска.
