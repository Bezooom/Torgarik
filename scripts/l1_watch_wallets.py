#!/usr/bin/env python3
"""
L1 watch: poll filtered wallets' positions → emit delayed/decayed signals for risk shell.

Reads:  data/l1/watched_wallets.json  (from l1_leaderboard.py)
Writes: runtime/signals.jsonl         (consumed by --mode shell)
        data/l1/watch_state.json      (last positions per wallet)
        runtime/journal/…             (optional note events)

Safety defaults:
  - dry-run print only unless --emit
  - max wallets, poll interval, size cap
  - copy delay: only signal after position stable N polls (reduces noise)
  - NOT financial advice; leaderboard ≠ future returns

Usage:
  python3 scripts/l1_leaderboard.py
  python3 scripts/l1_watch_wallets.py --once
  python3 scripts/l1_watch_wallets.py --emit --loop --interval 120
"""

from __future__ import annotations

import argparse
import json
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INFO = "https://api.hyperliquid.xyz/info"


def post_info(body: dict, timeout: float = 30.0):
    req = urllib.request.Request(
        INFO,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "User-Agent": "trading-l1-watch/1.0"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def clearinghouse(user: str) -> dict:
    return post_info({"type": "clearinghouseState", "user": user})


def positions_from_state(state: dict) -> dict[str, dict]:
    """coin -> {side, size, entry, upnl}"""
    out = {}
    for ap in state.get("assetPositions") or []:
        pos = ap.get("position") or ap
        coin = pos.get("coin") or ""
        if not coin:
            continue
        szi = float(pos.get("szi") or 0)
        if abs(szi) < 1e-12:
            continue
        out[coin] = {
            "side": "buy" if szi > 0 else "sell",
            "size": abs(szi),
            "entry": float(pos.get("entryPx") or 0),
            "upnl": float(pos.get("unrealizedPnl") or 0),
        }
    return out


def load_wallets(path: Path, limit: int) -> list[dict]:
    data = json.loads(path.read_text())
    wallets = data.get("wallets") or []
    return wallets[:limit]


def emit_signal(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as f:
        f.write(json.dumps(payload) + "\n")


def journal_note(text: str) -> None:
    day = datetime.now(timezone.utc).strftime("%Y%m%d")
    jp = ROOT / "runtime" / "journal" / f"journal_{day}.jsonl"
    jp.parent.mkdir(parents=True, exist_ok=True)
    ev = {
        "kind": "note",
        "ts": datetime.now(timezone.utc).isoformat(),
        "text": text,
        "source": "l1_watch",
    }
    with jp.open("a") as f:
        f.write(json.dumps(ev) + "\n")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--wallets", type=Path, default=ROOT / "data" / "l1" / "watched_wallets.json")
    ap.add_argument("--state", type=Path, default=ROOT / "data" / "l1" / "watch_state.json")
    ap.add_argument("--signals", type=Path, default=ROOT / "runtime" / "signals.jsonl")
    ap.add_argument("--top", type=int, default=15, help="max wallets to poll")
    ap.add_argument("--emit", action="store_true", help="write signals.jsonl (else dry print)")
    ap.add_argument("--loop", action="store_true")
    ap.add_argument("--interval", type=float, default=120.0)
    ap.add_argument(
        "--size-scale",
        type=float,
        default=0.0,
        help="if >0, signal size = whale_size * scale (e.g. 0.001). 0 = side-only flat size hint",
    )
    ap.add_argument(
        "--default-size",
        type=float,
        default=0.0,
        help="if size-scale=0, shell will size by risk %; optional fixed size here",
    )
    ap.add_argument("--stable-polls", type=int, default=2, help="require N polls with same side before emit")
    ap.add_argument("--sleep-between", type=float, default=0.15)
    args = ap.parse_args()

    if not args.wallets.exists():
        print(f"missing {args.wallets} — run: python3 scripts/l1_leaderboard.py")
        return 1

    state: dict = {}
    if args.state.exists():
        try:
            state = json.loads(args.state.read_text())
        except Exception:
            state = {}
    # state[addr][coin] = {side, seen, emitted}

    def tick() -> None:
        wallets = load_wallets(args.wallets, args.top)
        print(
            f"[{datetime.now(timezone.utc).isoformat()}] polling {len(wallets)} wallets emit={args.emit}"
        )
        for w in wallets:
            addr = w["address"]
            try:
                ch = clearinghouse(addr)
            except Exception as e:
                print(f"  {addr[:10]}… error {e}")
                time.sleep(args.sleep_between)
                continue
            pos = positions_from_state(ch)
            prev = state.get(addr) or {}
            # closed coins
            for coin, old in list(prev.items()):
                if coin.startswith("_"):
                    continue
                if coin not in pos and old.get("side"):
                    # closed
                    sig = {
                        "coin": coin,
                        "side": "flat",
                        "note": f"L1 copy: whale closed {coin}",
                        "source": f"l1:{addr[:10]}",
                    }
                    print(f"  CLOSE {addr[:10]}… {coin}")
                    if args.emit:
                        emit_signal(args.signals, sig)
                        journal_note(f"L1 signal flat {coin} from {addr}")
                    prev.pop(coin, None)

            for coin, p in pos.items():
                entry = prev.get(coin) or {"side": None, "seen": 0, "emitted": None}
                if entry.get("side") == p["side"]:
                    entry["seen"] = int(entry.get("seen") or 0) + 1
                else:
                    entry = {"side": p["side"], "seen": 1, "emitted": None}
                prev[coin] = entry

                if entry["seen"] < args.stable_polls:
                    print(
                        f"  wait {addr[:10]}… {coin} {p['side']} seen={entry['seen']}/{args.stable_polls}"
                    )
                    continue
                if entry.get("emitted") == p["side"]:
                    continue  # already signaled this side

                size = None
                if args.size_scale > 0:
                    size = max(0.0, p["size"] * args.size_scale)
                elif args.default_size > 0:
                    size = args.default_size

                sig = {
                    "coin": coin,
                    "side": p["side"],
                    "note": (
                        f"L1: {addr[:10]}… {p['side']} sz={p['size']:.4f} "
                        f"entry={p['entry']:.4f} upnl={p['upnl']:.1f}"
                    ),
                    "source": f"l1:{addr[:10]}",
                }
                if size is not None and size > 0:
                    sig["size"] = size

                print(f"  SIGNAL {sig}")
                if args.emit:
                    emit_signal(args.signals, sig)
                    journal_note(sig["note"])
                entry["emitted"] = p["side"]
                prev[coin] = entry

            state[addr] = prev
            time.sleep(args.sleep_between)

        args.state.parent.mkdir(parents=True, exist_ok=True)
        args.state.write_text(json.dumps(state, indent=2))
        print(f"state → {args.state}")

    if args.loop:
        while True:
            try:
                tick()
            except Exception as e:
                print(f"tick error: {e}")
            time.sleep(args.interval)
    else:
        tick()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
