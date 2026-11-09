#!/usr/bin/env python3
"""
Простой советник «для дурака» (S1 + shell).

Собирает факты из runtime/ + OOS, затем:
  1) всегда — жёсткие правила (без LLM);
  2) если жив SGLang на :30000 — пересказ простым русским + 1–3 действия.

Не торгует. Не «печатает alpha». Только советы.

Примеры:
  python3 scripts/advisor.py
  python3 scripts/advisor.py --no-llm
  python3 scripts/advisor.py --serve 8790
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
SGLANG = os.environ.get("ADVISOR_LLM", "http://127.0.0.1:30000/v1")
MODEL = os.environ.get("ADVISOR_MODEL", "default")


def read_json(path: Path) -> Any | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return None


def read_jsonl_tail(path: Path, n: int = 40) -> list[dict]:
    if not path.exists():
        return []
    lines = [ln for ln in path.read_text(encoding="utf-8", errors="replace").splitlines() if ln.strip()]
    out: list[dict] = []
    for ln in lines[-n:]:
        try:
            out.append(json.loads(ln))
        except json.JSONDecodeError:
            continue
    return out


def load_s1_hours() -> dict:
    path = ROOT / "runtime" / "s1" / "hours.csv"
    week_target, shell_target = 15.0, 5
    if not path.exists():
        return {
            "week_hours": 0.0,
            "week_target": week_target,
            "shell_days_7": 0,
            "shell_target_7": shell_target,
            "need_hours": week_target,
            "recent": [],
        }
    rows = []
    for i, line in enumerate(path.read_text(encoding="utf-8").splitlines()):
        if i == 0 or not line.strip():
            continue
        # naive CSV: date,hours,track,shell,note,mood,logged_at
        parts: list[str] = []
        cur, q = "", False
        for ch in line:
            if ch == '"':
                q = not q
            elif ch == "," and not q:
                parts.append(cur)
                cur = ""
            else:
                cur += ch
        parts.append(cur)
        if len(parts) < 4:
            continue
        try:
            hours = float(parts[1] or 0)
        except ValueError:
            hours = 0.0
        rows.append(
            {
                "date": parts[0],
                "hours": hours,
                "track": parts[2],
                "shell": parts[3].lower() in ("yes", "y", "1", "true"),
                "note": parts[4] if len(parts) > 4 else "",
            }
        )
    today = datetime.now(timezone.utc).date()
    week_start = today.fromordinal(today.toordinal() - today.weekday())
    last7 = today.fromordinal(today.toordinal() - 6)
    week_h = 0.0
    shell_7 = 0
    for r in rows:
        try:
            d = datetime.strptime(r["date"], "%Y-%m-%d").date()
        except ValueError:
            continue
        if d >= week_start:
            week_h += r["hours"]
        if d >= last7 and r["shell"]:
            shell_7 += 1
    return {
        "week_hours": round(week_h, 2),
        "week_target": week_target,
        "shell_days_7": shell_7,
        "shell_target_7": shell_target,
        "need_hours": round(max(0.0, week_target - week_h), 2),
        "recent": list(reversed(rows[-7:])),
    }


def load_oos() -> dict:
    prefer = [
        "wf_trend_ethsol_realistic.json",
        "wf_trend_core_realistic.json",
        "wf_latest.json",
    ]
    reports = ROOT / "data" / "reports"
    path = next((reports / n for n in prefer if (reports / n).exists()), None)
    if path is None:
        return {"available": False, "ready": False, "summary": "нет OOS-отчётов"}
    raw = read_json(path) or {}
    folds = raw.get("folds") or []
    pnls, sharpes, n_neg = [], [], 0
    for f in folds:
        t = f.get("test") or {}
        p = t.get("total_pnl")
        s = t.get("sharpe_ratio")
        if isinstance(p, (int, float)):
            pnls.append(float(p))
            if p < 0:
                n_neg += 1
        if isinstance(s, (int, float)):
            sharpes.append(float(s))
    avg_p = sum(pnls) / len(pnls) if pnls else 0.0
    avg_s = sum(sharpes) / len(sharpes) if sharpes else 0.0
    ready = avg_p > 0 and avg_s > 0 and (not pnls or n_neg * 2 <= len(pnls))
    return {
        "available": True,
        "file": path.name,
        "avg_pnl": round(avg_p, 2),
        "avg_sharpe": round(avg_s, 2),
        "ready": ready,
        "summary": (
            "OOS зелёный — можно думать о live только после чеклиста BACKTEST"
            if ready
            else "OOS красный/слабый — live auto-alpha ЗАПРЕЩЁН"
        ),
    }


def load_shell_alive(journal: list[dict]) -> dict:
    last_hb = None
    for e in reversed(journal):
        if e.get("kind") == "heartbeat":
            last_hb = e
            break
    if not last_hb:
        return {"alive": False, "age_secs": None, "equity": None, "mode": None}
    ts = last_hb.get("ts")
    age = None
    if ts:
        try:
            t = datetime.fromisoformat(ts.replace("Z", "+00:00"))
            age = max(0, int((datetime.now(timezone.utc) - t).total_seconds()))
        except Exception:
            age = None
    return {
        "alive": age is not None and age <= 60,
        "age_secs": age,
        "equity": last_hb.get("equity"),
        "daily_pnl": last_hb.get("daily_pnl"),
        "mode": last_hb.get("mode"),
        "n_positions": last_hb.get("n_positions"),
    }


def summarize_signals(signals: list[dict]) -> dict:
    if not signals:
        return {"count": 0, "last": None, "l1_noise": 0, "manual": 0}
    l1 = sum(1 for s in signals if str(s.get("source") or "").startswith("l1:"))
    manual = sum(1 for s in signals if str(s.get("source") or "") in ("me", "manual", "smoke", ""))
    last = signals[-1]
    return {
        "count": len(signals),
        "last": {
            "coin": last.get("coin"),
            "side": last.get("side"),
            "size": last.get("size"),
            "note": str(last.get("note") or "")[:120],
            "source": last.get("source"),
        },
        "l1_noise": l1,
        "manual": manual,
    }


def gather_facts() -> dict:
    journal_dir = ROOT / "runtime" / "journal"
    journal_files = sorted(journal_dir.glob("journal_*.jsonl")) if journal_dir.is_dir() else []
    journal: list[dict] = []
    if journal_files:
        journal = read_jsonl_tail(journal_files[-1], 50)
    signals = read_jsonl_tail(ROOT / "runtime" / "signals.jsonl", 30)
    state = read_json(ROOT / "runtime" / "state.json") or {}
    s1 = load_s1_hours()
    oos = load_oos()
    shell = load_shell_alive(journal)
    sig = summarize_signals(signals)
    return {
        "ts": datetime.now(timezone.utc).isoformat(),
        "s1": s1,
        "oos": oos,
        "shell": shell,
        "signals": sig,
        "state": {
            "equity": state.get("equity"),
            "strategy": state.get("strategy"),
            "venue": state.get("venue"),
            "positions": len(state.get("positions") or []),
        },
        "journal_kinds": [e.get("kind") for e in journal[-12:]],
    }


def rule_advice(facts: dict) -> dict:
    """Жёсткие правила — правда, даже если LLM врёт."""
    actions: list[str] = []
    warnings: list[str] = []
    do_not: list[str] = []

    oos = facts["oos"]
    s1 = facts["s1"]
    shell = facts["shell"]
    sig = facts["signals"]

    # Alpha freeze
    if not oos.get("ready", False):
        do_not.append("Не включай live auto-alpha и не лей депозит в бота (OOS не готов).")
        do_not.append("Не копируй L1-китов «как есть» — это шум, не твой edge.")
    else:
        warnings.append("OOS выглядит лучше обычного — всё равно чеклист docs/BACKTEST.md перед live.")

    # S1 hours
    if s1["week_hours"] < s1["week_target"]:
        need = s1["need_hours"]
        actions.append(
            f"Главное сегодня: +{need:.1f} ч навыка (фриланс/код/оффер). "
            f"Запиши: python3 scripts/s1_daily_log.py --hours 2 --track skill --shell --note \"…\""
        )
    else:
        actions.append("Недельный план по skill-часам выполнен — держи темп, не подменяй его «настройкой бота».")

    # Shell hygiene
    if not shell["alive"]:
        actions.append(
            "По желанию включи щит (dry, без реальных ордеров): "
            "cargo run -p ml-trader -- --mode shell"
        )
        warnings.append("Risk shell сейчас не жив — это ок, если сознательно; не путай с «бот зарабатывает».")
    else:
        actions.append(
            f"Shell жив (режим {shell.get('mode') or '?'}, equity≈{shell.get('equity')}). "
            "Ничего не меняй в live. Просто оставь как страховку."
        )

    if s1["shell_days_7"] < s1["shell_target_7"]:
        actions.append(
            f"Отметь shell-день в дневнике ({s1['shell_days_7']}/{s1['shell_target_7']} за 7д) — флаг --shell в s1_daily_log."
        )

    # Signals
    if sig["l1_noise"] >= 5:
        warnings.append(
            f"В signals.jsonl много L1-строк ({sig['l1_noise']}). "
            "Не исполняй их live. Можно почистить файл или игнорить — shell в dry только логирует."
        )
    if sig["last"] and str(sig["last"].get("source") or "").startswith("l1:"):
        do_not.append(
            f"Последний сигнал {sig['last'].get('coin')} {sig['last'].get('side')} из L1 — "
            "это чужой кошелёк, не «рекомендация купить»."
        )

    # Priority one-liner
    if s1["week_hours"] < s1["week_target"]:
        headline = f"Сегодня: skill-часы (+{s1['need_hours']:.1f} ч до цели). Торговля — хобби, не ATM."
    elif not oos.get("ready", False):
        headline = "Alpha выключена (OOS). Держи shell-гигиену и skill. Не ищи «кнопку денег» в боте."
    else:
        headline = "База в порядке. Не раздувай размер. Проверь BACKTEST-чеклист, если думаешь о live."

    return {
        "headline": headline,
        "actions": actions[:5],
        "warnings": warnings[:5],
        "do_not": do_not[:5],
        "mode": "rules",
    }


def llm_explain(facts: dict, rules: dict) -> str | None:
    """Короткий человеческий пересказ через SGLang. При ошибке — None."""
    system = (
        "Ты — спокойный наставник по риск-дисциплине для трейдера из РФ. "
        "Говори по-русски, просто, без жаргона. "
        "НЕ предлагай live-торговлю если OOS не ready. "
        "НЕ выдумывай цены и сделки. "
        "Структура ответа строго:\n"
        "1) Одной фразой: что происходит\n"
        "2) Что сделать СЕГОДНЯ (2–4 пункта, конкретные команды если уместно)\n"
        "3) Чего НЕ делать\n"
        "Максимум 180 слов."
    )
    user = json.dumps(
        {
            "facts": facts,
            "rules_already_decided": rules,
            "hint": "Правила выше — закон. Ты только объясняешь их простым языком.",
        },
        ensure_ascii=False,
    )
    body = {
        "model": MODEL,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "max_tokens": 500,
        "temperature": 0.3,
    }
    req = urllib.request.Request(
        SGLANG.rstrip("/") + "/chat/completions",
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.loads(resp.read().decode("utf-8"))
        return data["choices"][0]["message"]["content"].strip()
    except Exception as e:
        return None


def llm_available() -> bool:
    try:
        with urllib.request.urlopen(SGLANG.rstrip("/") + "/models", timeout=2) as r:
            return r.status == 200
    except Exception:
        return False


def build_report(use_llm: bool = True) -> dict:
    facts = gather_facts()
    rules = rule_advice(facts)
    llm_text = None
    llm_status = "skipped"
    if use_llm:
        if llm_available():
            llm_text = llm_explain(facts, rules)
            llm_status = "ok" if llm_text else "error"
        else:
            llm_status = "offline"
    return {
        "facts": facts,
        "rules": rules,
        "llm": {"status": llm_status, "text": llm_text},
        "ts": facts["ts"],
    }


def format_text(report: dict) -> str:
    r = report["rules"]
    f = report["facts"]
    lines = [
        "══════════════════════════════════════",
        "  СОВЕТНИК S1  ·  просто и по делу",
        "══════════════════════════════════════",
        "",
        "▶ " + r["headline"],
        "",
        "— Факты (коротко) —",
        f"  Skill за неделю: {f['s1']['week_hours']} / {f['s1']['week_target']} ч  (ещё {f['s1']['need_hours']})",
        f"  Shell-дни за 7д: {f['s1']['shell_days_7']} / {f['s1']['shell_target_7']}",
        f"  Shell процесс:  {'ЖИВ' if f['shell']['alive'] else 'выкл'}  "
        f"(age={f['shell'].get('age_secs')}s, mode={f['shell'].get('mode')})",
        f"  OOS: {f['oos']['summary']}",
        f"  Сигналов в хвосте: {f['signals']['count']}  (L1-шум: {f['signals']['l1_noise']})",
        "",
        "— Что сделать —",
    ]
    for i, a in enumerate(r["actions"], 1):
        lines.append(f"  {i}. {a}")
    if r["warnings"]:
        lines.append("")
        lines.append("— Предупреждения —")
        for w in r["warnings"]:
            lines.append(f"  ! {w}")
    if r["do_not"]:
        lines.append("")
        lines.append("— Не делай —")
        for d in r["do_not"]:
            lines.append(f"  ✗ {d}")

    llm = report["llm"]
    lines.append("")
    if llm["status"] == "ok" and llm["text"]:
        lines.append("— Объяснение LLM (не приказ, пересказ) —")
        lines.append(llm["text"])
    elif llm["status"] == "offline":
        lines.append("(LLM offline — показаны только правила. Подними: bash scripts/start_sglang.sh)")
    elif llm["status"] == "error":
        lines.append("(LLM ошибка — показаны только правила.)")
    else:
        lines.append("(режим без LLM)")

    lines.append("")
    lines.append("Повторить:  python3 scripts/advisor.py")
    lines.append("Без LLM:    python3 scripts/advisor.py --no-llm")
    lines.append("Веб:        python3 scripts/advisor.py --serve 8790")
    lines.append("══════════════════════════════════════")
    return "\n".join(lines)


HTML = r"""<!DOCTYPE html>
<html lang="ru">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>Советник S1</title>
<style>
  :root { --bg:#0b0f14; --card:#151c26; --text:#e8eef6; --muted:#8b9bb0; --c:#22d3ee; --g:#34d399; --a:#fbbf24; --r:#fb7185; }
  * { box-sizing: border-box; }
  body { margin:0; font:16px/1.5 system-ui,sans-serif; background:var(--bg); color:var(--text);
    min-height:100vh; padding:20px; }
  .wrap { max-width:640px; margin:0 auto; }
  h1 { font-size:1.25rem; margin:0 0 4px; }
  .sub { color:var(--muted); font-size:0.9rem; margin-bottom:16px; }
  .card { background:var(--card); border:1px solid rgba(148,163,184,.15); border-radius:14px; padding:16px 18px; margin-bottom:12px; }
  .headline { font-size:1.1rem; font-weight:700; color:var(--c); margin:0 0 8px; }
  h2 { font-size:0.85rem; color:var(--muted); text-transform:uppercase; letter-spacing:.04em; margin:14px 0 8px; }
  ol, ul { margin:0; padding-left:1.2rem; }
  li { margin:6px 0; }
  .warn li { color:var(--a); }
  .no li { color:var(--r); }
  .facts { font-family:ui-monospace,monospace; font-size:0.8rem; color:var(--muted); line-height:1.6; }
  .llm { white-space:pre-wrap; font-size:0.95rem; }
  .bar { display:flex; gap:8px; flex-wrap:wrap; margin-bottom:14px; align-items:center; }
  button { appearance:none; border:1px solid rgba(148,163,184,.25); background:#121820; color:var(--text);
    border-radius:10px; padding:10px 14px; font-weight:600; cursor:pointer; }
  button.primary { background:rgba(34,211,238,.15); border-color:rgba(34,211,238,.4); color:var(--c); }
  .pill { font-size:0.75rem; padding:4px 10px; border-radius:999px; border:1px solid rgba(148,163,184,.2); color:var(--muted); }
  .pill.ok { color:var(--g); border-color:rgba(52,211,153,.35); }
  .pill.bad { color:var(--r); border-color:rgba(251,113,133,.35); }
  .err { color:var(--r); }
</style>
</head>
<body>
<div class="wrap">
  <h1>Советник S1</h1>
  <p class="sub">Для человека, не для «профи-терминала». Ордера не выставляет. Раз в 30 с обновляется.</p>
  <div class="bar">
    <button class="primary" type="button" id="go">Обновить совет</button>
    <span class="pill" id="status">…</span>
    <span class="pill" id="llm-pill">LLM …</span>
  </div>
  <div class="card">
    <p class="headline" id="headline">Загрузка…</p>
    <div class="facts" id="facts"></div>
  </div>
  <div class="card">
    <h2>Что сделать</h2>
    <ol id="actions"></ol>
    <h2>Предупреждения</h2>
    <ul class="warn" id="warnings"></ul>
    <h2>Не делай</h2>
    <ul class="no" id="donot"></ul>
  </div>
  <div class="card">
    <h2>Объяснение простым языком</h2>
    <div class="llm" id="llm">—</div>
  </div>
</div>
<script>
async function load() {
  const st = document.getElementById('status');
  const go = document.getElementById('go');
  go.disabled = true; go.textContent = 'Думаю…';
  try {
    const r = await fetch('/api/advice?t=' + Date.now(), { cache: 'no-store' });
    if (!r.ok) throw new Error('HTTP ' + r.status);
    const d = await r.json();
    const rules = d.rules || {};
    const f = d.facts || {};
    document.getElementById('headline').textContent = rules.headline || '—';
    document.getElementById('facts').textContent =
      'Skill: ' + (f.s1?.week_hours ?? '?') + ' / ' + (f.s1?.week_target ?? 15) + ' ч · ' +
      'Shell: ' + (f.shell?.alive ? 'жив' : 'выкл') + ' · ' +
      'OOS: ' + (f.oos?.ready ? 'ок' : 'красный') + ' · ' +
      'Сигналов: ' + (f.signals?.count ?? 0);
    document.getElementById('actions').innerHTML = (rules.actions || []).map(a => '<li>' + a + '</li>').join('') || '<li>—</li>';
    document.getElementById('warnings').innerHTML = (rules.warnings || []).map(a => '<li>' + a + '</li>').join('') || '<li>нет</li>';
    document.getElementById('donot').innerHTML = (rules.do_not || []).map(a => '<li>' + a + '</li>').join('') || '<li>нет</li>';
    const llm = d.llm || {};
    const lp = document.getElementById('llm-pill');
    if (llm.status === 'ok') { lp.textContent = 'LLM ок'; lp.className = 'pill ok'; }
    else if (llm.status === 'offline') { lp.textContent = 'LLM выкл'; lp.className = 'pill bad'; }
    else { lp.textContent = 'LLM ' + (llm.status || '?'); lp.className = 'pill'; }
    document.getElementById('llm').textContent = llm.text || (
      llm.status === 'offline'
        ? 'Локальная модель не запущена. Советы выше — по правилам. Чтобы включить LLM: bash scripts/start_sglang.sh'
        : '—'
    );
    st.textContent = 'обновлено ' + new Date().toLocaleTimeString('ru-RU');
    st.className = 'pill ok';
  } catch (e) {
    st.textContent = 'ошибка: ' + e.message;
    st.className = 'pill bad';
  } finally {
    go.disabled = false; go.textContent = 'Обновить совет';
  }
}
document.getElementById('go').onclick = load;
load();
setInterval(load, 30000);
</script>
</body>
</html>
"""


def serve(port: int, use_llm: bool) -> None:
    class H(BaseHTTPRequestHandler):
        def log_message(self, fmt, *args):
            print(f"[advisor] {self.address_string()} {fmt % args}", flush=True)

        def do_GET(self):
            path = self.path.split("?", 1)[0]
            if path in ("/", "/index.html"):
                body = HTML.encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if path == "/api/advice":
                report = build_report(use_llm=use_llm)
                body = json.dumps(report, ensure_ascii=False).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "application/json; charset=utf-8")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            body = b'{"error":"not found"}'
            self.send_response(404)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    httpd = ThreadingHTTPServer(("127.0.0.1", port), H)
    print(f"Советник → http://127.0.0.1:{port}/", flush=True)
    print("Ctrl+C чтобы остановить", flush=True)
    httpd.serve_forever()


def main() -> int:
    ap = argparse.ArgumentParser(description="Простой советник S1")
    ap.add_argument("--no-llm", action="store_true", help="только правила, без SGLang")
    ap.add_argument("--serve", type=int, metavar="PORT", help="веб-UI (например 8790)")
    ap.add_argument("--json", action="store_true", help="сырой JSON")
    args = ap.parse_args()
    os.chdir(ROOT)

    if args.serve:
        serve(args.serve, use_llm=not args.no_llm)
        return 0

    report = build_report(use_llm=not args.no_llm)
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        print(format_text(report))
    return 0


if __name__ == "__main__":
    sys.exit(main())
