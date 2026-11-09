#!/usr/bin/env python3
"""Snapshot Hyperliquid funding + mids for offline analysis / backtest seeds."""

import json
import os
import time
import urllib.request
from datetime import datetime, timezone

OUT_DIR = os.path.join(os.path.dirname(__file__), "..", "data")
URL = "https://api.hyperliquid.xyz/info"


def post(body: dict):
    req = urllib.request.Request(
        URL,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
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
                "sz_decimals": a.get("szDecimals"),
            }
        )
    rows.sort(key=lambda x: abs(x["funding"]), reverse=True)
    ts = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    path = os.path.join(OUT_DIR, f"hl_funding_{ts}.json")
    payload = {
        "ts": datetime.now(timezone.utc).isoformat(),
        "count": len(rows),
        "top": rows[:30],
        "all": rows,
    }
    with open(path, "w") as f:
        json.dump(payload, f, indent=2)
    # also write latest pointer
    latest = os.path.join(OUT_DIR, "hl_funding_latest.json")
    with open(latest, "w") as f:
        json.dump(payload, f, indent=2)
    print(f"wrote {path}")
    print("top 10 |funding|:")
    for r in rows[:10]:
        print(f"  {r['coin']:12} {r['funding']:+.6e} mid={r['mid']} oi={r['open_interest']:.0f}")


if __name__ == "__main__":
    main()
