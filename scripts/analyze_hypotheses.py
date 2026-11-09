#!/usr/bin/env python3
"""
Offline tests for structural hypotheses on data/history/*_1h.csv

H1 — Funding trap:
  After |funding| > thr, does mid move *against* the funding-side over H hours?
  (positive funding → longs crowded → next ret often negative?)

H2 — Vol expansion:
  After quiet regime (low mean |ret|), is next N-hour |ret| larger than baseline?

Usage:
  python3 scripts/analyze_hypotheses.py --coins ETH,SOL,BTC --days-file data/history
"""

from __future__ import annotations

import argparse
import csv
import statistics
from pathlib import Path


def load_bars(path: Path) -> list[dict]:
    rows = []
    with path.open() as f:
        for r in csv.DictReader(f):
            rows.append(
                {
                    "ts": int(r["ts"]),
                    "mid": float(r["mid"]),
                    "funding": float(r["funding"]),
                }
            )
    return rows


def rets(mids: list[float], h: int) -> list[float]:
    out = []
    for i in range(len(mids) - h):
        if mids[i] > 0:
            out.append((mids[i + h] - mids[i]) / mids[i])
    return out


def analyze_h1(bars: list[dict], thr: float, horizons: list[int]) -> None:
    print(f"\n=== H1 funding trap |funding|>{thr} ===")
    fund = [b["funding"] for b in bars]
    mids = [b["mid"] for b in bars]
    n = len(bars)
    for h in horizons:
        # when funding extreme positive, expect negative forward ret if trap
        pos_fwd = []
        neg_fwd = []
        base = []
        for i in range(n - h):
            r = (mids[i + h] - mids[i]) / mids[i] if mids[i] > 0 else 0.0
            base.append(r)
            f = fund[i]
            if f >= thr:
                pos_fwd.append(r)
            elif f <= -thr:
                neg_fwd.append(r)
        def avg(xs):
            return statistics.mean(xs) if xs else float("nan")
        print(
            f"  h={h}h  n_pos={len(pos_fwd)} mean_ret_after_+fund={avg(pos_fwd):+.5f}  "
            f"n_neg={len(neg_fwd)} mean_ret_after_-fund={avg(neg_fwd):+.5f}  "
            f"baseline={avg(base):+.5f}"
        )
        # trap score: after +funding, ret should be < 0 for "longs lose"
        if pos_fwd:
            hit = sum(1 for r in pos_fwd if r < 0) / len(pos_fwd)
            print(f"       P(ret<0 | +funding)={hit:.2%}  (trap if >>50%)")
        if neg_fwd:
            hit = sum(1 for r in neg_fwd if r > 0) / len(neg_fwd)
            print(f"       P(ret>0 | -funding)={hit:.2%}  (trap if >>50%)")


def analyze_h2(bars: list[dict], quiet_thr: float, lookback: int, forward: int) -> None:
    print(f"\n=== H2 vol expansion quiet<{quiet_thr} lookback={lookback} forward={forward}h ===")
    mids = [b["mid"] for b in bars]
    n = len(mids)
    quiet_fwd = []
    loud_fwd = []
    all_fwd = []
    for i in range(lookback, n - forward):
        # mean abs ret in lookback
        s = 0.0
        c = 0
        for k in range(i - lookback + 1, i + 1):
            if mids[k - 1] > 0:
                s += abs((mids[k] - mids[k - 1]) / mids[k - 1])
                c += 1
        mar = s / c if c else 0.0
        fr = abs((mids[i + forward] - mids[i]) / mids[i]) if mids[i] > 0 else 0.0
        all_fwd.append(fr)
        if mar < quiet_thr:
            quiet_fwd.append(fr)
        elif mar > quiet_thr * 2:
            loud_fwd.append(fr)
    def avg(xs):
        return statistics.mean(xs) if xs else float("nan")
    print(
        f"  n_quiet={len(quiet_fwd)} mean_|ret|_fwd={avg(quiet_fwd):+.5f}  "
        f"n_loud={len(loud_fwd)} mean_|ret|_fwd={avg(loud_fwd):+.5f}  "
        f"baseline={avg(all_fwd):+.5f}"
    )
    if quiet_fwd and all_fwd:
        print(
            f"  quiet/baseline ratio={avg(quiet_fwd)/avg(all_fwd):.2f}  "
            f"(H2 weak if ≈1; interesting if quiet then *larger* expansion later — rare)"
        )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default="data/history")
    ap.add_argument("--coins", default="ETH,SOL,BTC")
    # majors often sit near 1.25e-5; default slightly above for tails
    ap.add_argument("--funding-thr", type=float, default=0.00002)
    args = ap.parse_args()
    d = Path(args.dir)
    coins = [c.strip() for c in args.coins.split(",") if c.strip()]
    print("Hypothesis offline analysis (not a trading signal)")
    for coin in coins:
        path = d / f"{coin}_1h.csv"
        if not path.exists():
            print(f"\n{coin}: missing {path}")
            continue
        bars = load_bars(path)
        print(f"\n######## {coin} n={len(bars)} ########")
        analyze_h1(bars, args.funding_thr, [1, 4, 8, 24])
        analyze_h2(bars, quiet_thr=0.0015, lookback=24, forward=8)
    print(
        "\nInterpretation:\n"
        "  H1: if P(ret wrong-way | extreme funding) >> 50%, trap is real → "
        "use as *veto* or fade carefully (still need costs).\n"
        "  H2: if quiet periods don't predict expansion, skip vol-breakout ideas.\n"
        "  Neither result alone is a money machine without OOS + costs."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
