# Дорожная карта Trading (S1-first)

## Status (2026-08)

| Phase | Theme | Status |
|-------|--------|--------|
| 0 | Green build / HL domain | **Done** |
| 1 | Risk shell + journal | **Done** |
| 2 | Dashboard + MCP observability | **Done** |
| 3 | Backtest realism + holdout | **Done** |
| 4 | Kill-switch e2e + live checklist | **Done** |
| 5 | Ops automation: WF cron-refresh + Telegram alerts | **Done** |
| 6 | Shield hardening + point-in-time + RF core | **Done** (2026-11) |

**Advisory production path (S1 + shell):** ready.  
**Live auto-alpha:** **closed** – see [LIVE_UNLOCK.md](LIVE_UNLOCK.md).  
Quality: `cargo test --workspace` green.

---

## 1. Strategy

**Advisory Copilot & Risk Shell + S1** – не «бот-ATM».

1. **S1 / manual** – primary income; bot = shield + microscope.
2. **Live auto-alpha frozen** until walk-forward OOS is green (Sharpe > 0 and PnL > 0) on a realistic simulator **and** holdout green.
3. **Hyperliquid only** for money path. Legacy PM venues stay compiled but `#[deprecated]`.

---

## 2. Phase 0 – Green build (done / keep green)

**DoD:** `cargo check --workspace --all-targets` clean of errors.

| Task | Status |
|------|--------|
| Domain types (`PortfolioSnapshot`, `Quote`, `OrderRequest`, …) in `common` | Done |
| Export HL stack from `exchange` (`factory`, `traits`, `paper`, …) | Done |
| `mcp-server` → Hyperliquid tools (no MultiExchange money path) | Done |
| `wf_cli` format string + win rate display | Done |
| Safe `partial_cmp` sort in `rank_by_funding` | Done |
| Legacy PM clients marked `#[deprecated]` | Done |

---

## 3. Phase 1 – S1 Risk Shell hardening (weeks 1–2)

**Goal:** reliable shield for **manual** HL ops.

| Task | Status |
|------|--------|
| Journal `version` + major-compat fail-soft (`UnsupportedVersion`) | Done |
| Event `oos_status`; flush on write | Done |
| RiskShell: `check_order`, whitelist, size/mid gates | Done |
| OOS forces dry on live+execute (`force_dry_run`) | Done |
| HL `nonce` without panic; funding sort safe | Done |
| Unit tests journal + dry policy + risk_gate | Done |
| Smoke: `ml-trader --mode shell` | Operator / CI |

**Gate → Phase 2:** shell runs end-to-end; journal survives restart; live still locked while OOS red.

---

## 4. Phase 2 – Observability & MCP (weeks 2–3)

| Task | Status |
|------|--------|
| Dashboard block panel (`block.*` reasons) | Done |
| OOS age / mtime / stale + `oos_blocking` | Done |
| RiskBlock / kill_switch / oos_status in API + UI | Done |
| S1 hours on real `/api/status` UI | Done |
| Replace fake “quantum terminal” SPA with ops UI | Done |
| MCP `get_kill_switch_status` + `get_shell_status` | Done |
| Telegram/Slack | Deferred (journal = backup) |

**Gate:** operator can answer “am I blocked and why?” from dashboard alone.

---

## 5. Phase 3 – Research realism (weeks 3–5)

| Task | Status |
|------|--------|
| Fees HL taker 3.5 bps (default) | Done |
| Base slip + volume impact √participation | Done |
| Queue delay bars | Done |
| Report schema 1.2: + deflated_sharpe, pbo, prereg, n_trials | Done |
| Holdout tail outside grid-search | Done |
| Sim vs paper protocol in BACKTEST.md (≤5% target) | Done |
| Copytrader HL + BetaBelief::decay | Dropped (legacy PM copytrader removed in 0.1.0) |

**Gate:** sim vs paper protocol documented; holdout + realistic exec in WF reports.

---

## 6. Phase 4 – Integration gates (weeks 5–6)

| Task | Status |
|------|--------|
| E2E policy tests: red OOS → live blocked, paper/shell allowed | Done |
| Holdout red participates in `OosStats::is_blocking` | Done |
| `validate_with_oos` injectable for tests | Done |
| Parallel HL `refresh_market_data` (`tokio::join!`) | Done |
| Live unlock checklist | Done → [LIVE_UNLOCK.md](LIVE_UNLOCK.md) |

**Gate “production-ready advisory”:** S1+shell stable (Phases 0–2).  
**Gate “auto-alpha live”:** only after [LIVE_UNLOCK.md](LIVE_UNLOCK.md) – currently **closed** (OOS/holdout red).

---

## 7. KPIs

| Gate | Criteria | Status |
|------|----------|--------|
| G0 Build | `cargo check --workspace --all-targets` no errors | Done |
| G1 Shell | RiskShell paper cycle + journal + OOS display | Done |
| G2 Ops | Dashboard answers block/reason without reading code | Done |
| G3 Research | Realistic fees/slippage; report metadata; holdout | Done |
| G4 E2E | Automated kill-switch tests; live locked on red OOS | Done |

### Phase 6 - shield hardening + point-in-time + RF core (done)

| Task | Status |
|------|--------|
| `protections`: StoplossGuard / Cooldown / MaxDrawdown, persisted, gated before `check_order` | Done |
| Dry-книга: SL/TP репетируются в dry (раньше брекет умирал без позиции) | Done |
| `runtime/paper_state.json`: paper-книга переживает рестарт + сверка брекетов | Done |
| Look-ahead audit: prefix-replay инвариант, `--no-audit` только для research | Done |
| `--as-of`: point-in-time срез истории для воспроизводимого прогона | Done |
| `MarketData` отделён от `Exchange` (read-only площадки без пути ордера) | Done |
| `exchange::moex`: MOEX ISS read-only (quote / list / orderbook / candles) | Done |
| `common::tax`: FIFO-лоты в RUB, НДФЛ 13/15% (2.4 млн), ИИС-3 | Done |
| `stats`: probabilistic/deflated Sharpe, expected max SR, CSCV PBO | Done |
| Kill-switch читает `deflated_sharpe_min` / `pbo_avg` (schema 1.2) | Done |
| `strategies::signals`: один генератор для бэктеста и shell (`--mode signals`) | Done |
| Pre-registration hash + `--prereg-only` / `--prereg-expect` | Done |
| `common::fx` + `exchange::cbr`: курс ЦБ с кэшем на дату | Done |
| MOEX дивиденды (ISS, при отказе – честная ошибка доступа) | Done |
| Remote stop flag + Telegram control (`/status` `/block` `/stop` `/resume`) | Done |

### Next (optional backlog)

- Tinkoff Invest / Alor адаптер как `MarketData` + `Broker` trait (нужны боевые ключи)
- Сплиты и делистинг MOEX (дивиденды уже есть)
- Автозаполнение `unit_cost_rub` из `FxRates` в отчёте по сделкам
- Единый event-driven движок для бэктеста и paper (сейчас общий только генератор сигналов)
- (optional) HL wallet follower built on `BetaBelief::decay`
- Funding-carry (delta-neutral spot/perp) prototype in paper
- Slack alerts (Telegram done via `scripts/tg_notify.sh`)
- Cache consolidation / DashMap (perf only)
- Stale-report hard reject by age in `OosStats`

---

## 8. Risks

| Risk | Level | Mitigation |
|------|-------|------------|
| Overfit “green” OOS | High | Holdout + multi-fold; don’t unlock on one 60d grid |
| Stale `wf_latest` | Med | Timestamp/strategy in report; reject too-old files later |
| Overfit «зелёный» Sharpe после grid-search | High | Look-ahead audit + holdout; deflated Sharpe в бэклоге |
| Dry-репетиция не отражает реальные филлы | Low | Dry-книга повторяет paper-путь, но без слиппеджа биржи |
| API/geo failures | Med | Opt-in proxy; timeouts; no panic on bad JSON |
| Alert outage | Low | Journal is source of truth |

---

## 9. Explicit deprioritization

- Exporting / investing in `MultiExchangeManager` as product surface
- JSON Schema tooling before simple journal versioning
- BinaryHeap / DashMap as phase blockers
- Live auto-alpha feature work while OOS is red
