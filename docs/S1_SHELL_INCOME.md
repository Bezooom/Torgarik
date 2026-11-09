# S1 — Shell-only + skill income (30 days)

**Цель:** деньги и устойчивость **не** от auto-alpha (OOS/holdout red → live frozen), а от:

1. **Skill / freelance / job** → ₽  
2. **Risk shell + journal** → не слить депозит gambling’ом  
3. Trading size = **0** или pure education (paper only)

Alpha / L1 = **опциональный hobby**, не KPI месяца.  
Разморозка live (если когда-либо): [LIVE_UNLOCK.md](LIVE_UNLOCK.md) — сейчас **закрыто**.

---

## Rules (non-negotiable)

| # | Rule |
|---|------|
| 1 | **No live auto-trading** (no `--mode alpha` live, no L1 `--emit` live, no HB live) |
| 2 | **No new deposits** into “bot experiments” this month |
| 3 | Shell **≥5 days/week** (even 10 min) — journal heartbeat |
| 4 | Skill work **≥15 h/week** tracked |
| 5 | Weekly review Sunday (template below) |
| 6 | If urge to “revenge trade” → write note in journal, walk away |

---

## Daily routine (~45–90 min total)

### A. Risk shell (10–15 min)

```bash
cargo run -p ml-trader -- --mode shell
# leave running or run, check one heartbeat cycle, Ctrl-C
```

Optional (research only, no orders):

```bash
# anomaly in background optional
cargo run -p ml-trader -- --mode anomaly
```

**Do not** append to `runtime/signals.jsonl` for live risk this month  
unless pure paper education and you log *why*.

### B. Skill block (main money path) — 2–4 h preferred

Pick **one** track (don’t switch weekly):

| Track | Examples (RF-friendly) | First deliverable |
|-------|------------------------|-------------------|
| **Dev / automation** | bots, scrapers, APIs, Rust/Python freelance | portfolio piece or Upwork/Kwork profile |
| **Data / analytics** | dashboards, SQL, reports | 1 case study PDF |
| **Support / ops** | crypto support, community, QA | applications sent |
| **Content** | tech notes, trading *process* (not signals) | 4 posts / month |
| **Local services** | what you already sell | 3 offers / week |

Track hours in `runtime/s1/hours.csv`.

### C. Evening log (5 min)

```bash
python3 scripts/s1_daily_log.py
# or edit runtime/s1/daily.md
```

---

## Weekly (Sunday, 30–45 min)

```bash
python3 scripts/s1_weekly_review.py
# fills runtime/s1/weekly_YYYY-Www.md template
```

Answer:

1. Hours skill / target 15+?  
2. Income in (any)?  
3. Broke rule 1–2?  
4. Urge to live-trade — what triggered?  
5. Next week one focus only  

---

## Metrics (what “win” means for S1)

| Metric | Target 30d |
|--------|------------|
| Skill hours | ≥ **60** (15×4) |
| Shell journal days | ≥ **20** |
| Live trading loss from bots | **0** |
| Income outside bots | **> 0 ₽** *or* clear pipeline (interviews/offers) |
| Alpha experiments | optional paper only |

If skill hours hit but 0 income: tighten offer (niche + price + outbound), not more bot features.

---

## What to freeze

| Component | S1 status |
|-----------|-----------|
| `--mode alpha` live | **OFF** |
| L1 `--emit` + live shell | **OFF** |
| Funding/EMA “fix fixes” | **OFF** |
| Risk shell / journal | **ON** |
| Anomaly logger | optional hobby |
| Skill work | **ON — primary** |

---

## After day 30

| Outcome | Next |
|---------|------|
| Skill income > 0 | Scale skill; keep shell as hygiene |
| Pipeline strong, $0 yet | Continue S1 +2 weeks, no trading expansion |
| 0 hours skill, lots bot tinkering | S1 failed → restart rules, cut bot time |
| Paper MM/L1 showed *process* discipline | Still need OOS before live |

Only then reconsider L1 emit — with same OOS discipline as before.

---

## Commands cheat sheet

```bash
# daily shell
cargo run -p ml-trader -- --mode shell

# local ops UI (S1 meters + journal + OOS)
cargo run -p dashboard
# → http://127.0.0.1:8080

# log day
python3 scripts/s1_daily_log.py --hours 3 --track dev --shell --note "API client for client X"

# weekly
python3 scripts/s1_weekly_review.py

# status board (CLI)
python3 scripts/s1_status.py
```

Files: `runtime/s1/` · UI: [DASHBOARD.md](DASHBOARD.md)
