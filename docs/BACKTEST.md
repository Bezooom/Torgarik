# Funding history + walk-forward backtest

**Status:** Phase 3 realism is live — fees **3.5 bps**, base slip + volume **impact**, optional **queue delay**, **holdout** tail, report schema **1.1** (`generated_at`, `exec`, `window`, `holdout`).  
Kill-switch (RiskShell / dashboard / MCP) reads `data/reports/wf_latest.json`. Live unlock: [LIVE_UNLOCK.md](LIVE_UNLOCK.md).

## Pipeline

```text
collect_hl_history.py  →  data/history/*_1h.csv|json
                       →  data/snapshots/universe_hourly.jsonl  (cron)
                              ↓
cargo run -p backtester -- --walk-forward --holdout-frac 0.15 \
  --taker-bps 3.5 --slip-bps 2 --impact-bps 1 \
  --report data/reports/wf_latest.json
                              ↓
OOS + holdout verdict → kill-switch + dashboard (not auto live)
```

## 1. Collect real HL data

```bash
# ~14 days fundingHistory + 1h candles for majors + top |funding|
python3 scripts/collect_hl_history.py --days 14 --auto-top 10

# specific coins
python3 scripts/collect_hl_history.py --days 30 --coins BTC,ETH,ACE,SOL --auto-top 0
```

API sources:

- `fundingHistory` — historical funding rates  
- `candleSnapshot` interval `1h` — mid proxy (close)  
- live `metaAndAssetCtxs` — universe snapshot for scanner research  

## 2. Hourly cron (optional)

```bash
bash scripts/install_hl_cron.sh
# or manually:
# 5 * * * * cd /path/to/Trading && python3 scripts/collect_hl_history.py --cron --days 14 >> logs/hl_collect.log 2>&1
```

Cron appends compact lines to `data/snapshots/universe_hourly.jsonl` and refreshes series.

## 3. Walk-forward backtest

```bash
cargo run -p backtester -- --walk-forward --data-dir data/history --folds 3 --capital 10000

# report JSON
cargo run -p backtester -- --walk-forward --report data/reports/wf_latest.json

# fixed threshold full sample
cargo run -p backtester -- --walk-forward --enter 0.00004 --coins BTC,ETH
```

### What walk-forward does

For each coin and fold:

1. **Train** window: grid-search `funding_enter` for best train PnL  
2. **Test** (OOS): apply that enter on the next period  
3. Aggregate **test** metrics only for the verdict  

Suggested `funding_enter` = median of fold picks → put into `config/dryrun.toml`.

## 4. Tight filters (B)

```bash
# Collect majors only
python3 scripts/collect_hl_history.py --majors --days 14

# Walk-forward majors preset
cargo run -p backtester -- --walk-forward --preset majors --report data/reports/wf_majors.json

# Compare old toxic set
cargo run -p backtester -- --walk-forward --preset meme --report data/reports/wf_meme.json
```

Live/paper config (`config/dryrun.toml`):

- `universe_mode = "allowlist"`
- `symbol_whitelist` = majors
- `blacklist` = memes
- `min_oi_notional_usd`, `max_abs_funding`, `max_positions`

## 5. Majors alpha (C) — trend + funding veto

```bash
# OOS trend (default alpha)
cargo run -p backtester -- --walk-forward --alpha trend --preset majors \
  --report data/reports/wf_trend_majors.json

# Compare old funding carry
cargo run -p backtester -- --walk-forward --alpha funding --preset majors \
  --report data/reports/wf_funding_majors.json

# Paper with majors_alpha
cargo run -p ml-trader -- --mode dryrun
# or force funding scanner:
cargo run -p ml-trader -- --mode dryrun --strategy funding_scanner
```

Signal: EMA(fast) vs EMA(slow) on mid.  
Veto: no long if funding ≥ `funding_veto`; no short if funding ≤ −veto.

## 6. Long history + core OOS

```bash
# fundingHistory is paginated (~500/page) — collector walks startTime forward
python3 scripts/collect_hl_history.py --coins BTC,ETH,SOL --days 60

# strict core
cargo run -p backtester -- --walk-forward --alpha trend --coins BTC,ETH,SOL --folds 5 \
  --report data/reports/wf_trend_btc_eth_sol_60d.json

cargo run -p backtester -- --walk-forward --alpha funding --coins BTC,ETH,SOL --folds 5 \
  --report data/reports/wf_funding_btc_eth_sol_60d.json

# preset core == BTC,ETH,SOL
cargo run -p backtester -- --walk-forward --alpha trend --preset core --folds 5
```

Reports: `data/reports/wf_trend_btc_eth_sol_60d.json`, `wf_funding_btc_eth_sol_60d.json`.

## 7. Realistic execution (Phase 3)

Defaults in `ExecConfig`:

| Param | Default | Meaning |
|-------|---------|---------|
| `taker_bps` | **3.5** | HL taker fee |
| `slip_bps` | 2 | base half-spread / adverse |
| `impact_bps` | 1 | extra × √(order_notional / bar_volume) |
| `next_bar` | true | signal on close → fill next bar |
| `queue_delay_bars` | 0 | extra latency after next-bar |
| `default_bar_volume_usd` | 5e6 | when history has no volume |
| `embargo` | 2 | gap train → test |
| `holdout_frac` | **0.15** | **tail never used in grid-search** |

```bash
# ETH+SOL realistic WFO + holdout
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --folds 5 --embargo 2 --slip-bps 2 --impact-bps 1 --taker-bps 3.5 \
  --holdout-frac 0.15 --queue-delay 0 \
  --report data/reports/wf_latest.json

# Optimistic (no cost) for comparison only
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --folds 5 --slip-bps 0 --impact-bps 0 --taker-bps 0 --holdout-frac 0 \
  --report data/reports/wf_trend_ethsol_optimistic.json
```

### Report schema `1.1`

JSON includes:

- `schema_version`, `generated_at`, `strategy` / `alpha`
- `window` { n_folds, train_frac, expanding, embargo_bars, holdout_frac }
- `exec` { slip_bps, impact_bps, taker_bps, next_bar, queue_delay_bars, … }
- `folds[]` train/test metrics
- `holdout` optional pure holdout summary (params frozen from WF)
- `realism_notes[]`

### Sim vs paper protocol (target |ΔPnL| ≤ 5%)

Same:

1. symbols + capital + `risk_fraction`  
2. fees 3.5 bps taker  
3. base slip + impact model  
4. next-bar (and same `queue_delay` if used)  
5. time window (use holdout dates or a fixed calendar range)

Paper (`PaperExchange`) uses real mids + `PaperFees.taker_bps=3.5`. Compare paper journal PnL to backtest on the **same** bars — not against WF train metrics.

## 8. Go / no-go

| Result | Action |
|--------|--------|
| OOS total PnL ≤ 0 **or** Sharpe &lt; 0 | Do not live (kill-switch) |
| Holdout red while WF green | Do not unlock live |
| OOS > 0 but most folds red | Fragile — paper only |
| WF + holdout green majority | Paper 1–2 weeks, then tiny live |

**Caveats:** past funding ≠ future; volume often missing → default notional impact; alts can gap through stop.

## 9. Защита от самообмана (schema 1.2)

| Механизм | Что ловит | Как проверить |
|---|---|---|
| `look-ahead audit` | сигнал, зависящий от будущего бара | `look-ahead audit: N series clean` в логе |
| `--as-of YYYY-MM-DD` | «мир на дату» для воспроизводимого прогона | `point-in-time cut ... dropped N bars` |
| `deflated_sharpe` | победитель grid-search, неотличимый от удачи | `deflated_sharpe: min=... avg=...` |
| `pbo` (CSCV) | in-sample победитель, который не выживает OOS | `pbo: 0.76` |
| `prereg_hash` | гипотеза или данные изменились после факта | `--prereg-expect <hash>` → exit 4 |

Порядок честной работы:

```bash
# 1. зафиксировать гипотезу ДО просмотра результата
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol --prereg-only

# 2. прогнать и проверить воспроизводимость
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --prereg-expect sha3:... --report data/reports/wf_latest.json
```

Kill-switch читает `deflated_sharpe_min` и `pbo_avg`: `DSR <= 0` или `PBO > 0.5`
блокируют live независимо от сырого Sharpe. Отчёты без этих полей читаются
по-старому, поэтому старый `wf_latest.json` не ломает shell.

Единственный генератор сигналов – `trading_strategies::signals`. Бэктестер и
`ml-trader --mode signals` вызывают его же, поэтому «стратегия, прошедшая WF» и
«стратегия, которая пишет сигналы» – один и тот же код.

## Files

| Path | Role |
|------|------|
| `scripts/collect_hl_history.py` | collector |
| `scripts/install_hl_cron.sh` | user crontab |
| `data/history/{COIN}_1h.*` | series |
| `data/history/manifest.json` | last collect meta |
| `data/snapshots/universe_hourly.jsonl` | hourly top-funding log |
| `crates/backtester` | engine + walk-forward |
