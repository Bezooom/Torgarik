# Данные: история, point-in-time, отпечаток

**Инвариант.** Любой прогон должен воспроизводиться на тех же данных. Если файл
истории дописывается, прогон обязан иметь возможность «увидеть мир на дату»
(`--as-of`) и обязан уметь доказать, что не подсмотрел будущее (look-ahead audit).

Код: `crates/backtester/src/data.rs`, `crates/ml-trader/src/signal_gen.rs`.

## 1. Где лежат данные

| Путь | Что |
|---|---|
| `data/history/{COIN}_1h.json` | бары: `{"coin": "...", "bars": [{"ts","mid","funding","premium","volume","volume_usd"}]}` |
| `data/history/{COIN}_1h.csv` | альтернатива: `ts,ts_ms,mid,funding,premium,volume_usd` |
| `data/reports/wf_latest.json` | последний WF-отчёт, источник kill-switch |
| `data/hl_funding_latest.json` | снимок фандинга |
| `data/l1/` | наблюдения за кошельками |

Загрузчик предпочитает JSON, CSV – запасной формат. Бары с `mid <= 0`
отбрасываются на входе: ноль в середине ряда ломает доходности.

## 2. Обновление истории

```bash
python3 scripts/collect_hl_history.py --days 14      # докачать HL
python3 scripts/fetch_hl_funding_snapshot.py         # снимок фандинга
bash scripts/install_hl_cron.sh                      # ежечасный сбор
```

Докачка **удлиняет** файл. Именно поэтому нужен `--as-of`: без него вчерашний
прогон сегодня уже не воспроизводится.

## 3. Point-in-time

```bash
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol --as-of 2026-07-15
```

Что происходит:

1. `--as-of` принимает unix-секунды или `YYYY-MM-DD` (UTC-полночь).
2. Каждый ряд фильтруется: остаются бары с `ts <= as_of`.
3. Фильтр, а не обрезка: бары не обязаны быть отсортированы.
4. В лог пишется, сколько выброшено: `point-in-time cut as_of=... dropped N bars`.

Проверка (`cargo test -p backtester --lib data`) фиксирует включительность границы
и устойчивость к неотсортированному входу.

## 4. Look-ahead audit

**Инвариант.** Сигнал на баре `i` зависит только от баров `0..=i`.

Аудит не ищет конкретные ошибки в коде, он проверяет свойство: генератор
перезапускается на усечённых префиксах, и общий префикс обязан совпасть
**бит-в-бит**.

```bash
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol
# look-ahead audit ETH: 8 probes clean
# look-ahead audit: 2 series clean
```

| Поведение | Результат |
|---|---|
| утечки нет | прогон продолжается, в лог `N probes clean` |
| утечка есть | `LOOK-AHEAD ...`, затем exit 3, PnL не печатается |
| `--no-audit` | аудит пропускается, только для намеренно «дырявых» экспериментов |

Тест `audit_detects_a_signal_that_peeks_at_the_next_bar` доказывает, что аудит
ловит зависимость от следующего бара, а не просто «всегда зелёный».

## 5. Отпечаток данных

В pre-registration попадает не содержимое, а границы ряда:

```json
{"coin": "ETH", "bars": 1440, "first_ts": 1780875600, "last_ts": 1786053600}
```

Достаточно, чтобы отличить «тот же ряд» от «ряд, куда докачали ещё месяц».
Подробнее: [PREREG.md](PREREG.md).

## 6. Данные MOEX

| Что | Как |
|---|---|
| Котировки, стакан | `MoexClient` (`crates/exchange/src/moex.rs`), ISS JSON |
| Свечи | `MoexClient::candles(secid, from, interval)` |
| Дивиденды | `MoexClient::dividends(secid)` |
| Валюта | RUB; `Quote` не несёт валюту, это соглашение клиента |

Стакан и дивиденды на части сетей отдают HTML вместо JSON: клиент возвращает
`TradingError::Auth`, а не пустой результат. «Нет доступа» не должно выглядеть как
«нет данных». Подробнее: [RF.md](RF.md).

## 7. Гигиена

| Правило | Почему |
|---|---|
| не редактировать историю руками | отпечаток перестанет совпадать с pre-registration |
| `data/history` не в git | объём и воспроизводимость обеспечиваются скриптом сбора |
| отчёт всегда с `generated_at` | dashboard считает возраст и ругается на старый |
| `--as-of` для сравнений | иначе сравниваете разные ряды |
