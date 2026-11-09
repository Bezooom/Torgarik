# Pre-registration — RF low-turnover hypotheses (written 2026-11-09, before any backtest result was computed)

Periods (fixed): IS = start..2020-12-31; OOS = 2021-01-01..2024-12-31; HOLDOUT = 2025-01-01..latest.
No parameter is tuned on anything. All parameters below are fixed a priori (textbook defaults).
Signals use data up to close t, executed at close t+1 (1-day lag). Rebalance weekly (every 5th trading day) unless stated.

## Common trend signal ("BLEND")
s = mean( sign(r_63), sign(r_126), sign(r_252) ) where r_n = log return over n trading days. s in {-1,-1/3,1/3,1}.
Secondary (info only, not scored): SMA50 > SMA200 -> +1 else -1.
Vol: sigma = std of daily log returns over 60 days * sqrt(252) (crypto: sqrt(365)).

## H1 MOEX futures daily trend (long/short)
Assets: Si, RI, BR, GD, MX(MIX/MXI old+new codes as available), NG (if available). Continuous back-adjusted (ratio) series,
roll to next contract when its volume exceeds front volume, or 5 trading days before expiry at the latest.
Weight w_i = s_i * (0.15/sqrt(N_active)) / sigma_i, |w_i| <= 3. Costs per side: 2 bps + 1 tick/price; rolls charged both legs.
Futures P&L is excess return (collateral interest not added) -> hurdle = 0.

## H2 MOEX shares cross-sectional momentum (long only)
Universe each month-end: TQBR shares with month-end traded value; rank by mean traded value over last 3 month-end snapshots,
top 35 that have >=13 months of history. Momentum = P(t-1m)/P(t-12m)-1. Long top tercile, equal weight, monthly rebalance.
Costs 0.05% commission + 0.05% slippage per side on turnover. Price-only (no dividends); benchmark IMOEX price index (also price-only).
Edge = strategy CAGR - IMOEX CAGR.

## H3 Crypto daily trend
Binance spot USDT pairs, fixed list (liquid at start of 2019): BTC ETH BNB XRP LTC ADA TRX EOS XLM ETC.
Primary (scored): long/flat, w_i = max(s_i,0) * (0.20/sqrt(N_active))/sigma_i, capped at 1/N_active each (no leverage).
Secondary (info): long/short same weights. Costs 0.10% per side. Hurdle = 0 (cash in USDT, no yield added).

## H4 Crypto funding carry
Binance USD-M perps + spot: BTC ETH BNB XRP LTC ADA. Daily decision: if trailing 7-day mean funding (annualised, 3x/day) > 10%/yr
-> hold long spot / short perp (equal notional 1/N of capital per coin), else flat. P&L = spot ret - perp ret + funding received.
Costs per enter/exit: spot taker 0.10% + perp taker 0.05% + 0.02% slippage per leg. Hurdle = 0.

## Metrics (per hypothesis, per period)
CAGR, Sharpe (rf=0), max DD, #trades (position entries/exits/flips per asset, incl. rebalances that change sign; H2: names bought/sold),
annual turnover, net edge vs hurdle/benchmark, % positive calendar years, stationary block bootstrap (block 20 days / 3 months for H2)
1000 resamples -> 5th pct Sharpe. Special windows: 2020-02-15..2020-04-30 (covid), 2022 calendar year (MOEX closed 2022-02-25..2022-03-23:
returns across the gap are taken as one jump on reopen, no trading inside).

## Scoring (OOS period, 1-5 each, total /25)
Sharpe: <0:1, 0-0.3:2, 0.3-0.6:3, 0.6-1.0:4, >=1:5
Calmar: <0:1, 0-0.2:2, 0.2-0.5:3, 0.5-1:4, >=1:5
MaxDD: >50%:1, 35-50:2, 25-35:3, 15-25:4, <15%:5
Trades (OOS): <30:1, 30-60:2, 60-120:3, 120-250:4, >=250:5
After-cost edge (ann.): <0:1, 0-2%:2, 2-5%:3, 5-10%:4, >=10%:5
PASS = total >= 18 AND trades(OOS+holdout) >= 30 AND net edge > 0 in BOTH OOS and holdout.
Only primary variants are scored.
