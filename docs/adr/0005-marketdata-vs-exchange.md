# 0005. Чтение отделено от торговли на уровне типов

**Status:** Accepted

## Context

Трейт `Exchange` требовал `place_order` и `portfolio`. Любая новая площадка
вынуждена была либо реализовывать ордера, либо возвращать заглушки-ошибки. MOEX
через ISS – источник данных без брокера; реализовывать для него «ордера, которые
не работают» значит врать типам.

## Decision

Две поверхности:

- `MarketData` – `venue`, `list_instruments`, `get_quote`, `get_orderbook`;
- `Exchange` – деньги: плюс `place_order`, `cancel_order`, `portfolio`.

`MarketData` автоматически реализуется для любого `Exchange` через blanket impl,
поэтому существующий код не менялся. Площадка без ордеров реализует только
`MarketData` и доступна как `dyn MarketData`.

## Consequences

- `factory::build_exchange` для `venue = "moex"` возвращает ошибку конфигурации,
  а не брокера-заглушку.
- Research, dashboard и бэктестер могут принимать `dyn MarketData`.
- Появляется риск двух похожих трейтов; правило: новый read-only источник идёт в
  `MarketData`, и только площадка с реальными ордерами – в `Exchange`.

## Verification

- `client_has_expected_venue_and_board_path` (MOEX реализует `MarketData`).
- `cargo check --workspace --all-targets` – существующие реализации не изменились.
