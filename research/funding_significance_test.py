#!/usr/bin/env python3
"""
Permutation / randomization significance test for the funding-carry alpha's
out-of-sample (holdout) returns across the coin universe.

Research-only, read-only analysis. No trading logic is changed.

Null hypothesis (H0): the funding-carry cross-sectional edge is noise - the
sign / magnitude of each coin's holdout return is exchangeable under random
relabeling. We build a "noise floor" by permuting signs and bootstrapping coins,
then ask whether the real aggregate holdout performance clears it.

Usage:
    python3 research/funding_significance_test.py \
        data/reports/wf_funding_majors_holdout.json
"""
import json
import sys
import argparse
from collections import Counter
import random
import math

# ---------------------------------------------------------------------------
# STATS
# ---------------------------------------------------------------------------


def mean(xs):
    return sum(xs) / len(xs) if xs else 0.0


def sharpe_of(returns):
    """Annualised-ish Sharpe = mean / std (per-bar, sign of returns)."""
    if not returns:
        return 0.0
    m = mean(returns)
    var = sum((r - m) ** 2 for r in returns) / len(returns)
    sd = math.sqrt(var)
    if sd == 0:
        return 0.0
    return m / sd


# ---------------------------------------------------------------------------
# LOAD
# ---------------------------------------------------------------------------


def load_coin_returns(path):
    """Return list of (coin, per-bar_holdout_pnl_list, total_pnl, sharpe)."""
    with open(path) as f:
        d = json.load(f)
    holdout = d["holdout"]
    out = []
    for c in holdout["coins"]:
        res = c["result"]
        pnl = res["total_pnl"]
        sharpe = res.get("sharpe_ratio", 0.0)
        # bars count as a rough per-coin sample size proxy
        n = c.get("bars", 0)
        out.append((c["coin"], pnl, sharpe, n))
    return out, holdout


# ---------------------------------------------------------------------------
# PERMUTATION TEST - sign flip across the cross-section
# ---------------------------------------------------------------------------


def permutation_sign_flip(coin_data, real_stat, n_perm=20000, seed=1337):
    """
    Randomly flip the sign of each coin's holdout PnL (exchangeability under
    H0). Recompute the cross-sectional sum of PnL and mean |Sharpe|.

    Returns (pval_sum, pval_abs_sharpe, dist_sum, dist_abs_sharpe).
    """
    rng = random.Random(seed)
    pnls = [d[1] for d in coin_data]
    sharps = [abs(d[2]) for d in coin_data]

    dist_sum = []
    dist_abs_sharpe = []
    for _ in range(n_perm):
        s = 0.0
        a = 0.0
        for p in pnls:
            v = p if rng.random() < 0.5 else -p
            s += v
            a += abs(v)
        dist_sum.append(s)
        dist_abs_sharpe.append(a)

    p_sum = sum(1 for x in dist_sum if x >= real_stat["sum_pnl"]) / n_perm
    p_abs = sum(1 for x in dist_abs_sharpe if x >= real_stat["abs_sum_sharpe"]) / n_perm
    return p_sum, p_abs, dist_sum, dist_abs_sharpe


# ---------------------------------------------------------------------------
# BOOTSTRAP TEST - resample coins with replacement
# ---------------------------------------------------------------------------


def bootstrap_coins(coin_data, real_stat, n_boot=20000, seed=1337):
    """Bootstrap-resample coins (with replacement); aggregate holdout PnL."""
    rng = random.Random(seed)
    pnls = [d[1] for d in coin_data]
    n = len(pnls)
    dist = []
    for _ in range(n_boot):
        s = 0.0
        for _ in range(n):
            s += rng.choice(pnls)
        dist.append(s)
    p = sum(1 for x in dist if x >= real_stat["sum_pnl"]) / n_boot
    return p, dist


# ---------------------------------------------------------------------------
# BINOMIAL TEST - fraction of profitable coins
# ---------------------------------------------------------------------------


def binomial_profitable(coin_data, n_coins, n_profitable):
    """
    Under H0 (p=0.5), P(X <= n_profitable) for X ~ Binomial(n_coins, 0.5).
    One-sided: is the fraction of profitable coins surprisingly low?
    """
    from math import comb
    k = n_profitable
    N = n_coins
    p_low = sum(comb(N, j) * (0.5 ** N) for j in range(0, k + 1))
    return p_low


# ---------------------------------------------------------------------------
# EFFECTIVE SAMPLE SIZE (autocorrelation-aware, Newey-West style)
# ---------------------------------------------------------------------------


def effective_df(n, acf_lag1):
    """Newey-West effective degrees of freedom with first-order autocorrelation."""
    denom = 1 + 2 * acf_lag1 * (n - 1) / n
    return n / denom if denom > 0 else n


# ---------------------------------------------------------------------------
# MAIN
# ---------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("report", help="walk-forward report JSON with holdout.coins")
    ap.add_argument("--perm", type=int, default=20000, help="permutation iterations")
    ap.add_argument("--boot", type=int, default=20000, help="bootstrap iterations")
    args = ap.parse_args()

    coin_data, holdout = load_coin_returns(args.report)
    n = len(coin_data)
    real_sum_pnl = sum(d[1] for d in coin_data)
    real_mean_sharpe = mean([d[2] for d in coin_data])
    real_abs_sum_sharpe = sum(abs(d[2]) for d in coin_data)
    n_profitable = sum(1 for d in coin_data if d[1] > 0)
    n_zero = sum(1 for d in coin_data if d[1] == 0)

    print("=" * 72)
    print("FUNDING-CARRY HOLDOUT SIGNIFICANCE TEST (permutation / randomization)")
    print("=" * 72)
    print(f"report      : {args.report}")
    print(f"coins        : {n}   (holdout_frac={holdout['holdout_frac']})")
    print(f"holdout real : total_pnl={holdout['total_pnl']:.2f}  avg_sharpe={holdout['avg_sharpe']:.2f}  profitable={holdout['profitable_coins']}/{holdout['n_coins']}")
    print("-" * 72)
    print("per-coin holdout PnL:")
    for coin, pnl, sharpe, bars in sorted(coin_data, key=lambda x: -x[1]):
        flag = "PROF" if pnl > 0 else ("ZERO" if pnl == 0 else "neg ")
        print(f"  {coin:6s} {flag} pnl={pnl:9.2f}  sharpe={sharpe:7.2f}  bars={bars}")
    print("-" * 72)

    real_stat = {
        "sum_pnl": real_sum_pnl,
        "abs_sum_sharpe": real_abs_sum_sharpe,
    }

    # 1) Permutation sign-flip (cross-section exchangeability)
    p_sum, p_abs, _, _ = permutation_sign_flip(coin_data, real_stat, args.perm)
    print(f"[1] PERMUTATION sign-flip ({args.perm} iters)")
    print(f"    real sum PnL   = {real_sum_pnl:9.2f}   -> p(>= real) = {p_sum:.4f}")
    print(f"    real |sum Sharpe| = {real_abs_sum_sharpe:8.2f} -> p(>= real) = {p_abs:.4f}")

    # 2) Bootstrap coins (with replacement)
    p_boot, dist_b = bootstrap_coins(coin_data, real_stat, args.boot)
    med = sorted(dist_b)[len(dist_b) // 2]
    print(f"[2] BOOTSTRAP coins ({args.boot} iters)")
    print(f"    real sum PnL = {real_sum_pnl:9.2f}  | bootstrap median = {med:9.2f}")
    print(f"    p(>= real)   = {p_boot:.4f}")

    # 3) Binomial test on fraction of profitable coins
    p_bin = binomial_profitable(coin_data, n, n_profitable)
    print(f"[3] BINOMIAL (p=0.5) profitable coins: {n_profitable}/{n}")
    print(f"    P(X <= {n_profitable}) = {p_bin:.4f}  (one-sided, low fraction)")
    print(f"    zero-return coins      : {n_zero} (no trade in holdout)")

    # 4) Effective sample size (Newey-West) - rough acf from per-coin signs
    #    use the count of profitable as a proxy series for autocorrelation check
    signs = [1.0 if d[1] > 0 else (-1.0 if d[1] < 0 else 0.0) for d in coin_data]
    m = mean(signs)
    num = sum((signs[i] - m) * (signs[i + 1] - m) for i in range(len(signs) - 1))
    den = sum((s - m) ** 2 for s in signs)
    acf1 = num / den if den > 0 else 0.0
    eff = effective_df(n, acf1)
    print(f"[4] CROSS-SECTION autocorr(lag1) = {acf1:+.3f}  -> effective N ~ {eff:.1f} (vs {n})")

    print("=" * 72)
    verdict = "EDGE NOT SIGNIFICANT (likely noise / data-artifact)"
    if p_sum >= 0.05 and p_boot >= 0.05 and p_bin >= 0.05 and real_sum_pnl > 0:
        verdict = "EDGE MAY BE REAL - clears noise floor"
    elif real_sum_pnl > 0 and (p_sum < 0.05 or p_boot < 0.05):
        verdict = "EDGE POSITIVE but does NOT clear strict noise floor - fragile"
    print(f"VERDICT: {verdict}")
    print("=" * 72)


if __name__ == "__main__":
    main()
