# Pre-registration round 2 — crypto (written 2026-11-09 before any round-2 backtest was run)

Periods (fixed): IS = start..2022-12-31; OOS = 2023-01-01..2024-12-31; HOLDOUT = 2025-01-01..latest.
Nothing tuned. Daily UTC bars. Decision at close t, executed at close t+1 (lib.run_weights), ann=365.
Data: Binance USD-M perpetual daily klines + funding history (REST), symbol list from data.binance.vision bucket
(includes delisted symbols). Only USDT-margined perps; exclude stablecoin/fiat bases (USDC BUSD TUSD FDUSD USDP DAI EUR GBP).
Funding P&L: position w pays w*f each day (f = sum of that UTC day's funding prints); shorts receive positive funding.
Costs: 0.05% taker + 0.05% slippage = 0.10% per side on |dw|.
Vol: sigma_i = 60-day std of daily log returns * sqrt(365).

Point-in-time universe (C1, C2): at each rebalance, top 25 perps by trailing 30-day mean quote volume,
requiring >= 100 days of history and non-zero volume on each of the last 5 days.

## C1 Cross-sectional momentum (primary = long/short)
Score = average of cross-sectional ranks of 7d, 30d and 90d past returns. Long top quintile (5), short bottom quintile (5).
w_i = ±(0.20/sqrt(10))/sigma_i, |w_i| <= 0.25. Rebalance every 7 days.
Variant (info, not scored): long-only top quintile, w_i=(0.20/sqrt(5))/sigma_i, cap 0.25, sum<=1.

## C2 Cross-sectional funding carry (primary = long/short)
Score = trailing 3-day mean daily funding. Short top quintile (highest funding), long bottom quintile (lowest).
w_i = ±(0.20/sqrt(10))/sigma_i, cap 0.25. Rebalance every 3 days. P&L = price P&L + funding P&L - costs.

## C3 H3 trend with funding filter (primary = long/flat)
Universe = H3 list (BTC ETH BNB XRP LTC ADA TRX EOS XLM ETC), spot prices, same BLEND signal and weights as H3 long/flat
(w=max(s,0)*(0.20/sqrt(N))/sigma, cap 1/N), weekly rebalance.
Funding feature: 7-day mean of daily funding of the coin's perp; percentile vs its own trailing 365 days (min 180 obs).
Rule: if percentile >= 0.90 (crowded longs) -> long weight halved. Otherwise unchanged.
Variant (info): additionally go short (same size as a long would be) when BLEND s<0 AND percentile >= 0.90; shorts pay/receive funding.
Benchmark comparison: plain H3 long/flat on the same periods.

## Metrics and scoring: as prereg.md (lib.metrics / lib.score), scored on OOS 2023-2024.
PASS = total >= 18 AND trades(OOS+holdout) >= 30 AND net return > 0 in BOTH OOS and holdout. Hurdle = 0.
