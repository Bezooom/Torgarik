# Live unlock checklist

**Default:** live auto-alpha and live shell `--execute` stay **locked**.  
Unlock only after **every** box below is true. S1 + paper/shell dry always remain available.

## A. Research gates (must be green)

| # | Check | How |
|---|--------|-----|
| A1 | Walk-forward OOS `test_avg_sharpe > 0` | `data/reports/wf_latest.json` or dashboard OOS card |
| A2 | OOS `avg/test PnL > 0` and majority folds green | same report / dashboard `block.reasons` |
| A3 | **Holdout** green (`holdout.total_pnl > 0` and `holdout.avg_sharpe > 0`) | schema 1.2 report |
| A4 | Report fresh (`generated_at` / age &lt; 7d preferred) | dashboard `report_age_human` |
| A5 | Realistic exec in report (`taker_bps≈3.5`, slip/impact &gt; 0) | report `exec` object |
| A6 | Same symbols as paper/live whitelist | `config/*.toml` vs WF preset |
| A7 | Look-ahead audit чист (`look-ahead audit: N series clean`) | прогон без `--no-audit` |
| A8 | Protections включены и осмысленны (`[risk.protections]`) | `config/live.toml` |
| A9 | **Deflated Sharpe** > 0 и **PBO** <= 0.5 | `deflated_sharpe_min`, `pbo_avg` в отчёте |
| A10 | Отчёт воспроизводит pre-registration hash | `--prereg-expect <hash>` |

If any of A1–A3 fail → **stop**. Kill-switch will block `Settings::validate()` in Live and force shell dry.

Reproducible run: add `--as-of YYYY-MM-DD`, чтобы прогон видел только данные,
# существовавшие на ту дату (сборщик продолжает дописывать историю).

```bash
# Re-run realistic WF + holdout
cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
  --holdout-frac 0.15 --taker-bps 3.5 --slip-bps 2 --impact-bps 1 \
  --report data/reports/wf_latest.json

# Or MCP
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_kill_switch_status","arguments":{}}}' \
  | cargo run -p mcp-server --quiet
```

## B. Operational gates

| # | Check | How |
|---|--------|-----|
| B1 | Paper shell ran ≥ several days with sane journal | `cargo run -p ml-trader -- --mode shell` |
| B2 | Dashboard: shell alive, no surprise kill_switch floods | http://127.0.0.1:8080 |
| B3 | Proxy smoke (if RF network needs it) | `HTTPS_PROXY` / `exchange.proxy_url` opt-in only |
| B4 | Sim vs paper protocol reviewed (same window, ≤ ~5% PnL gap target) | [BACKTEST.md](BACKTEST.md) §7 |
| B5 | Paper-книга пережила рестарт без потери позиций | `runtime/paper_state.json` + лог `paper state restored` |
| B6 | Dry-репетиция стопа отработала (`bracket trigger_sl` + `protection_*`) | [RISK_SHELL.md](RISK_SHELL.md) §Dry-книга |
| B7 | Remote stop проверен (`runtime/stop.flag` блокирует входы) | [RISK_SHELL.md](RISK_SHELL.md) §Stop-флаг |
| B8 | Курс ЦБ доступен (tax не считает по вчерашнему молча) | `runtime/fx_cbr.json` |

## C. Keys & mode (Live only)

| # | Check |
|---|--------|
| C1 | `RUN_MODE=live` |
| C2 | `CONFIRM_LIVE=1` (explicit operator confirm) |
| C3 | `WALLET_PRIVATE_KEY` or `HL_API_PRIVATE_KEY` set |
| C4 | `WALLET_ADDRESS` matches key |
| C5 | `allow_synthetic = false` |
| C6 | Risk limits sane (`max_daily_loss`, exposure, whitelist) |

```bash
export RUN_MODE=live
export CONFIRM_LIVE=1
export WALLET_PRIVATE_KEY=0x...
export WALLET_ADDRESS=0x...
# optional: export HTTPS_PROXY=socks5://127.0.0.1:1080

# Advisory shell first (still dry if OOS red)
cargo run -p ml-trader -- --mode shell

# Live shell intents only after A–C green
cargo run -p ml-trader -- --mode shell --execute
```

## D. What stays frozen even after unlock

- Auto-alpha “ATM” mindset – size tiny, journal everything  
- L1 emit live / new bot deposits (see dashboard policy)  
- Multi-venue PM paths (deprecated)

## E. RF-контур (MOEX / налоги)

MOEX – только данные, ордеров нет. Перед любым РФ-использованием прочитать
[docs/RF.md](RF.md): что считается в RUB, как работает НДФЛ и ИИС-3 и чего
в контуре пока нет.

## F. Abort criteria (re-lock immediately)

- New WF/holdout turns red  
- Daily loss kill-switch in journal  
- Connectivity loss / bad proxy  
- Unexpected exchange errors or fill anomalies
- `protection_max_drawdown` сработал (equity -10% от пика) – разобраться до снятия
- Сработал `/stop` (флаг `runtime/stop.flag`) – снимать только после разбора  

On abort: stop `--execute`, set `RUN_MODE=paper` or `dryrun`, keep S1 + shell dry.

## Policy summary

| Mode | Red OOS | Green OOS |
|------|---------|-----------|
| DryRun / Paper shell | allowed | allowed |
| Live `validate()` | **blocked** | needs C1–C5 |
| Live shell `--execute` | **forced dry** | may place if C ok |
| S1 manual | always allowed | always allowed |

See also: [RISK_SHELL.md](RISK_SHELL.md) · [BACKTEST.md](BACKTEST.md) · [DASHBOARD.md](DASHBOARD.md) · [ROADMAP.md](ROADMAP.md)
