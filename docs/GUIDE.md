# Guide — Hyperliquid risk stack (RF-ready)

> **Актуально (Phases 0–4 done):** primary venue = **Hyperliquid**.  
> Default path = **S1** (skill income) + **risk shell** (journal, no auto-alpha).  
> Live auto-alpha **frozen** until [LIVE_UNLOCK.md](LIVE_UNLOCK.md).  
> Only Hyperliquid (perps) and MOEX ISS (read-only data) are supported.

Smoke: [VENUE_SMOKE.md](VENUE_SMOKE.md) · Ops UI: [DASHBOARD.md](DASHBOARD.md) · Architecture: [ARCHITECTURE.md](ARCHITECTURE.md)

---

## 0. Mental model

| Layer | Role | Money? |
|-------|------|--------|
| **S1 skill work** | Freelance / job / products | **Primary ₽** |
| **Risk shell** | Kill-switch, signals inbox, versioned journal | Protect capital |
| **Dashboard / MCP** | “Am I blocked and why?” | Ops visibility |
| **Anomaly / L1 paper** | Research hobby | Not KPI |
| **Alpha live** | Frozen until OOS + holdout green | No (today) |

OOS under realistic costs (3.5 bps taker, slip, impact, holdout) is currently **red**. See [BACKTEST.md](BACKTEST.md).

### Kill-switch policy

| Mode | Red OOS / holdout | Green OOS |
|------|-------------------|-----------|
| DryRun / Paper shell | allowed | allowed |
| Live `Settings::validate` | **blocked** | needs `CONFIRM_LIVE` + keys |
| Live shell `--execute` | **forced dry** | may place if checklist ok |
| S1 manual | always | always |

Rule: `avg_sharpe < 0` **OR** `avg_pnl < 0` **OR** holdout red → block live.

---

## 1. Quick start (recommended)

### 1.1 Dashboard

```bash
cargo run -p dashboard
# → http://127.0.0.1:8080
```

Block panel, S1 hours, OOS age/Sharpe/PnL, risk_blocks, journal, equity from heartbeats. **No orders.**

### 1.2 Risk shell

```bash
cargo run -p ml-trader -- --mode shell
```

- Paper/dry by default  
- Journal: `runtime/journal/journal_YYYYMMDD.jsonl` (`version: "1.0"`)  
- Signals: only lines **appended after shell start**  
- Whitelist + `check_order` + OOS force-dry on live execute  

```json
{"coin":"ETH","side":"buy","size":0.05,"note":"manual","source":"me"}
{"coin":"ETH","side":"flat"}
```

Full: [RISK_SHELL.md](RISK_SHELL.md)

### 1.3 S1 daily loop

```bash
python3 scripts/s1_daily_log.py --hours 3 --track skill --shell --note "what you shipped"
python3 scripts/s1_status.py
python3 scripts/s1_weekly_review.py   # Sunday
```

Rules: skill ≥15h/week, shell ≥5d/week, **live bot loss = 0**.  
→ [S1_SHELL_INCOME.md](S1_SHELL_INCOME.md)

### 1.4 MCP (optional agents)

```bash
cargo run -p mcp-server
# tools: get_kill_switch_status, get_shell_status, get_quote, rank_funding, get_portfolio, …
```

---

## 2. Research modes (optional)

### Anomaly logger

```bash
cargo run -p ml-trader -- --mode anomaly
python3 scripts/collect_hl_history.py --coins BTC,ETH,SOL --days 60
python3 scripts/analyze_hypotheses.py --coins ETH,SOL,BTC
```

→ [ANOMALIES.md](ANOMALIES.md)

### L1 leaderboard (filter; emit live frozen under S1)

```bash
python3 scripts/l1_leaderboard.py --top 30
python3 scripts/l1_watch_wallets.py --once --top 10
```

→ [L1_LEADERBOARD.md](L1_LEADERBOARD.md)

### Backtest (truth serum)

```bash
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --folds 3 --embargo 2 --slip-bps 2 --impact-bps 1 --taker-bps 3.5 \
  --holdout-frac 0.15 \
  --report data/reports/wf_latest.json
```

Schema **1.1** report feeds kill-switch + dashboard.  
→ [BACKTEST.md](BACKTEST.md)

---

## 3. Config & keys

| Item | Path / env |
|------|------------|
| Dry-run config | `config/dryrun.toml` |
| Live config | `config/live.toml` (careful) |
| Run mode | `RUN_MODE=dryrun\|paper\|live` |
| Wallet (live only) | `WALLET_PRIVATE_KEY` / `HL_API_PRIVATE_KEY`, `WALLET_ADDRESS` |
| Explicit live confirm | `CONFIRM_LIVE=1` + shell `--execute` |
| OOS report override (tests) | `WF_REPORT_PATH` |
| Proxy (opt-in only) | `HTTPS_PROXY` / `ALL_PROXY` |

**S1 rule:** no new bot deposits; no live alpha without [LIVE_UNLOCK.md](LIVE_UNLOCK.md).

```bash
bash scripts/smoke_venue.sh
```

→ [VENUE_SMOKE.md](VENUE_SMOKE.md)

---

## 4. Layout

```text
crates/common        domain, Settings, OosStats
crates/exchange      Hyperliquid + Paper + factory (legacy PM deprecated)
crates/strategies    funding, majors_alpha, risk_gate, Bayesian
crates/ml-trader     shell | anomaly | alpha
crates/backtester    walk-forward + holdout + realism
crates/dashboard     local ops UI
crates/mcp-server    HL tools + kill-switch
runtime/journal/     versioned JSONL
runtime/signals.jsonl
runtime/s1/          skill hours
data/reports/        wf_latest.json (kill-switch source)
data/history/        HL 1h series
docs/                this tree
```

---

## 5. Doc index

| Doc | Topic |
|------|--------|
| [S1_SHELL_INCOME.md](S1_SHELL_INCOME.md) | Primary 30-day money path |
| [RISK_SHELL.md](RISK_SHELL.md) | Shell + signals + journal |
| [DASHBOARD.md](DASHBOARD.md) | Ops UI + `/api/status` |
| [LIVE_UNLOCK.md](LIVE_UNLOCK.md) | Live unlock checklist |
| [BACKTEST.md](BACKTEST.md) | OOS / holdout / fees / sim↔paper |
| [ANOMALIES.md](ANOMALIES.md) | Structural hunts |
| [L1_LEADERBOARD.md](L1_LEADERBOARD.md) | Wallet filter |
| [MONEY_PATH_RF.md](MONEY_PATH_RF.md) | RF constraints |
| [VENUE_SMOKE.md](VENUE_SMOKE.md) | Connectivity |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Design + debt |
| [ROADMAP.md](ROADMAP.md) | Phases 0–4 (done) |

---

## 6. Quality gates

```bash
cargo test --workspace
cargo check --workspace --all-targets
```

Phases 0–4 (build, shell/journal, observability, backtest realism, e2e kill-switch) are **done**.  
Live auto-alpha remains closed until research + ops checklist is green.

---
