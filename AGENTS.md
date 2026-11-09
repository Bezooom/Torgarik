# Trading — agent notes

## Product policy (do not “fix” by enabling live alpha)

- **Money path:** S1 (skill) + **risk shell** on **Hyperliquid**.
- **Live auto-alpha frozen** until OOS + holdout green and [docs/LIVE_UNLOCK.md](docs/LIVE_UNLOCK.md).
- Only Hyperliquid (orders, gated) and MOEX ISS (read-only data). Legacy prediction-market venues were removed in 0.1.0 — do not re-add.

## Do not dump the whole tree

This repo has a huge Rust `target/` (~9G) and often large `node_modules`. A recursive tree on the root produces multi‑MB output and gets **truncated**.

**Prefer:**
- `list_directory` on one path (single level)
- `find path -maxdepth 2` / targeted `grep` / `read` with offset
- exclude: `target`, `.git`, `node_modules`, `.kilo/node_modules`, `data/history` (if large)

**Never** call `directory_tree` on the repo root without excludes.

## Layout (source only)

| Path | Role |
|------|------|
| `crates/common` | Domain types, `Settings`, `OosStats` kill-switch |
| `crates/exchange` | HL client, paper, `dyn Exchange`, factory |
| `crates/strategies` | funding, majors_alpha, risk_gate, Bayesian |
| `crates/ml-trader` | `--mode shell` (default) \| anomaly \| alpha |
| `crates/backtester` | walk-forward + holdout + realism |
| `crates/dashboard` | local ops UI (`/api/status`) |
| `crates/mcp-server` | HL MCP + kill-switch tools |
| `scripts/` | S1, HL history, L1, smoke |
| `docs/` | Operator guides ([docs/README.md](docs/README.md)) |
| `docs/ARCHITECTURE.md` | Layers + invariants |
| `docs/ROADMAP.md` | Phases 0–4 status |
| `config/` | dryrun/live toml |
| `runtime/` | journal, signals, s1, state |
| `data/reports/` | `wf_latest.json` (kill-switch) |
| `README.md` | entrypoint |

Ignore for exploration: `target/`, `.git/`, `node_modules/`.

## Commands agents should know

```bash
cargo test --workspace
cargo check --workspace --all-targets
bash scripts/start.sh                 # shell + dashboard (dry-run)
bash scripts/start.sh status|stop|logs
cargo run -p ml-trader -- --mode shell
cargo run -p dashboard
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --holdout-frac 0.15 --report data/reports/wf_latest.json
```

## Key invariants

1. No production `.unwrap()` on `Option`/`Result` outside tests.
2. Secrets via env only (`WALLET_PRIVATE_KEY`, `CONFIRM_LIVE`).
3. Journal lines include `version`; major mismatch → reject on read.
4. Shell signal offset starts at EOF (do not replay full history).
5. `OosStats::is_blocking()`: Sharpe&lt;0 OR PnL&lt;0 OR holdout red **or non-finite (NaN/Inf)** → live locked.
