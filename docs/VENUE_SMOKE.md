# Venue smoke-test (RF operators)

Перед рефакторингом / live-депозитом заполните матрицу **с вашего IP** (без Cloudflare WARP, если возможно — и отдельно с ним).

> ⚠️ Hyperliquid относит Россию к Restricted Territories в своих условиях. Проверка доступности — не разрешение торговать.

## Зачем

| Venue | Роль в v1 |
|-------|-----------|
| **Hyperliquid** | Primary money path (perps) |

**Правило:** money path = venue, где **place + cancel** проходят. Read-only недостаточно.

## Быстрый скрипт

```bash
# из корня репо, direct IP (без proxy)
bash scripts/smoke_venue.sh

# опционально через proxy
HTTPS_PROXY=socks5://127.0.0.1:1080 bash scripts/smoke_venue.sh
```

Скрипт проверяет **только read** (HTTP info/markets).  
Place/cancel — **вручную** в UI или через tiny signed order (после PR2).

## Матрица (заполнить)

```text
venue        | IP mode       | read | place | cancel | notes
Hyperliquid  | RU direct     |      |       |        |
Hyperliquid  | NL+CF proxy   |      |       |        |
```

Легенда: `OK` / `FAIL` / `N/A`.

## Hyperliquid manual place/cancel

1. Кошелёк (отдельный hot-кошелёк).
2. UI: [https://app.hyperliquid.xyz](https://app.hyperliquid.xyz) — депозит минимальный USDC.
3. Открыть крошечную позицию BTC/ETH perp → reduce/close.
4. Записать: получилось ли с RU direct.

## Решение по результатам

| Результат | Действие |
|-----------|----------|
| HL place OK | Primary = Hyperliquid (план как есть) |
| Оба fail | Spike Bybit/OKX API (вне v1 plan) |

## Зафиксированные решения продукта

- Proxy **opt-in only** (env), без auto-WARP.
- Live требует `CONFIRM_LIVE=1` + wallet key.
- ML/LLM default off.
