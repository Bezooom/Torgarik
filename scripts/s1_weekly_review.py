#!/usr/bin/env python3
"""Generate S1 weekly review markdown from hours.csv."""

from __future__ import annotations

import csv
from collections import defaultdict
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
S1 = ROOT / "runtime" / "s1"
HOURS = S1 / "hours.csv"


def iso_week(d: date) -> str:
    y, w, _ = d.isocalendar()
    return f"{y}-W{w:02d}"


def main() -> int:
    S1.mkdir(parents=True, exist_ok=True)
    today = date.today()
    week = iso_week(today)
    # last 7 days including today
    start = today - timedelta(days=6)

    rows = []
    if HOURS.exists():
        with HOURS.open() as f:
            for r in csv.DictReader(f):
                try:
                    d = date.fromisoformat(r["date"])
                except Exception:
                    continue
                if start <= d <= today:
                    rows.append(r)

    hours = sum(float(r.get("hours") or 0) for r in rows)
    shell_days = sum(1 for r in rows if (r.get("shell") or "").lower() in ("yes", "y", "1", "true"))
    by_track: dict[str, float] = defaultdict(float)
    for r in rows:
        by_track[r.get("track") or "other"] += float(r.get("hours") or 0)

    path = S1 / f"weekly_{week}.md"
    tracks = "\n".join(f"  - {k}: {v:.1f}h" for k, v in sorted(by_track.items(), key=lambda x: -x[1])) or "  - (none)"
    body = f"""# S1 weekly review {week}

Generated: {datetime.now(timezone.utc).isoformat()}
Period: {start.isoformat()} → {today.isoformat()}

## Numbers

| Metric | Value | Target |
|--------|-------|--------|
| Skill hours | **{hours:.1f}** | ≥ 15 |
| Shell days (logs) | **{shell_days}** | ≥ 5 |
| Live bot loss | _fill_ | **0** |
| Income ₽ | _fill_ | > 0 or pipeline |

### Hours by track
{tracks}

## Reflection (fill in)

1. **What shipped** (PR, offer sent, client reply, post)?
   - 

2. **Broke S1 rules?** (live alpha / new deposit / revenge trade)
   - 

3. **Urge to trade live** — trigger & what you did instead
   - 

4. **One focus next week** (single sentence)
   - 

5. **Keep / kill hobby bot time** (MM paper, L1 dry, anomaly)
   - keep max __ h  |  kill

## Decision

- [ ] Continue S1 as-is
- [ ] S1 +2 weeks (pipeline only)
- [ ] Resume only paper research (no live)
- [ ] Consider live tiny only if OOS green (unlikely this week)

---
*Rules: docs/S1_SHELL_INCOME.md*
"""
    path.write_text(body)
    print(f"wrote {path}")
    print(f"hours={hours:.1f} shell_days={shell_days}")
    if hours < 15:
        print("⚠️  under 15h skill — primary KPI for S1")
    else:
        print("✓ skill hours OK for the week")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
