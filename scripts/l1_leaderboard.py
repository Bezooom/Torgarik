#!/usr/bin/env python3
"""
L1: Hyperliquid leaderboard → filter consistent wallets → export JSON/CSV.

Source (public, read-only):
  https://stats-data.hyperliquid.xyz/Mainnet/leaderboard

Does NOT place orders. Feeds L1 watch / risk shell.

Usage:
  python3 scripts/l1_leaderboard.py
  python3 scripts/l1_leaderboard.py --top 50 --min-month-pnl 10000 --out data/l1
"""

from __future__ import annotations

import argparse
import csv
import json
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

LB_URL = "https://stats-data.hyperliquid.xyz/Mainnet/leaderboard"
ROOT = Path(__file__).resolve().parents[1]


def fetch_leaderboard() -> list[dict]:
    req = urllib.request.Request(
        LB_URL,
        headers={"User-Agent": "trading-l1/1.0"},
    )
    with urllib.request.urlopen(req, timeout=120) as r:
        data = json.load(r)
    return data.get("leaderboardRows") or []


def window_map(row: dict) -> dict[str, dict]:
    out = {}
    for name, perf in row.get("windowPerformances") or []:
        out[name] = {
            "pnl": float(perf.get("pnl") or 0),
            "roi": float(perf.get("roi") or 0),
            "vlm": float(perf.get("vlm") or 0),
        }
    return out


def score_wallet(w: dict) -> float:
    """Higher = more 'consistent' candidate (heuristic, not proof of edge)."""
    m, w_, a = w["month"], w["week"], w["allTime"]
    # prefer positive multi-window pnl, penalize insane short-term ROI
    s = 0.0
    if m["pnl"] > 0:
        s += 2.0
    if w_["pnl"] > 0:
        s += 1.5
    if a["pnl"] > 0:
        s += 1.0
    # stability: week and month same sign
    if m["pnl"] * w_["pnl"] > 0:
        s += 1.0
    # volume activity
    s += min(2.0, m["vlm"] / 5e7)  # up to +2 for $50M+ monthly vol
    # penalize lottery ROI (month)
    if m["roi"] > 1.0:  # >100%
        s -= 2.0
    if m["roi"] > 3.0:
        s -= 3.0
    # penalize day >> month (one-hit day)
    d = w["day"]
    if m["pnl"] > 0 and d["pnl"] > 0.8 * m["pnl"] and m["pnl"] > 1e4:
        s -= 1.5
    return s


def filter_rows(
    rows: list[dict],
    *,
    min_account: float,
    max_account: float,
    min_month_pnl: float,
    min_week_pnl: float,
    min_month_vlm: float,
    max_month_roi: float,
    require_alltime_pos: bool,
) -> list[dict]:
    out = []
    for row in rows:
        addr = (row.get("ethAddress") or "").lower()
        if not addr.startswith("0x"):
            continue
        av = float(row.get("accountValue") or 0)
        if av < min_account or av > max_account:
            continue
        wm = window_map(row)
        if "month" not in wm or "week" not in wm or "allTime" not in wm:
            continue
        m, w, a, d = wm["month"], wm["week"], wm["allTime"], wm.get("day", {"pnl": 0, "roi": 0, "vlm": 0})
        if m["pnl"] < min_month_pnl:
            continue
        if w["pnl"] < min_week_pnl:
            continue
        if m["vlm"] < min_month_vlm:
            continue
        if m["roi"] > max_month_roi:
            continue
        if require_alltime_pos and a["pnl"] <= 0:
            continue
        # skip pure day lottery
        if m["pnl"] > 0 and d["pnl"] > max(0.0, m["pnl"]) * 0.9 and d["pnl"] > 5e4:
            continue
        rec = {
            "address": addr,
            "displayName": row.get("displayName"),
            "accountValue": av,
            "day": d,
            "week": w,
            "month": m,
            "allTime": a,
            "score": 0.0,
        }
        rec["score"] = score_wallet(rec)
        out.append(rec)
    out.sort(key=lambda x: (-x["score"], -x["month"]["pnl"]))
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description="L1 HL leaderboard filter")
    ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--min-account", type=float, default=50_000.0)
    ap.add_argument("--max-account", type=float, default=50_000_000.0)
    ap.add_argument("--min-month-pnl", type=float, default=5_000.0)
    ap.add_argument("--min-week-pnl", type=float, default=0.0)
    ap.add_argument("--min-month-vlm", type=float, default=1_000_000.0)
    ap.add_argument("--max-month-roi", type=float, default=1.5, help="1.5 = 150%")
    ap.add_argument("--require-alltime-pos", action="store_true", default=True)
    ap.add_argument("--allow-alltime-neg", action="store_true")
    ap.add_argument("--out", type=Path, default=ROOT / "data" / "l1")
    args = ap.parse_args()

    require_at = not args.allow_alltime_neg
    print("Fetching leaderboard...")
    rows = fetch_leaderboard()
    print(f"raw wallets: {len(rows)}")

    filtered = filter_rows(
        rows,
        min_account=args.min_account,
        max_account=args.max_account,
        min_month_pnl=args.min_month_pnl,
        min_week_pnl=args.min_week_pnl,
        min_month_vlm=args.min_month_vlm,
        max_month_roi=args.max_month_roi,
        require_alltime_pos=require_at,
    )
    top = filtered[: args.top]
    print(f"after filters: {len(filtered)}  exporting top {len(top)}")

    args.out.mkdir(parents=True, exist_ok=True)
    ts = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    payload = {
        "fetched_at": datetime.now(timezone.utc).isoformat(),
        "source": LB_URL,
        "filters": {
            "min_account": args.min_account,
            "max_account": args.max_account,
            "min_month_pnl": args.min_month_pnl,
            "min_week_pnl": args.min_week_pnl,
            "min_month_vlm": args.min_month_vlm,
            "max_month_roi": args.max_month_roi,
            "require_alltime_pos": require_at,
        },
        "n_raw": len(rows),
        "n_filtered": len(filtered),
        "wallets": top,
    }
    json_path = args.out / "watched_wallets.json"
    snap_path = args.out / f"leaderboard_filtered_{ts}.json"
    json_path.write_text(json.dumps(payload, indent=2))
    snap_path.write_text(json.dumps(payload, indent=2))

    csv_path = args.out / "watched_wallets.csv"
    with csv_path.open("w", newline="") as f:
        w = csv.writer(f)
        w.writerow(
            [
                "rank",
                "address",
                "score",
                "accountValue",
                "month_pnl",
                "month_roi",
                "month_vlm",
                "week_pnl",
                "week_roi",
                "allTime_pnl",
                "day_pnl",
            ]
        )
        for i, rec in enumerate(top, 1):
            w.writerow(
                [
                    i,
                    rec["address"],
                    f"{rec['score']:.2f}",
                    f"{rec['accountValue']:.2f}",
                    f"{rec['month']['pnl']:.2f}",
                    f"{rec['month']['roi']:.4f}",
                    f"{rec['month']['vlm']:.2f}",
                    f"{rec['week']['pnl']:.2f}",
                    f"{rec['week']['roi']:.4f}",
                    f"{rec['allTime']['pnl']:.2f}",
                    f"{rec['day']['pnl']:.2f}",
                ]
            )

    print(f"wrote {json_path}")
    print(f"wrote {csv_path}")
    print(f"snapshot {snap_path}")
    print("\nTop 10:")
    for i, rec in enumerate(top[:10], 1):
        print(
            f"  {i:2d}. {rec['address'][:10]}…  score={rec['score']:.1f}  "
            f"AV=${rec['accountValue']/1e6:.2f}M  "
            f"monthPnL=${rec['month']['pnl']/1e3:.1f}k  "
            f"monthROI={rec['month']['roi']*100:.1f}%  "
            f"weekPnL=${rec['week']['pnl']/1e3:.1f}k"
        )
    print("\nNext: python3 scripts/l1_watch_wallets.py")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
