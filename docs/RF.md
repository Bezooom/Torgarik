# РФ-контур: MOEX, RUB, налоги

Что в проекте есть для российского рынка и чего сознательно нет.
Крипто-путь (Hyperliquid) описан в [ARCHITECTURE.md](ARCHITECTURE.md) и [RISK_SHELL.md](RISK_SHELL.md).

## 1. Что уже работает

| Слой | Где | Что умеет |
|---|---|---|
| Данные MOEX | `crates/exchange/src/moex.rs` | `list_instruments`, `get_quote`, `get_orderbook`, `candles`, `dividends` через публичный ISS JSON |
| Курс ЦБ | `crates/common/src/fx.rs`, `crates/exchange/src/cbr.rs` | `FxRates` (USD/EUR/CNY → RUB), суточный кэш `runtime/fx_cbr.json` |
| Валюта | `crates/common/src/tax.rs` | `Currency::{Rub, Usd, Usdc}`, `AssetClass::{Crypto, Equity, Bond, Fund, Deposit}` |
| Налоги | `crates/common/src/tax.rs` | FIFO-лоты, НДФЛ 13/15% с порогом 2.4 млн ₽, ИИС-3 |
| Read-only контракт | `crates/exchange/src/traits.rs` | `MarketData` отделён от `Exchange`: у площадки может не быть пути ордера |

MOEX подключён **только на чтение**. `factory::build_exchange` для `venue = "moex"`
возвращает `Config`-ошибку, а не брокера: у проекта нет РФ-брокерского адаптера.

```bash
# проверить доступ к ISS и ЦБ (сеть, РФ-география)
cargo test -p trading-exchange --lib moex_live -- --ignored --nocapture
cargo test -p trading-exchange --lib cbr_live -- --ignored --nocapture
# SBER bid/ask/mid в RUB + честные отказы по orderbook и dividends
```

## 2. Правила учёта, зашитые в код

1. **Вся себестоимость в RUB.** `TaxLot::unit_cost_rub` – цена покупки, пересчитанная
   по курсу ЦБ на дату сделки. Клиент обязан передать RUB-стоимость сам: контур не
   тянет курс (в бэклоге).
2. **FIFO.** `LotBook::sell` списывает самые старые лоты первыми, частичный лот разрешён,
   продажа больше остатка – ошибка, и леджер при этом не меняется.
3. **НДФЛ двухступенчатый.** 13% до 2.4 млн ₽ базы, 15% выше (`THRESHOLD_RUB`,
   `RATE_LOW`, `RATE_HIGH`). База = max(0, реализованный PnL + дивиденды − расходы).
   Убыточный год даёт 0, не отрицательный налог.
4. **Дивиденды** складываются в базу в этой модели. В реальности их удерживает
   эмитент/брокер у источника – двойной учёт надо исключать на уровне вызывающего кода.
5. **ИИС-3** освобождает результат после 5 полных лет владения (`IIS3_MIN_YEARS`).
   **ИИС-1** освобождением от налога на результат **не** является: там вычет на взнос,
   поэтому `tax_due_for` для него возвращает обычный налог.
6. **Округление** только в `tax_due()` – до целых рублей.

Проверки: `cargo test -p trading-common --lib tax` (18 тестов).

## 3. Границы: чего нет и почему

| Нет | Причина | Что делать |
|---|---|---|
| Ордера на MOEX / брокерский адаптер | Money path проекта – Hyperliquid; РФ-брокер требует своей авторизации и подтверждений | Новый `Broker` trait + адаптер (Tinkoff Invest / Alor), затем гейты как у live shell |
| Автозаполнение `unit_cost_rub` | `FxRates` есть, но `LotBook::buy` ждёт готовую RUB-стоимость | Соединить `CbrClient::daily()` → `cost_basis_rub` в отчёте по сделкам |
| Orderbook MOEX | ISS отдаёт HTML-паспорт вместо JSON (нужна авторизация) | `get_orderbook` возвращает `Auth`, а не пустой стакан |
| Сплиты MOEX | Дивиденды распарсены (`MoexClient::dividends`), дробления акций нет | ISS corporate actions + пересчёт лотов |
| Доступ к дивидендам | На части сетей ISS отдаёт HTML вместо JSON | `dividends()` возвращает `Auth`, «0 дивидендов» не врёт |
| Крипто-режим РФ (259-ФЗ) | Правила меняются; майнинг и операции облагаются по-разному | Отдельный `AssetClass::Crypto` уже есть; ставки те же 13/15% – уточнять на дату |
| ИИС-3 «3 года» для старых счетов | Переходные правила зависят от даты открытия | `TaxAccount::Iis3 { opened }` уже принимает дату – добавить правило переходного периода |

## 4. Как считать налог по своим сделкам

```rust
use trading_common::tax::{AnnualTaxReport, LotBook, TaxAccount};
use chrono::{NaiveDate, Utc};

let mut book = LotBook::new();
book.buy(10.0, 250.0, Utc::now())?;          // 10 акций по 250 ₽
let trade = book.sell(4.0, 300.0, Utc::now())?;  // FIFO, 4 акции

let mut report = AnnualTaxReport::new(2026);
report.add_realized(&trade);
report.add_fee(12.0);

let account = TaxAccount::Iis3 { opened: NaiveDate::from_ymd_opt(2022, 3, 1).unwrap() };
let due = account.tax_due_for(&report, NaiveDate::from_ymd_opt(2026, 12, 31).unwrap());
```

## 5. Почему это «про универсальность»

Один и тот же слой считает и крипту, и акции: различается только `AssetClass`
и валюта, а лоты, FIFO и ставки общие. Поэтому РФ-инвестор получает единый отчёт
по обоим контурам, а не две несовместимые таблицы.
