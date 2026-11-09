#!/usr/bin/env python3
"""
Охотник за мелочью на Hyperliquid (funding carry).

Идея «по чуть-чуть, но часто»:
  - funding > 0 → шорт собирает funding (лонги платят)
  - funding < 0 → лонг собирает funding
  - берём только |funding| выше порога и нормальный OI

Это НЕ гарантия прибыли. На 60d OOS funding_carry в репо был красный.
Режим по умолчанию: смотреть + paper-сигналы в shell (dry).

Примеры:
  python3 scripts/hunt.py
  python3 scripts/hunt.py --top 15
  python3 scripts/hunt.py --emit          # в runtime/signals.jsonl для shell
  python3 scripts/hunt.py --serve 8791   # простой веб
  python3 scripts/hunt.py --loop 60      # обновлять каждые 60с
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
HL = "https://api.hyperliquid.xyz/info"

# Монеты, где funding часто «шум» / тонкая книга — по умолчанию режем
DEFAULT_BLACKLIST = {
    "ACE", "NOT", "MOVE", "kNEIRO", "NEIRO", "SUPER", "CASHCAT", "POPCAT",
    "WIF", "MEW", "PNUT", "GOAT", "MOODENG", "FARTCOIN", "BRETT", "GIGA",
    "BONK", "FLOKI", "BOME", "DOGS", "HMSTR", "MEME", "TURBO", "PUMP",
}


def post(body: dict) -> Any:
    req = urllib.request.Request(
        HL,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "User-Agent": "trading-hunt/1.0"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def fetch_universe() -> list[dict]:
    meta_ctx = post({"type": "metaAndAssetCtxs"})
    mids = post({"type": "allMids"})
    meta, ctxs = meta_ctx[0], meta_ctx[1]
    rows = []
    for i, a in enumerate(meta["universe"]):
        if a.get("isDelisted"):
            continue
        name = a["name"]
        ctx = ctxs[i] if i < len(ctxs) else {}
        funding = float(ctx.get("funding") or 0)
        mark = float(ctx.get("markPx") or 0)
        oi = float(ctx.get("openInterest") or 0)
        mid = float(mids.get(name) or mark or 0)
        # HL funding is typically 1h rate for perps in their API ctx
        # Daily approx if constant: funding * 24 (rough; for UI only)
        daily_approx = funding * 24.0
        rows.append(
            {
                "coin": name,
                "funding": funding,
                "funding_bps": funding * 1e4,  # per period in bps
                "daily_approx_pct": daily_approx * 100.0,
                "mid": mid,
                "mark": mark,
                "open_interest": oi,
                "oi_notional": oi * mid if mid > 0 else 0.0,
                "side": "short" if funding > 0 else "long" if funding < 0 else "flat",
                "action_ru": (
                    "ШОРТ (собираешь funding — лонги платят)"
                    if funding > 0
                    else "ЛОНГ (собираешь funding — шорты платят)"
                    if funding < 0
                    else "мимо"
                ),
            }
        )
    return rows


def filter_rank(
    rows: list[dict],
    *,
    min_abs_funding: float,
    min_oi_usd: float,
    blacklist: set[str],
    top: int,
) -> list[dict]:
    out = []
    for r in rows:
        if r["coin"] in blacklist:
            continue
        if abs(r["funding"]) < min_abs_funding:
            continue
        if r["oi_notional"] < min_oi_usd:
            continue
        if r["mid"] <= 0:
            continue
        out.append(r)
    out.sort(key=lambda x: abs(x["funding"]), reverse=True)
    return out[:top]


def plan_size(equity: float, risk_frac: float, mid: float) -> float:
    if mid <= 0:
        return 0.0
    notional = equity * risk_frac
    return round(notional / mid, 6)


def emit_signals(opps: list[dict], equity: float, risk_frac: float, max_n: int = 3) -> Path:
    path = ROOT / "runtime" / "signals.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    # append only top few, small size
    with path.open("a", encoding="utf-8") as f:
        for r in opps[:max_n]:
            side = "sell" if r["side"] == "short" else "buy"
            size = plan_size(equity, risk_frac, r["mid"])
            if size <= 0:
                continue
            line = {
                "coin": r["coin"],
                "side": side,
                "size": size,
                "note": f"hunt funding={r['funding']:+.2e} daily≈{r['daily_approx_pct']:.3f}%",
                "source": "hunt",
            }
            f.write(json.dumps(line, ensure_ascii=False) + "\n")
    return path


def build_snapshot(args: argparse.Namespace) -> dict:
    rows = fetch_universe()
    bl = set(DEFAULT_BLACKLIST)
    if args.allow_meme:
        bl = set()
    opps = filter_rank(
        rows,
        min_abs_funding=args.min_funding,
        min_oi_usd=args.min_oi,
        blacklist=bl,
        top=args.top,
    )
    honesty = (
        "Честно: walk-forward funding_carry на 60d в этом репо был в минусе. "
        "Охотник = live-радар + paper. Не лей депозит, пока paper-PnL не стабильно > 0."
    )
    return {
        "ts": datetime.now(timezone.utc).isoformat(),
        "venue": "hyperliquid",
        "n_universe": len(rows),
        "n_opps": len(opps),
        "params": {
            "min_funding": args.min_funding,
            "min_oi_usd": args.min_oi,
            "top": args.top,
            "equity": args.equity,
            "risk_frac": args.risk,
        },
        "honesty": honesty,
        "opportunities": opps,
        "how_to": {
            "paper_shell": "cargo run -p ml-trader -- --mode shell",
            "paper_alpha_scanner": "cargo run -p ml-trader -- --mode alpha --strategy funding_scanner",
            "emit": "python3 scripts/hunt.py --emit",
            "loop": "python3 scripts/hunt.py --loop 60",
        },
    }


def print_table(snap: dict) -> None:
    print("══════════════════════════════════════════════════")
    print("  HUNT · Hyperliquid funding (мелочь, часто)")
    print("══════════════════════════════════════════════════")
    print(snap["honesty"])
    print()
    print(f"Вселенная: {snap['n_universe']}  ·  после фильтра: {snap['n_opps']}  ·  {snap['ts'][:19]}Z")
    print()
    if not snap["opportunities"]:
        print("Сейчас нет монет выше порога. Подожди / снизь --min-funding.")
        print("══════════════════════════════════════════════════")
        return
    print(f"{'#':>2} {'COIN':8} {'SIDE':6} {'funding':>12} {'≈%/сут':>8} {'mid':>12} {'OI$':>12}  действие")
    print("-" * 90)
    eq = snap["params"]["equity"]
    rf = snap["params"]["risk_frac"]
    for i, r in enumerate(snap["opportunities"], 1):
        sz = plan_size(eq, rf, r["mid"])
        print(
            f"{i:2} {r['coin']:8} {r['side']:6} {r['funding']:+.4e} "
            f"{r['daily_approx_pct']:+7.3f}% {r['mid']:12.4f} {r['oi_notional']:12.0f}  "
            f"{r['action_ru']}"
        )
        if i <= 3:
            side = "sell" if r["side"] == "short" else "buy"
            print(f"     → paper size≈{sz} ({side})  notional≈${eq*rf:.0f}")
    print()
    print("Дальше:")
    print("  1) Держи shell dry:  cargo run -p ml-trader -- --mode shell")
    print("  2) Эмит в paper:     python3 scripts/hunt.py --emit")
    print("  3) Авто-сканер:      cargo run -p ml-trader -- --mode alpha --strategy funding_scanner")
    print("  4) Веб-радар:        python3 scripts/hunt.py --serve 8791")
    print()
    print("Рынки ставок: для РФ-оператора часто недоступны → сейчас только HL perps.")
    print("══════════════════════════════════════════════════")


HTML = r"""<!DOCTYPE html>
<html lang="ru">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Hunt · funding</title>
<style>
  :root { --bg:#0b0f14; --card:#151c26; --t:#e8eef6; --m:#8b9bb0; --c:#22d3ee; --g:#34d399; --a:#fbbf24; --r:#fb7185; }
  body { margin:0; font:15px/1.45 system-ui,sans-serif; background:var(--bg); color:var(--t); padding:18px; }
  .w { max-width:900px; margin:0 auto; }
  h1 { margin:0 0 6px; font-size:1.2rem; }
  .sub { color:var(--m); margin-bottom:14px; font-size:0.9rem; }
  .warn { background:rgba(251,191,36,.1); border:1px solid rgba(251,191,36,.3); padding:10px 12px; border-radius:10px; color:#fde68a; font-size:0.88rem; margin-bottom:14px; }
  table { width:100%; border-collapse:collapse; font-family:ui-monospace,monospace; font-size:0.78rem; }
  th { text-align:left; color:var(--m); padding:8px 6px; border-bottom:1px solid #243041; }
  td { padding:8px 6px; border-bottom:1px solid rgba(255,255,255,.04); }
  .long { color:var(--g); } .short { color:var(--r); }
  .bar { display:flex; gap:8px; flex-wrap:wrap; margin-bottom:12px; }
  button { appearance:none; border:1px solid #334155; background:#121820; color:var(--t); border-radius:8px; padding:8px 12px; font-weight:600; cursor:pointer; }
  button.p { background:rgba(34,211,238,.15); border-color:rgba(34,211,238,.4); color:var(--c); }
  .pill { font-size:0.75rem; color:var(--m); padding:4px 8px; border:1px solid #334155; border-radius:999px; }
</style>
</head>
<body>
<div class="w">
  <h1>Hunt · мелочь на funding (HL)</h1>
  <p class="sub">Ловим перекос funding. Не «гарантия денег». Paper-first.</p>
  <div class="warn" id="honesty">…</div>
  <div class="bar">
    <button class="p" id="go">Обновить</button>
    <button id="emit">В paper (signals)</button>
    <span class="pill" id="st">…</span>
  </div>
  <table>
    <thead><tr><th>#</th><th>Монета</th><th>Сторона</th><th>funding</th><th>≈%/сут*</th><th>mid</th><th>OI $</th><th>смысл</th></tr></thead>
    <tbody id="tb"></tbody>
  </table>
  <p class="sub" style="margin-top:14px">* грубая оценка если funding не изменится. Реально funding прыгает, + есть funding vs price risk.</p>
</div>
<script>
async function load(){
  const st=document.getElementById('st');
  document.getElementById('go').disabled=true;
  try{
    const r=await fetch('/api/scan?t='+Date.now(),{cache:'no-store'});
    const d=await r.json();
    document.getElementById('honesty').textContent=d.honesty||'';
    const tb=document.getElementById('tb');
    tb.innerHTML=(d.opportunities||[]).map((x,i)=>{
      const cls=x.side==='long'?'long':'short';
      return `<tr>
        <td>${i+1}</td><td><b>${x.coin}</b></td>
        <td class="${cls}">${x.side}</td>
        <td>${x.funding>0?'+':''}${x.funding.toExponential(3)}</td>
        <td class="${cls}">${x.daily_approx_pct.toFixed(3)}%</td>
        <td>${x.mid}</td><td>${Math.round(x.oi_notional).toLocaleString('ru-RU')}</td>
        <td>${x.action_ru}</td></tr>`;
    }).join('') || '<tr><td colspan=8>пусто — снизь порог</td></tr>';
    st.textContent='обновлено '+new Date().toLocaleTimeString('ru-RU')+' · n='+(d.n_opps||0);
  }catch(e){ st.textContent='ошибка '+e.message; }
  document.getElementById('go').disabled=false;
}
document.getElementById('go').onclick=load;
document.getElementById('emit').onclick=async()=>{
  const r=await fetch('/api/emit',{method:'POST'});
  const d=await r.json();
  alert(d.ok?('Записано в '+d.path+' — нужен shell dry'): (d.error||'fail'));
};
load(); setInterval(load, 45000);
</script>
</body>
</html>
"""


def serve(port: int, args: argparse.Namespace) -> None:
    class H(BaseHTTPRequestHandler):
        def log_message(self, fmt, *args_):
            print(f"[hunt] {fmt % args_}", flush=True)

        def do_GET(self):
            path = self.path.split("?", 1)[0]
            if path in ("/", "/index.html"):
                body = HTML.encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if path == "/api/scan":
                try:
                    snap = build_snapshot(args)
                    body = json.dumps(snap, ensure_ascii=False).encode()
                    code = 200
                except Exception as e:
                    body = json.dumps({"error": str(e)}).encode()
                    code = 500
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            self.send_response(404)
            self.end_headers()

        def do_POST(self):
            path = self.path.split("?", 1)[0]
            if path == "/api/emit":
                try:
                    snap = build_snapshot(args)
                    p = emit_signals(
                        snap["opportunities"],
                        args.equity,
                        args.risk,
                        max_n=args.emit_n,
                    )
                    body = json.dumps({"ok": True, "path": str(p), "n": min(args.emit_n, snap["n_opps"])}).encode()
                except Exception as e:
                    body = json.dumps({"ok": False, "error": str(e)}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            self.send_response(404)
            self.end_headers()

    httpd = ThreadingHTTPServer(("127.0.0.1", port), H)
    print(f"Hunt UI → http://127.0.0.1:{port}/", flush=True)
    httpd.serve_forever()


def main() -> int:
    ap = argparse.ArgumentParser(description="HL funding hunt")
    ap.add_argument("--top", type=int, default=12)
    ap.add_argument("--min-funding", type=float, default=0.00003, help="|funding| порог (1h rate)")
    ap.add_argument("--min-oi", type=float, default=2_000_000.0, help="min OI notional USD")
    ap.add_argument("--equity", type=float, default=1000.0)
    ap.add_argument("--risk", type=float, default=0.05, help="доля equity на одну идею")
    ap.add_argument("--allow-meme", action="store_true")
    ap.add_argument("--emit", action="store_true", help="дописать top в signals.jsonl")
    ap.add_argument("--emit-n", type=int, default=3)
    ap.add_argument("--loop", type=int, metavar="SEC", help="повторять каждые N сек")
    ap.add_argument("--serve", type=int, metavar="PORT", help="веб UI")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
    os.chdir(ROOT)

    if args.serve:
        serve(args.serve, args)
        return 0

    def once() -> None:
        snap = build_snapshot(args)
        if args.json:
            print(json.dumps(snap, ensure_ascii=False, indent=2))
        else:
            print_table(snap)
        if args.emit and snap["opportunities"]:
            p = emit_signals(snap["opportunities"], args.equity, args.risk, args.emit_n)
            print(f"\n→ signals appended: {p}  (нужен: cargo run -p ml-trader -- --mode shell)")

    if args.loop:
        while True:
            try:
                once()
            except Exception as e:
                print(f"error: {e}", file=sys.stderr)
            time.sleep(max(15, args.loop))
    else:
        once()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
