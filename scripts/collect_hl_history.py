#!/usr/bin/env python3
"""
Collect Hyperliquid mid + funding history for walk-forward backtests.

Sources:
  - fundingHistory  (hourly-ish funding rate series)
  - candleSnapshot  (1h OHLC → close as mid proxy)

Also appends a point-in-time universe snapshot (for scanner research).

Usage:
  python3 scripts/collect_hl_history.py                  # default coins + top funding
  python3 scripts/collect_hl_history.py --days 14 --coins BTC,ETH,ACE
  python3 scripts/collect_hl_history.py --cron           # append snapshot only + refresh series

Cron example (every hour):
  5 * * * * cd /path/to/Trading && python3 scripts/collect_hl_history.py --cron >> logs/hl_collect.log 2>&1
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data"
HIST = DATA / "history"
SNAP = DATA / "snapshots"
URL = "https://api.hyperliquid.xyz/info"

DEFAULT_COINS = [
    "BTC", "ETH", "SOL", "XRP", "BNB", "LINK", "AVAX", "DOGE",
    "ARB", "SUI", "LTC", "UNI", "AAVE", "TRX", "DOT", "ATOM",
    "NEAR", "APT", "OP", "HYPE",
]

# Memes / toxic funding names — excluded unless --include-memes
BLACKLIST = {
    "ACE", "NOT", "MOVE", "KNEIRO", "NEIRO", "SUPER", "CASHCAT", "POPCAT",
    "WIF", "MEW", "PNUT", "GOAT", "MOODENG", "FARTCOIN", "BRETT", "GIGA",
    "BONK", "FLOKI", "BOME", "DOGS", "HMSTR", "MEME", "TURBO", "PEOPLE",
    "RATS", "ORDI", "1000SATS", "ALT",  # ALT often illiquid trash funding
}


def post(body: dict, timeout: float = 45.0) -> Any:
    req = urllib.request.Request(
        URL,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "User-Agent": "trading-bot-history/1.0"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def now_ms() -> int:
    return int(time.time() * 1000)


def utc_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _parse_funding_rows(coin: str, data: list) -> list[dict]:
    out = []
    for row in data:
        try:
            out.append(
                {
                    "ts_ms": int(row["time"]),
                    "funding": float(row["fundingRate"]),
                    "premium": float(row.get("premium") or 0),
                    "coin": row.get("coin", coin),
                }
            )
        except (KeyError, TypeError, ValueError):
            continue
    return out


def fetch_funding(coin: str, start_ms: int, end_ms: int | None = None) -> list[dict]:
    """Paginate fundingHistory.

    HL returns at most ~500 rows *from startTime forward*. Large windows
    therefore miss recent data unless we walk startTime forward.
    """
    end = end_ms if end_ms is not None else now_ms()
    cursor = start_ms
    by_ts: dict[int, dict] = {}
    # safety: max ~200 pages
    for _ in range(200):
        if cursor >= end:
            break
        body: dict[str, Any] = {
            "type": "fundingHistory",
            "coin": coin,
            "startTime": cursor,
            "endTime": end,
        }
        data = post(body)
        if not isinstance(data, list) or not data:
            break
        rows = _parse_funding_rows(coin, data)
        if not rows:
            break
        for r in rows:
            if start_ms <= r["ts_ms"] <= end:
                by_ts[r["ts_ms"]] = r
        last_ts = max(r["ts_ms"] for r in rows)
        # advance past last point; if no progress, stop
        next_cursor = last_ts + 1
        if next_cursor <= cursor:
            break
        cursor = next_cursor
        # full page ≈ 500 → keep going; short page → likely done
        if len(data) < 400 and last_ts >= end - 3_600_000:
            break
        time.sleep(0.05)
    out = sorted(by_ts.values(), key=lambda x: x["ts_ms"])
    return out


def fetch_candles_1h(coin: str, start_ms: int, end_ms: int | None = None) -> list[dict]:
    """Chunk candleSnapshot (~500 limit per call; chunk ~14d)."""
    end = end_ms if end_ms is not None else now_ms()
    chunk_ms = 14 * 86400 * 1000
    by_ts: dict[int, dict] = {}
    t = start_ms
    while t < end:
        te = min(t + chunk_ms, end)
        body = {
            "type": "candleSnapshot",
            "req": {
                "coin": coin,
                "interval": "1h",
                "startTime": t,
                "endTime": te,
            },
        }
        data = post(body)
        if isinstance(data, list):
            for row in data:
                try:
                    ts = int(row["t"])
                    by_ts[ts] = {
                        "ts_ms": ts,
                        "open": float(row["o"]),
                        "high": float(row["h"]),
                        "low": float(row["l"]),
                        "close": float(row["c"]),
                        "volume": float(row.get("v") or 0),
                    }
                except (KeyError, TypeError, ValueError):
                    continue
        t = te
        time.sleep(0.05)
    return sorted(by_ts.values(), key=lambda x: x["ts_ms"])


def merge_bars(funding: list[dict], candles: list[dict]) -> list[dict]:
    """Align funding to nearest candle close (forward-fill mid)."""
    if not funding and not candles:
        return []
    # index candles by hour bucket
    by_hour: dict[int, dict] = {}
    for c in candles:
        hour = c["ts_ms"] // 3_600_000
        by_hour[hour] = c

    bars = []
    if funding:
        last_mid = None
        # seed last_mid from earliest candle if any
        if candles:
            last_mid = candles[0]["close"]
        for f in funding:
            hour = f["ts_ms"] // 3_600_000
            c = by_hour.get(hour)
            if c:
                last_mid = c["close"]
            elif last_mid is None:
                # look nearby hours
                for dh in range(0, 6):
                    c2 = by_hour.get(hour - dh) or by_hour.get(hour + dh)
                    if c2:
                        last_mid = c2["close"]
                        break
            if last_mid is None or last_mid <= 0:
                continue
            bars.append(
                {
                    "ts": f["ts_ms"] // 1000,
                    "ts_ms": f["ts_ms"],
                    "mid": last_mid,
                    "funding": f["funding"],
                    "premium": f.get("premium", 0.0),
                }
            )
    else:
        for c in candles:
            bars.append(
                {
                    "ts": c["ts_ms"] // 1000,
                    "ts_ms": c["ts_ms"],
                    "mid": c["close"],
                    "funding": 0.0,
                    "premium": 0.0,
                }
            )
    return bars


def write_series_csv(path: Path, bars: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["ts", "ts_ms", "mid", "funding", "premium"])
        w.writeheader()
        for b in bars:
            w.writerow(b)


def write_series_json(path: Path, coin: str, bars: list[dict], meta: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "coin": coin,
        "collected_at": utc_iso(),
        "n_bars": len(bars),
        "meta": meta,
        "bars": bars,
    }
    path.write_text(json.dumps(payload, indent=2))


def universe_snapshot() -> dict:
    meta_ctx = post({"type": "metaAndAssetCtxs"})
    mids = post({"type": "allMids"})
    meta, ctxs = meta_ctx[0], meta_ctx[1]
    rows = []
    for i, a in enumerate(meta["universe"]):
        if a.get("isDelisted"):
            continue
        name = a["name"]
        ctx = ctxs[i]
        funding = float(ctx.get("funding") or 0)
        mark = float(ctx.get("markPx") or 0)
        oi = float(ctx.get("openInterest") or 0)
        mid = float(mids.get(name) or mark or 0)
        rows.append(
            {
                "coin": name,
                "funding": funding,
                "mark": mark,
                "mid": mid,
                "open_interest": oi,
            }
        )
    rows.sort(key=lambda x: abs(x["funding"]), reverse=True)
    return {
        "ts": utc_iso(),
        "ts_ms": now_ms(),
        "count": len(rows),
        "top": rows[:40],
        "all": rows,
    }


def append_snapshot_jsonl(snap: dict) -> Path:
    SNAP.mkdir(parents=True, exist_ok=True)
    path = SNAP / "universe_hourly.jsonl"
    # compact line: ts + top funding only (space)
    line = {
        "ts": snap["ts"],
        "ts_ms": snap["ts_ms"],
        "top": [
            {
                "coin": r["coin"],
                "funding": r["funding"],
                "mid": r["mid"],
                "oi": r["open_interest"],
            }
            for r in snap["top"][:20]
        ],
    }
    with path.open("a") as f:
        f.write(json.dumps(line) + "\n")
    latest = DATA / "hl_funding_latest.json"
    latest.write_text(json.dumps(snap, indent=2))
    return path


def pick_coins(args: argparse.Namespace) -> list[str]:
    coins: list[str] = []
    if args.majors:
        coins = list(DEFAULT_COINS)
    elif args.coins:
        coins.extend([c.strip() for c in args.coins.split(",") if c.strip()])
        # preserve kNEIRO-style casing for HL; also accept upper
        coins = [c if c.startswith("k") else c.upper() for c in coins]
    if args.auto_top and not args.majors:
        try:
            snap = universe_snapshot()
            for r in snap["top"][: args.auto_top]:
                c = r["coin"]
                cu = c.upper()
                if not args.include_memes and (cu in BLACKLIST or c in BLACKLIST):
                    continue
                if c not in coins and cu not in {x.upper() for x in coins}:
                    coins.append(c)
            for m in ("BTC", "ETH", "SOL"):
                if m not in {x.upper() for x in coins}:
                    coins.insert(0, m)
        except Exception as e:
            print(f"warn: auto_top failed: {e}", file=sys.stderr)
    if not coins:
        coins = list(DEFAULT_COINS)
    # de-dupe preserve order; drop blacklist unless include_memes
    seen = set()
    out = []
    for c in coins:
        key = c.upper()
        if key in seen:
            continue
        if not args.include_memes and key in BLACKLIST:
            continue
        seen.add(key)
        out.append(c if not c.startswith("k") else c)
    return out


def collect_coin(coin: str, days: float) -> tuple[list[dict], dict]:
    end = now_ms()
    start = end - int(days * 86400 * 1000)
    funding = fetch_funding(coin, start, end)
    candles = fetch_candles_1h(coin, start, end)
    bars = merge_bars(funding, candles)
    meta = {
        "days": days,
        "n_funding": len(funding),
        "n_candles": len(candles),
        "start_ms": start,
        "end_ms": end,
    }
    return bars, meta


def main() -> int:
    ap = argparse.ArgumentParser(description="Collect HL funding+mid history")
    ap.add_argument(
        "--days",
        type=float,
        default=60.0,
        help="lookback days (default 60; funding is paginated ~500/page)",
    )
    ap.add_argument("--coins", type=str, default="", help="comma list e.g. BTC,ETH,ACE")
    ap.add_argument(
        "--auto-top",
        type=int,
        default=0,
        metavar="N",
        help="also include top-N |funding| from live universe (0=off; default 0 for majors-only)",
    )
    ap.add_argument(
        "--include-memes",
        action="store_true",
        help="do not drop blacklisted coins from auto-top",
    )
    ap.add_argument(
        "--majors",
        action="store_true",
        help="force DEFAULT_COINS majors list only (ignore auto-top)",
    )
    ap.add_argument(
        "--cron",
        action="store_true",
        help="cron mode: universe snapshot append + refresh series for coin set",
    )
    ap.add_argument("--sleep", type=float, default=0.15, help="pause between coins (rate limit)")
    args = ap.parse_args()

    DATA.mkdir(parents=True, exist_ok=True)
    HIST.mkdir(parents=True, exist_ok=True)

    # Always snapshot universe for scanner research
    try:
        snap = universe_snapshot()
        path = append_snapshot_jsonl(snap)
        print(f"[{utc_iso()}] universe snapshot top1={snap['top'][0]['coin']} "
              f"funding={snap['top'][0]['funding']:+.6e} → {path}")
    except Exception as e:
        print(f"ERROR universe snapshot: {e}", file=sys.stderr)
        if args.cron:
            return 1
        snap = None

    coins = pick_coins(args)
    print(f"collecting history for {len(coins)} coins, days={args.days}: {coins}")

    manifest = {
        "collected_at": utc_iso(),
        "days": args.days,
        "coins": {},
    }

    for coin in coins:
        try:
            bars, meta = collect_coin(coin, args.days)
            if len(bars) < 5:
                print(f"  {coin}: WARN only {len(bars)} bars")
            csv_path = HIST / f"{coin}_1h.csv"
            json_path = HIST / f"{coin}_1h.json"
            write_series_csv(csv_path, bars)
            write_series_json(json_path, coin, bars, meta)
            print(f"  {coin}: {len(bars)} bars → {csv_path.name}")
            manifest["coins"][coin] = {
                "n_bars": len(bars),
                "csv": str(csv_path.relative_to(ROOT)),
                "json": str(json_path.relative_to(ROOT)),
                **meta,
            }
        except urllib.error.HTTPError as e:
            print(f"  {coin}: HTTP {e.code}", file=sys.stderr)
        except Exception as e:
            print(f"  {coin}: ERROR {e}", file=sys.stderr)
        time.sleep(args.sleep)

    man_path = HIST / "manifest.json"
    man_path.write_text(json.dumps(manifest, indent=2))
    print(f"manifest → {man_path}")
    print("next: cargo run -p backtester -- --walk-forward --data-dir data/history")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
